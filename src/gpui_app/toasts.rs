use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::gpui_app::{fonts, theme};
use crate::icons;
use crate::toasts::{Toast, Toasts};

/// Geometry copied from the egui widget so the cards keep the same footprint.
const MARGIN: f32 = 12.;
const SPACING: f32 = 8.;
const WIDTH: f32 = 300.;
const PAD_X: f32 = 12.;
const PAD_Y: f32 = 10.;
const ICON_BOX: f32 = 20.;
const ICON_SIZE: f32 = 17.;
const CLOSE_BOX: f32 = 20.;
const CLOSE_SIZE: f32 = 13.;
const GAP: f32 = 8.;
const BAR_HEIGHT: f32 = 3.;
const SLIDE_PX: f32 = 18.;

/// Which card the pointer is on. The shell owns this because egui's widget read it straight
/// from the frame's input state, which a gpui view cannot do.
#[derive(Clone, Copy, Default)]
pub struct Hover {
    pub card: Option<u64>,
    pub close: Option<u64>,
}

/// What the shell has to know about a card: hover pauses its countdown, close dismisses it.
#[derive(Clone, Copy, Debug)]
pub enum Event {
    Hover(u64, bool),
    HoverClose(u64, bool),
    Dismiss(u64),
}

/// The toast stack, anchored to the bottom-left of the *canvas* and growing upwards, oldest card
/// nearest the anchor. `left` is the width of whatever panel sits on that side and `bottom` is where
/// the shell's own chrome ends, i.e. the height of the scrub bar: egui anchored on
/// `ctx.content_rect()`, which excludes both side panels and both bars (HEAD `src/toasts.rs:146-157`).
pub fn render<F>(
    toasts: &Toasts,
    palette: theme::Palette,
    left: f32,
    bottom: f32,
    hover: Hover,
    on_event: F,
) -> Option<Div>
where
    F: Fn(Event, &mut App) + Clone + 'static,
{
    let items = toasts.items();
    if items.is_empty() {
        return None;
    }
    Some(
        div()
            .absolute()
            .left(px(left + MARGIN))
            .bottom(px(bottom + MARGIN))
            .w(px(WIDTH))
            .flex()
            .flex_col()
            .gap(px(SPACING))
            .children(
                items
                    .iter()
                    .rev()
                    .map(|toast| card(toast, palette, hover, on_event.clone())),
            ),
    )
}

fn card<F>(toast: &Toast, palette: theme::Palette, hover: Hover, on_event: F) -> Stateful<Div>
where
    F: Fn(Event, &mut App) + Clone + 'static,
{
    let id = toast.id;
    let on_card = on_event.clone();
    let on_close = on_event.clone();
    let eased = ease_out_cubic(toast.shown);

    div()
        .id(("toast", id))
        // egui drew every toast as an `Area` with `Order::Foreground` (HEAD `src/toasts.rs:176`), and
        // its pointer gate asked `ctx.layer_id_at(pointer)` for any layer that was not Background
        // (HEAD `src/app.rs:816-820`), so a card held the pointer over exactly its own rect: no pan,
        // no wheel zoom, no double-click-to-fullscreen underneath it. Occluding is gpui's equivalent
        // of that, because every one of those handlers sits on the root div and the events bubble to
        // it from wherever the pointer is.
        .occlude()
        .on_hover(move |&is_hovered, _, app| on_card(Event::Hover(id, is_hovered), app))
        .w_full()
        // Relative so the slide leaves the rest of the stack where it is.
        .relative()
        .top(px((1. - eased) * SLIDE_PX))
        .rounded(px(8.))
        .bg(palette.panel)
        .border_1()
        .border_color(palette.accent.opacity(0.5))
        .shadow_lg()
        .px(px(PAD_X))
        .py(px(PAD_Y))
        .opacity(eased)
        .child(
            div()
                .flex()
                .items_center()
                .child(
                    div()
                        .w(px(ICON_BOX))
                        .h(px(ICON_BOX))
                        .flex()
                        .items_center()
                        .text_size(px(ICON_SIZE))
                        .font_family(fonts::ICONS)
                        .text_color(palette.accent)
                        .child(toast.level.icon()),
                )
                .child(
                    div()
                        .ml(px(GAP))
                        .flex_1()
                        // taffy's default `min-width: auto` makes an unbroken path one indivisible word, so
                        // the item grew past the card and pushed the close box off its right end. egui gave
                        // the galley a definite `text_w` instead (HEAD `src/toasts.rs:242-245`), which wrapped
                        // the text and let the card grow downwards, so zeroing the minimum reproduces it.
                        .min_w(px(0.))
                        .text_color(palette.text)
                        .child(toast.text.clone()),
                )
                .child(close_button(
                    toast,
                    palette,
                    hover.close == Some(id),
                    on_close,
                )),
        )
        .children(progress_bar(toast, palette))
}

fn close_button<F>(
    toast: &Toast,
    palette: theme::Palette,
    hovered: bool,
    on_event: F,
) -> Stateful<Div>
where
    F: Fn(Event, &mut App) + Clone + 'static,
{
    let id = toast.id;
    let on_hover = on_event.clone();
    div()
        .id(("toast_close", id))
        .on_hover(move |&is_hovered, _, app| on_hover(Event::HoverClose(id, is_hovered), app))
        .w(px(CLOSE_BOX))
        .h(px(CLOSE_BOX))
        .ml(px(GAP))
        .when(hovered, |this| {
            this.bg(palette.text.opacity(0.08)).rounded(px(4.))
        })
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(CLOSE_SIZE))
        .font_family(fonts::ICONS)
        .text_color(palette.text.opacity(if hovered { 1. } else { 0.6 }))
        .child(icons::X)
        .cursor_pointer()
        .on_mouse_up(MouseButton::Left, move |_, _, app| {
            on_event(Event::Dismiss(id), app);
        })
}

fn progress_bar(toast: &Toast, palette: theme::Palette) -> Option<Div> {
    let total = toast.total?;
    let remaining = toast.remaining?;
    if total <= 0. {
        return None;
    }
    let fraction = (remaining / total).clamp(0., 1.);
    Some(
        div()
            .mt(px(8.))
            .w_full()
            .h(px(BAR_HEIGHT))
            .rounded(px(1.5))
            .bg(palette.accent.opacity(0.25))
            .child(
                div()
                    .w(px((WIDTH - PAD_X * 2.) * fraction))
                    .h_full()
                    .rounded(px(1.5))
                    .bg(palette.accent),
            ),
    )
}

fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    1. - (1. - t).powi(3)
}
