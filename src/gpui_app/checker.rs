//! egui's transparency grid: `res/checker.png` repeated behind every image copy.
//!
//! egui could tile it from one quad because it loaded the texture with
//! `TextureWrapMode::Repeat` and gave that quad a UV running `0..image_size / 128`
//! (`src/app.rs:1028`). gpui's sprite atlas has no repeat wrap — `Window::paint_image` always
//! maps the whole sprite (or a cropped part of it) onto the bounds it is given — so the same
//! pattern is enumerated cell by cell here.

use std::sync::Arc;

use gpui_kit::{Bounds, Pixels, RenderImage, point, px, size};

use crate::gpui_app::texture;
use crate::utils::ColorChannel;

/// One texture side, in both screen points and source texels. egui divided the image's on-screen
/// size by the texture's own width to get its UV repeat, so the pattern is a fixed size at every
/// zoom and stays anchored to the copy's corner.
const TILE: f32 = 128.;

/// The tile, baked for the window's device grid.
///
/// egui magnified it with `TextureFilter::Nearest`, while gpui's atlas always samples linearly: a
/// 128-texel sprite spread over 192 device pixels blurs every cell edge into a two-pixel ramp of a
/// grey the checker does not have, and bleeds the atlas gutter into the pattern at each tile seam
/// (measured: 74 and 115 off a 100/159 grid). Nearest-resampling into `128 * scale` texels first
/// makes the upload land 1:1 on the device grid, which is both egui's crisp plateau runs and no
/// edge to bleed.
pub fn tile_texture(scale: f32) -> Option<Arc<RenderImage>> {
    let data = include_bytes!("../../res/checker.png");
    let source = image::load_from_memory(data).ok()?.to_rgba8();
    let cells = source.width();
    debug_assert_eq!(
        cells as f32, TILE,
        "TILE must stay equal to the checker texture's width"
    );
    let side = (TILE * scale).round().max(1.) as u32;
    let mut baked = image::RgbaImage::new(side, side);
    for y in 0..side {
        for x in 0..side {
            // Integer floor, so a boundary texel never wobbles between two source cells.
            let pixel = source.get_pixel(x * cells / side, y * cells / side);
            baked.put_pixel(x, y, *pixel);
        }
    }
    Some(texture::to_render_image(
        &image::DynamicImage::ImageRgba8(baked),
        ColorChannel::Rgba,
    ))
}

/// One cell of the grid, in window points.
///
/// `slot` is where a whole texture lands and `visible` the part of it that egui would have shown,
/// which is exactly the pair `Window::paint_image` crops a sprite by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    slot: [f32; 4],
    visible: [f32; 4],
}

impl Cell {
    /// The `(bounds, image_bounds)` pair to paint the sprite with.
    pub fn paint(&self) -> (Bounds<Pixels>, Bounds<Pixels>) {
        (bounds(self.visible), bounds(self.slot))
    }
}

/// The cells covering `rect`, cut back to `clip`, both as `[x, y, width, height]`.
///
/// The lattice is phased off `rect` rather than `clip`, so panning the image moves the pattern with
/// it and a copy that starts part-way into a cell still begins that cell at its first texel.
pub fn grid(rect: [f32; 4], clip: [f32; 4]) -> Vec<Cell> {
    let Some(area) = intersect(rect, clip) else {
        return Vec::new();
    };
    // The index of the first and of one past the last cell touching `from..to`.
    let range = |from: f32, to: f32, origin: f32| {
        let first = ((from - origin) / TILE).floor() as i32;
        let last = ((to - origin) / TILE).ceil() as i32;
        first.max(0)..last.max(0)
    };
    let columns = range(area[0], area[0] + area[2], rect[0]);
    let rows = range(area[1], area[1] + area[3], rect[1]);
    let mut cells = Vec::new();
    for row in rows {
        for column in columns.clone() {
            let slot = [
                rect[0] + column as f32 * TILE,
                rect[1] + row as f32 * TILE,
                TILE,
                TILE,
            ];
            if let Some(visible) = intersect(slot, area) {
                cells.push(Cell { slot, visible });
            }
        }
    }
    cells
}

fn bounds(rect: [f32; 4]) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(rect[0]), px(rect[1])),
        size: size(px(rect[2]), px(rect[3])),
    }
}

/// The overlap of two `[x, y, width, height]` rects, or `None` when they do not share an area.
fn intersect(a: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    let left = a[0].max(b[0]);
    let top = a[1].max(b[1]);
    let right = (a[0] + a[2]).min(b[0] + b[2]);
    let bottom = (a[1] + a[3]).min(b[1] + b[3]);
    (right > left && bottom > top).then_some([left, top, right - left, bottom - top])
}

#[cfg(test)]
mod tests {
    use super::{TILE, grid};

    const WINDOW: [f32; 4] = [0., 0., 1560., 950.];

    #[test]
    fn whole_cells_cover_an_exact_multiple_of_the_tile() {
        let cells = grid([0., 0., 256., 128.], WINDOW);
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].slot, [0., 0., TILE, TILE]);
        assert_eq!(cells[0].visible, cells[0].slot);
        assert_eq!(cells[1].slot, [128., 0., TILE, TILE]);
    }

    #[test]
    fn the_last_cell_is_cut_at_the_image_edge_like_clamped_uv() {
        // egui's quad was the image rect itself, so a partial cell showed a partial texture and
        // never painted checker outside the image.
        let cells = grid([0., 0., 200., 100.], WINDOW);
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[1].slot, [128., 0., TILE, TILE]);
        assert_eq!(cells[1].visible, [128., 0., 72., 100.]);
    }

    #[test]
    fn the_lattice_stays_phased_to_the_image_when_it_is_panned() {
        let cells = grid([-300., -100., 1000., 800.], [0., 0., 800., 600.]);
        // Columns start at the cell containing x=0, whose slot began 300 points to the left.
        assert_eq!(cells[0].slot, [-44., -100., TILE, TILE]);
        assert_eq!(cells[0].visible, [0., 0., 84., 28.]);
        // Every slot edge falls on the copy's origin plus a whole number of tiles.
        for cell in &cells {
            assert_eq!(((cell.slot[0] + 300.) / TILE) % 1., 0.);
            assert_eq!(((cell.slot[1] + 100.) / TILE) % 1., 0.);
        }
    }

    #[test]
    fn only_visible_cells_are_queued() {
        // A 16000x12000 view would otherwise cost 11750 primitives a frame; a window costs
        // ceil(1560/128) columns by ceil(950/128) rows, partial edge cells included.
        let cells = grid([0., 0., 16000., 12000.], WINDOW);
        assert_eq!(cells.len(), 13 * 8);
        assert!(grid([2000., 0., 100., 100.], WINDOW).is_empty());
        assert!(grid([0., 0., 0., 0.], WINDOW).is_empty());
        assert!(grid([0., 0., -100., 100.], WINDOW).is_empty());
    }

    #[test]
    fn the_baked_tile_puts_one_texel_on_every_device_pixel() {
        // 128 points at a 1.5 scale factor is 192 device pixels, so the upload has to be 192 texels.
        let image = super::tile_texture(1.5).expect("res/checker.png must decode");
        assert_eq!(image.size(0).width.0 as usize, 192);
        assert_eq!(image.size(0).height.0 as usize, 192);
        let source = image::load_from_memory(include_bytes!("../../res/checker.png"))
            .expect("res/checker.png must decode")
            .to_rgba8();
        let bytes = image.as_bytes(0).expect("frame 0");
        // egui magnified the tile with Nearest, so one source texel owns a whole device run and no
        // texel is averaged with its neighbour. The oracle is the source itself: its light cells
        // alternate 158 and 159, its dark ones 99 and 100.
        for y in 0..192 {
            for x in 0..192 {
                let i = (y * 192 + x) * 4;
                assert_eq!(
                    [bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]],
                    source
                        .get_pixel((x * 128 / 192) as u32, (y * 128 / 192) as u32)
                        .0,
                    "x={x} y={y}"
                );
            }
        }
    }

    #[test]
    fn a_unit_scale_bakes_the_png_texel_for_texel() {
        let image = super::tile_texture(1.).expect("res/checker.png must decode");
        assert_eq!(image.size(0).width.0 as usize, TILE as usize);
        let width = image.size(0).width.0 as usize;
        let bytes = image.as_bytes(0).expect("frame 0");
        let pixel = |x: usize, y: usize| {
            let i = (y * width + x) * 4;
            [bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]
        };
        assert_eq!(pixel(0, 0), [159, 159, 159, 255]);
        assert_eq!(pixel(8, 0), [100, 100, 100, 255]);
        assert_eq!(pixel(0, 8), [100, 100, 100, 255]);
        assert_eq!(pixel(127, 127), [159, 159, 159, 255]);
    }
}
