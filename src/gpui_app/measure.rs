//! egui's Measure overlay: the rectangle dragged over the image with the right button, its size in
//! source pixels, and the hairlines through its middle.
//!
//! egui painted it from `ImageOperation::Measure`'s own `ui` arm (HEAD `src/image_editing.rs`, the
//! `Self::Measure` branch) onto `LayerId::new(Order::Middle, "measure_overlay")`, which is above the
//! CentralPanel's image and below the side panels. The port reaches the same depth by being a child
//! of the root div between the image tiles and the panels.
//!
//! Only the rect shape is painted: egui's `MeasureShape::Line` arm exists, but nothing in either
//! shell can put a line in the stack — `measure_ui` builds `new_rect([(0,0),(0,0)])` and that is the
//! only construction site.

use gpui_kit::{
    App, Bounds, Font, Hsla, Pixels, TextAlign, TextRun, Window, black, fill, point, px, rgb, size,
};

use crate::gpui_app::fonts;

/// `Color32::from_rgba_unmultiplied(0, 0, 0, 80)` over the whole box.
const FILL: f32 = 80. / 255.;
/// The two `Stroke::new(1., from_rgba_unmultiplied(255, 255, 255, 10))` centre lines.
const HAIRLINE: f32 = 1.;
const HAIRLINE_FILL: f32 = 10. / 255.;
/// egui centred the label on `rect.expand(14.).center_bottom()`.
const LABEL_GAP: f32 = 14.;
/// `FontId::proportional(14.)`.
const LABEL_SIZE: f32 = 14.;

/// One measured rect, in window points.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    /// `[left, top, width, height]` of the box the two transformed points span.
    pub area: [f32; 4],
    /// The centre of the size label.
    pub label: [f32; 2],
    pub text: String,
    /// `Color32::from_rgb(color[0], color[1], color[2])` — the shape's own colour, which egui used
    /// only for the text. The stroke is black whatever the shape carries.
    pub color: [u8; 3],
    /// The shape's `width`, which `MeasureShape::new_rect` sets to 4, as egui's
    /// `Stroke::new(*width as f32, Color32::BLACK)`.
    pub stroke: f32,
}

/// The mark for one `MeasureShape::Rect`: every point goes through egui's `geo.scale * p +
/// geo.offset`, with `origin` the image's offset plus the chrome the canvas is pushed under.
///
/// The two corners are kept apart on purpose. egui built its `Rect { min, max }` from them without
/// normalising, so `left_center()`/`center_bottom()` and the `{}x{}` label read off the *span* while
/// the text reads the signed source delta — a drag up and left draws a box like any other and prints
/// a negative size.
pub fn rect(
    points: [(u32, u32); 2],
    color: [u8; 4],
    width: u8,
    scale: f32,
    origin: (f32, f32),
) -> Mark {
    let at = |p: (u32, u32)| [scale * p.0 as f32 + origin.0, scale * p.1 as f32 + origin.1];
    let (first, second) = (at(points[0]), at(points[1]));
    Mark {
        area: [
            first[0].min(second[0]),
            first[1].min(second[1]),
            (second[0] - first[0]).abs(),
            (second[1] - first[1]).abs(),
        ],
        label: [(first[0] + second[0]) / 2., second[1] + LABEL_GAP],
        text: format!(
            "{}x{}",
            (points[1].0 as f32 - points[0].0 as f32) as i32,
            (points[1].1 as f32 - points[0].1 as f32) as i32
        ),
        color: [color[0], color[1], color[2]],
        stroke: width as f32,
    }
}

impl Mark {
    /// egui's paint order: the stroke, the translucent fill over it, the label, then the hairlines.
    pub fn paint(&self, window: &mut Window, cx: &mut App) {
        // `rect_stroke(rect, 0.0, Stroke::new(width, BLACK), StrokeKind::Inside)`: the band lies
        // inside the box rather than straddling its edge, so it is four quads, not an outline.
        let [left, top, width, height] = self.area;
        let edge = self.stroke;
        for strip in [
            [left, top, width, edge],
            [left, top + height - edge, width, edge],
            [left, top, edge, height],
            [left + width - edge, top, edge, height],
        ] {
            window.paint_quad(fill(bounds(strip), black()));
        }
        window.paint_quad(fill(bounds(self.area), alpha(0x00_00_00, FILL)));
        self.paint_label(window, cx);
        for hair in [
            [left, top + height / 2. - HAIRLINE / 2., width, HAIRLINE],
            [left + width / 2. - HAIRLINE / 2., top, HAIRLINE, height],
        ] {
            window.paint_quad(fill(bounds(hair), alpha(0xff_ff_ff, HAIRLINE_FILL)));
        }
    }

    /// egui's `painter.text(rect.expand(14.).center_bottom(), Align2::CENTER_CENTER, ..., 14.)`:
    /// gpui's line box is centred on that point, so the glyph box's own middle lands there.
    fn paint_label(&self, window: &mut Window, cx: &mut App) {
        let runs = [TextRun {
            len: self.text.len(),
            font: Font {
                family: fonts::BODY.into(),
                ..Default::default()
            },
            color: rgb8(self.color),
            background_color: None,
            underline: None,
            strikethrough: None,
        }];
        let line =
            window
                .text_system()
                .shape_line(self.text.clone().into(), px(LABEL_SIZE), &runs, None);
        let height = line.ascent + line.descent;
        let origin = point(
            px(self.label[0] - line.width().as_f32() / 2.),
            px(self.label[1] - height.as_f32() / 2.),
        );
        let _ = line.paint(origin, height, TextAlign::Left, None, window, cx);
    }
}

fn alpha(color: u32, alpha: f32) -> Hsla {
    rgb(color).opacity(alpha).into()
}

/// `Color32::from_rgb`, which drops the shape's alpha byte.
fn rgb8([r, g, b]: [u8; 3]) -> Hsla {
    rgb((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)).into()
}

/// `[x, y, width, height]` widened to the bounds the paint API takes, which is logical points.
fn bounds(rect: [f32; 4]) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(rect[0]), px(rect[1])),
        size: size(px(rect[2]), px(rect[3])),
    }
}

#[cfg(test)]
mod tests {
    use super::rect;

    /// egui's `geo.scale * p + geo.offset`, at 2x with the image's top-left at (40, 20).
    #[test]
    fn the_points_are_transformed_like_the_image_tiles() {
        let mark = rect(
            [(10, 20), (30, 45)],
            [255, 255, 255, 255],
            4,
            2.,
            (40., 20.),
        );
        assert_eq!(mark.area, [60., 60., 40., 50.]);
        assert_eq!(mark.text, "20x25".to_string());
        assert_eq!(mark.stroke, 4.);
        assert_eq!(mark.color, [255, 255, 255]);
    }

    #[test]
    fn the_label_is_centred_on_the_span_and_sits_below_the_second_point() {
        let mark = rect([(10, 20), (30, 45)], [255, 255, 255, 255], 4, 2., (0., 0.));
        assert_eq!(mark.label, [40., 104.]);
    }

    /// A drag up and left keeps egui's un-normalised arithmetic: the box spans the same area, but
    /// the size it prints is the signed source delta and the label follows the *second* point,
    /// which is now the top corner.
    #[test]
    fn a_drag_backwards_draws_the_same_box_with_a_negative_size() {
        let forward = rect([(10, 10), (30, 30)], [255, 255, 255, 255], 4, 1., (0., 0.));
        let backward = rect([(30, 30), (10, 10)], [255, 255, 255, 255], 4, 1., (0., 0.));
        assert_eq!(forward.area, backward.area);
        assert_eq!(forward.text, "20x20".to_string());
        assert_eq!(backward.text, "-20x-20".to_string());
        assert_eq!(forward.label, [20., 44.]);
        assert_eq!(backward.label, [20., 24.]);
    }

    #[test]
    fn a_click_that_never_moved_leaves_a_zero_size_box() {
        let mark = rect([(7, 9), (7, 9)], [255, 255, 255, 255], 4, 1.5, (8., 35.));
        assert_eq!(mark.area, [7. * 1.5 + 8., 9. * 1.5 + 35., 0., 0.]);
        assert_eq!(mark.text, "0x0".to_string());
    }
}
