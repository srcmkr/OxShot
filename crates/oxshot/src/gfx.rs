//! Upload of CPU images to GPUI. Large images are split into tiles so they never exceed
//! the sprite atlas limits; every texture is released explicitly when replaced.

use std::sync::Arc;

use gpui_kit::{Bounds, Corners, Pixels, RenderImage, Window, point, px, size};
use image::RgbaImage;
use smallvec::SmallVec;

pub const TILE: u32 = 1024;

pub struct Tile {
    /// Position of the tile inside the source image, in image pixels.
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub image: Arc<RenderImage>,
}

/// An image uploaded as tiles, placed at `origin` in document coordinates.
pub struct TiledImage {
    pub origin: (i32, i32),
    pub width: u32,
    pub height: u32,
    pub tiles: Vec<Tile>,
}

impl TiledImage {
    pub fn new(img: &RgbaImage, origin: (i32, i32)) -> Self {
        let (w, h) = img.dimensions();
        let mut tiles = Vec::new();
        let mut y = 0;
        while y < h {
            let th = TILE.min(h - y);
            let mut x = 0;
            while x < w {
                let tw = TILE.min(w - x);
                tiles.push(Tile { x, y, w: tw, h: th, image: to_render_image(img, x, y, tw, th) });
                x += tw;
            }
            y += th;
        }
        Self { origin, width: w, height: h, tiles }
    }

    /// Paints the image. `to_screen` maps a document point to window pixels; `zoom` is
    /// window pixels per document pixel. `offset` shifts the image in document space.
    pub fn paint(&self, window: &mut Window, clip: Bounds<Pixels>, map: &ViewMap, offset: (f32, f32)) {
        for t in &self.tiles {
            let x = self.origin.0 as f32 + t.x as f32 + offset.0;
            let y = self.origin.1 as f32 + t.y as f32 + offset.1;
            let o = map.to_screen(x, y);
            let b = Bounds { origin: o, size: size(px(t.w as f32 * map.zoom), px(t.h as f32 * map.zoom)) };
            let _ = window.paint_image(clip, b, Corners::default(), t.image.clone(), 0, false);
        }
    }

    /// Frees the GPU textures. Must be called before dropping a replaced image.
    pub fn release(self, window: &mut Window) {
        for t in self.tiles {
            let _ = window.drop_image(t.image);
        }
    }
}

/// Mapping between document pixel coordinates and window coordinates.
#[derive(Clone, Copy, Debug)]
pub struct ViewMap {
    /// Window position of document point (0,0).
    pub origin: gpui_kit::Point<Pixels>,
    pub zoom: f32,
}

impl ViewMap {
    pub fn to_screen(self, x: f32, y: f32) -> gpui_kit::Point<Pixels> {
        point(self.origin.x + px(x * self.zoom), self.origin.y + px(y * self.zoom))
    }
    pub fn to_doc(self, p: gpui_kit::Point<Pixels>) -> oxshot_core::Point {
        oxshot_core::Point::new(f32::from(p.x - self.origin.x) / self.zoom, f32::from(p.y - self.origin.y) / self.zoom)
    }
}

/// Converts a region of a straight-RGBA image into a GPUI image (BGRA, straight alpha:
/// the GPUI sprite shader premultiplies itself).
pub fn to_render_image(img: &RgbaImage, x0: u32, y0: u32, w: u32, h: u32) -> Arc<RenderImage> {
    let stride = img.width() as usize * 4;
    let src = img.as_raw();
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in y0..y0 + h {
        let row = &src[y as usize * stride + x0 as usize * 4..y as usize * stride + (x0 + w) as usize * 4];
        let (pixels, _) = row.as_chunks::<4>();
        for [r, g, b, a] in pixels {
            out.extend_from_slice(&[*b, *g, *r, *a]);
        }
    }
    let buf = image::RgbaImage::from_raw(w, h, out).expect("buffer size matches");
    let frames: SmallVec<[image::Frame; 1]> = SmallVec::from_elem(image::Frame::new(buf), 1);
    Arc::new(RenderImage::new(frames))
}
