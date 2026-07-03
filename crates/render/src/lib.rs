//! On-screen rendering and the linked-image proxy cache.
//!
//! Real rendering will use a GPU canvas (`skia-safe`, evaluating `vello`). This scaffold implements
//! the proxy-cache *policy* and now the real **proxy pixels** — decoded, downsampled screen proxies
//! so full-resolution art is only touched at export — which is central to staying fast on 500-page,
//! image-heavy books. PNG proxies land first (spec 0021); JPEG/other formats are later increments.

use std::collections::HashMap;

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

/// Holds the decoded, downsampled screen proxy for each linked asset, so the on-screen renderer
/// composites small proxies instead of full-resolution art.
#[derive(Debug, Default)]
pub struct ProxyCache {
    proxies: HashMap<String, Proxy>,
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
                self.proxies.insert(asset_id.to_string(), proxy);
                true
            }
            None => false,
        }
    }

    /// The cached proxy for an asset, if present.
    pub fn get(&self, asset_id: &str) -> Option<&Proxy> {
        self.proxies.get(asset_id)
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
        let png = encode_png(16, 16, png::ColorType::Rgba, &vec![10u8; 16 * 16 * 4]);
        let mut cache = ProxyCache::new();
        assert!(cache.insert_png("hero", &png));
        let proxy = cache.get("hero").expect("cached");
        assert_eq!((proxy.width, proxy.height), (16, 16));
        assert_eq!(proxy.rgba.len(), 16 * 16 * 4);
        assert!(cache.get("missing").is_none());
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
