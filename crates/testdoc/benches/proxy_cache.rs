//! Linked-image proxy cache throughput — see `specs/0027-perf-harness.md`.
//!
//! The proxy cache is the core performance strategy (`CLAUDE.md`: never composite full-res on
//! screen). Two properties matter and both are measured here: generating a proxy is proportional to
//! source pixels, and *re-*populating an unchanged asset is nearly free (spec 0024's mtime+size
//! invalidation). If the second ever regresses, every edit to a 500-page book re-decodes its art.

use std::fs;
use std::path::PathBuf;

use quill_core_model::Asset;
use quill_render::ProxyCache;

mod budget;

/// Build a valid PNG of the given size without pulling in an encoder: a single IDAT of raw
/// scanlines, stored uncompressed via zlib's "stored" block type.
fn png(width: u32, height: u32) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut table = [0u32; 256];
        for (i, e) in table.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *e = c;
        }
        let mut c = 0xFFFF_FFFFu32;
        for b in data {
            c = table[((c ^ *b as u32) & 0xFF) as usize] ^ (c >> 8);
        }
        c ^ 0xFFFF_FFFF
    }
    fn chunk(tag: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        let mut body = tag.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
        out
    }

    // Raw pixel data: one filter byte per scanline, then RGB triples.
    let mut raw = Vec::with_capacity((height * (1 + width * 3)) as usize);
    for y in 0..height {
        raw.push(0u8);
        for x in 0..width {
            raw.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, 0x80]);
        }
    }

    // zlib stream with stored (uncompressed) deflate blocks — avoids depending on a compressor.
    let mut z = vec![0x78, 0x01];
    for (i, part) in raw.chunks(65_535).enumerate() {
        let last = (i + 1) * 65_535 >= raw.len();
        z.push(if last { 1 } else { 0 });
        z.extend_from_slice(&(part.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(part.len() as u16)).to_le_bytes());
        z.extend_from_slice(part);
    }
    // adler32 of the raw data.
    let (mut a, mut b) = (1u32, 0u32);
    for byte in &raw {
        a = (a + *byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut ihdr = width.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend_from_slice(&chunk(b"IHDR", &ihdr));
    out.extend_from_slice(&chunk(b"IDAT", &z));
    out.extend_from_slice(&chunk(b"IEND", b""));
    out
}

fn main() {
    let budgets = budget::Budgets::load();
    let mut failures = Vec::new();

    let dir: PathBuf =
        std::env::temp_dir().join(format!("quill-bench-proxy-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");

    // 24 plates at 1200x900 — enough work to measure, small enough not to dominate CI.
    const COUNT: usize = 24;
    let bytes = png(1200, 900);
    let assets: Vec<Asset> = (0..COUNT)
        .map(|i| {
            let name = format!("plate{i}.png");
            fs::write(dir.join(&name), &bytes).expect("write fixture");
            Asset {
                id: format!("plate{i}"),
                path: name,
                px_w: 1200,
                px_h: 900,
                dpi: 300.0,
                line_art: false,
                has_alpha: false,
            }
        })
        .collect();

    // --- Cold: generate every proxy -----------------------------------------------------------
    let mut cache = ProxyCache::new();
    let start = std::time::Instant::now();
    let report = cache.populate_from_assets(&assets, &dir);
    let cold = start.elapsed();
    assert_eq!(
        report.generated, COUNT,
        "cold pass must generate all proxies"
    );
    let ms_per_image = cold.as_secs_f64() * 1000.0 / COUNT as f64;
    println!(
        "cold populate: {COUNT} images in {:.1} ms ({ms_per_image:.2} ms/image)",
        cold.as_secs_f64() * 1000.0
    );
    budgets.check("proxy_cache.ms_per_image", ms_per_image, &mut failures);

    // --- Warm: nothing changed, so nothing should be re-decoded --------------------------------
    let warm = budget::min_of(3, || {
        let r = cache.populate_from_assets(&assets, &dir);
        assert_eq!(r.generated, 0, "an unchanged asset must not be re-decoded");
    });
    let speedup = cold.as_secs_f64() / warm.as_secs_f64().max(f64::EPSILON);
    println!(
        "warm populate: {:.3} ms  ({speedup:.0}x faster than cold)",
        warm.as_secs_f64() * 1000.0
    );
    // Expressed as a *floor* on the speedup rather than a ceiling on time: the claim spec 0024
    // makes is "skipping is dramatically cheaper than decoding", which is a ratio.
    budgets.check(
        "proxy_cache.warm_speedup_floor",
        1.0 / speedup,
        &mut failures,
    );

    let _ = fs::remove_dir_all(&dir);
    budget::report(failures);
}
