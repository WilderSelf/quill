//! On-screen rendering and the linked-image proxy cache.
//!
//! Real rendering will use a GPU canvas (`skia-safe`, evaluating `vello`). This scaffold implements
//! the proxy-cache *policy* and now the real **proxy pixels** — decoded, downsampled screen proxies
//! so full-resolution art is only touched at export — which is central to staying fast on 500-page,
//! image-heavy books. PNG proxies land first (spec 0021); JPEG/other formats are later increments.

use quill_core_model::Asset;
use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

/// Longest edge, in pixels, for cached on-screen image proxies.
pub const PROXY_MAX_EDGE_PX: u32 = 2048;

/// A decoded, downsampled screen proxy: RGBA8 pixels at the proxy dimensions. The GPU renderer
/// uploads `rgba` as a texture; nothing here touches full-resolution art after generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proxy {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, row-major, non-premultiplied RGBA8.
    pub rgba: Vec<u8>,
}

/// Proxy dimensions for a source image, downscaling so the longest edge is at most
/// [`PROXY_MAX_EDGE_PX`]. Never upscales; preserves aspect ratio. Shared sizing policy, also used by
/// [`decode_png_proxy`].
pub fn proxy_size(src_w: u32, src_h: u32) -> (u32, u32) {
    let longest = src_w.max(src_h);
    if longest <= PROXY_MAX_EDGE_PX || longest == 0 {
        return (src_w, src_h);
    }
    let scale = PROXY_MAX_EDGE_PX as f32 / longest as f32;
    (
        ((src_w as f32 * scale).round() as u32).max(1),
        ((src_h as f32 * scale).round() as u32).max(1),
    )
}

/// Decode PNG `bytes` and downsample to a screen [`Proxy`] whose longest edge is at most
/// [`PROXY_MAX_EDGE_PX`]. Returns `None` on any decode failure — a missing/corrupt screen proxy is
/// recoverable (skip and show nothing), unlike a press export, so we never panic here.
///
/// The PNG is normalized to 8-bit (`normalize_to_color8` expands indexed→RGB and strips 16-bit),
/// then any of Grayscale / GrayscaleAlpha / RGB / RGBA is widened to RGBA8 before an area-average
/// downscale to [`proxy_size`].
pub fn decode_png_proxy(bytes: &[u8]) -> Option<Proxy> {
    let src = decode_png_rgba(bytes)?;
    Some(downsample_rgba(&src))
}

/// A full-resolution decoded PNG as RGBA8, carrying its source dimensions.
struct Rgba8 {
    width: u32,
    height: u32,
    pixels: Vec<u8>, // width * height * 4
}

/// Decode PNG bytes to full-resolution RGBA8, or `None` on any decode error.
fn decode_png_rgba(bytes: &[u8]) -> Option<Rgba8> {
    use png::ColorType;

    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    // Expands indexed→RGB(A) and low-bit/16-bit samples to 8-bit, so only the four 8-bit color
    // types below reach the match (mirrors export-pdf's decode).
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;

    if info.bit_depth != png::BitDepth::Eight {
        return None; // defensive: normalization already forces 8-bit output.
    }
    let (w, h) = (info.width, info.height);
    let px = (w as usize) * (h as usize);
    let data = &buf[..info.buffer_size()];

    // Widen whatever channels the PNG carries to non-premultiplied RGBA8.
    let pixels: Vec<u8> = match info.color_type {
        ColorType::Grayscale => data[..px].iter().flat_map(|&g| [g, g, g, 255]).collect(),
        ColorType::GrayscaleAlpha => data
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        ColorType::Rgb => data
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        ColorType::Rgba => data[..px * 4].to_vec(),
        ColorType::Indexed => return None, // defensive: EXPAND already turned palette into RGB(A).
    };
    debug_assert_eq!(pixels.len(), px * 4);
    Some(Rgba8 {
        width: w,
        height: h,
        pixels,
    })
}

/// Decode baseline/progressive JPEG `bytes` and downsample to a screen [`Proxy`]. Returns `None` on
/// any decode failure — a missing/corrupt screen proxy is recoverable (skip and show nothing),
/// unlike a press export, so we never panic here.
///
/// `L8` grayscale and `RGB24` are widened to RGBA8 and downsampled via the shared [`downsample_rgba`].
/// `CMYK32` and `L16` are skipped (`None`): a CMYK JPEG is the ambiguity minefield `export-pdf`
/// documents (Adobe transform / YCCK inversion), so a color-correct screen proxy for it is deferred
/// to a later color-managed increment (a named non-goal of spec 0022).
pub fn decode_jpeg_proxy(bytes: &[u8]) -> Option<Proxy> {
    let src = decode_jpeg_rgba(bytes)?;
    Some(downsample_rgba(&src))
}

/// Decode PNG or JPEG image `bytes` into a screen [`Proxy`], dispatched on the leading magic bytes
/// (mirrors `export-pdf`'s `decode`): `\x89PNG…` → [`decode_png_proxy`], `\xFF\xD8\xFF` →
/// [`decode_jpeg_proxy`]. Returns `None` for unknown/unsupported formats or any decode failure — a
/// missing screen proxy is recoverable (skip, don't panic). Dispatch is by content, not by any
/// filename extension.
pub fn decode_image_proxy(bytes: &[u8]) -> Option<Proxy> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        decode_png_proxy(bytes)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        decode_jpeg_proxy(bytes)
    } else {
        None
    }
}

/// Decode JPEG bytes to full-resolution RGBA8, or `None` on a decode error or an unhandled pixel
/// format (`CMYK32` / `L16` — deferred, see [`decode_jpeg_proxy`]).
fn decode_jpeg_rgba(bytes: &[u8]) -> Option<Rgba8> {
    use jpeg_decoder::PixelFormat;

    let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
    let data = decoder.decode().ok()?;
    let info = decoder.info()?; // populated once decode() succeeds
    let (w, h) = (info.width as u32, info.height as u32);

    let pixels: Vec<u8> = match info.pixel_format {
        PixelFormat::L8 => data.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        PixelFormat::RGB24 => data
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        // CMYK JPEGs are ambiguous (see spec 0012) and 16-bit gray is uncommon; both are deferred to
        // a later color-managed proxy increment rather than shown in a wrong/approximate color.
        PixelFormat::CMYK32 | PixelFormat::L16 => return None,
    };
    debug_assert_eq!(pixels.len(), (w as usize) * (h as usize) * 4);
    Some(Rgba8 {
        width: w,
        height: h,
        pixels,
    })
}

/// Area-average downscale of a full-resolution RGBA8 image to its [`proxy_size`]. Each target pixel
/// is the mean of the source pixels in its cell `[tx*sw/tw .. (tx+1)*sw/tw) × [ty*sh/th ..]`. Since
/// `proxy_size` never upscales, every cell covers ≥ 1 source pixel; when no downscale is needed the
/// output equals the input.
fn downsample_rgba(src: &Rgba8) -> Proxy {
    let (sw, sh) = (src.width, src.height);
    let (tw, th) = proxy_size(sw, sh);

    // No downscale (or a degenerate/empty image): pass the pixels through unchanged.
    if tw == sw && th == sh {
        return Proxy {
            width: tw,
            height: th,
            rgba: src.pixels.clone(),
        };
    }

    let mut out = vec![0u8; (tw as usize) * (th as usize) * 4];
    for ty in 0..th {
        // Source row span for this target row (half-open, always ≥ 1 row wide).
        let y0 = (ty as u64 * sh as u64 / th as u64) as u32;
        let y1 = (((ty + 1) as u64 * sh as u64 / th as u64) as u32).max(y0 + 1);
        for tx in 0..tw {
            let x0 = (tx as u64 * sw as u64 / tw as u64) as u32;
            let x1 = (((tx + 1) as u64 * sw as u64 / tw as u64) as u32).max(x0 + 1);

            let (mut r, mut g, mut b, mut a) = (0u64, 0u64, 0u64, 0u64);
            let mut n = 0u64;
            for sy in y0..y1 {
                let row = (sy as usize) * (sw as usize) * 4;
                for sx in x0..x1 {
                    let i = row + (sx as usize) * 4;
                    r += src.pixels[i] as u64;
                    g += src.pixels[i + 1] as u64;
                    b += src.pixels[i + 2] as u64;
                    a += src.pixels[i + 3] as u64;
                    n += 1;
                }
            }
            let o = ((ty as usize) * (tw as usize) + tx as usize) * 4;
            out[o] = (r / n) as u8;
            out[o + 1] = (g / n) as u8;
            out[o + 2] = (b / n) as u8;
            out[o + 3] = (a / n) as u8;
        }
    }
    Proxy {
        width: tw,
        height: th,
        rgba: out,
    }
}

/// A cheap signature of a linked source file — `mtime + size` from a single `stat`, never the file
/// body. `populate_from_assets` reuses a cached proxy when the file's current signature still equals
/// the one stored when the proxy was generated, so re-populating a 500-page doc is hundreds of
/// `stat`s, not hundreds of decodes. `mtime + size` deliberately misses a same-size, same-mtime
/// in-place edit — content-hash invalidation is a named follow-up (spec 0024 non-goal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SourceSig {
    mtime: SystemTime,
    len: u64,
}

/// The `mtime + size` signature of `path`, or `None` if it can't be `stat`ed / has no modification
/// time (missing file, or a platform without mtime). A `None` never equals a stored signature, so
/// the asset is treated as changed and regenerated.
fn source_sig(path: &Path) -> Option<SourceSig> {
    let md = std::fs::metadata(path).ok()?;
    Some(SourceSig {
        mtime: md.modified().ok()?,
        len: md.len(),
    })
}

/// A cached proxy plus the source signature it was generated from. Byte-fed inserts
/// (`insert_png` / `insert_jpeg` / `insert_image`) carry no path, so their `sig` is `None`.
#[derive(Debug, Clone)]
struct CacheEntry {
    proxy: Proxy,
    sig: Option<SourceSig>,
}

/// Outcome counts for one [`ProxyCache::populate_from_assets`] pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PopulateReport {
    /// Assets decoded fresh this call — new, or their source file changed since last cached.
    pub generated: usize,
    /// Assets whose cached proxy was reused because the source file was unchanged (no decode).
    pub reused: usize,
    /// Assets with no proxy this call — missing, unreadable, or unsupported/undecodable. Any proxy
    /// cached for that id on a prior call is left intact (a vanished link shows its last-known art).
    pub skipped: usize,
}

/// Holds the decoded, downsampled screen proxy for each linked asset, so the on-screen renderer
/// composites small proxies instead of full-resolution art.
#[derive(Debug, Default)]
pub struct ProxyCache {
    proxies: HashMap<String, CacheEntry>,
}

impl ProxyCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode + downsample PNG `bytes` and cache the resulting [`Proxy`] under `asset_id`. Returns
    /// `false` (and stores nothing) when the PNG can't be decoded, so a bad asset can't evict or
    /// corrupt a previously cached proxy.
    pub fn insert_png(&mut self, asset_id: &str, bytes: &[u8]) -> bool {
        match decode_png_proxy(bytes) {
            Some(proxy) => {
                self.proxies
                    .insert(asset_id.to_string(), CacheEntry { proxy, sig: None });
                true
            }
            None => false,
        }
    }

    /// Decode + downsample JPEG `bytes` and cache the resulting [`Proxy`] under `asset_id`. Returns
    /// `false` (and stores nothing) when the JPEG can't be decoded or is an unhandled pixel format
    /// (CMYK / 16-bit), so a bad asset can't evict or corrupt a previously cached proxy.
    pub fn insert_jpeg(&mut self, asset_id: &str, bytes: &[u8]) -> bool {
        match decode_jpeg_proxy(bytes) {
            Some(proxy) => {
                self.proxies
                    .insert(asset_id.to_string(), CacheEntry { proxy, sig: None });
                true
            }
            None => false,
        }
    }

    /// Sniff `bytes` (PNG or JPEG, by magic bytes) and cache the resulting [`Proxy`] under
    /// `asset_id`. Returns `false` (and stores nothing) when the bytes aren't a supported, decodable
    /// image, so a bad asset can't evict or corrupt a previously cached proxy.
    pub fn insert_image(&mut self, asset_id: &str, bytes: &[u8]) -> bool {
        match decode_image_proxy(bytes) {
            Some(proxy) => {
                self.proxies
                    .insert(asset_id.to_string(), CacheEntry { proxy, sig: None });
                true
            }
            None => false,
        }
    }

    /// Read and cache a screen proxy for each asset, resolving `asset.path` against `base_dir` (the
    /// document's asset root), **skipping the decode for any asset whose source file is unchanged**
    /// since it was last cached (by `mtime + size`; see [`SourceSig`]). Each proxy is keyed by the
    /// asset's `id`.
    ///
    /// Missing, unreadable, or unsupported files are **skipped, not fatal** — a broken image link
    /// must not abort loading a 500-page document, and does not evict a proxy cached on a prior
    /// call. Returns a [`PopulateReport`] partitioning the assets into generated / reused / skipped.
    pub fn populate_from_assets(&mut self, assets: &[Asset], base_dir: &Path) -> PopulateReport {
        let mut report = PopulateReport::default();
        for asset in assets {
            let path = base_dir.join(&asset.path);
            let sig = source_sig(&path);

            // Reuse only when the file's current signature reads back byte-identical to the one the
            // cached proxy was generated from. A `None` signature (unstat-able, or a byte-fed entry)
            // never matches, so the asset falls through to a (re)decode.
            if let (Some(sig), Some(entry)) = (sig, self.proxies.get(&asset.id)) {
                if entry.sig == Some(sig) {
                    report.reused += 1;
                    continue;
                }
            }

            let Ok(bytes) = std::fs::read(&path) else {
                report.skipped += 1; // missing / unreadable link — skip, don't fail the load
                continue;
            };
            match decode_image_proxy(&bytes) {
                Some(proxy) => {
                    self.proxies
                        .insert(asset.id.clone(), CacheEntry { proxy, sig });
                    report.generated += 1;
                }
                None => report.skipped += 1, // unsupported / undecodable — leave any prior proxy
            }
        }
        report
    }

    /// The cached proxy for an asset, if present.
    pub fn get(&self, asset_id: &str) -> Option<&Proxy> {
        self.proxies.get(asset_id).map(|entry| &entry.proxy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode pixels as a PNG in-memory using the workspace's `png` encoder, so tests need no
    /// committed fixtures (CLAUDE.md: synthesize in-memory when the encoder is already a dep).
    fn encode_png(width: u32, height: u32, color: png::ColorType, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, width, height);
            enc.set_color(color);
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc.write_header().unwrap();
            writer.write_image_data(data).unwrap();
        }
        out
    }

    #[test]
    fn large_rgba_png_downsamples_to_proxy_dims() {
        // 4096×2048 RGBA → longest edge capped at 2048 → 2048×1024, full RGBA8 buffer.
        let (w, h) = (4096u32, 2048u32);
        let data = vec![128u8; (w * h * 4) as usize];
        let png = encode_png(w, h, png::ColorType::Rgba, &data);

        let proxy = decode_png_proxy(&png).expect("decodes");
        assert_eq!((proxy.width, proxy.height), (2048, 1024));
        assert_eq!(proxy.rgba.len(), (2048 * 1024 * 4) as usize);
        // A uniform source averages to the same uniform value.
        assert!(proxy.rgba.iter().all(|&b| b == 128));
    }

    #[test]
    fn small_png_is_not_upscaled_and_pixels_are_preserved() {
        // 8×8 RGB (< 2048) stays native size; each pixel widened to RGBA with A=255.
        let (w, h) = (8u32, 8u32);
        let rgb: Vec<u8> = (0..w * h)
            .flat_map(|i| [i as u8, (2 * i) as u8, (3 * i) as u8])
            .collect();
        let png = encode_png(w, h, png::ColorType::Rgb, &rgb);

        let proxy = decode_png_proxy(&png).expect("decodes");
        assert_eq!((proxy.width, proxy.height), (8, 8));
        assert_eq!(proxy.rgba.len(), (8 * 8 * 4) as usize);
        // Identity (no downscale) preserves pixels: i=0 → (0,0,0,255); i=1 → (1,2,3,255).
        assert_eq!(&proxy.rgba[0..4], &[0, 0, 0, 255]);
        assert_eq!(&proxy.rgba[4..8], &[1, 2, 3, 255]);
    }

    #[test]
    fn downscale_averages_straddling_pixels() {
        // A 4096×1 row: source pixels 0..2047 are 0, 2047..4096 are 200. Downscaling to width 2048
        // makes each target pixel x cover source [2x, 2x+2). Target pixel 1023 covers source
        // {2046, 2047} = {0, 200} and must average to 100 — proving the kernel means differing
        // pixels, not just passes clean blocks through. Interior pixels stay a clean 0 / 200.
        let w = 4096u32;
        let mut rgb = vec![0u8; (w * 3) as usize];
        for x in 2047..w {
            let i = (x * 3) as usize;
            rgb[i] = 200;
            rgb[i + 1] = 200;
            rgb[i + 2] = 200;
        }
        let png = encode_png(w, 1, png::ColorType::Rgb, &rgb);
        let proxy = decode_png_proxy(&png).expect("decodes");
        assert_eq!((proxy.width, proxy.height), (2048, 1));

        assert_eq!(proxy.rgba[0], 0, "leftmost target pixel (src 0,1 both 0)");
        assert_eq!(
            proxy.rgba[1023 * 4],
            100,
            "straddling target pixel averages 0 and 200"
        );
        let last = ((proxy.width - 1) * 4) as usize;
        assert_eq!(proxy.rgba[last], 200, "rightmost target pixel (both 200)");
    }

    #[test]
    fn non_integer_downscale_ratio_is_monotonic_and_safe() {
        // A 3000-wide 0..255 gradient downscales to 2048 at a non-integer ratio (3000/2048 ≈ 1.46),
        // exercising the varying cell sizes and remainder-absorbing last cell the 2× tests miss. The
        // area-averaged target row must stay ordered (a monotone source stays monotone under a mean)
        // and span nearly the full range — a cheap invariant that locks in the partition behavior
        // without hand-computing every cell.
        let w = 3000u32;
        let mut rgb = vec![0u8; (w * 3) as usize];
        for x in 0..w {
            let v = (x * 255 / (w - 1)) as u8;
            let i = (x * 3) as usize;
            rgb[i] = v;
            rgb[i + 1] = v;
            rgb[i + 2] = v;
        }
        let png = encode_png(w, 1, png::ColorType::Rgb, &rgb);
        let proxy = decode_png_proxy(&png).expect("decodes");
        assert_eq!((proxy.width, proxy.height), (2048, 1));

        let reds: Vec<u8> = proxy.rgba.chunks_exact(4).map(|p| p[0]).collect();
        assert!(
            reds.windows(2).all(|w| w[0] <= w[1]),
            "a monotone gradient stays monotone through the area-average"
        );
        assert!(reds[0] <= 2, "left edge near 0, got {}", reds[0]);
        assert!(
            *reds.last().unwrap() >= 253,
            "right edge near 255, got {}",
            reds.last().unwrap()
        );
    }

    #[test]
    fn no_downscale_is_identity_passthrough() {
        // Below the cap, downsample_rgba returns the source pixels verbatim.
        let src = Rgba8 {
            width: 2,
            height: 2,
            pixels: vec![
                0, 10, 0, 255, 100, 20, 0, 255, // row 0
                200, 30, 0, 255, 40, 40, 0, 255, // row 1
            ],
        };
        let proxy = downsample_rgba(&src);
        assert_eq!((proxy.width, proxy.height), (2, 2));
        assert_eq!(proxy.rgba, src.pixels);
    }

    #[test]
    fn grayscale_png_widens_to_opaque_rgba() {
        // Grayscale decodes to RGBA with R==G==B and A==255.
        let gray: Vec<u8> = (0..64u32).map(|i| i as u8).collect();
        let png = encode_png(8, 8, png::ColorType::Grayscale, &gray);
        let proxy = decode_png_proxy(&png).expect("decodes");
        assert_eq!((proxy.width, proxy.height), (8, 8));
        for px in proxy.rgba.chunks_exact(4) {
            assert_eq!(px[0], px[1], "R==G");
            assert_eq!(px[1], px[2], "G==B");
            assert_eq!(px[3], 255, "opaque");
        }
    }

    #[test]
    fn corrupt_png_is_skipped_not_fatal() {
        assert!(decode_png_proxy(b"not a real png at all").is_none());
        let mut cache = ProxyCache::new();
        assert!(!cache.insert_png("bad", b"garbage"));
        assert!(cache.get("bad").is_none());
    }

    #[test]
    fn cache_insert_and_get_round_trip() {
        let png = encode_png(16, 16, png::ColorType::Rgba, &[10u8; 16 * 16 * 4]);
        let mut cache = ProxyCache::new();
        assert!(cache.insert_png("hero", &png));
        let proxy = cache.get("hero").expect("cached");
        assert_eq!((proxy.width, proxy.height), (16, 16));
        assert_eq!(proxy.rgba.len(), 16 * 16 * 4);
        assert!(cache.get("missing").is_none());
    }

    // Tiny 8×8 JPEG fixtures (JPEG is lossy + `jpeg-decoder` is decode-only, so — unlike the PNG
    // tests — these are committed rather than synthesized in-memory; copied from export-pdf/assets).
    const TEST_JPEG_GRAY: &[u8] = include_bytes!("../assets/test_gray.jpg");
    const TEST_JPEG_RGB: &[u8] = include_bytes!("../assets/test_rgb.jpg");
    const TEST_JPEG_CMYK: &[u8] = include_bytes!("../assets/test_cmyk.jpg");

    #[test]
    fn rgb_jpeg_decodes_to_opaque_rgba_proxy() {
        let proxy = decode_jpeg_proxy(TEST_JPEG_RGB).expect("decodes rgb jpeg");
        assert_eq!((proxy.width, proxy.height), (8, 8)); // < 2048 → native size
        assert_eq!(proxy.rgba.len(), 8 * 8 * 4);
        for px in proxy.rgba.chunks_exact(4) {
            assert_eq!(px[3], 255, "opaque");
        }
    }

    #[test]
    fn grayscale_jpeg_widens_to_opaque_rgba() {
        let proxy = decode_jpeg_proxy(TEST_JPEG_GRAY).expect("decodes gray jpeg");
        assert_eq!((proxy.width, proxy.height), (8, 8));
        for px in proxy.rgba.chunks_exact(4) {
            assert_eq!(px[0], px[1], "R==G");
            assert_eq!(px[1], px[2], "G==B");
            assert_eq!(px[3], 255, "opaque");
        }
    }

    #[test]
    fn cmyk_jpeg_is_skipped_not_miscolored() {
        // CMYK JPEGs are the spec-0012 ambiguity minefield → deferred, not shown in a wrong color.
        // This same fixture is proven to decode as genuine CMYK32 (not an error) by export-pdf's
        // `decodes_transform0_cmyk_jpeg_to_clamped_cmyk` test, so the `None` here is the deliberate
        // CMYK32-skip arm — not a decode failure masquerading as a skip.
        assert!(decode_jpeg_proxy(TEST_JPEG_CMYK).is_none());
        let mut cache = ProxyCache::new();
        assert!(!cache.insert_jpeg("cmyk", TEST_JPEG_CMYK));
        assert!(cache.get("cmyk").is_none());
    }

    #[test]
    fn corrupt_jpeg_is_skipped_not_fatal() {
        assert!(decode_jpeg_proxy(b"\xff\xd8not a real jpeg").is_none());
    }

    #[test]
    fn cache_insert_jpeg_and_get_round_trip() {
        let mut cache = ProxyCache::new();
        assert!(cache.insert_jpeg("photo", TEST_JPEG_RGB));
        let proxy = cache.get("photo").expect("cached");
        assert_eq!((proxy.width, proxy.height), (8, 8));
        assert_eq!(proxy.rgba.len(), 8 * 8 * 4);
    }

    /// A unique-per-process temp dir for a test, so a stale dir from an aborted run or a concurrent
    /// runner can't collide (the process id disambiguates; `name` disambiguates within a process).
    fn temp_dir(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("quill_render_0023_{name}_{}", std::process::id()))
    }

    /// A minimal `Asset` referencing `path` under an id; only id/path matter for proxy generation.
    fn asset(id: &str, path: &str) -> Asset {
        Asset {
            id: id.into(),
            path: path.into(),
            px_w: 0,
            px_h: 0,
            dpi: 0.0,
            line_art: false,
            has_alpha: false,
        }
    }

    #[test]
    fn decode_image_proxy_dispatches_on_magic_bytes() {
        let png = encode_png(4, 4, png::ColorType::Rgba, &[7u8; 4 * 4 * 4]);
        assert!(
            decode_image_proxy(&png).is_some(),
            "PNG signature → png proxy"
        );
        assert!(
            decode_image_proxy(TEST_JPEG_RGB).is_some(),
            "JPEG SOI → jpeg proxy"
        );
        assert!(
            decode_image_proxy(b"GIF89a not supported").is_none(),
            "unknown magic → None"
        );
    }

    #[test]
    fn insert_image_sniffs_and_round_trips() {
        let png = encode_png(5, 6, png::ColorType::Rgb, &[3u8; 5 * 6 * 3]);
        let mut cache = ProxyCache::new();
        assert!(cache.insert_image("a", &png));
        assert_eq!(cache.get("a").map(|p| (p.width, p.height)), Some((5, 6)));
        assert!(!cache.insert_image("b", b"not an image"));
        assert!(cache.get("b").is_none());

        // Re-inserting the same id replaces the cached proxy (spec: "re-running replaces entries").
        let bigger = encode_png(9, 9, png::ColorType::Rgb, &[4u8; 9 * 9 * 3]);
        assert!(cache.insert_image("a", &bigger));
        assert_eq!(cache.get("a").map(|p| (p.width, p.height)), Some((9, 9)));
    }

    #[test]
    fn populate_from_assets_reads_real_files() {
        // Write a real PNG and a real JPEG into a temp dir; two assets reference them by relative
        // path. populate_from_assets must read both and cache a proxy keyed by each asset id.
        let dir = temp_dir("populate_ok");
        std::fs::create_dir_all(&dir).unwrap();
        let png = encode_png(12, 10, png::ColorType::Rgba, &[9u8; 12 * 10 * 4]);
        std::fs::write(dir.join("map.png"), &png).unwrap();
        std::fs::write(dir.join("photo.jpg"), TEST_JPEG_RGB).unwrap();

        let assets = [asset("map", "map.png"), asset("photo", "photo.jpg")];
        let mut cache = ProxyCache::new();
        let report = cache.populate_from_assets(&assets, &dir);

        assert_eq!(report.generated, 2, "both linked images generate a proxy");
        assert_eq!(report.reused, 0);
        assert_eq!(report.skipped, 0);
        assert_eq!(
            cache.get("map").map(|p| (p.width, p.height)),
            Some((12, 10))
        );
        assert_eq!(
            cache.get("photo").map(|p| (p.width, p.height)),
            Some((8, 8))
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn populate_from_assets_skips_missing_and_unsupported() {
        // One good PNG, one missing file, one non-image file. Only the good one is counted/cached;
        // no panic on the broken links (a broken link must not abort loading a 500-page doc).
        let dir = temp_dir("populate_skip");
        std::fs::create_dir_all(&dir).unwrap();
        let png = encode_png(4, 4, png::ColorType::Rgb, &[1u8; 4 * 4 * 3]);
        std::fs::write(dir.join("ok.png"), &png).unwrap();
        std::fs::write(dir.join("notes.txt"), b"this is not an image").unwrap();

        let assets = [
            asset("ok", "ok.png"),
            asset("gone", "does-not-exist.png"),
            asset("text", "notes.txt"),
        ];
        let mut cache = ProxyCache::new();
        let report = cache.populate_from_assets(&assets, &dir);

        assert_eq!(
            report.generated, 1,
            "only the readable, decodable asset counts"
        );
        assert_eq!(report.skipped, 2, "missing + non-image are both skipped");
        assert_eq!(report.reused, 0);
        assert!(cache.get("ok").is_some());
        assert!(cache.get("gone").is_none(), "missing file skipped");
        assert!(cache.get("text").is_none(), "non-image skipped");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unchanged_assets_are_reused_not_redecoded() {
        // Two untouched files, populated twice. The second pass must reuse both by signature —
        // generated == 0, reused == 2 — with the proxies still available.
        let dir = temp_dir("reuse");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("a.png"),
            encode_png(6, 6, png::ColorType::Rgba, &[9u8; 6 * 6 * 4]),
        )
        .unwrap();
        std::fs::write(dir.join("b.jpg"), TEST_JPEG_RGB).unwrap();

        let assets = [asset("a", "a.png"), asset("b", "b.jpg")];
        let mut cache = ProxyCache::new();

        let first = cache.populate_from_assets(&assets, &dir);
        assert_eq!(first.generated, 2);

        let second = cache.populate_from_assets(&assets, &dir);
        assert_eq!(second.reused, 2, "unchanged files reused, not re-decoded");
        assert_eq!(second.generated, 0);
        assert_eq!(second.skipped, 0);
        assert!(cache.get("a").is_some());
        assert!(cache.get("b").is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn changed_asset_is_regenerated_sibling_reused() {
        // Overwrite one file with a different-DIMENSION image: the byte length changes, so the
        // signature differs regardless of mtime granularity — a deterministic "changed" signal. The
        // untouched sibling must be reused in the same pass, and the changed proxy reflect new dims.
        let dir = temp_dir("change");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("hero.png"),
            encode_png(4, 4, png::ColorType::Rgba, &[1u8; 4 * 4 * 4]),
        )
        .unwrap();
        std::fs::write(
            dir.join("bg.png"),
            encode_png(5, 5, png::ColorType::Rgba, &[2u8; 5 * 5 * 4]),
        )
        .unwrap();

        let assets = [asset("hero", "hero.png"), asset("bg", "bg.png")];
        let mut cache = ProxyCache::new();
        assert_eq!(cache.populate_from_assets(&assets, &dir).generated, 2);
        assert_eq!(cache.get("hero").map(|p| (p.width, p.height)), Some((4, 4)));

        // Change only hero, to a clearly different size (16×20 ≠ 4×4 in byte length).
        std::fs::write(
            dir.join("hero.png"),
            encode_png(16, 20, png::ColorType::Rgba, &[3u8; 16 * 20 * 4]),
        )
        .unwrap();

        let report = cache.populate_from_assets(&assets, &dir);
        assert_eq!(report.generated, 1, "only the changed file is re-decoded");
        assert_eq!(report.reused, 1, "the untouched sibling is reused");
        assert_eq!(
            cache.get("hero").map(|p| (p.width, p.height)),
            Some((16, 20)),
            "the regenerated proxy reflects the new source dimensions"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_asset_between_calls_is_generated_others_reused() {
        // A second populate that adds a new asset generates only the newcomer; carried-over assets
        // are reused.
        let dir = temp_dir("newcomer");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("one.png"),
            encode_png(4, 4, png::ColorType::Rgb, &[1u8; 4 * 4 * 3]),
        )
        .unwrap();
        let mut cache = ProxyCache::new();
        assert_eq!(
            cache
                .populate_from_assets(&[asset("one", "one.png")], &dir)
                .generated,
            1
        );

        std::fs::write(
            dir.join("two.png"),
            encode_png(7, 7, png::ColorType::Rgb, &[2u8; 7 * 7 * 3]),
        )
        .unwrap();
        let report =
            cache.populate_from_assets(&[asset("one", "one.png"), asset("two", "two.png")], &dir);
        assert_eq!(report.generated, 1, "only the new asset is decoded");
        assert_eq!(report.reused, 1, "the carried-over asset is reused");
        assert!(cache.get("two").is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn byte_fed_entry_is_regenerated_by_a_later_populate() {
        // A byte-fed insert (`insert_png`) stores `sig: None`, which never equals a file's real
        // signature — so a later `populate_from_assets` for the same id must *generate* (read from
        // disk), not falsely reuse the signature-less entry. Locks the invariant the spec calls out.
        let dir = temp_dir("bytefed");
        std::fs::create_dir_all(&dir).unwrap();
        let mut cache = ProxyCache::new();
        assert!(cache.insert_png(
            "hero",
            &encode_png(3, 3, png::ColorType::Rgb, &[1u8; 3 * 3 * 3])
        ));

        // The on-disk file for the same id is a different size, so a false reuse would be visible.
        std::fs::write(
            dir.join("hero.png"),
            encode_png(11, 9, png::ColorType::Rgb, &[2u8; 11 * 9 * 3]),
        )
        .unwrap();

        let report = cache.populate_from_assets(&[asset("hero", "hero.png")], &dir);
        assert_eq!(
            report.generated, 1,
            "byte-fed (sig: None) entry is regenerated, not reused"
        );
        assert_eq!(report.reused, 0);
        assert_eq!(
            cache.get("hero").map(|p| (p.width, p.height)),
            Some((11, 9)),
            "the file-backed proxy replaced the byte-fed one"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn vanished_link_is_skipped_without_evicting_prior_proxy() {
        // A file cached on the first pass, then deleted, must be counted `skipped` on the next pass
        // — but its previously cached proxy is left intact (show last-known art, don't blank it).
        let dir = temp_dir("vanish");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("art.png");
        std::fs::write(
            &file,
            encode_png(8, 8, png::ColorType::Rgb, &[5u8; 8 * 8 * 3]),
        )
        .unwrap();

        let assets = [asset("art", "art.png")];
        let mut cache = ProxyCache::new();
        assert_eq!(cache.populate_from_assets(&assets, &dir).generated, 1);

        std::fs::remove_file(&file).unwrap();
        let report = cache.populate_from_assets(&assets, &dir);
        assert_eq!(report.skipped, 1, "the vanished link is skipped");
        assert_eq!(report.generated, 0);
        assert_eq!(report.reused, 0);
        assert!(
            cache.get("art").is_some(),
            "the prior proxy survives a vanished link"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn proxy_size_caps_longest_edge_and_never_upscales() {
        assert_eq!(
            proxy_size(8000, 4000),
            (PROXY_MAX_EDGE_PX, PROXY_MAX_EDGE_PX / 2)
        );
        assert_eq!(proxy_size(800, 600), (800, 600));
        assert_eq!(proxy_size(0, 0), (0, 0));
    }
}
