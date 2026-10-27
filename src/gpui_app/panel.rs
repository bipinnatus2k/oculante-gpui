//! The chrome egui's side panels shared.
//!
//! egui got these styles from its global `Visuals`; gpui has no equivalent, so every panel builds
//! the same few frames explicitly. They live here so the panels cannot drift apart.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::gpui_app::{fonts, theme};
use crate::icons;

/// egui's `PANEL_WIDTH`, which both side panels defaulted to.
pub const WIDTH: f32 = 260.;
/// egui's panel inner padding.
pub const PAD: f32 = 10.;
/// Twice egui's `resize_grab_radius_side` (3pt), which is how wide its resize strip was.
const GRAB: f32 = 6.;
/// The range egui's `"editing"` panel clamped to: `.min_size(100.).max_size(500.)`.
pub const EDIT_MIN: f32 = 100.;
pub const EDIT_MAX: f32 = 500.;
/// The range egui's `"info"` panel got by not overriding `Panel::left`'s vertical default.
pub const INFO_MIN: f32 = 96.;

/// Which side panel the pointer is currently resizing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// egui's `"info"`.
    Left,
    /// egui's `"editing"`.
    Right,
}

/// egui's `prepare_resizing_response`: a panel's width became the pointer's distance from the window
/// edge it is pinned to, clamped to its own range and *then* to what was left of the row. The order
/// matters, because it lets a window narrower than the minimum still squeeze the panel below it.
pub fn resize_to(side: Side, from_edge: f32, available: f32) -> f32 {
    match side {
        Side::Left => from_edge.max(INFO_MIN).min(available),
        Side::Right => from_edge.clamp(EDIT_MIN, EDIT_MAX).min(available),
    }
}

/// A panel pinned between the top and bottom bars, with its own scrollbar and, on the edge that
/// faces the canvas, egui's resize handle.
///
/// `dragging` is egui's `is_resizing`: the separator switches to `widgets.active.fg_stroke`. egui
/// also brightened it on plain hover (`widgets.hovered.fg_stroke`); that one state is missing here,
/// since gpui's hover styling cannot reach the parent's border. Its 2pt active width stays 1px.
pub fn side_panel(
    scroll_id: &'static str,
    side: Side,
    palette: theme::Palette,
    top: f32,
    bottom: f32,
    width: f32,
    dragging: bool,
    on_resize: impl Fn(&mut App) + 'static,
    body: impl FnOnce(Div) -> Div,
) -> Div {
    let edge = if dragging {
        // egui's `widgets.active.fg_stroke`, which oculante's theme never overrode: `Color32::WHITE`
        // on the dark theme, near-black on the light one.
        if palette.dark {
            rgb(0xff_ff_ff)
        } else {
            rgb(0x06_06_06)
        }
    } else {
        palette.text.opacity(0.12)
    };
    let outer = div()
        .absolute()
        .top(px(top))
        .bottom(px(bottom))
        .w(px(width))
        .occlude()
        .bg(palette.panel)
        .border_color(edge);
    let outer = match side {
        Side::Right => outer.right_0().border_l_1(),
        Side::Left => outer.left_0().border_r_1(),
    };

    outer
        .child(
            div()
                .id(scroll_id)
                .size_full()
                .overflow_y_scroll()
                .child(body(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .p(px(PAD))
                        .font_family(fonts::BODY)
                        .text_size(px(13.))
                        .text_color(palette.text),
                )),
        )
        .child(resize_handle(side, on_resize))
}

/// egui's `resize_panel`: a drag strip the width of the panel, `resize_grab_radius_side` on each
/// side of its canvas-facing edge.
///
/// egui's strip straddled the edge, so 3pt of it lay over the canvas; this one stays inside the
/// panel, because a press outside it would bubble past the panel's `occlude` and start an image
/// pan on the viewport as well.
fn resize_handle(side: Side, on_resize: impl Fn(&mut App) + 'static) -> Div {
    let handle = div()
        .absolute()
        .top_0()
        .bottom_0()
        .w(px(GRAB))
        .cursor(CursorStyle::ResizeLeftRight)
        .on_mouse_down(MouseButton::Left, move |_, _, cx| on_resize(cx));
    match side {
        Side::Right => handle.left_0(),
        Side::Left => handle.right_0(),
    }
}

/// egui's `dark_panel`: the sunken frame a section's contents sit in.
pub fn dark_panel(palette: theme::Palette, body: Vec<AnyElement>) -> Div {
    let gray = if palette.dark { 13 } else { 217 };
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .p(px(6.))
        .rounded(px(4.))
        .bg(rgb(gray * 0x0001_0101))
        .children(body)
}

/// egui's `CollapsingHeader`: a backdrop band with a chevron that opens and closes its body.
pub fn section<T, E, I>(
    title: &'static str,
    open: bool,
    palette: theme::Palette,
    toggle: T,
    on_event: impl Fn(T, &mut App) + 'static,
    body: I,
) -> Div
where
    T: Clone + 'static,
    E: IntoElement,
    I: IntoIterator<Item = E>,
{
    // egui's `caret_icon` draws CARET_RIGHT and rotates it by `openness`, so closed points right and
    // open points down.
    let glyph = if open {
        icons::CHEVRON_DOWN
    } else {
        icons::CARET_RIGHT
    };
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .id(title)
                .w_full()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(6.))
                .py(px(4.))
                .rounded(px(4.))
                .cursor_pointer()
                .bg(palette.backdrop)
                .child(
                    div()
                        .w(px(13.))
                        .flex_shrink_0()
                        .text_center()
                        .text_size(px(11.))
                        .font_family(fonts::ICONS)
                        .child(glyph),
                )
                .child(title)
                .on_click(move |_, _, app| on_event(toggle.clone(), app)),
        )
        .when(open, |this| {
            this.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .pl(px(6.))
                    .children(body),
            )
        })
}

/// The chrome every panel button shares: egui's padded, rounded, sunken frame plus its click.
pub fn chrome<L, E>(
    id: impl Into<ElementId>,
    label: L,
    event: E,
    palette: theme::Palette,
    on_event: impl Fn(E, &mut App) + 'static,
) -> Stateful<Div>
where
    L: IntoElement,
    E: Clone + 'static,
{
    div()
        .id(id)
        .px(px(10.))
        .py(px(4.))
        .flex()
        .items_center()
        .rounded(px(4.))
        .cursor_pointer()
        .bg(palette.backdrop)
        .child(label)
        .on_click(move |_, _, app| on_event(event.clone(), app))
}

/// A button whose whole label is plain text, which is also its identity within the panel.
pub fn panel_button<E>(
    label: impl Into<SharedString>,
    event: E,
    palette: theme::Palette,
    on_event: impl Fn(E, &mut App) + 'static,
) -> Stateful<Div>
where
    E: Clone + 'static,
{
    let label = label.into();
    chrome(label.clone(), label, event, palette, on_event)
}

/// egui wrote its buttons as `"{GLYPH} text"` in one label; gpui needs the icon font on its own
/// span, since the family is a per-node style.
pub fn labeled(glyph: &'static str, text: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .child(icon(glyph))
        .child(text)
}

pub fn icon(glyph: &'static str) -> Div {
    div().flex_shrink_0().font_family(fonts::ICONS).child(glyph)
}

pub fn separator() -> Div {
    div().w_full().h(px(1.)).my(px(6.)).bg(rgb(0).opacity(0.25))
}

/// The window-space bounds of an element, published by a `canvas` during prepaint so the pointer
/// handlers beside it can turn a window coordinate into a position inside the element.
///
/// gpui hands those handlers window coordinates only, and the layout above them — the cards in
/// front of it, the panel's scroll offset — is not something they can ask about. The canvas sees
/// both, and every frame it is laid out it writes before any handler of the same frame runs.
#[derive(Clone, Default)]
pub struct ElementBounds(Rc<RefCell<Option<Bounds<Pixels>>>>);

impl ElementBounds {
    /// Called from the canvas' prepaint with the bounds it was given.
    pub fn capture(&self, bounds: Bounds<Pixels>) {
        *self.0.borrow_mut() = Some(bounds);
    }

    /// Where `window_x` falls across the element: 0 at its left edge, 1 at its right, clamped to
    /// both. `None` before the element has been laid out once.
    pub fn fraction(&self, window_x: Pixels) -> Option<f32> {
        let borrowed = self.0.borrow();
        let bounds = borrowed.as_ref()?;
        let width: f32 = bounds.size.width.into();
        let inside = f32::from(window_x) - f32::from(bounds.origin.x);
        Some((inside / width.max(1.)).clamp(0., 1.))
    }
}

// Named imports only: `use super::*` here would also drag in gpui's `test` attribute macro, and the
// builtin `#[test]` would then expand to itself.
#[cfg(test)]
mod tests {
    use crate::gpui_app::panel::{EDIT_MAX, EDIT_MIN, INFO_MIN, Side, WIDTH, resize_to};

    /// The two ranges egui's panels were built with, and the order its clamps ran in.
    #[test]
    fn side_panels_clamp_to_eguis_own_ranges() {
        assert_eq!(resize_to(Side::Right, 300., 1200.), 300.);
        assert_eq!(resize_to(Side::Right, 40., 1200.), EDIT_MIN);
        assert_eq!(resize_to(Side::Right, 900., 1200.), EDIT_MAX);

        assert_eq!(resize_to(Side::Left, 300., 1200.), 300.);
        assert_eq!(resize_to(Side::Left, 20., 1200.), INFO_MIN);
        // `"info"` never set a maximum, so it keeps growing past the right panel's 500.
        assert_eq!(resize_to(Side::Left, 900., 1200.), 900.);

        // `at_most(available_rect.width())` came last, so a row narrower than the minimum wins.
        assert_eq!(resize_to(Side::Right, 120., 110.), 110.);
        assert_eq!(resize_to(Side::Left, 120., 80.), 80.);
        // Both panels start at egui's `PANEL_WIDTH`, which neither range clips.
        assert_eq!(resize_to(Side::Right, WIDTH, 1200.), WIDTH);
        assert_eq!(resize_to(Side::Left, WIDTH, 1200.), WIDTH);
    }
}
