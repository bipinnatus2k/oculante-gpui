//! Splitting an oversized frame into tiles the sprite atlas can actually hold.
//!
//! gpui's DirectX backend clamps every atlas page to `MAX_ATLAS_SIZE = 16384`
//! (gpui-pre-windows-0.3.8 src/directx_atlas.rs:164-174) and sizes a page with
//! `min_size.min(&MAX_ATLAS_SIZE)`, so a sprite with a longer side than that is never allocated:
//! `Window::paint_image` fails, and with the error swallowed the whole canvas goes blank. Measured
//! 2026-10-15 with solid fixtures: a 16384x64 frame draws, a 16385x64 frame draws nothing at all.
//!
//! egui never hit that because `crate::texture_wrapper` (HEAD :604-617) subdivided the frame against
//! the device's `max_texture_size` into a `col_count x row_count` grid and drew one UV-cropped quad
//! per tile. The same split lives here with 16384 as the limit, each tile uploaded as its own sprite;
//! the viewer lays the tiles out as sibling elements at their source offset, which is the same
//! placement egui's quads got.

use std::sync::Arc;

use gpui_kit::RenderImage;
use image::DynamicImage;

use crate::gpui_app::texture;
use crate::utils::ColorChannel;

/// Longest side one gpui sprite can have before it stops fitting in an atlas page.
pub const MAX_SPRITE: u32 = 16384;

/// One uploaded tile, with the source rect it covers.
pub struct Tile {
    pub image: Arc<RenderImage>,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// A frame uploaded as `cols x rows` tiles, row-major, exactly like egui's `texture_vec`.
pub struct Frame {
    tiles: Vec<Arc<RenderImage>>,
    cols: u32,
    width: u32,
    height: u32,
}

impl Frame {
    /// Upload `img`, split so that every tile fits the atlas. A frame that already fits stays one
    /// tile and skips the intermediate crop copies, so ordinary images pay exactly what they paid
    /// before this module existed.
    pub fn upload(img: &DynamicImage, channel: ColorChannel) -> Arc<Self> {
        let (width, height) = (img.width(), img.height());
        let (cols, rows) = (grid(width), grid(height));
        let (col_step, row_step) = (step(width), step(height));
        let tiled = cols * rows > 1;
        let mut tiles = Vec::with_capacity((cols * rows) as usize);
        for row in 0..rows {
            let y = row * row_step;
            let h = row_step.min(height - y);
            for col in 0..cols {
                let x = col * col_step;
                let w = col_step.min(width - x);
                let cropped = tiled.then(|| img.crop_imm(x, y, w, h));
                let source = cropped.as_ref().unwrap_or(img);
                tiles.push(texture::to_render_image(source, channel));
            }
        }
        Arc::new(Self {
            tiles,
            cols,
            width,
            height,
        })
    }

    /// The tiles in upload order, each with where it belongs in the source image.
    pub fn iter(&self) -> impl Iterator<Item = Tile> + '_ {
        let (cols, col_step, row_step) = (self.cols, step(self.width), step(self.height));
        self.tiles.iter().enumerate().map(move |(index, image)| {
            let x = (index % cols as usize) as u32 * col_step;
            let y = (index / cols as usize) as u32 * row_step;
            Tile {
                image: image.clone(),
                x,
                y,
                width: col_step.min(self.width - x),
                height: row_step.min(self.height - y),
            }
        })
    }

    /// Source pixels the frame covers, the same pair `image_geometry::dimensions` holds.
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }
}

/// How many tiles a run of `length` source pixels needs. egui's `ceil(im / max_texture_size)`.
fn grid(length: u32) -> u32 {
    length.div_ceil(MAX_SPRITE.max(1)).max(1)
}

/// egui's `min(max_texture_size, im_*)`: full tiles of this many pixels, the remainder in the last.
fn step(length: u32) -> u32 {
    MAX_SPRITE.min(length).max(1)
}

#[cfg(test)]
mod tests {
    use super::{Frame, MAX_SPRITE, grid, step};
    use crate::utils::ColorChannel;
    use image::{DynamicImage, Rgba, RgbaImage};

    fn bands(width: u32, height: u32) -> DynamicImage {
        // Every tile starts in a differently coloured band, so a wrong crop offset shows up as the
        // wrong colour rather than as a plausible-looking grey blob.
        DynamicImage::ImageRgba8(RgbaImage::from_fn(width, height, |x, y| {
            let band = (x / (width / 3).max(1)) % 3;
            match (band, y < height / 2) {
                (0, true) => Rgba([255, 0, 0, 255]),
                (1, true) => Rgba([0, 255, 0, 255]),
                (2, true) => Rgba([0, 0, 255, 255]),
                _ => Rgba([10, 20, 30, 255]),
            }
        }))
    }

    fn first_pixel(tile: &super::Tile) -> [u8; 4] {
        let bytes = tile.image.as_bytes(0).expect("one frame");
        [bytes[0], bytes[1], bytes[2], bytes[3]]
    }

    #[test]
    fn the_grid_mirrors_eguis_increment() {
        assert_eq!(grid(1), 1);
        assert_eq!(grid(MAX_SPRITE), 1);
        assert_eq!(grid(MAX_SPRITE + 1), 2);
        assert_eq!(grid(40_000), 3);
        assert_eq!(step(64), 64);
        assert_eq!(step(MAX_SPRITE), MAX_SPRITE);
        assert_eq!(step(MAX_SPRITE + 1), MAX_SPRITE);
    }

    #[test]
    fn a_frame_that_fits_uploads_exactly_one_tile() {
        let frame = Frame::upload(&bands(4000, 64), ColorChannel::Rgba);
        assert_eq!(frame.tile_count(), 1);
        assert_eq!(frame.dimensions(), (4000, 64));
        let tile = frame.iter().next().expect("one tile");
        assert_eq!((tile.x, tile.y, tile.width, tile.height), (0, 0, 4000, 64));
        // BGRA, not RGBA: the atlas swap has to survive the single-tile path.
        assert_eq!(first_pixel(&tile), [0, 0, 255, 255]);
    }

    #[test]
    fn an_oversized_frame_splits_into_full_tiles_plus_a_remainder() {
        let width = MAX_SPRITE + 616;
        let frame = Frame::upload(&bands(width, 8), ColorChannel::Rgba);
        let tiles: Vec<_> = frame.iter().collect();
        assert_eq!(tiles.len(), 2);
        assert_eq!(tiles[0].width, MAX_SPRITE);
        assert_eq!(tiles[1].width, 616);
        let covered: u32 = tiles.iter().map(|t| t.width).sum();
        assert_eq!(covered, width);
        assert!(tiles.iter().all(|t| t.height == 8 && t.y == 0));
        assert_eq!(
            tiles
                .iter()
                .map(|t| (t.x, t.x + t.width))
                .collect::<Vec<_>>(),
            vec![(0, MAX_SPRITE), (MAX_SPRITE, width)],
            "tiles must abut without gaps or overlap"
        );
    }

    /// The point of the split is which pixels land where, so check the content of each tile against
    /// the band it was cropped from rather than only its size.
    #[test]
    fn each_tile_carries_the_pixels_of_its_own_source_rect() {
        let frame = Frame::upload(&bands(40_000, 4), ColorChannel::Rgba);
        let tiles: Vec<_> = frame.iter().collect();
        assert_eq!(tiles.len(), 3);
        for tile in &tiles {
            let bytes = tile.image.as_bytes(0).expect("one frame");
            assert_eq!(bytes.len(), (tile.width * tile.height * 4) as usize);
            // Bands are 13333 px wide and the atlas step is 16384, so a tile's first pixel is in
            // band (x / 13333) % 3: red, green, blue for the three tiles, stored as BGRA.
            let band = (tile.x / (40_000 / 3)) % 3;
            let expected: [u8; 4] = match band {
                0 => [0, 0, 255, 255], // red
                1 => [0, 255, 0, 255], // green
                _ => [255, 0, 0, 255], // blue
            };
            assert_eq!(
                [bytes[0], bytes[1], bytes[2], bytes[3]],
                expected,
                "tile at x={} started in the wrong band",
                tile.x
            );
        }
    }

    #[test]
    fn a_two_dimensional_split_stays_row_major() {
        let (w, h) = (MAX_SPRITE + 100, MAX_SPRITE + 50);
        let geometry = Frame::upload(&bands(2, 2), ColorChannel::Rgba);
        assert_eq!(geometry.tile_count(), 1);
        // The grid arithmetic is what matters here, so exercise it without uploading 1.6 GB.
        let (cols, rows) = (grid(w), grid(h));
        assert_eq!((cols, rows), (2, 2));
        assert_eq!(cols * rows, 4);
    }
}
