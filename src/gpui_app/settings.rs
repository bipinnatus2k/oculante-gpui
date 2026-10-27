//! The egui shell's `settings_ui`: the Preferences window, its five sections and the keybinding
//! editor that lives in the Input one.

use std::collections::HashMap;

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::color_picker::{ColorPicker, ColorPickerState};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::appstate::OculanteState;
use crate::gpui_app::edit_panel::{bytes_of, hsla_of};
use crate::gpui_app::{fonts, panel, theme};
use crate::icons;
use crate::settings::{ColorTheme, HeifLimits, Limit, PersistentSettings};
use crate::shortcuts::{InputEvent, Shortcut, keypresses_as_string, lookup};
use crate::utils::Frame;

/// egui's `Window::default_width(600.)` for the Preferences window.
const WIDTH: f32 = 600.;
/// egui's `ScrollArea::min_scrolled_height(400.)`, and the smallest card it could be shown in.
const MIN_HEIGHT: f32 = 400.;
/// The gap egui's rows and panels were spaced by.
const GAP: f32 = 10.;
/// egui's nav column, which is a plain stack of `styled_button`s.
const NAV: f32 = 110.;
/// What is left of the card's content row once the nav column and the gap between them are spent.
/// egui let its `dark_panel` fill that row; gpui will not shrink a `flex_1` child into what the nav
/// column leaves, so the width is written out here.
const PANEL_W: f32 = WIDTH - 2. * GAP - NAV - GAP;

/// The nav column's targets. egui kept the sections in one long scrollable page and scrolled it to
/// the matching heading, which is what [`Section`] anchors do here.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Section {
    General,
    Visual,
    Input,
    Decoders,
    Debug,
}

impl Section {
    const ALL: [Section; 5] = [
        Section::General,
        Section::Visual,
        Section::Input,
        Section::Decoders,
        Section::Debug,
    ];

    fn title(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Visual => "Visual",
            Section::Input => "Input",
            Section::Decoders => "Decoders",
            Section::Debug => "Debug",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Section::General => icons::OPTIONS,
            Section::Visual => icons::DISPLAY,
            Section::Input => icons::MOUSE,
            Section::Decoders => icons::DECODER,
            Section::Debug => icons::DEBUG,
        }
    }
}

/// One of the settings the window can write. egui bound its widgets straight to the field they were
/// showing, so this is what stands in for that `&mut`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    Vsync,
    ScrubBar,
    WrapFolder,
    MaxCache,
    MaxRecents,
    KeepView,
    KeepEdits,
    ForceRedraw,
    Mipmaps,
    FitOnResize,
    ZoomMultiplier,
    AutoScale,
    Borderless,
    MinWidth,
    MinHeight,
    Theme,
    UiScale,
    Accent,
    Background,
    Checker,
    ShowFrame,
    LinearMag,
    LinearMin,
    ZenMode,
    ZenTimeout,
    ZenNotification,
    Title,
    Experimental,
    SvgScale,
    HeifOverride,
    Heif(Heif),
}

/// A HEIF security limit: stored as a [`Limit`], edited as free text, the way egui kept them in its
/// `SettingsUiState` strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Heif {
    ImageSize,
    MemoryBlock,
    Tiles,
    BayerPattern,
    Items,
    ColorProfile,
    Components,
    IlocExtents,
    EntityGroup,
    ChildrenPerBox,
}

impl Heif {
    const ALL: [Heif; 10] = [
        Heif::ImageSize,
        Heif::MemoryBlock,
        Heif::Tiles,
        Heif::BayerPattern,
        Heif::Items,
        Heif::ColorProfile,
        Heif::Components,
        Heif::IlocExtents,
        Heif::EntityGroup,
        Heif::ChildrenPerBox,
    ];

    fn title(self) -> &'static str {
        match self {
            Heif::ImageSize => "HEIF max image size",
            Heif::MemoryBlock => "HEIF memory block size",
            Heif::Tiles => "HEIF number of tiles",
            Heif::BayerPattern => "HEIF bayer pattern pixels",
            Heif::Items => "HEIF items",
            Heif::ColorProfile => "HEIF color profile size",
            Heif::Components => "HEIF components",
            Heif::IlocExtents => "HEIF iloc extents",
            Heif::EntityGroup => "HEIF entity group size",
            Heif::ChildrenPerBox => "HEIF children per box",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Heif::ImageSize => {
                "Sets the maximum image size in pixels that libheif will decode (0 = unlimited). A restart is required to take effect."
            }
            Heif::MemoryBlock => {
                "Sets the max memory block size per image (0 = unlimited). A restart is required to take effect."
            }
            Heif::Tiles => {
                "Sets the max number of tiles to attempt decoding (0 = unlimited). A restart is required to take effect."
            }
            Heif::BayerPattern => {
                "Sets the max number of bayer pattern pixels (0 = unlimited). A restart is required to take effect."
            }
            Heif::Items => {
                "Set the max number of image items (0 = unlimited). A restart is required to take effect."
            }
            Heif::ColorProfile => {
                "Set the max color profile size (0 = unlimited). A restart is required to take effect."
            }
            Heif::Components => {
                "Set the max number of components to decode (0 = unlimited). A restart is required to take effect."
            }
            Heif::IlocExtents => {
                "Set the max number of image locations (0 = unlimited). A restart is required to take effect."
            }
            Heif::EntityGroup => {
                "Set the max entity group size (0 = unlimited). A restart is required to take effect."
            }
            Heif::ChildrenPerBox => {
                "Set the max number of metadata per box (0 = unlimited). A restart is required to take effect."
            }
        }
    }

    fn get(self, limits: HeifLimits) -> Limit {
        match self {
            Heif::ImageSize => limits.image_size_pixels,
            Heif::MemoryBlock => limits.memory_block_size,
            Heif::Tiles => limits.number_of_tiles,
            Heif::BayerPattern => limits.bayer_pattern_pixels,
            Heif::Items => limits.items,
            Heif::ColorProfile => limits.color_profile_size,
            Heif::Components => limits.components,
            Heif::IlocExtents => limits.iloc_extents_per_item,
            Heif::EntityGroup => limits.size_entity_group,
            Heif::ChildrenPerBox => limits.children_per_box,
        }
    }

    fn set(self, limits: &mut HeifLimits, value: Limit) {
        match self {
            Heif::ImageSize => limits.image_size_pixels = value,
            Heif::MemoryBlock => limits.memory_block_size = value,
            Heif::Tiles => limits.number_of_tiles = value,
            Heif::BayerPattern => limits.bayer_pattern_pixels = value,
            Heif::Items => limits.items = value,
            Heif::ColorProfile => limits.color_profile_size = value,
            Heif::Components => limits.components = value,
            Heif::IlocExtents => limits.iloc_extents_per_item = value,
            Heif::EntityGroup => limits.size_entity_group = value,
            Heif::ChildrenPerBox => limits.children_per_box = value,
        }
    }

    /// egui parsed the two byte-sized limits as `u64` and the rest as `u32`, and an empty field or
    /// text that parsed as nothing both came out as `Limit::default()`.
    fn parse(self, text: &str) -> Limit {
        if text.is_empty() {
            return Limit::Default;
        }
        match self {
            Heif::ImageSize | Heif::MemoryBlock => {
                text.parse::<u64>().map(Limit::U64).unwrap_or_default()
            }
            _ => text.parse::<u32>().map(Limit::U32).unwrap_or_default(),
        }
    }
}

/// The value a row wrote.
#[derive(Clone)]
pub enum Value {
    Bool(bool),
    Num(f64),
    Text(String),
    Color([u8; 3]),
    Theme(ColorTheme),
}

/// A `DragValue` on the pointer, carrying everything the drag needs so the viewer's overlay can
/// keep writing the field while the pointer moves.
#[derive(Clone, Copy)]
pub struct Drag {
    pub field: Field,
    /// Where the pointer was when the drag began, in window pixels.
    pub x: f32,
    pub value: f64,
    pub speed: f64,
    pub lo: f64,
    pub hi: f64,
    pub integer: bool,
}

#[derive(Clone)]
pub enum Event {
    Close,
    Set(Field, Value),
    /// A drag started; the viewer owns it until the pointer is released.
    Drag(Drag),
    /// Which combo is unfolded, egui's own "only one at a time".
    Combo(Option<Field>),
    /// Bind the chord that is held to the action the clicked button belongs to.
    Assign(InputEvent, Shortcut),
    /// egui's "Reset", which replaced the whole table rather than one field.
    Reset,
    Repository,
    DeleteThumbnails,
    ThumbnailsDir,
    /// Info, warn or err, in egui's button order.
    TestMessage(usize),
}

/// The widget state the window owns. gpui keeps a text field's text and a picker's colour in an
/// entity, where egui bound both straight to the setting, so [`Form::apply`] copies them back.
#[derive(Default)]
pub struct Form {
    texts: HashMap<Field, Entity<InputState>>,
    colors: HashMap<Field, Entity<ColorPickerState>>,
    /// The page's scroll position, which the nav column moves.
    scroll: ScrollHandle,
    pub combo: Option<Field>,
}

impl Form {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop every widget so the next time the window opens, its fields are read from the settings
    /// again — egui's `SettingsUiState` was built once, on the window's first frame.
    pub fn clear(&mut self) {
        self.drop_widgets();
        self.combo = None;
        self.scroll.set_offset(point(px(0.), px(0.)));
    }

    /// [`Self::clear`] without the combo or the scroll reset: egui's Reset rebuilt nothing it did
    /// not have to, leaving the page sitting where it was. Only the widgets whose value the settings
    /// do not own are dropped, since [`Form::apply`] would otherwise copy their old text back.
    pub fn drop_widgets(&mut self) {
        self.texts.clear();
        self.colors.clear();
    }

    fn text(
        &mut self,
        field: Field,
        value: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<InputState> {
        if let Some(existing) = self.texts.get(&field) {
            return existing.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).default_value(value));
        self.texts.insert(field, input.clone());
        input
    }

    fn color(
        &mut self,
        field: Field,
        value: [u8; 3],
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<ColorPickerState> {
        if let Some(existing) = self.colors.get(&field) {
            return existing.clone();
        }
        let picker = cx.new(|cx| {
            ColorPickerState::new(window, cx)
                .default_value(hsla_of([value[0], value[1], value[2], 255]))
        });
        self.colors.insert(field, picker.clone());
        picker
    }

    /// Copy what the pickers own back into the settings. egui's colour button wrote its button's own
    /// `&mut [u8; 3]` and set the matching `*_is_custom` flag in the same line; the two are the same
    /// event here, so the flag is part of the write.
    pub fn apply(&mut self, state: &mut OculanteState, cx: &App) {
        for (field, picker) in &self.colors {
            let Some(color) = picker.read(cx).value() else {
                continue;
            };
            let [r, g, b, _] = bytes_of(color);
            let current = match field {
                Field::Accent => state.persistent_settings.accent_color,
                Field::Background => state.persistent_settings.background_color,
                _ => continue,
            };
            if [r, g, b] != current {
                apply(state, *field, &Value::Color([r, g, b]));
            }
        }
        for (field, input) in &self.texts {
            let current = match field {
                Field::Title => state.persistent_settings.title_format.clone(),
                Field::Heif(heif) => heif
                    .get(state.persistent_settings.decoders.heif)
                    .to_string(),
                _ => continue,
            };
            let next = input.read(cx).value().to_string();
            if next != current {
                apply(state, *field, &Value::Text(next));
            }
        }
    }

    /// The widgets whose value the settings do not own. gpui repaints the widget that changed and
    /// not the view above it, so the viewer watches these to run [`Form::apply`] the way egui's
    /// immediate-mode loop re-read its `SettingsUiState` on every frame.
    pub fn inputs(&self) -> Vec<Entity<InputState>> {
        self.texts.values().cloned().collect()
    }

    pub fn pickers(&self) -> Vec<Entity<ColorPickerState>> {
        self.colors.values().cloned().collect()
    }
}

/// egui's `Window::new("Preferences")`, floating over the app.
pub fn render<F>(
    state: &OculanteState,
    palette: theme::Palette,
    form: &mut Form,
    pending: Option<&Shortcut>,
    window: &mut Window,
    cx: &mut App,
    on_event: F,
) -> Option<AnyElement>
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    if !state.settings_enabled {
        return None;
    }

    let height = (window.bounds().size.height.as_f32() - 80.).clamp(MIN_HEIGHT, 720.);
    // The card's header, then the row under it; the page gets whatever is left.
    let page = height - 24. - GAP * 2. - 12.;

    // One anchor per heading, shared with the nav button that scrolls to it: the heading records its
    // own un-offset layout origin while the page builds, and the button hands that anchor to
    // `scroll_to`. A second anchor over the same handle would never have seen an origin.
    let anchors: Vec<ScrollAnchor> = Section::ALL
        .iter()
        .map(|_| ScrollAnchor::for_handle(form.scroll.clone()))
        .collect();

    let sections: Vec<AnyElement> = Section::ALL
        .iter()
        .zip(&anchors)
        .map(|(section, anchor)| {
            let rows: Vec<AnyElement> = match section {
                Section::General => general(state, palette, on_event.clone()),
                Section::Visual => visual(state, palette, form, on_event.clone(), window, cx),
                Section::Input => input(state, pending, palette, on_event.clone()),
                Section::Decoders => decoders(state, palette, form, on_event.clone(), window, cx),
                Section::Debug => debug(state, palette, on_event.clone()),
            };
            // egui's `heading.scroll_to_me(Some(Align::TOP))`, which is exactly what an anchor on
            // the same handle does.
            let anchor = anchor.clone();
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .id(SharedString::from(format!("heading {}", section.title())))
                        .anchor_scroll(Some(anchor))
                        .text_size(px(16.))
                        .font_weight(FontWeight::BOLD)
                        .child(section.title()),
                )
                // egui's `light_panel`, the panel colour the rows sit on inside the sunken
                // `dark_panel` around them.
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(GAP))
                        .p(px(6.))
                        .rounded(px(4.))
                        .bg(palette.panel)
                        .children(rows),
                )
                .into_any()
        })
        .collect();

    let scroll = div()
        .id("settings-scroll")
        .w_full()
        .h(px(page))
        .overflow_y_scroll()
        .track_scroll(&form.scroll)
        // The scroll area sizes its one child by content, and taffy will not shrink it back into the
        // area, so an unbroken description line would widen the whole page past the card. Given the
        // width, the text wraps inside it the way egui's `Label::wrap` did.
        .child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(GAP))
                .p(px(6.))
                .children(sections),
        );

    let nav = div()
        .w(px(NAV))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(2.))
        .children(Section::ALL.iter().zip(&anchors).map(|(section, anchor)| {
            let anchor = anchor.clone();
            chrome(
                SharedString::from(format!("nav {}", section.title())),
                panel::labeled(section.glyph(), section.title()),
                move |window, app| anchor.scroll_to(window, app),
                palette,
            )
            .w_full()
            .into_any()
        }));

    let close = on_event.clone();
    Some(
        // egui's `Window` dims the app behind it by nothing and closes on its own box or the menu
        // item that opened it, so this layer is a bare centring wrapper that takes no clicks.
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .id("settings-card")
                    .w(px(WIDTH))
                    .h(px(height))
                    .flex()
                    .flex_col()
                    .gap(px(GAP))
                    .p(px(10.))
                    .rounded(px(8.))
                    // egui's `window_fill`, which its theme set to the panel colour rather than to
                    // the backdrop the image sits on.
                    .bg(palette.panel)
                    .font_family(fonts::BODY)
                    .text_size(px(13.))
                    .text_color(palette.text)
                    .shadow_lg()
                    .occlude()
                    .child(
                        // egui's window title bar: the name on the left, its close box on the right.
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::BOLD)
                                    .child("Preferences"),
                            )
                            .child(
                                div()
                                    .id("settings-close")
                                    .w(px(22.))
                                    .h(px(22.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .font_family(fonts::ICONS)
                                    .hover(move |this| this.bg(palette.text.opacity(0.08)))
                                    .child(icons::X)
                                    .on_click(move |_, window, app| {
                                        close(Event::Close, window, app)
                                    }),
                            ),
                    )
                    .child(
                        div().flex().items_start().gap(px(GAP)).child(nav).child(
                            panel::dark_panel(palette, vec![scroll.into_any()]).w(px(PANEL_W)),
                        ),
                    ),
            )
            .into_any(),
    )
}

/// `panel::chrome` with the egui look, but a handler that also gets the window: the nav column
/// scrolls through it, which is a window call.
fn chrome<L, E>(
    id: impl Into<ElementId>,
    label: L,
    event: E,
    palette: theme::Palette,
) -> Stateful<Div>
where
    L: IntoElement,
    E: Fn(&mut Window, &mut App) + Clone + 'static,
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
        .on_click(move |_, window, app| event.clone()(window, app))
}

/// egui's `configuration_item_ui`: title on the left, the control pushed to the right edge, and the
/// description under both in the weak text colour.
fn item(
    title: &'static str,
    description: &'static str,
    control: impl IntoElement,
    palette: theme::Palette,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(GAP))
                .child(title)
                .child(control),
        )
        .when(!description.is_empty(), |this| {
            this.child(
                div()
                    .text_size(px(11.))
                    .text_color(palette.text.opacity(0.6))
                    .child(description),
            )
        })
}

/// egui's `styled_checkbox(&mut setting, "")`: the row's title is the label, so the box is drawn
/// bare at the right edge.
fn toggle<F>(
    title: &'static str,
    description: &'static str,
    checked: bool,
    field: Field,
    palette: theme::Palette,
    on_event: F,
) -> Div
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    let on_click = on_event;
    item(
        title,
        description,
        Checkbox::new(SharedString::from(title))
            .checked(checked)
            .on_click(move |checked, window, app| {
                on_click(Event::Set(field, Value::Bool(*checked)), window, app)
            }),
        palette,
    )
}

/// egui's `DragValue`: a framed readout the pointer drags sideways.
fn drag<F>(shown: String, grab: Drag, palette: theme::Palette, on_event: F) -> Stateful<Div>
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    let frame = if palette.dark {
        rgb(0x2a_2a_2a)
    } else {
        rgb(0xE6_E6_E6)
    };
    let on_click = on_event;
    div()
        .id(SharedString::from(format!("drag {:?}", grab.field)))
        .flex()
        .items_center()
        .px(px(6.))
        .py(px(1.))
        .rounded(px(4.))
        .bg(frame)
        .cursor_ew_resize()
        .child(shown)
        .on_mouse_down(MouseButton::Left, move |event, window, app| {
            on_click(
                Event::Drag(Drag {
                    x: event.position.x.into(),
                    ..grab
                }),
                window,
                app,
            )
        })
}

/// A `DragValue` row, with the number shown the way egui's default formatter showed it.
fn dragged<F>(
    title: &'static str,
    description: &'static str,
    shown: String,
    grab: Drag,
    palette: theme::Palette,
    on_event: F,
) -> Div
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    item(
        title,
        description,
        drag(shown, grab, palette, on_event).into_any(),
        palette,
    )
}

fn general<F>(state: &OculanteState, palette: theme::Palette, on_event: F) -> Vec<AnyElement>
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    let settings = &state.persistent_settings;
    let on_repository = on_event.clone();

    vec![
        toggle(
            "VSync",
            "VSync eliminates tearing and saves CPU usage. Toggling VSync off will make some operations such as panning and zooming snappier. A restart is required to take effect.",
            settings.vsync,
            Field::Vsync,
            palette,
            on_event.clone(),
        )
        .into_any(),
        toggle(
            "Show index slider",
            "Enables an index slider to quickly scrub through lots of images.",
            settings.show_scrub_bar,
            Field::ScrubBar,
            palette,
            on_event.clone(),
        )
        .into_any(),
        toggle(
            "Wrap images at folder boundaries",
            "Repeats the current directory when you move past the first or last file in the current directory.",
            settings.wrap_folder,
            Field::WrapFolder,
            palette,
            on_event.clone(),
        )
        .into_any(),
        dragged(
            "Number of images to cache",
            "Keeps this many images in memory for faster opening.",
            format!("{}", settings.max_cache),
            Drag {
                field: Field::MaxCache,
                x: 0.,
                value: settings.max_cache as f64,
                speed: 1.,
                lo: 0.,
                hi: 10000.,
                integer: true,
            },
            palette,
            on_event.clone(),
        )
        .into_any(),
        dragged(
            "Number of recent images",
            "Remember this many recently opened images.",
            format!("{}", settings.max_recents),
            Drag {
                field: Field::MaxRecents,
                x: 0.,
                value: f64::from(settings.max_recents),
                speed: 1.,
                lo: 0.,
                hi: 12.,
                integer: true,
            },
            palette,
            on_event.clone(),
        )
        .into_any(),
        toggle(
            "Do not reset image view",
            "When a new image is loaded, keep the current zoom and offset.",
            settings.keep_view,
            Field::KeepView,
            palette,
            on_event.clone(),
        )
        .into_any(),
        toggle(
            "Keep image edits",
            "When a new image is loaded, keep current edits on the previously edited image.",
            settings.keep_edits,
            Field::KeepEdits,
            palette,
            on_event.clone(),
        )
        .into_any(),
        toggle(
            "Redraw every frame",
            "Turns off optimisations and redraws everything each frame. This will consume more CPU but gives you instant feedback if new images come in or if modifications are made. A restart is required to take effect.",
            settings.force_redraw,
            Field::ForceRedraw,
            palette,
            on_event.clone(),
        )
        .into_any(),
        toggle(
            "Use mipmaps",
            "When zooming out, less memory will be used. Faster performance, but blurry.",
            settings.use_mipmaps,
            Field::Mipmaps,
            palette,
            on_event.clone(),
        )
        .into_any(),
        toggle(
            "Fit image on window resize",
            "Fits the image to the window while resizing.",
            settings.fit_image_on_window_resize,
            Field::FitOnResize,
            palette,
            on_event.clone(),
        )
        .into_any(),
        dragged(
            "Zoom multiplier",
            "Multiplier of how fast the image will change size when using your mouse wheel or trackpad.",
            format!("{:.2}", settings.zoom_multiplier),
            Drag {
                field: Field::ZoomMultiplier,
                x: 0.,
                value: f64::from(settings.zoom_multiplier),
                speed: 0.01,
                lo: 0.05,
                hi: 10.,
                integer: false,
            },
            palette,
            on_event.clone(),
        )
        .into_any(),
        toggle(
            "Auto scale to fit",
            "Automatically scale images up to fit the window.",
            settings.auto_scale,
            Field::AutoScale,
            palette,
            on_event.clone(),
        )
        .into_any(),
        toggle(
            "Borderless mode",
            "Prevents drawing OS window decorations. A restart is required to take effect.",
            settings.borderless,
            Field::Borderless,
            palette,
            on_event.clone(),
        )
        .into_any(),
        // egui put the two axes of the minimum size side by side in one row.
        item(
            "Minimum window size",
            "Set the minimum size of the main window.",
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(
                    drag(
                        format!("x : {}", settings.min_window_size.0),
                        Drag {
                            field: Field::MinWidth,
                            x: 0.,
                            value: f64::from(settings.min_window_size.0),
                            speed: 0.01,
                            lo: 1.,
                            hi: 2000.,
                            integer: true,
                        },
                        palette,
                        on_event.clone(),
                    )
                    .into_any(),
                )
                .child(
                    drag(
                        format!("y : {}", settings.min_window_size.1),
                        Drag {
                            field: Field::MinHeight,
                            x: 0.,
                            value: f64::from(settings.min_window_size.1),
                            speed: 0.01,
                            lo: 1.,
                            hi: 2000.,
                            integer: true,
                        },
                        palette,
                        on_event.clone(),
                    )
                    .into_any(),
                )
                .into_any(),
            palette,
        )
        .into_any(),
        item(
            "Visit GitHub Repository",
            "Check out the source code, request a feature, submit a bug, or leave a star if you like it!",
            div()
                .id("repository")
                .px(px(10.))
                .py(px(4.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .cursor_pointer()
                .text_color(palette.accent)
                .hover(move |this| this.bg(palette.text.opacity(0.08)))
                .tooltip(move |window, cx| {
                    Tooltip::new("https://github.com/woelper/oculante").build(window, cx)
                })
                .child("Check it out!")
                .on_click(move |_, window, app| {
                    on_repository(Event::Repository, window, app)
                })
                .into_any(),
            palette,
        )
        .into_any(),
        item(
            "Reset all settings",
            "Reset Oculante to default settings.",
            div()
                .id("reset")
                .px(px(10.))
                .py(px(4.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .cursor_pointer()
                .bg(palette.backdrop)
                .child("Reset")
                .on_click(move |_, window, app| on_event(Event::Reset, window, app))
                .into_any(),
            palette,
        )
        .into_any(),
    ]
}

fn visual<F>(
    state: &OculanteState,
    palette: theme::Palette,
    form: &mut Form,
    on_event: F,
    window: &mut Window,
    cx: &mut App,
) -> Vec<AnyElement>
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    let settings = &state.persistent_settings;
    let shown = match settings.theme {
        ColorTheme::Dark => "Dark",
        ColorTheme::Light => "Light",
        ColorTheme::System => "Same as system",
    };
    let open = form.combo == Some(Field::Theme);
    let accent = form.color(Field::Accent, settings.accent_color, window, cx);
    let background = form.color(Field::Background, settings.background_color, window, cx);
    let title = form.text(Field::Title, &settings.title_format, window, cx);

    vec![
        theme_combo(&settings.theme, shown, open, palette, on_event.clone()).into_any(),
        dragged(
            "Scale",
            "Adjust the User Interface scale to be larger or smaller.",
            format!("{:.2}", settings.ui_scale),
            Drag {
                field: Field::UiScale,
                x: 0.,
                value: f64::from(settings.ui_scale),
                speed: 0.01,
                lo: 0.1,
                hi: 4.,
                integer: false,
            },
            palette,
            on_event.clone(),
        )
        .into_any(),
        item(
            "Accent color",
            "Customize the primary color used in the UI.",
            ColorPicker::new(&accent).accessibility_label("Accent color"),
            palette,
        )
        .into_any(),
        item(
            "Background color",
            "The color used as a background for images.",
            ColorPicker::new(&background).accessibility_label("Background color"),
            palette,
        )
        .into_any(),
        toggle(
            "Transparency Grid",
            "Replaces image transparency with a checker background.",
            settings.show_checker_background,
            Field::Checker,
            palette,
            on_event.clone(),
        )
        .into_any(),
        toggle(
            "Draw frame around image",
            "Draws a frame around images which can help see its edges when there are many transparent areas. It is centered on the outmost pixel.",
            settings.show_frame,
            Field::ShowFrame,
            palette,
            on_event.clone(),
        )
        .into_any(),
        toggle(
            "Interpolate when zooming in",
            "When zooming in, do you prefer to see individual pixels or an interpolation?",
            settings.linear_mag_filter,
            Field::LinearMag,
            palette,
            on_event.clone(),
        )
        .into_any(),
        toggle(
            "Interpolate when zooming out",
            "When zooming out, do you prefer crisper or smoother pixels?",
            settings.linear_min_filter,
            Field::LinearMin,
            palette,
            on_event.clone(),
        )
        .into_any(),
        toggle(
            "Zen mode",
            "Hides all UI and fits images to the frame.",
            settings.zen_mode,
            Field::ZenMode,
            palette,
            on_event.clone(),
        )
        .into_any(),
        dragged(
            "Zen mode cursor timeout",
            "Hides the mouse cursor after this many seconds of inactivity while in zen mode. Set to 0 to disable.",
            format!("{:.2}", settings.zen_mode_cursor_timeout),
            Drag {
                field: Field::ZenTimeout,
                x: 0.,
                value: f64::from(settings.zen_mode_cursor_timeout),
                speed: 0.1,
                lo: 0.,
                hi: 15.,
                integer: false,
            },
            palette,
            on_event.clone(),
        )
        .into_any(),
        toggle(
            "Zen mode enabled notification",
            "Shows a notification when zen mode is turned on.",
            settings.show_zen_mode_notification,
            Field::ZenNotification,
            palette,
            on_event.clone(),
        )
        .into_any(),
        // egui drew this one without the row chrome: a label, the field, and the placeholder list
        // on hover.
        div()
            .flex()
            .items_center()
            .gap(px(GAP))
            .child("Window title")
            .child(
                div()
                    .id("title-field")
                    .flex_1()
                    // egui committed the field as soon as it lost focus; gpui's field keeps its own
                    // text, which the viewer copies out on every repaint.
                    .child(Input::new(&title).w_full().text_size(px(12.)))
                    .tooltip(move |window, cx| {
                        Tooltip::new(
                            "Configures the window title. Valid options are: {APP}, {VERSION}, {FULLPATH}, {FILENAME}, {NUM}, and {RES}.",
                        )
                        .build(window, cx)
                    }),
            )
            .into_any(),
    ]
}

/// egui's color theme `ComboBox` with its three `selectable_value`s, unfolded the same in-house way
/// the edit panel's combos are.
fn theme_combo<F>(
    current: &ColorTheme,
    shown: &'static str,
    open: bool,
    palette: theme::Palette,
    on_event: F,
) -> Div
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    let toggle = Event::Combo(if open { None } else { Some(Field::Theme) });
    let on_toggle = on_event.clone();

    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(GAP))
                .child("Color theme")
                .child(
                    div()
                        .id("theme-combo")
                        .px(px(10.))
                        .py(px(4.))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .bg(palette.backdrop)
                        .child(shown)
                        .child(
                            div()
                                .font_family(fonts::ICONS)
                                .text_size(px(11.))
                                .child(if open {
                                    icons::CHEVRON_UP
                                } else {
                                    icons::CHEVRON_DOWN
                                }),
                        )
                        .on_click(move |_, window, app| on_toggle(toggle.clone(), window, app)),
                ),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(palette.text.opacity(0.6))
                .child("Customize the look and feel."),
        )
        .when(open, |this| {
            this.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .rounded(px(4.))
                    .p(px(4.))
                    .bg(palette.backdrop)
                    .children(
                        [
                            (ColorTheme::Dark, "Dark"),
                            (ColorTheme::Light, "Light"),
                            (ColorTheme::System, "Same as system"),
                        ]
                        .into_iter()
                        .map(|(theme, label)| {
                            let on_pick = on_event.clone();
                            let selected = theme == *current;
                            let themed = theme.clone();
                            div()
                                .id(SharedString::from(format!("theme {label}")))
                                .px(px(10.))
                                .py(px(4.))
                                .flex()
                                .items_center()
                                .rounded(px(4.))
                                .cursor_pointer()
                                .child(label)
                                .on_click(move |_, window, app| {
                                    on_pick(
                                        Event::Set(Field::Theme, Value::Theme(themed.clone())),
                                        window,
                                        app,
                                    )
                                })
                                .when(selected, |this| {
                                    this.bg(palette.accent).text_color(palette.accent_text)
                                })
                                .into_any()
                        }),
                    ),
            )
        })
}

/// egui's `keybinding_ui`: the notice, then a row per action with its current binding and the
/// button that takes the chord the pointer is holding.
fn input<F>(
    state: &OculanteState,
    pending: Option<&Shortcut>,
    palette: theme::Palette,
    on_event: F,
) -> Vec<AnyElement>
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    let shortcuts = &state.persistent_settings.shortcuts;
    let held = pending.map(keypresses_as_string);
    let mut rows = vec![
        div()
            .flex()
            .items_center()
            .gap(px(GAP))
            .child("While this is open, regular shortcuts will not work.")
            .when(held.is_none(), |this| {
                this.child(
                    div()
                        .text_color(palette.accent)
                        .child("Please press & hold a key"),
                )
            })
            .into_any(),
    ];

    // `Shortcuts` is a BTreeMap, so this is egui's sorted-by-action order already.
    for (event, _) in shortcuts {
        // The Assign button owns the action it binds, rather than borrowing it from the map.
        let event = *event;
        let label = lookup(shortcuts, &event);
        let assign = match pending {
            Some(chord) => {
                let on_assign = on_event.clone();
                // egui's `*shortcut = current.clone()`: the button binds the chord that is held, not
                // whatever the row happens to answer to today.
                let shortcut = chord.clone();
                let chord = keypresses_as_string(chord);
                div()
                    .id(SharedString::from(format!("assign {event:?}")))
                    .px(px(10.))
                    .py(px(4.))
                    .flex()
                    .items_center()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .bg(palette.backdrop)
                    .child(format!("Assign {chord}"))
                    .on_click(move |_, window, app| {
                        on_assign(Event::Assign(event, shortcut.clone()), window, app)
                    })
                    .into_any()
            }
            None => div()
                .id(SharedString::from(format!("assign {event:?}")))
                .px(px(10.))
                .py(px(4.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .bg(palette.backdrop)
                .opacity(0.5)
                .child("Press key(s)...")
                .into_any(),
        };
        rows.push(
            div()
                .flex()
                .items_center()
                .gap(px(GAP))
                .child(div().flex_1().child(format!("{event:?}")))
                .child(div().w(px(120.)).flex_shrink_0().child(label))
                .child(assign)
                .into_any(),
        );
    }
    rows
}

fn decoders<F>(
    state: &OculanteState,
    palette: theme::Palette,
    form: &mut Form,
    on_event: F,
    window: &mut Window,
    cx: &mut App,
) -> Vec<AnyElement>
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    let settings = &state.persistent_settings;
    let override_all = settings.decoders.heif.override_all;
    let mut rows = vec![
        dragged(
            "SVG scale",
            "Adjusts resolution that SVGs are displayed at.",
            format!("{:.2}", settings.decoders.svg_scale),
            Drag {
                field: Field::SvgScale,
                x: 0.,
                value: f64::from(settings.decoders.svg_scale),
                speed: 0.01,
                lo: 0.01,
                hi: 100.,
                integer: false,
            },
            palette,
            on_event.clone(),
        )
        .into_any(),
        toggle(
            "HEIF security override",
            "Disable all HEIF security limits. A restart is required to take effect.",
            override_all,
            Field::HeifOverride,
            palette,
            on_event.clone(),
        )
        .into_any(),
    ];

    for heif in Heif::ALL {
        let field = Field::Heif(heif);
        let seed = heif.get(settings.decoders.heif).to_string();
        let input = form.text(field, &seed, window, cx);
        rows.push(
            item(
                heif.title(),
                heif.description(),
                Input::new(&input)
                    .w(px(160.))
                    .text_size(px(12.))
                    .disabled(override_all),
                palette,
            )
            .into_any(),
        );
    }
    rows
}

fn debug<F>(state: &OculanteState, palette: theme::Palette, on_event: F) -> Vec<AnyElement>
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    let mut rows = Vec::new();

    #[cfg(debug_assertions)]
    {
        let buttons = ["Info", "Warn", "Err"]
            .into_iter()
            .enumerate()
            .map(|(index, label)| {
                let on_click = on_event.clone();
                div()
                    .id(SharedString::from(format!("test {label}")))
                    .px(px(10.))
                    .py(px(4.))
                    .flex()
                    .items_center()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .bg(palette.backdrop)
                    .child(label)
                    .on_click(move |_, window, app| {
                        on_click(Event::TestMessage(index), window, app)
                    })
                    .into_any()
            })
            .collect::<Vec<_>>();
        rows.push(
            item(
                "Send test message",
                "Send some messages.",
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .children(buttons)
                    .into_any(),
                palette,
            )
            .into_any(),
        );
    }

    rows.push(
        toggle(
            "Enable experimental features",
            "Turn on features that are not yet finished.",
            state.persistent_settings.experimental_features,
            Field::Experimental,
            palette,
            on_event.clone(),
        )
        .into_any(),
    );

    let delete = on_event.clone();
    let open = on_event.clone();
    rows.push(
        item(
            "Thumbnails",
            "Functionality to test thumbnail generation.",
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(
                    div()
                        .id("delete-thumbnails")
                        .px(px(10.))
                        .py(px(4.))
                        .flex()
                        .items_center()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .bg(palette.backdrop)
                        .child("Delete thumbnails")
                        .on_click(move |_, window, app| {
                            delete(Event::DeleteThumbnails, window, app)
                        })
                        .into_any(),
                )
                .child(
                    div()
                        .id("open-thumbnails")
                        .px(px(10.))
                        .py(px(4.))
                        .flex()
                        .items_center()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .bg(palette.backdrop)
                        .child("Open thumbnails directory")
                        .on_click(move |_, window, app| open(Event::ThumbnailsDir, window, app))
                        .into_any(),
                )
                .into_any(),
            palette,
        )
        .into_any(),
    );

    rows.push(
        item(
            "Version",
            "Build version and commit hash. This is useful when reporting bugs.",
            div().child(crate::utils::detailed_version()).into_any(),
            palette,
        )
        .into_any(),
    );
    rows
}

/// egui wrote its widgets through a `&mut` to the setting itself, so an assignment and the side
/// effect it ran beside it were one line. This is where those live now.
pub fn apply(state: &mut OculanteState, field: Field, value: &Value) {
    match (field, value) {
        (Field::Vsync, Value::Bool(on)) => state.persistent_settings.vsync = *on,
        (Field::ScrubBar, Value::Bool(on)) => state.persistent_settings.show_scrub_bar = *on,
        (Field::WrapFolder, Value::Bool(on)) => {
            state.persistent_settings.wrap_folder = *on;
            state.scrubber.wrap = *on;
        }
        (Field::MaxCache, Value::Num(v)) => {
            let max = *v as usize;
            state.persistent_settings.max_cache = max;
            state.player.cache.cache_size = max;
            state.player.cache.clear();
        }
        (Field::MaxRecents, Value::Num(v)) => {
            let max = (*v as f64).clamp(0., 12.) as u8;
            state.persistent_settings.max_recents = max;
            state
                .volatile_settings
                .recent_images
                .truncate(usize::from(max));
        }
        (Field::KeepView, Value::Bool(on)) => state.persistent_settings.keep_view = *on,
        (Field::KeepEdits, Value::Bool(on)) => state.persistent_settings.keep_edits = *on,
        (Field::ForceRedraw, Value::Bool(on)) => state.persistent_settings.force_redraw = *on,
        (Field::Mipmaps, Value::Bool(on)) => {
            state.persistent_settings.use_mipmaps = *on;
            state.send_frame(Frame::UpdateTexture);
        }
        (Field::FitOnResize, Value::Bool(on)) => {
            state.persistent_settings.fit_image_on_window_resize = *on
        }
        (Field::ZoomMultiplier, Value::Num(v)) => {
            state.persistent_settings.zoom_multiplier = *v as f32
        }
        (Field::AutoScale, Value::Bool(on)) => state.persistent_settings.auto_scale = *on,
        (Field::Borderless, Value::Bool(on)) => state.persistent_settings.borderless = *on,
        (Field::MinWidth, Value::Num(v)) => state.persistent_settings.min_window_size.0 = *v as u32,
        (Field::MinHeight, Value::Num(v)) => {
            state.persistent_settings.min_window_size.1 = *v as u32
        }
        (Field::Theme, Value::Theme(theme)) => {
            // egui's `apply_theme`; here the palette is re-resolved from the settings every frame,
            // and `theme::resolve` is what restores a theme's own accent and background under a
            // colour the user never picked.
            state.persistent_settings.theme = theme.clone()
        }
        (Field::UiScale, Value::Num(v)) => {
            // Divergence: egui applied this on the spot through `set_pixels_per_point`. GPUI only
            // lets a window's scale factor be overridden behind its test-support feature, and its
            // `set_rem_size` would rescale only the widgets that size themselves in rems, so this
            // stores the value and leaves the repaint to a later shell that has a real API.
            state.persistent_settings.ui_scale = (*v).clamp(0.1, 4.) as f32;
        }
        (Field::Accent, Value::Color(rgb)) => {
            state.persistent_settings.accent_color = *rgb;
            // egui's `.changed()` on the colour button.
            state.persistent_settings.accent_color_is_custom = true;
        }
        (Field::Background, Value::Color(rgb)) => {
            state.persistent_settings.background_color = *rgb;
            state.persistent_settings.background_color_is_custom = true;
        }
        (Field::Checker, Value::Bool(on)) => {
            state.persistent_settings.show_checker_background = *on
        }
        (Field::ShowFrame, Value::Bool(on)) => state.persistent_settings.show_frame = *on,
        (Field::LinearMag, Value::Bool(on)) => {
            state.persistent_settings.linear_mag_filter = *on;
            state.send_frame(Frame::UpdateTexture);
        }
        (Field::LinearMin, Value::Bool(on)) => {
            state.persistent_settings.linear_min_filter = *on;
            state.send_frame(Frame::UpdateTexture);
        }
        (Field::ZenMode, Value::Bool(on)) => state.persistent_settings.zen_mode = *on,
        (Field::ZenTimeout, Value::Num(v)) => {
            state.persistent_settings.zen_mode_cursor_timeout = *v as f32
        }
        (Field::ZenNotification, Value::Bool(on)) => {
            state.persistent_settings.show_zen_mode_notification = *on
        }
        (Field::Title, Value::Text(text)) => state.persistent_settings.title_format = text.clone(),
        (Field::Experimental, Value::Bool(on)) => {
            state.persistent_settings.experimental_features = *on
        }
        (Field::SvgScale, Value::Num(v)) => {
            state.persistent_settings.decoders.svg_scale = *v as f32;
            state
                .player
                .set_decoder_opts(state.persistent_settings.decoders);
            state.player.cache.clear();
            // egui reloaded only when the file on screen was the SVG whose scale just moved.
            if let Some(path) = state.current_path.clone()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
            {
                state.is_loaded = false;
                state.player.load(&path);
            }
        }
        (Field::HeifOverride, Value::Bool(on)) => {
            state.persistent_settings.decoders.heif.override_all = *on
        }
        (Field::Heif(heif), Value::Text(text)) => {
            let limit = heif.parse(text);
            heif.set(&mut state.persistent_settings.decoders.heif, limit);
        }
        _ => {}
    }
}

/// egui's Reset wrote a fresh default table over the settings, then re-applied the theme. Its
/// `SettingsUiState` strings survived, so the HEIF fields went on showing the old numbers. Here they
/// would also be copied back over the defaults by `Form::apply`, so the widgets are dropped instead.
pub fn reset(state: &mut OculanteState, form: &mut Form) {
    state.persistent_settings = PersistentSettings::default();
    form.drop_widgets();
}

#[cfg(test)]
mod tests {
    use crate::gpui_app::settings::Heif;
    use crate::settings::Limit;

    /// egui committed the field on a lost focus or Enter: an empty one became the default, text
    /// that parsed to nothing became the default too, and the two byte-sized limits were the only
    /// ones read back as 64-bit. `Limit` has no `PartialEq`, so its variant is read through `Debug`.
    #[test]
    fn heif_text_parses_the_way_egui_committed_it() {
        fn shown(limit: Limit) -> String {
            format!("{limit:?}")
        }

        assert_eq!(shown(Heif::ImageSize.parse("")), shown(Limit::Default));
        assert_eq!(shown(Heif::ImageSize.parse("abc")), shown(Limit::Default));
        assert_eq!(shown(Heif::ImageSize.parse("0")), shown(Limit::U64(0)));
        assert_eq!(
            shown(Heif::MemoryBlock.parse("4096")),
            shown(Limit::U64(4096))
        );
        assert_eq!(shown(Heif::Tiles.parse("7")), shown(Limit::U32(7)));
        assert_eq!(shown(Heif::Items.parse("nope")), shown(Limit::Default));
    }

    /// `Limit::Display` is what seeds the field, so a round trip through it has to be stable.
    #[test]
    fn heif_limits_round_trip() {
        for heif in Heif::ALL {
            for limit in [Limit::Default, Limit::NoLimit, Limit::U32(5)] {
                let text = limit.to_string();
                let parsed = heif.parse(&text);
                let again = parsed.to_string();
                assert_eq!(text, again, "{heif:?} {text}");
            }
        }
    }
}
