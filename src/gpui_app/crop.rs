//! egui's perspective-crop overlay: four handles on the source image's corners, a scrim over
//! everything else and the crop quad between the handles.
//!
//! egui painted it from `ImageOperation::CropPerspective`'s own `ui` arm (HEAD
//! `src/image_editing.rs:733`), through the edit panel's `ui.painter()`. That painter carries the
//! panel's layer and a clip rect of `ctx.content_rect()` (`egui::SidePanel` sets it in
//! `panel.rs:776`), so the shapes were positioned in whole-window coordinates — which is what the
//! arm's own comment asks for ("make a black background covering everything"). The port therefore
//! paints above the panels, not in the Measure overlay's slot between the image and them.

use gpui_kit::{App, Bounds, Hsla, Path, PathBuilder, Pixels, Point, Window, fill, point, px, rgb};

/// `Vec2::splat(15.)`: every handle is a 15 pt square.
const HANDLE: f32 = 15.;
/// The `corner_radius` of 2 that square is drawn with, chamfered here because the paint API has no
/// rounded polygon.
const RADIUS: f32 = 2.;
/// egui's `maxdist`: how near the pointer has to be for a handle to light up and to be grabbed.
pub const GRAB: f32 = 20.;
/// `Color32::from_rgba_premultiplied(0, 0, 0, 70)`, the scrim.
const SCRIM: f32 = 70. / 255.;
/// `Color32::from_rgba_unmultiplied(255, 255, 255, 10)`, the quad's fill.
const QUAD: f32 = 10. / 255.;
/// `Stroke::new(1., Color32::GOLD)`, the quad's outline.
const OUTLINE: f32 = 1.;
/// `Color32::GOLD`.
const GOLD: u32 = 0xff_d7_00;
/// `Color32::LIGHT_BLUE`, what a handle turns while the pointer sits within `GRAB` of it.
const NEAR: u32 = 0xad_d8_e6;

/// One operator's overlay, in window points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Overlay {
    /// `[left, top, width, height]` the scrim covers: egui's `Rect::EVERYTHING`, clipped to the
    /// window's content rect.
    pub scrim: [f32; 4],
    /// The four handles, in the operator's own point order.
    pub handles: [[f32; 2]; 4],
    /// Which handles the pointer is within `GRAB` of, which is what picks their colour.
    pub hot: [bool; 4],
    /// The quad's corners. egui's `PathShape` wanted them as 0, 1, 3, 2 rather than the
    /// top-left/top-right/bottom-left/bottom-right order the operator stores.
    pub quad: [[f32; 2]; 4],
}

/// egui's `geo.scale * p + geo.offset` for one source point.
pub fn transform(at: (u32, u32), scale: f32, origin: (f32, f32)) -> [f32; 2] {
    [
        scale * at.0 as f32 + origin.0,
        scale * at.1 as f32 + origin.1,
    ]
}

/// The overlay for one `CropPerspective` operator. `cursor` is the pointer egui compared against
/// the transformed handles; the drag itself is answered by [`grabbed`].
pub fn project(
    points: [(u32, u32); 4],
    scale: f32,
    origin: (f32, f32),
    scrim: [f32; 4],
    cursor: [f32; 2],
) -> Overlay {
    let handles = points.map(|point| transform(point, scale, origin));
    let hot = handles.map(|handle| near(&handle, &cursor));
    Overlay {
        scrim,
        quad: [handles[0], handles[1], handles[3], handles[2]],
        handles,
        hot,
    }
}

/// egui's `d < maxdist`: the window a handle gives itself for both its colour and a grab.
pub fn near(at: &[f32; 2], cursor: &[f32; 2]) -> bool {
    distance(at, cursor) < GRAB
}

/// Which handle egui grabbed. Its loop ran over all four and stored the index for every one within
/// `GRAB` of the cursor, so the last match won wherever two sat under the same pointer.
pub fn grabbed(handles: &[[f32; 2]; 4], cursor: [f32; 2]) -> Option<usize> {
    (0..4).filter(|i| near(&handles[*i], &cursor)).last()
}

fn distance(a: &[f32; 2], b: &[f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

impl Overlay {
    /// egui's paint order: the handles first, then the scrim over them, then the quad. A grabbed
    /// handle is therefore dimmed like everything else under it.
    pub fn paint(&self, window: &mut Window, _cx: &mut App) {
        for (handle, hot) in self.handles.iter().zip(self.hot) {
            let corners = chamfer(handle, HANDLE, RADIUS);
            if let Some(path) = filled(&corners) {
                window.paint_path(path, color(if hot { NEAR } else { GOLD }));
            }
        }
        window.paint_quad(fill(bounds(self.scrim), color_alpha(0x00_00_00, SCRIM)));
        let quad: Vec<Point<Pixels>> = self
            .quad
            .iter()
            .map(|p| point(px(p[0]), px(p[1])))
            .collect();
        if let Some(path) = filled(&quad) {
            window.paint_path(path, color_alpha(0xff_ff_ff, QUAD));
        }
        if let Some(path) = outlined(&quad) {
            window.paint_path(path, color(GOLD));
        }
    }
}

/// The eight points of a `size` square centred on `at`, cut at `radius`.
fn chamfer(at: &[f32; 2], size: f32, radius: f32) -> Vec<Point<Pixels>> {
    let half = size / 2.;
    let (x, y) = (at[0] - half, at[1] - half);
    [
        [x + radius, y],
        [x + size - radius, y],
        [x + size, y + radius],
        [x + size, y + size - radius],
        [x + size - radius, y + size],
        [x + radius, y + size],
        [x, y + size - radius],
        [x, y + radius],
    ]
    .iter()
    .map(|p| point(px(p[0]), px(p[1])))
    .collect()
}

fn filled(area: &[Point<Pixels>]) -> Option<Path<Pixels>> {
    let mut path = PathBuilder::fill();
    path.add_polygon(area, true);
    path.build().ok()
}

fn outlined(corners: &[Point<Pixels>]) -> Option<Path<Pixels>> {
    let mut path = PathBuilder::stroke(px(OUTLINE));
    path.add_polygon(corners, true);
    path.build().ok()
}

fn color(value: u32) -> Hsla {
    rgb(value).into()
}

fn color_alpha(value: u32, alpha: f32) -> Hsla {
    rgb(value).opacity(alpha).into()
}

fn bounds(rect: [f32; 4]) -> Bounds<gpui_kit::Pixels> {
    Bounds {
        origin: point(px(rect[0]), px(rect[1])),
        size: gpui_kit::size(px(rect[2]), px(rect[3])),
    }
}

#[cfg(test)]
mod tests {
    use super::{GRAB, chamfer, distance, grabbed, near, project};

    /// The handles share the transform the image tiles and the Measure overlay use, so a handle
    /// sits on its source pixel at any zoom or pan.
    #[test]
    fn the_handles_are_transformed_like_the_image_tiles() {
        let points = [(0, 0), (400, 0), (0, 300), (400, 300)];
        let overlay = project(points, 2., (40., 20.), [0., 0., 1000., 800.], [500., 500.]);
        assert_eq!(
            overlay.handles,
            [[40., 20.], [840., 20.], [40., 620.], [840., 620.]]
        );
        // egui's path order: top-left, top-right, bottom-right, bottom-left.
        assert_eq!(
            overlay.quad,
            [[40., 20.], [840., 20.], [840., 620.], [40., 620.]]
        );
        assert_eq!(overlay.scrim, [0., 0., 1000., 800.]);
    }

    /// `d < maxdist` with `maxdist` of 20, measured in window points, so it stays 20 at every zoom.
    /// The comparison is strict: a handle exactly 20 points away is still cold.
    #[test]
    fn a_handle_lights_up_within_twenty_points() {
        let corners = [(0, 0), (100, 0), (0, 100), (100, 100)];
        let hot =
            |cursor: [f32; 2]| project(corners, 1., (0., 0.), [0., 0., 500., 500.], cursor).hot;
        assert_eq!(hot([10., 10.]), [true, false, false, false]);
        assert_eq!(hot([19., 0.]), [true, false, false, false]);
        assert_eq!(hot([GRAB, 0.]), [false, false, false, false]);
        // 15 across and 15 down is 21.2 points away, which is why that cursor lights nothing up.
        assert_eq!(distance(&[0., 0.], &[15., 15.]), (450_f32).sqrt());
        assert_eq!(hot([15., 15.]), [false, false, false, false]);
        assert!(near(&[0., 0.], &[19., 0.]));
    }

    /// egui stored the grabbed index for every handle it passed, so the last one won.
    #[test]
    fn the_last_handle_under_the_cursor_is_the_one_egui_grabbed() {
        let handles = [[0., 0.], [10., 0.], [0., 0.], [500., 500.]];
        assert_eq!(grabbed(&handles, [2., 2.]), Some(2));
        assert_eq!(grabbed(&handles, [400., 400.]), None);
    }

    /// A handle is a 15 pt square with 2 pt corners, so its span is the same 15 in both axes and the
    /// chamfer never reaches the centre.
    #[test]
    fn the_handle_square_is_centred_on_its_point() {
        let points = chamfer(&[100., 100.], 15., 2.);
        let xs = points.iter().map(|p| f32::from(p.x)).collect::<Vec<_>>();
        let ys = points.iter().map(|p| f32::from(p.y)).collect::<Vec<_>>();
        assert_eq!(xs.iter().copied().fold(f32::MIN, f32::max), 107.5);
        assert_eq!(xs.iter().copied().fold(f32::MAX, f32::min), 92.5);
        assert_eq!(ys.len(), 8);
    }
}
