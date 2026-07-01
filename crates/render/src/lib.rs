//! On-screen rendering and the linked-image proxy cache.
//!
//! Real rendering will use a GPU canvas (`skia-safe`, evaluating `vello`). This scaffold
//! implements the proxy-cache *policy* — downsampled screen proxies so full-resolution art is
//! only touched at export — which is central to staying fast on 500-page, image-heavy books.

use std::collections::HashMap;

/// Longest edge, in pixels, for cached on-screen image proxies.
pub const PROXY_MAX_EDGE_PX: u32 = 2048;

/// Tracks which assets have a screen proxy and at what pixel size. (Actual pixel data is
/// attached once the GPU renderer lands.)
#[derive(Debug, Default)]
pub struct ProxyCache {
    proxies: HashMap<String, (u32, u32)>,
}

impl ProxyCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Proxy dimensions for a source image, downscaling so the longest edge is at most
    /// [`PROXY_MAX_EDGE_PX`]. Never upscales; preserves aspect ratio.
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

    /// Record (or replace) the proxy for an asset given its source pixel dimensions.
    pub fn insert(&mut self, asset_id: &str, src_w: u32, src_h: u32) {
        self.proxies
            .insert(asset_id.to_string(), Self::proxy_size(src_w, src_h));
    }

    /// The cached proxy dimensions for an asset, if present.
    pub fn get(&self, asset_id: &str) -> Option<(u32, u32)> {
        self.proxies.get(asset_id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_images_are_downsampled_preserving_aspect() {
        let (w, h) = ProxyCache::proxy_size(8000, 4000);
        assert_eq!(w, PROXY_MAX_EDGE_PX);
        assert_eq!(h, PROXY_MAX_EDGE_PX / 2);
    }

    #[test]
    fn small_images_are_not_upscaled() {
        assert_eq!(ProxyCache::proxy_size(800, 600), (800, 600));
    }

    #[test]
    fn insert_and_get_round_trip() {
        let mut cache = ProxyCache::new();
        cache.insert("hero", 4096, 2048);
        assert_eq!(cache.get("hero"), Some((2048, 1024)));
        assert_eq!(cache.get("missing"), None);
    }
}
