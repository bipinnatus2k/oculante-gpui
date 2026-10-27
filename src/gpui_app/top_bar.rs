use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Sizable, Size};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::collections::VecDeque;
use std::path::PathBuf;
use strum::IntoEnumIterator;

use crate::appstate::OculanteState;
use crate::gpui_app::{fonts, panel, theme};
use crate::icons;
use crate::shortcuts::{self, InputEvent, Shortcuts};
use crate::utils::ColorChannel;

/// egui drew the bar as a 36pt top panel above the image area.
pub const HEIGHT: f32 = 36.;
/// egui's `menu_zen` panel: `min_size(40.)` of unframed strip holding only the hamburger, shown
/// when zen mode and borderless are both on.
pub const ZEN_HEIGHT: f32 = 40.;
/// egui's `ICON_SIZE` for the unframed bar buttons.
const ICON: f32 = 19.2;
const HIT: f32 = 28.;
const MENU_WIDTH: f32 = 220.;
/// egui's `{CLOCK} Recent` submenu: `ui.set_max_width(300.0)` with rows whose button is
/// `min_size(300., 0.)`.
const RECENT_WIDTH: f32 = 300.;
/// egui's `Spacing::DEFAULT.icon_spacing` (`icon_spacing: 4.0`), which its `window_x` subtracts twice.
const ICON_SPACING: f32 = 4.;
/// egui's `window_x = state.window_size.x - icon_spacing * 2. - 100.` (top_bar.rs:11). The hundred
/// points are the browse and hamburger buttons it drew *after* the drag strip and never gated, so
/// nothing before that strip may reach past the line.
const RIGHT_RESERVE: f32 = 100.;
/// Where egui's `ui.cursor().left()` started: the bar's own left padding.
const LEFT: f32 = 4.;
/// The room each widget asked for past its own start: the channel combo 110, every icon button 80,
/// and the caret pair none at all.
const ROOM_COMBO: f32 = 110.;
const ROOM_BUTTON: f32 = 80.;
/// The combo's advance: 16pt of `px(8.)` padding either side of a five-character uppercase channel
/// name at the bar's 13pt size.
const COMBO: f32 = 58.;

/// Which of the bar's gated widgets fit at this width, in egui's draw order.
#[derive(Clone, Copy, Default)]
struct Gates {
    combo: bool,
    info: bool,
    edit: bool,
    fullscreen: bool,
    on_top: bool,
    trash: bool,
    clear: bool,
    carets: bool,
}

/// egui's per-widget room test, `window_x > ui.cursor().left() + reserve`. A widget that fails it is
/// never added, so it advances the cursor by nothing: the survivors keep their left-to-right order
/// instead of closing the gap, and a later widget with a smaller reserve (the carets, which ask for
/// none) can still be drawn after the ones before it vanished.
fn fits(cursor: &mut f32, line: f32, reserve: f32, advance: f32) -> bool {
    if room(*cursor, line, reserve) {
        *cursor += advance;
        true
    } else {
        false
    }
}

/// egui's test on its own, without the advance.
fn room(cursor: f32, line: f32, reserve: f32) -> bool {
    line > cursor + reserve
}

/// Run egui's gate sequence over the bar's widgets. `width` is the whole client width, since the bar
/// spans it; egui read the same from `ctx.content_rect()`.
fn gates(borderless: bool, has_image: bool, has_path: bool, paging: bool, width: f32) -> Gates {
    let line = width - ICON_SPACING * 2. - RIGHT_RESERVE;
    let mut cursor = LEFT + if borderless { HIT } else { 0. };
    let mut gates = Gates::default();
    // The close button, the first thing egui added, is drawn on the borderless path ungated.
    gates.combo = fits(&mut cursor, line, ROOM_COMBO, COMBO);
    // egui gated Info and Edit behind one shared test, `state.current_image.is_some() && window_x >
    // cursor + 80.`, drew Info on the strength of it, and only then tested Edit again.
    if has_image && room(cursor, line, ROOM_BUTTON) {
        cursor += HIT;
        gates.info = true;
        gates.edit = fits(&mut cursor, line, ROOM_BUTTON, HIT);
    }
    gates.fullscreen = fits(&mut cursor, line, ROOM_BUTTON, HIT);
    gates.on_top = fits(&mut cursor, line, ROOM_BUTTON, HIT);
    gates.trash = has_path && fits(&mut cursor, line, ROOM_BUTTON, HIT);
    gates.clear = has_image && fits(&mut cursor, line, ROOM_BUTTON, HIT);
    gates.carets = paging && fits(&mut cursor, line, 0., HIT * 2.);
    gates
}

/// Which dropdown the bar is showing, if any.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Popup {
    Channel,
    List,
    /// The `{CLOCK} Recent` submenu, which egui opened *inside* the hamburger menu: the parent
    /// stays drawn while this one is showing.
    Recent,
}

#[derive(Clone)]
pub enum Event {
    Command(InputEvent),
    Channel(ColorChannel),
    Popup(Option<Popup>),
    /// egui's `styled_menu_button("{CLOCK} Recent")` list entry.
    Recent(PathBuf),
    /// egui's "Clear recent", which empties `volatile_settings.recent_images` without saving.
    RecentClear,
    /// egui's `{GEAR} Preferences` entry, which toggled `state.settings_enabled` rather than going
    /// through an `InputEvent`, since no shortcut is bound to it.
    Settings,
    /// egui's hamburger `{FRAME} View 1:1`, which called `set_zoom(1.0, Some(window_size / 2.))` and
    /// so anchored on the window's centre, while the same action on the keyboard passed `None` and
    /// anchored on the cursor (utils.rs:983). Two routes, two anchors — which is why the menu entry
    /// cannot simply emit `ZoomActualSize`.
    ViewOneToOne,
}

/// A row of one of the bar's dropdowns. egui's menu rows carry an icon and a label only, so
/// `hotkey` is an addition here: a menu is the only place the channel and list popups can show what
/// key does the same thing.
struct Item {
    glyph: Option<&'static str>,
    label: String,
    hotkey: String,
    selected: bool,
    event: Event,
}

pub fn render<F>(
    state: &OculanteState,
    width: f32,
    palette: theme::Palette,
    popup: Option<Popup>,
    on_event: F,
) -> Option<Div>
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    if state.persistent_settings.zen_mode {
        return zen_strip(state, palette, popup, on_event);
    }

    let shortcuts = &state.persistent_settings.shortcuts;
    let has_path = state.current_path.is_some();
    let gates = gates(
        state.persistent_settings.borderless,
        state.current_image.is_some(),
        has_path,
        state.scrubber.len() > 1,
        width,
    );

    Some(
        div()
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .h(px(HEIGHT))
            .flex()
            .items_center()
            .pl(px(LEFT))
            .pr(px(6.))
            .bg(palette.panel)
            .font_family(fonts::BODY)
            .text_size(px(13.))
            .text_color(palette.text)
            .when(state.persistent_settings.borderless, |this| {
                this.child(icon_button(
                    "close",
                    icons::X,
                    "Close",
                    hotkeys(shortcuts, &InputEvent::Quit),
                    false,
                    InputEvent::Quit,
                    palette,
                    on_event.clone(),
                ))
            })
            .when(gates.combo, |this| {
                this.child(channel_dropdown(
                    state,
                    palette,
                    popup == Some(Popup::Channel),
                    on_event.clone(),
                ))
            })
            .when(gates.info, |this| {
                this.child(icon_button(
                    "info",
                    icons::INFO,
                    "Show image info",
                    hotkeys(shortcuts, &InputEvent::InfoMode),
                    state.persistent_settings.info_enabled,
                    InputEvent::InfoMode,
                    palette,
                    on_event.clone(),
                ))
            })
            .when(gates.edit, |this| {
                this.child(icon_button(
                    "edit",
                    icons::PENCIL_SIMPLE_LINE,
                    "Edit the image",
                    hotkeys(shortcuts, &InputEvent::EditMode),
                    state.persistent_settings.edit_enabled,
                    InputEvent::EditMode,
                    palette,
                    on_event.clone(),
                ))
            })
            .when(gates.fullscreen, |this| {
                this.child(icon_button(
                    "fullscreen",
                    icons::ARROWS_OUT_SIMPLE,
                    "Toggle fullscreen",
                    hotkeys(shortcuts, &InputEvent::Fullscreen),
                    false,
                    InputEvent::Fullscreen,
                    palette,
                    on_event.clone(),
                ))
            })
            // egui's `ARROW_LINE_UP`, which sent `ViewportCommand::WindowLevel`. Its bar button was
            // only drawn while the remaining width allowed, the same room test the neighbouring
            // buttons share.
            .when(gates.on_top, |this| {
                this.child(icon_button(
                    "on-top",
                    icons::ARROW_LINE_UP,
                    "Always on top",
                    hotkeys(shortcuts, &InputEvent::AlwaysOnTop),
                    state.always_on_top,
                    InputEvent::AlwaysOnTop,
                    palette,
                    on_event.clone(),
                ))
            })
            .when(gates.trash, |this| {
                this.child(icon_button(
                    "trash",
                    icons::TRASH,
                    "Move file to trash",
                    hotkeys(shortcuts, &InputEvent::DeleteFile),
                    false,
                    InputEvent::DeleteFile,
                    palette,
                    on_event.clone(),
                ))
            })
            .when(gates.clear, |this| {
                this.child(icon_button(
                    "clear",
                    icons::PLACEHOLDER,
                    "Clear image",
                    hotkeys(shortcuts, &InputEvent::ClearImage),
                    false,
                    InputEvent::ClearImage,
                    palette,
                    on_event.clone(),
                ))
            })
            .when(gates.carets, |this| {
                this.child(icon_button(
                    "prev",
                    icons::CARET_LEFT,
                    "Previous image",
                    hotkeys(shortcuts, &InputEvent::PreviousImage),
                    false,
                    InputEvent::PreviousImage,
                    palette,
                    on_event.clone(),
                ))
            })
            .when(gates.carets, |this| {
                this.child(icon_button(
                    "next",
                    icons::CARET_RIGHT,
                    "Next image",
                    hotkeys(shortcuts, &InputEvent::NextImage),
                    false,
                    InputEvent::NextImage,
                    palette,
                    on_event.clone(),
                ))
            })
            .when(has_path && !state.is_loaded, |this| {
                this.child(loading_indicator(palette, state))
            })
            // egui's `drag_area` filled the bar's remaining width when borderless, since the OS
            // caption that would normally move the window was gone. The icon buttons are siblings, so
            // their own hitboxes still win the hit test over this region.
            .child(
                div()
                    .flex_1()
                    // With `items_center` a content-less child would be 0pt tall, and the drag
                    // region needs the bar's full height to be hittable.
                    .h(px(HEIGHT))
                    .when(state.persistent_settings.borderless, |this| {
                        this.window_control_area(WindowControlArea::Drag)
                    }),
            )
            .child(icon_button(
                "browse",
                icons::FOLDER,
                "Browse for an image",
                hotkeys(shortcuts, &InputEvent::Browse),
                false,
                InputEvent::Browse,
                palette,
                on_event.clone(),
            ))
            .child(list_dropdown(state, palette, popup, on_event)),
    )
}

/// egui's `menu_zen` panel. The strip is unframed, so it takes the viewport's own background, and
/// the empty width left of the hamburger is the counterpart of egui's `drag_area` sibling — with the
/// OS caption hidden it is the only region left that can move the window. The drag region has to be
/// a sibling rather than the strip container: a window control hitbox covering the hamburger would
/// own the hit test over it and the click would go to Windows instead of the menu.
fn zen_strip<F>(
    state: &OculanteState,
    palette: theme::Palette,
    popup: Option<Popup>,
    on_event: F,
) -> Option<Div>
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    state.persistent_settings.borderless.then(|| {
        div()
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .h(px(ZEN_HEIGHT))
            .flex()
            .items_center()
            .pr(px(6.))
            .child(
                div()
                    .flex_1()
                    .h(px(ZEN_HEIGHT))
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(list_dropdown(state, palette, popup, on_event))
    })
}

fn icon_button<F>(
    name: &'static str,
    glyph: &'static str,
    text: &'static str,
    hotkey: String,
    active: bool,
    command: InputEvent,
    palette: theme::Palette,
    on_event: F,
) -> Stateful<Div>
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    let event = Event::Command(command);
    div()
        .id(name)
        .w(px(HIT))
        .h(px(HIT))
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .rounded(px(4.))
        .cursor_pointer()
        .text_size(px(ICON))
        .font_family(fonts::ICONS)
        .text_color(if active { palette.accent } else { palette.text })
        .hover(move |this| this.bg(palette.text.opacity(0.08)))
        .child(glyph)
        .tooltip(move |window, cx| {
            let hotkey = hotkey.clone();
            Tooltip::element(move |_, _| {
                div().flex().items_center().gap(px(8.)).child(text).child(
                    div()
                        .px(px(4.))
                        .rounded(px(3.))
                        .bg(palette.accent)
                        .text_color(palette.accent_text)
                        .child(hotkey.clone()),
                )
            })
            .build(window, cx)
        })
        .on_click(move |_, window, app| on_event(event.clone(), window, app))
}

fn channel_dropdown<F>(
    state: &OculanteState,
    palette: theme::Palette,
    open: bool,
    on_event: F,
) -> Div
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    // egui greyed the channel combo out while the edit panel was open, since its edits
    // are made on RGBA.
    let enabled = !state.persistent_settings.edit_enabled;
    let channel = state.persistent_settings.current_channel;
    let label = channel.to_string().to_uppercase();
    let toggle = Event::Popup(if open { None } else { Some(Popup::Channel) });

    // egui painted this combo as an inverted pill in light mode — black fill, white ink, and no
    // hover change — while dark mode kept the theme's transparent button.
    let inverted = !palette.dark;
    let fill = if inverted {
        rgb(0x00_00_00)
    } else {
        palette.panel
    };
    let ink = if inverted {
        rgb(0xff_ff_ff)
    } else {
        palette.text
    };
    let hover = if inverted {
        fill
    } else {
        palette.text.opacity(0.08)
    };

    let items: Vec<Item> = ColorChannel::iter()
        .map(|c| Item {
            glyph: None,
            label: c.to_string().to_uppercase(),
            hotkey: c.hotkey(&state.persistent_settings.shortcuts),
            selected: c == channel,
            event: Event::Channel(c),
        })
        .collect();

    let on_click = on_event.clone();
    let trigger = div()
        .id("bar-channel")
        .h(px(HIT))
        .px(px(8.))
        .flex()
        .items_center()
        .flex_shrink_0()
        .rounded(px(4.))
        .bg(fill)
        .text_color(ink)
        .when(enabled, move |this| {
            this.cursor_pointer()
                .hover(move |this| this.bg(hover))
                .on_click(move |_, window, app| on_click(toggle.clone(), window, app))
        })
        .when(!enabled, |this| this.opacity(0.5))
        .child(label);

    div()
        .relative()
        .flex_shrink_0()
        .child(trigger)
        .when(open, |this| {
            this.child(menu(items, palette, false, on_event))
        })
}

fn list_dropdown<F>(
    state: &OculanteState,
    palette: theme::Palette,
    popup: Option<Popup>,
    on_event: F,
) -> Div
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    let shortcuts = &state.persistent_settings.shortcuts;
    let menu_open = matches!(popup, Some(Popup::List) | Some(Popup::Recent));
    let recent_open = popup == Some(Popup::Recent);

    let mut items = vec![
        Item {
            glyph: Some(icons::MOVE),
            label: "Reset view".into(),
            hotkey: hotkeys(shortcuts, &InputEvent::ResetView),
            selected: false,
            event: Event::Command(InputEvent::ResetView),
        },
        Item {
            glyph: Some(icons::FRAME),
            label: "View 1:1".into(),
            hotkey: hotkeys(shortcuts, &InputEvent::ZoomActualSize),
            selected: false,
            event: Event::ViewOneToOne,
        },
    ];

    // egui drew only one of these two rows per frame, and picked it with the shift key: with shift
    // held `{FILE} Copy Path` (needs a path), otherwise `{COPY} Copy` (needs an image). Holding a
    // modifier is not something this shell can repaint on, so both rows stand on their own here —
    // same commands, both reachable without a modifier, and the gating on `current_path` /
    // `current_image` is kept.
    if state.current_image.is_some() {
        items.push(Item {
            glyph: Some(icons::COPY),
            label: "Copy".into(),
            hotkey: hotkeys(shortcuts, &InputEvent::Copy),
            selected: false,
            event: Event::Command(InputEvent::Copy),
        });
    }
    if state.current_path.is_some() {
        items.push(Item {
            glyph: Some(icons::FILE),
            label: "Copy path".into(),
            hotkey: hotkeys(shortcuts, &InputEvent::CopyPath),
            selected: false,
            event: Event::Command(InputEvent::CopyPath),
        });
    }

    items.extend([
        Item {
            glyph: Some(icons::CLIPBOARD),
            label: "Paste".into(),
            hotkey: hotkeys(shortcuts, &InputEvent::Paste),
            selected: false,
            event: Event::Command(InputEvent::Paste),
        },
        Item {
            glyph: Some(icons::GEAR),
            label: "Preferences".into(),
            hotkey: String::new(),
            selected: false,
            event: Event::Settings,
        },
        Item {
            glyph: Some(icons::EXIT),
            label: "Quit".into(),
            hotkey: hotkeys(shortcuts, &InputEvent::Quit),
            selected: false,
            event: Event::Command(InputEvent::Quit),
        },
        // egui's nested menu button, and the last row of the list. Its submenu stays a separate
        // panel because gpui has no nested popup primitive.
        Item {
            glyph: Some(icons::CLOCK),
            label: "Recent".into(),
            hotkey: String::new(),
            selected: false,
            event: Event::Popup(if recent_open {
                None
            } else {
                Some(Popup::Recent)
            }),
        },
    ]);

    let toggle = Event::Popup(if menu_open { None } else { Some(Popup::List) });
    let on_click = on_event.clone();
    let trigger = div()
        .id("bar-list")
        .w(px(HIT))
        .h(px(HIT))
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .rounded(px(4.))
        .cursor_pointer()
        .text_size(px(ICON))
        .font_family(fonts::ICONS)
        .text_color(palette.text)
        .hover(move |this| this.bg(palette.text.opacity(0.08)))
        .child(icons::LIST)
        .on_click(move |_, window, app| on_click(toggle.clone(), window, app));

    div()
        .relative()
        .flex_shrink_0()
        .child(trigger)
        .when(menu_open, {
            let on_menu = on_event.clone();
            move |this| this.child(menu(items, palette, true, on_menu))
        })
        .when(recent_open, move |this| {
            this.child(recent_menu(
                &state.volatile_settings.recent_images,
                palette,
                on_event,
            ))
        })
}

/// egui's `{CLOCK} Recent` submenu: a 300pt-wide list of `volatile_settings.recent_images`, each row
/// a 24pt accent-tinted chip carrying the uppercase extension next to the file stem. Opening it
/// leaves the hamburger menu drawn, so it hangs to the left of the parent rather than over it.
fn recent_menu<F>(
    recents: &VecDeque<PathBuf>,
    palette: theme::Palette,
    on_event: F,
) -> Stateful<Div>
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    let list = div()
        .id("bar-recent")
        .absolute()
        .top(px(HIT))
        .right(px(MENU_WIDTH + 4.))
        .w(px(RECENT_WIDTH))
        .py(px(4.))
        .px(px(4.))
        .flex()
        .flex_col()
        .gap(px(2.))
        .rounded(px(6.))
        .bg(palette.panel)
        .border_1()
        .border_color(palette.accent.opacity(0.5))
        .text_size(px(13.))
        .text_color(palette.text)
        .shadow_lg()
        .occlude();

    if recents.is_empty() {
        return list.child(div().px(px(8.)).py(px(5.)).child("No recent images"));
    }

    let clear = Event::RecentClear;
    let on_clear = on_event.clone();

    list.children(recents.iter().enumerate().map(|(index, path)| {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_uppercase())
            .unwrap_or_default();
        let filename = path
            .file_stem()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();
        let event = Event::Recent(path.clone());
        let on_click = on_event.clone();

        div()
            .id(("bar-recent-item", index as u64))
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .py(px(2.))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(move |this| this.bg(palette.text.opacity(0.08)))
            .child(
                div()
                    .w(px(24.))
                    .h(px(24.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .bg(theme::gamma_multiply(palette.accent, 0.1))
                    .font_family(fonts::BODY)
                    .text_size(px(9.))
                    .text_color(theme::gamma_multiply(palette.accent, 0.8))
                    .child(ext),
            )
            .child(div().flex_1().truncate().child(filename))
            .on_click(move |_, window, app| on_click(event.clone(), window, app))
    }))
    .child(panel_separator())
    .child(
        div()
            .id("bar-recent-clear")
            .w_full()
            .flex()
            .items_center()
            .px(px(8.))
            .py(px(5.))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(move |this| this.bg(palette.text.opacity(0.08)))
            .child("Clear recent")
            .on_click(move |_, window, app| on_clear(clear.clone(), window, app)),
    )
}

/// egui's popup separator is lighter than the panels', since the menu background is already
/// `panel_color`.
fn panel_separator() -> Div {
    panel::separator().mx(px(4.))
}

fn menu<F>(items: Vec<Item>, palette: theme::Palette, right: bool, on_event: F) -> Stateful<Div>
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    div()
        .id("bar-menu")
        .absolute()
        .top(px(HIT))
        .when(right, |this| this.right_0())
        .when(!right, |this| this.left_0())
        // A definite width, not `min_w`: the Recent submenu offsets itself from this edge, and a
        // content-sized menu would make that gap depend on the longest label.
        .w(px(MENU_WIDTH))
        .py(px(4.))
        .px(px(4.))
        .flex()
        .flex_col()
        .gap(px(2.))
        .rounded(px(6.))
        .bg(palette.panel)
        .border_1()
        .border_color(palette.accent.opacity(0.5))
        .text_size(px(13.))
        .text_color(palette.text)
        .shadow_lg()
        .occlude()
        .children(items.into_iter().enumerate().map(|(index, item)| {
            let Item {
                glyph,
                label,
                hotkey,
                selected,
                event,
            } = item;
            let trailing = if selected {
                icons::CHECK.to_string()
            } else {
                hotkey
            };
            let on_click = on_event.clone();
            div()
                .id(("bar-menu-item", index as u64))
                .w_full()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(8.))
                .py(px(5.))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(move |this| this.bg(palette.text.opacity(0.08)))
                .when_some(glyph, |this, glyph| {
                    this.child(
                        div()
                            .w(px(16.))
                            .flex()
                            .justify_center()
                            .font_family(fonts::ICONS)
                            .text_color(palette.accent)
                            .child(glyph),
                    )
                })
                .child(label)
                .child(
                    div()
                        .ml_auto()
                        .when(selected, |this| {
                            this.font_family(fonts::ICONS).text_color(palette.accent)
                        })
                        .when(!selected, |this| this.text_color(palette.text.opacity(0.6)))
                        .child(trailing),
                )
                .on_click(move |_, window, app| on_click(event.clone(), window, app))
        }))
}

fn loading_indicator(palette: theme::Palette, state: &OculanteState) -> Div {
    let name = state
        .current_path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();

    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .ml(px(8.))
        .flex_shrink_0()
        .text_color(palette.text)
        .child(Spinner::new().with_size(Size::Small))
        .child(format!("Loading {name}"))
}

/// egui painted the "viewing a single channel" notice at the bottom of the image area rather
/// than inside the bar, so it is rendered separately here. `bottom` is how much of the window
/// edge the scrub bar claims. egui centred the box *on* that edge, which clipped half of it
/// away whenever the scrub bar was hidden; here it clears the bar instead.
pub fn channel_notice(state: &OculanteState, palette: theme::Palette, bottom: f32) -> Option<Div> {
    let channel = state.persistent_settings.current_channel;
    if channel == ColorChannel::Rgba {
        return None;
    }
    let revert = ColorChannel::Rgba.hotkey(&state.persistent_settings.shortcuts);

    Some(
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(px(bottom + 8.))
            .flex()
            .justify_center()
            .child(
                div()
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .bg(palette.panel.opacity(0.7))
                    .font_family(fonts::BODY)
                    .text_size(px(13.))
                    .text_color(palette.text)
                    .child(format!(
                        "Viewing {channel} channel. Press '{revert}' to revert."
                    )),
            ),
    )
}

fn hotkeys(shortcuts: &Shortcuts, command: &InputEvent) -> String {
    shortcuts::lookup(shortcuts, command)
}

/// egui's debug-only frame indication (src/ui/top_bar.rs:300-320): a red disc whenever a repaint was
/// pending, and `{:.0} fps  pass {}` in 11pt red next to it, both hanging off the content rect's
/// bottom-centre.
const DEBUG_OFFSET: (f32, f32) = (-30., -60.);
/// egui's `painter.circle(pos, 6., Color32::RED, Stroke::NONE)`.
const DEBUG_RADIUS: f32 = 6.;
/// egui's `pos + vec2(12., 0.)` text offset.
const DEBUG_TEXT_GAP: f32 = 12.;
/// egui's `FontId::proportional(11.)`.
const DEBUG_FONT: f32 = 11.;
/// egui's `Color32::RED`.
const DEBUG_RED: u32 = 0xff_00_00;

/// egui's `ctx.content_rect().center_bottom() + vec2(-30., -60.)`. The content rect is the window
/// minus the four panel strips, so the bottom-centre only depends on the side panels, the bottom bar
/// and the window size — not on the top bar.
pub fn debug_anchor(width: f32, height: f32, left: f32, right: f32, bottom: f32) -> (f32, f32) {
    (
        (left + width - right) / 2. + DEBUG_OFFSET.0,
        height - bottom + DEBUG_OFFSET.1,
    )
}

/// egui's `format!("{:.0} fps  pass {}", fps, ctx.cumulative_pass_nr())`.
pub fn debug_readout(fps: f32, passes: u64) -> String {
    format!("{fps:.0} fps  pass {passes}")
}

/// Drawn as the viewport's last child, which is where egui's `Order::Foreground` layer painter put
/// it — over the panels as well as the image.
pub fn debug_overlay(anchor: (f32, f32), fps: f32, passes: u64, repaint_pending: bool) -> Div {
    let (x, y) = anchor;
    div()
        .absolute()
        .inset_0()
        .when(repaint_pending, |this| {
            this.child(
                div()
                    .absolute()
                    .left(px(x - DEBUG_RADIUS))
                    .top(px(y - DEBUG_RADIUS))
                    // A square whose radius is half its side is gpui's circle; there is no
                    // `rounded_full` in gpui-pre 0.3.8.
                    .size(px(DEBUG_RADIUS * 2.))
                    .rounded(px(DEBUG_RADIUS))
                    .bg(rgb(DEBUG_RED)),
            )
        })
        .child(
            // egui's `Align2::LEFT_CENTER`: the string begins 12 points right of the anchor and is
            // vertically centred on it.
            div()
                .absolute()
                .left(px(x + DEBUG_TEXT_GAP))
                .top(px(y - DEBUG_FONT))
                .h(px(DEBUG_FONT * 2.))
                .flex()
                .items_center()
                .font_family(fonts::BODY)
                .text_size(px(DEBUG_FONT))
                .text_color(rgb(DEBUG_RED))
                .child(debug_readout(fps, passes)),
        )
}

#[cfg(test)]
mod tests {
    use crate::gpui_app::top_bar::{debug_anchor, debug_readout, gates};

    /// The widgets egui's room test still draws at this width, left to right.
    fn shown(width: f32) -> Vec<&'static str> {
        let g = gates(false, true, true, true, width);
        let mut names: Vec<&'static str> = Vec::new();
        for (on, name) in [
            (g.combo, "combo"),
            (g.info, "info"),
            (g.edit, "edit"),
            (g.fullscreen, "fullscreen"),
            (g.on_top, "on-top"),
            (g.trash, "trash"),
            (g.clear, "clear"),
            (g.carets, "carets"),
        ] {
            if on {
                names.push(name);
            }
        }
        names
    }

    #[test]
    fn a_wide_bar_keeps_everything() {
        assert_eq!(
            shown(1400.),
            vec![
                "combo",
                "info",
                "edit",
                "fullscreen",
                "on-top",
                "trash",
                "clear",
                "carets"
            ]
        );
    }

    /// egui's reserves (110 and 80) are far wider than the 28pt buttons they protect, so the buttons
    /// that ask for room vanish long before the bar runs out of pixels — while the caret pair, which
    /// asks for none, survives them all. Failing a test consumes nothing, so the survivors do not
    /// slide left to close the gap.
    #[test]
    fn a_narrow_bar_loses_the_reserved_room_widgets_first() {
        assert_eq!(shown(300.), vec!["combo", "info", "edit", "carets"]);
    }

    /// With the combo out of the run-up, Info's own test passes again at 200pt even though the width
    /// shrank: the gate is relative to where the previous survivor ended, not to the bar's total.
    #[test]
    fn a_widget_that_fits_once_an_earlier_one_vanished_is_still_drawn() {
        assert_eq!(shown(200.), vec!["info", "carets"]);
    }

    #[test]
    fn nothing_survives_an_impossibly_narrow_bar() {
        assert!(shown(60.).is_empty());
    }

    /// The borderless close button is the first child and is never gated, so it spends 28pt of the
    /// run-up that the gated widgets then do not have.
    #[test]
    fn borderless_spends_its_close_button_off_the_front() {
        let g = gates(true, true, true, true, 300.);
        assert!(g.combo && g.info, "the two that still fit are drawn");
        assert!(
            !g.edit,
            "one 28pt button earlier than the framed bar costs Edit its 80pt reserve"
        );
    }

    /// egui anchored the overlay on the *content* rect, so a panel on either side walks the anchor
    /// by half the difference, and only the bottom bar lifts it.
    #[test]
    fn the_overlay_anchors_on_the_content_bottom_centre() {
        assert_eq!(debug_anchor(1000., 600., 0., 0., 40.), (470., 500.));
        assert_eq!(debug_anchor(1000., 600., 200., 300., 40.), (420., 500.));
        assert_eq!(debug_anchor(1000., 600., 0., 0., 0.), (470., 540.));
    }

    #[test]
    fn the_overlay_formats_eguis_readout() {
        assert_eq!(debug_readout(59.7, 1234), "60 fps  pass 1234");
        assert_eq!(debug_readout(0., 0), "0 fps  pass 0");
    }
}
