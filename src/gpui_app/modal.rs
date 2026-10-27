use gpui_kit::*;

use crate::gpui_app::{fonts, theme};
use crate::icons;

const WIDTH: f32 = 320.;
const MIN_HEIGHT: f32 = 200.;
const ICON: f32 = 100.;
/// egui tinted the warning glyph with `visuals.warn_fg_color`, which the theme pins to
/// `Color32::from_rgb(255, 204, 0)` for both light and dark mode. Only the "Yes" button used red.
const WARN_GLYPH: u32 = 0xff_cc_00;
const WARN: u32 = 0xff4d_4d;

/// The delete confirmation dialog: egui showed it as a modal over the whole window.
pub fn confirm<F>(text: String, palette: theme::Palette, on_event: F) -> Stateful<Div>
where
    F: Fn(bool, &mut App) + Clone + 'static,
{
    let warn = rgb(WARN);
    let on_backdrop = on_event.clone();
    let on_yes = on_event.clone();
    let on_cancel = on_event;

    div()
        .id("modal-backdrop")
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(0).opacity(0.4))
        .on_click(move |_, _, app| on_backdrop(false, app))
        .child(
            div()
                .id("modal-card")
                .w(px(WIDTH))
                .min_h(px(MIN_HEIGHT))
                .flex()
                .flex_col()
                .items_center()
                .pt(px(10.))
                .px(px(16.))
                .pb(px(20.))
                .gap(px(20.))
                .rounded(px(8.))
                .bg(palette.panel)
                .font_family(fonts::BODY)
                .text_color(palette.text)
                .shadow_lg()
                .occlude()
                .child(
                    div()
                        .text_size(px(ICON))
                        .font_family(fonts::ICONS)
                        .text_color(rgb(WARN_GLYPH))
                        .child(icons::WARNING_CIRCLE),
                )
                .child(div().text_size(px(14.)).child(text))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(10.))
                        .child(button("Yes", warn, rgb(0xff_ff_ff), on_yes))
                        .child(button(
                            "Cancel",
                            palette.text.opacity(0.1),
                            palette.text,
                            on_cancel,
                        )),
                ),
        )
}

fn button<F>(label: &'static str, bg: Rgba, fg: Rgba, on_event: F) -> Stateful<Div>
where
    F: Fn(bool, &mut App) + 'static,
{
    let yes = label == "Yes";
    div()
        .id(label)
        .px(px(16.))
        .py(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .cursor_pointer()
        .text_size(px(14.))
        .bg(bg)
        .text_color(fg)
        .child(label)
        .on_click(move |_, _, app| on_event(yes, app))
}
