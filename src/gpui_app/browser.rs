//! egui's `filebrowser`: oculante's own file browser and its thumbnail grid.
//!
//! The listing half — drives, scanning, filtering, the filename guard — was already lifted out of
//! egui into [`crate::filebrowser`]; this is the widget half that was deleted along with it. egui
//! only drew it in builds without `file_open`, and so does the port.
//!
//! egui showed it as a plain `egui::Window` (822x600, floating over the UI, no scrim, draggable and
//! resizable, Escape or its title-bar cross closing it). The port keeps the size, the title bar and
//! Escape, centred in the window instead of cascaded. Dragging and resizing are dropped: gpui gives
//! an element no position of its own to move.
//!
//! Two of the tile's hover affordances stay deliberately unlike egui's
//! (`crate::ui::thumbnail_rendering::render_file_icon`), which painted them itself instead of using
//! a widget: its untruncated caption was a box on the Tooltip layer pinned under the thumbnail, and
//! gpui's equivalent is the platform tooltip by the pointer. Its spinner is dropped too, since a
//! `RenderImage` is already on the CPU side and never has the pending-texture frame egui waited for.
//! The grey placeholder with the uppercase extension covers the state egui drew for a thumbnail that
//! is still being generated.
//!
//! One piece of egui's window is not in the port: its `state.listview_active` is dead state — nothing
//! ever set it, and its `if` branch is empty — so there is nothing to mirror.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use strum::IntoEnumIterator;

use crate::appstate::OculanteState;
use crate::file_encoder::{CompressionLevel, FileEncoder};
use crate::filebrowser::{BrowserDir, BrowserState, Disk, read_dir_entries, validated_filename};
use crate::gpui_app::{edit_panel, fonts, texture, theme};
use crate::icons;
use crate::utils::ColorChannel;

/// egui's `Window::default_width` / `default_height`.
pub const WIDTH: f32 = 822.;
pub const HEIGHT: f32 = 600.;
/// egui's `BUTTON_HEIGHT_LARGE`, which every button in its nav bar and sidebar was sized to.
const NAV: f32 = 35.;
/// egui's `get_rounding(BUTTON_HEIGHT_LARGE)`: the theme's corner radius (4) doubled for anything
/// taller than 25pt.
const RADIUS: f32 = 8.;
/// egui's `item_spacing`, which its theme set to `vec2(8., 6.)`.
const GAP_X: f32 = 8.;
const GAP_Y: f32 = 6.;
/// The window margin egui's theme set, which the card's own padding replaces.
const PAD: f32 = 5.;
/// egui's `allocate_ui_with_layout(Vec2::new(120., ...))` sidebar, and the 140pt its `styled_button`
/// asked for inside it — wider than the column it was placed in, which is why its rows overhang.
const SIDEBAR: f32 = 120.;
const SIDEBAR_BUTTON: f32 = 140.;
/// The width egui's `animate_bool` grew the search field to. Its ease is dropped: the field simply
/// appears at that width.
const SEARCH_WIDTH: f32 = 88.;
/// egui's grid cell: `THUMB_SIZE` plus the 24pt caption line, laid out by `horizontal_wrapped`.
const TILE_W: f32 = 120.;
const THUMB_H: f32 = 90.;
const CAPTION_H: f32 = 24.;
const TILE_H: f32 = THUMB_H + CAPTION_H;
/// egui painted an ungenerated thumbnail as `Color32::from_gray(80)` with the uppercase extension at
/// 25pt, and washed the tile under the pointer with `Color32::from_white_alpha(50)`.
const PLACEHOLDER: u32 = 80 * 0x0001_0101;
const HOVER_WASH: f32 = 50. / 255.;
/// egui's breadcrumb budget: 11.5pt per character of a folder's stem, plus `button_padding.x` twice
/// and `item_spacing.x` twice per crumb.
const CRUMB_CHAR: f32 = 11.5;
const CRUMB_BUTTON_PAD: f32 = 4.;
/// What `available_size_before_wrap().x` had left once the nav bar's own buttons were placed. The
/// port cannot ask the layout, so it uses the figure the default card width leaves.
pub const CRUMB_WIDTH: f32 = WIDTH - PAD * 2. - (NAV + GAP_X) * 3. - GAP_X;
/// Thumbnails one poll tick may decode. egui streamed them through its own texture loader; here the
/// PNG is read on the main context, so the work is capped rather than done in one frame.
const POLL_BUDGET: usize = 32;
/// How often the poll looks for thumbnails that finished generating, while the browser is open.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(120);
/// egui's `styled_slider(quality, 0..=100)` bounds. The viewer builds its slider entity from the same
/// numbers, so the rail and the write side cannot drift apart.
pub const QUALITY_MAX: u32 = 100;
/// egui's compression slider ran over `0..=2`, one step per `CompressionLevel` variant.
pub const COMPRESSION_MAX: u32 = 2;
/// The width egui's `styled_slider` reserved for its monospace value readout.
const READOUT: f32 = 40.;

/// Which of egui's three popups the browser stands in for. Its `save` flag is the same answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// egui's `"OPEN"`: clicking a file loads it.
    Open,
    /// egui's `"SAVE"`: clicking a file only takes its name; "Save file" writes the image.
    Save,
    /// egui's `"SAVEASE"`: like `Save`, for the sampled palette.
    Palette,
    /// egui's `Id::new("LUT")` popup, which its default build opened from the LUT row's "Load lut"
    /// button. It was called with `save: false` and `SUPPORTED_EXTENSIONS`, so the window it drew was
    /// the opener's; only its callback differed, writing the picked path into that operator's row.
    Lut(edit_panel::Stack, usize),
}

impl Purpose {
    /// egui's `save` argument: the footer with the filename field, and no callback on a click.
    pub fn saves(self) -> bool {
        matches!(self, Self::Save | Self::Palette)
    }

    /// The title egui's `Window::new` drew. It keyed on the `save` flag alone — the `popup_id` it was
    /// also handed never reached the title — so the LUT popup read "Open" like the opener.
    pub fn title(self) -> &'static str {
        match self {
            Self::Save | Self::Palette => "Save",
            Self::Open | Self::Lut(..) => "Open",
        }
    }
}

/// What the browser asks the viewer to do. Everything that reaches past the listing itself — the
/// image being loaded, the file being written, the bookmarks being stored — leaves as one of these.
#[derive(Clone)]
pub enum Event {
    /// Escape, or the window's cross.
    Close,
    /// egui's search button: it flipped `search_active` and cleared the term on the way out.
    ToggleSearch,
    /// The chevron, and egui's "Empty directory" label, which was clickable for the same thing.
    Up,
    /// egui's TERMINAL/FOLDER button, swapping the breadcrumb for an editable path field.
    TogglePath,
    /// A directory was picked: breadcrumb, sidebar, drive, bookmark or folder tile.
    Go(PathBuf),
    /// A file tile was clicked. egui loaded it, or took its name when saving.
    Pick(PathBuf),
    /// The sidebar's outlined "+" button, which bookmarks the directory on show.
    AddBookmark,
    /// egui's bookmark delete, which fired on `d` or a right-click over the row.
    RemoveBookmark(PathBuf),
    /// A bookmark row started or ended being the pointer's target. egui read `res.hovered()` inside
    /// the same frame as its `d` test; gpui has no frame loop, so the row reports the transition and
    /// the ending row may only clear its own entry, which is what the frame-order-independent read did
    /// for free.
    HoverBookmark(PathBuf, bool),
    /// A footer extension button, which re-ends the filename.
    Extension(String),
    /// The footer's "Save file".
    Save,
}

/// The browser's own state, which egui kept in two `ctx.data()` temp keys (`FBSTATE`, `FBPATH`).
pub struct State {
    /// egui's `FBPATH`: the directory on show, which outlives the window.
    pub path: PathBuf,
    pub purpose: Purpose,
    /// The extensions a listing keeps. egui passed `SUPPORTED_EXTENSIONS` for the openers and the
    /// encoder list for the savers.
    pub filter: Vec<String>,
    /// The listing, its flags, the bookmarks it edits and the thumbnail generator queue.
    pub browser: BrowserState,
    /// Generated thumbnails, decoded into the atlas format. egui held these in its own texture store
    /// and keyed them by `file://` URI.
    pub thumbs: HashMap<PathBuf, Arc<RenderImage>>,
    /// The files still waiting on a thumbnail, in listing order, and where the last poll got to.
    pending: Vec<PathBuf>,
    cursor: usize,
    /// The bookmark row under the pointer, which egui's `d` branch read in the same frame as its
    /// `key_released(Key::D)` test (HEAD src/filebrowser.rs:432-437).
    pub hovered_bookmark: Option<PathBuf>,
}

impl State {
    /// Open on `path`. egui's first frame found `entries` missing, marked the path dirty and so
    /// scanned; the same happens here, and the drives are scanned with it.
    pub fn new(path: PathBuf, purpose: Purpose, filter: Vec<String>) -> Self {
        let mut browser = BrowserState::default();
        browser.refresh_drives();
        let mut this = Self {
            path,
            purpose,
            filter,
            browser,
            thumbs: HashMap::new(),
            pending: Vec::new(),
            cursor: 0,
            hovered_bookmark: None,
        };
        this.rescan();
        this
    }

    /// egui's `check_refresh_entries` reset. The parked listing went wholesale — search term,
    /// filename, the nav flags and the thumbnails it had generated with it — and only the directory
    /// mode was carried over, so the frame that followed re-read the directory and re-queued the
    /// icons.
    pub fn reset_listing(&mut self, last_dir: BrowserDir) {
        self.browser.reset_for_dir(last_dir);
        self.browser.refresh_drives();
        self.thumbs.clear();
        self.pending.clear();
        self.cursor = 0;
    }

    /// egui wrote `*path` and re-read the directory at the end of that frame.
    pub fn set_path(&mut self, path: PathBuf) {
        if path == self.path && self.browser.entries.is_some() {
            return;
        }
        self.path = path;
        self.rescan();
    }

    /// egui's chevron: `path.parent()`, which a root path has none of.
    pub fn go_up(&mut self) {
        if let Some(parent) = self.path.parent() {
            let parent = parent.to_path_buf();
            self.set_path(parent);
        }
    }

    /// egui cleared the search term when a tile changed directory and when a file was picked to
    /// open. The field itself stays open.
    pub fn clear_search(&mut self) {
        self.browser.search_term.clear();
    }

    /// The directory egui compared its breadcrumb entries against. Its `styled_selectable_label`
    /// took that as `active` and then ignored it, so neither shell highlights the current crumb.
    pub fn current_dir(&self) -> PathBuf {
        if self.path.is_dir() {
            self.path.clone()
        } else {
            self.path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default()
        }
    }

    /// The breadcrumb, capped by egui's width walk: ancestors are measured from the deepest, and the
    /// ones that fit are then reversed back into root-first order.
    pub fn breadcrumb(&self, available: f32) -> Vec<PathBuf> {
        let mut width = available;
        let mut fitted = 0;
        for ancestor in self.path.ancestors() {
            let stem = ancestor
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "Computer".to_string());
            let taken = stem.len() as f32 * CRUMB_CHAR + CRUMB_BUTTON_PAD * 2. + GAP_X * 2.;
            if width - taken > 0. {
                width -= taken;
                fitted += 1;
            } else {
                break;
            }
        }
        let mut ancestors = self
            .path
            .ancestors()
            .take(fitted.max(1))
            .collect::<Vec<_>>();
        ancestors.reverse();
        ancestors.into_iter().map(Path::to_path_buf).collect()
    }

    /// The tiles egui drew in two passes: every directory of the filtered listing, then every file.
    /// Without its row virtualisation the two passes simply run over the whole listing.
    pub fn tiles(&self) -> Vec<&PathBuf> {
        let entries = self.browser.filtered_entries();
        let dirs = entries
            .iter()
            .filter(|entry| entry.is_dir())
            .copied()
            .collect::<Vec<_>>();
        let files = entries
            .iter()
            .filter(|entry| entry.is_file())
            .copied()
            .collect::<Vec<_>>();
        dirs.into_iter().chain(files).collect()
    }

    /// The drive rows egui's sidebar listed under "Home", from the scan `FBSTATE` cached.
    pub fn drives(&self) -> &[Disk] {
        self.browser.drives.as_deref().unwrap_or_default()
    }

    /// The extension egui's footer buttons marked selected.
    pub fn extension(&self) -> String {
        Path::new(&self.browser.filename)
            .extension()
            .map(|ext| ext.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    /// egui's footer: a chosen extension is swapped onto the filename.
    pub fn set_extension(&mut self, ext: &str) {
        self.browser.filename = Path::new(&self.browser.filename)
            .with_extension(ext)
            .to_string_lossy()
            .to_string();
        self.validate_filename();
    }

    pub fn set_filename(&mut self, filename: &str) {
        self.browser.filename = filename.to_string();
        self.validate_filename();
    }

    /// egui ran this guard at the top of every footer draw, so a name it could not encode came back
    /// with the filter's first extension.
    pub fn validate_filename(&mut self) {
        let filter = self.str_filter();
        let (filename, _) = validated_filename(&self.browser.filename, &filter);
        self.browser.filename = filename;
    }

    pub fn str_filter(&self) -> Vec<&str> {
        self.filter.iter().map(String::as_str).collect()
    }

    /// One poll tick: ask the generator for the files still missing, and decode whatever it has
    /// finished. Returns whether anything new arrived, which is when a repaint is worth asking for.
    pub fn poll_thumbnails(&mut self) -> bool {
        let total = self.pending.len();
        if total == 0 {
            return false;
        }
        self.cursor %= total;
        let mut arrived = false;
        let mut decoded = 0;
        for _ in 0..total {
            let path = self.pending[self.cursor].clone();
            self.cursor = (self.cursor + 1) % total;
            // `Thumbnails::get` starts the generator on its first call and reports `Err` until the
            // cached file exists; a failed decode means it is still being written. Either way the
            // entry stays queued and comes round again on a later lap.
            let Ok(cached) = self.browser.thumbnails.get(&path) else {
                continue;
            };
            let Ok(image) = image::open(&cached) else {
                continue;
            };
            self.thumbs
                .insert(path, texture::to_render_image(&image, ColorChannel::Rgba));
            arrived = true;
            decoded += 1;
            if decoded >= POLL_BUDGET {
                break;
            }
        }
        arrived
    }

    /// egui's end-of-frame re-read: the filter skips dotfiles and foreign extensions, and dirs come
    /// first. A directory that cannot be read leaves `entries` empty rather than stale.
    fn rescan(&mut self) {
        let filter = self.str_filter();
        self.browser.entries = read_dir_entries(&self.path, &filter);
        self.pending = self
            .browser
            .entries
            .iter()
            .flatten()
            .filter(|entry| entry.is_file() && !self.thumbs.contains_key(*entry))
            .cloned()
            .collect();
        self.cursor = 0;
    }
}

/// egui's `fe.ui(ui)`: the save footer's encoder options, drawn for whichever encoder of
/// `settings.encoding_options` owns the filename's extension. `Bmp`, `WebP` and `Avif` had empty arms,
/// which is why they draw no row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncoderRow {
    /// `ui.label("Quality")`, then `styled_slider(quality, 0..=100)`.
    Quality { value: u32 },
    /// `ui.label("Compression Level")`, then `styled_slider(level, 0..=2)` and the level's name.
    Compression { level: u32 },
}

impl EncoderRow {
    /// The number egui's readout printed, `{:.0}` of the slider's own value.
    fn value(self) -> u32 {
        match self {
            Self::Quality { value } => value,
            Self::Compression { level } => level,
        }
    }

    /// The caption only the compression row had, and which follows the slider rather than the rail.
    fn caption(self) -> Option<&'static str> {
        match self {
            Self::Quality { .. } => None,
            Self::Compression { level } => match level {
                0 => Some("(Fast)"),
                1 => Some("(Default)"),
                _ => Some("(Best)"),
            },
        }
    }
}

/// Which row egui would have drawn for `ext`. Its test was `ext.to_lowercase() == fe.ext()`, and the
/// save side picks the encoder that writes the file by scanning the same list the same way
/// ([`FileEncoder::matching_variant`]), so the row on screen is the encoder about to be used.
pub fn encoder(ext: &str, encoders: &[FileEncoder]) -> Option<EncoderRow> {
    let lower = ext.to_lowercase();
    row_of(encoders.iter().find(|fe| fe.ext() == lower)?)
}

/// The two slider seeds, so the footer's rows start on the values the settings hold. gpui's slider
/// range is fixed when the entity is built, so the footer keeps one entity per row and re-reads both
/// whenever the extension swaps which row is on show.
pub fn encoder_values(encoders: &[FileEncoder]) -> (u32, u32) {
    let mut values = (0, 1);
    for fe in encoders {
        match row_of(fe) {
            Some(EncoderRow::Quality { value }) => values.0 = value,
            Some(EncoderRow::Compression { level }) => values.1 = level,
            None => {}
        }
    }
    values
}

fn row_of(fe: &FileEncoder) -> Option<EncoderRow> {
    match fe {
        FileEncoder::Jpg { quality } => Some(EncoderRow::Quality { value: *quality }),
        FileEncoder::Png { compressionlevel } => Some(EncoderRow::Compression {
            level: match compressionlevel {
                CompressionLevel::Fast => 0,
                CompressionLevel::Default => 1,
                CompressionLevel::Best => 2,
            },
        }),
        FileEncoder::Bmp | FileEncoder::WebP | FileEncoder::Avif => None,
    }
}

/// The write half of [`encoder`]: egui's slider held a `&mut` into `settings.encoding_options` and
/// wrote it on every frame the pointer moved, so a drag landed on the encoder that owned the extension.
/// Like upstream this visits every match rather than the first. Returns whether anything changed.
pub fn set_encoder(ext: &str, encoders: &mut [FileEncoder], value: u32) -> bool {
    let lower = ext.to_lowercase();
    for fe in encoders.iter_mut().filter(|fe| fe.ext() == lower) {
        match fe {
            FileEncoder::Jpg { quality } => {
                let next = value.min(QUALITY_MAX);
                if *quality != next {
                    *quality = next;
                    return true;
                }
            }
            FileEncoder::Png { compressionlevel } => {
                let next = match value.clamp(0, COMPRESSION_MAX) {
                    0 => CompressionLevel::Fast,
                    1 => CompressionLevel::Default,
                    _ => CompressionLevel::Best,
                };
                if *compressionlevel != next {
                    *compressionlevel = next;
                    return true;
                }
            }
            FileEncoder::Bmp | FileEncoder::WebP | FileEncoder::Avif => {}
        }
    }
    false
}

/// The browser card. Its events go out through `on_event`, which the viewer answers with the window
/// handle egui had anyway.
pub fn render<F>(
    state: &OculanteState,
    browser: &State,
    palette: theme::Palette,
    search: &Entity<InputState>,
    path_field: &Entity<InputState>,
    filename_field: &Entity<InputState>,
    quality: &Entity<SliderState>,
    compression: &Entity<SliderState>,
    cx: &mut App,
    on_event: F,
) -> AnyElement
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    let term = search.read(cx).value().to_string();

    backdrop(
        div()
            .id("browser")
            .w(px(WIDTH))
            .h(px(HEIGHT))
            .flex()
            .flex_col()
            .gap(px(GAP_Y))
            .p(px(PAD))
            .rounded(px(RADIUS))
            .bg(palette.panel)
            .font_family(fonts::BODY)
            .text_size(px(13.))
            .text_color(palette.text)
            .shadow_lg()
            .occlude()
            .child(title_bar(browser.purpose, palette, on_event.clone()))
            .child(nav_bar(
                browser,
                palette,
                &term,
                search,
                path_field,
                on_event.clone(),
            ))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.))
                    .gap(px(GAP_X))
                    .child(sidebar(state, browser, palette, on_event.clone()))
                    .child(grid(browser, palette, on_event.clone())),
            )
            .when(browser.purpose.saves(), |this| {
                this.child(footer(
                    browser,
                    &state.volatile_settings.encoding_options,
                    palette,
                    filename_field,
                    quality,
                    compression,
                    on_event.clone(),
                ))
            })
            .into_any(),
    )
}

/// The centring layer. egui dimmed nothing behind its window, so this only centres, and keeps the
/// pointer from reaching the canvas behind the card.
fn backdrop(card: AnyElement) -> AnyElement {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .child(card)
        .into_any()
}

/// egui's window title bar: the "Open"/"Save" heading and the cross that closed the popup.
fn title_bar<F>(purpose: Purpose, palette: theme::Palette, on_event: F) -> Div
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    div()
        .flex()
        .items_center()
        .justify_between()
        .px(px(GAP_X))
        .child(div().text_size(px(15.)).child(purpose.title()))
        .child(icon_button(
            "browser-close",
            icons::X,
            Event::Close,
            palette,
            on_event,
        ))
}

/// egui's navigation row: search, up one level, the path/breadcrumb switch.
fn nav_bar<F>(
    browser: &State,
    palette: theme::Palette,
    term: &str,
    search: &Entity<InputState>,
    path_field: &Entity<InputState>,
    on_event: F,
) -> Div
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    let glyph = if browser.browser.search_active {
        icons::BOLDX
    } else {
        icons::SEARCH
    };
    let path_glyph = if browser.browser.path_active {
        icons::FOLDER
    } else {
        icons::TERMINAL
    };
    // egui gave the field the whole remaining width only once the term reached 10 *bytes*, and the
    // nav row wrapped right after it.
    let wide = term.len() >= 10;

    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(GAP_X))
        .px(px(GAP_X))
        .child(icon_button(
            "browser-search",
            glyph,
            Event::ToggleSearch,
            palette,
            on_event.clone(),
        ))
        .when(browser.browser.search_active, |this| {
            this.child(
                div()
                    .when(wide, |this| this.flex_1())
                    .when(!wide, |this| this.w(px(SEARCH_WIDTH)))
                    .child(Input::new(search).h(px(NAV))),
            )
        })
        .child(icon_button(
            "browser-up",
            icons::CHEVRON_UP,
            Event::Up,
            palette,
            on_event.clone(),
        ))
        .child(icon_button(
            "browser-path",
            path_glyph,
            Event::TogglePath,
            palette,
            on_event.clone(),
        ))
        .when(browser.browser.path_active, |this| {
            this.child(div().flex_1().child(Input::new(path_field).h(px(NAV))))
        })
        .when(!browser.browser.path_active, |this| {
            this.children(browser.breadcrumb(CRUMB_WIDTH).into_iter().map(|ancestor| {
                let label = ancestor
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Computer".to_string());
                crumb(label, ancestor, palette, on_event.clone())
            }))
        })
}

/// egui's `styled_selectable_label`: a padded button with the caret drawn at its right edge.
fn crumb<F>(label: String, path: PathBuf, palette: theme::Palette, on_event: F) -> AnyElement
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    div()
        .id(format!("crumb {label}"))
        .px(px(CRUMB_BUTTON_PAD * 2.))
        .h(px(NAV))
        .flex()
        .items_center()
        .gap(px(2.))
        .rounded(px(RADIUS))
        .cursor_pointer()
        .bg(palette.backdrop)
        .child(label)
        .child(
            div()
                .font_family(fonts::ICONS)
                .text_size(px(12.))
                .child(icons::CARET_RIGHT),
        )
        .on_click(move |_, window, app| on_event(Event::Go(path.clone()), window, app))
        .into_any()
}

/// egui's 120pt sidebar: "Home", this machine's drives, the four known folders, the bookmarks and
/// the outlined "+" that adds one.
fn sidebar<F>(
    state: &OculanteState,
    browser: &State,
    palette: theme::Palette,
    on_event: F,
) -> AnyElement
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    let settings = &state.volatile_settings;
    let mut rows: Vec<AnyElement> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        rows.push(
            side_row(
                icons::FOLDER,
                "Home".into(),
                Event::Go(home),
                palette,
                on_event.clone(),
            )
            .into_any(),
        );
    }
    for drive in browser.drives() {
        rows.push(
            side_row(
                icons::DRIVE,
                drive.name.clone(),
                Event::Go(drive.path.clone()),
                palette,
                on_event.clone(),
            )
            .into_any(),
        );
    }
    for (glyph, label, dir) in [
        (icons::FOLDERDESKTOP, "Desktop", dirs::desktop_dir()),
        (icons::FOLDERDOCUMENT, "Documents", dirs::document_dir()),
        (icons::FOLDERDOWNLOAD, "Downloads", dirs::download_dir()),
        (icons::FOLDERIMAGE, "Pictures", dirs::picture_dir()),
    ] {
        if let Some(path) = dir {
            rows.push(
                side_row(
                    glyph,
                    label.into(),
                    Event::Go(path),
                    palette,
                    on_event.clone(),
                )
                .into_any(),
            );
        }
    }
    for folder in &settings.folder_bookmarks {
        let label = folder
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        rows.push(
            side_row(
                icons::FOLDERBOOKMARK,
                label,
                Event::Go(folder.clone()),
                palette,
                on_event.clone(),
            )
            .tooltip(move |window, app| {
                Tooltip::new("Right click or 'd' to delete!").build(window, app)
            })
            .on_mouse_down(MouseButton::Right, {
                let folder = folder.clone();
                let on_event = on_event.clone();
                move |_, window, app| on_event(Event::RemoveBookmark(folder.clone()), window, app)
            })
            // egui tested `res.hovered()` in the same frame as `key_released(Key::D)`. gpui ends a
            // hover when the keyboard is used, so the listener has to stay position-driven to be
            // readable at the keystroke, which is where egui's own hover always sat.
            .hover_listener_mode(HoverListenerMode::InputModalityIndependent)
            .on_hover({
                let folder = folder.clone();
                let on_event = on_event.clone();
                move |hovered, window, app| {
                    on_event(Event::HoverBookmark(folder.clone(), *hovered), window, app)
                }
            })
            .into_any(),
        );
    }
    rows.push(add_bookmark(palette, on_event));

    div()
        .id("browser-sidebar")
        .w(px(SIDEBAR))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(GAP_Y))
        .overflow_y_scroll()
        .children(rows)
        .into_any()
}

/// egui's `styled_button`: 140x35, rounded, the glyph in the accent colour at its left edge.
fn side_row<F>(
    glyph: &'static str,
    label: String,
    event: Event,
    palette: theme::Palette,
    on_event: F,
) -> Stateful<Div>
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    div()
        .id(format!("browser-side {label}"))
        .w(px(SIDEBAR_BUTTON))
        .h(px(NAV))
        .flex()
        .items_center()
        .gap(px(6.))
        .pl(px(16.))
        .rounded(px(RADIUS))
        .cursor_pointer()
        .bg(palette.backdrop)
        .child(
            div()
                .flex_shrink_0()
                .font_family(fonts::ICONS)
                .text_size(px(16.))
                .text_color(palette.accent)
                .child(glyph),
        )
        .child(label)
        .on_click(move |_, window, app| on_event(event.clone(), window, app))
}

/// egui's "+" row: a transparent frame with a 2pt `weak_bg_fill` stroke around it.
fn add_bookmark<F>(palette: theme::Palette, on_event: F) -> AnyElement
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    div()
        .id("browser-add-bookmark")
        .w(px(SIDEBAR_BUTTON))
        .h(px(NAV))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS))
        .cursor_pointer()
        .border_2()
        .border_color(palette.text.opacity(0.35))
        .child(
            div()
                .font_family(fonts::ICONS)
                .text_size(px(16.))
                .text_color(palette.text.opacity(0.35))
                .child(icons::PLUS),
        )
        .on_click(move |_, window, app| on_event(Event::AddBookmark, window, app))
        .into_any()
}

/// A square nav-bar button whose whole label is an icon in the accent colour, as egui's were.
fn icon_button<F>(
    id: &'static str,
    glyph: &'static str,
    event: Event,
    palette: theme::Palette,
    on_event: F,
) -> Stateful<Div>
where
    F: Fn(Event, &mut Window, &mut App) + 'static,
{
    div()
        .id(id)
        .w(px(NAV))
        .h(px(NAV))
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .rounded(px(RADIUS))
        .cursor_pointer()
        .bg(palette.backdrop)
        .child(
            div()
                .font_family(fonts::ICONS)
                .text_size(px(16.))
                .text_color(palette.accent)
                .child(glyph),
        )
        .on_click(move |_, window, app| on_event(event.clone(), window, app))
}

/// The sunken grid panel: egui's `Frame` of `Color32::from_gray(13)` (217 when light), 10pt of
/// margin, and its `ScrollArea` inside.
fn grid<F>(browser: &State, palette: theme::Palette, on_event: F) -> Div
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    let gray = if palette.dark { 13 } else { 217 };
    let tiles = browser.tiles();
    div()
        .flex_1()
        .flex()
        .flex_col()
        .min_h(px(0.))
        .rounded(px(RADIUS))
        .bg(rgb(gray * 0x0001_0101))
        .p(px(10.))
        .child(
            div()
                .id("browser-scroll")
                .flex_1()
                .overflow_y_scroll()
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(GAP_X))
                        .children(
                            tiles
                                .is_empty()
                                .then(|| empty_directory(palette, on_event.clone())),
                        )
                        .children(tiles.iter().enumerate().map(|(index, entry)| {
                            tile(index, entry, browser, palette, on_event.clone())
                        })),
                ),
        )
}

/// egui's "Empty directory" label, which was clickable and went one level up.
fn empty_directory<F>(palette: theme::Palette, on_event: F) -> AnyElement
where
    F: Fn(Event, &mut Window, &mut App) + 'static,
{
    div()
        .id("browser-empty")
        .cursor_pointer()
        .text_color(palette.text)
        .child("Empty directory")
        .on_click(move |_, window, app| on_event(Event::Up, window, app))
        .into_any()
}

/// egui's `render_file_icon`: a 120x114 cell, the folder glyph or the thumbnail over 90pt and the
/// name in the 24pt line under it.
fn tile<F>(
    index: usize,
    entry: &Path,
    browser: &State,
    palette: theme::Palette,
    on_event: F,
) -> AnyElement
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    let name = entry
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let directory = entry.is_dir();
    let event = if directory {
        Event::Go(entry.to_path_buf())
    } else {
        Event::Pick(entry.to_path_buf())
    };
    let thumbnail = browser.thumbs.get(entry).cloned();
    let waiting = !directory && thumbnail.is_none();

    div()
        .id(format!("browser-tile {index} {name}"))
        .w(px(TILE_W))
        .h(px(TILE_H))
        .relative()
        .flex()
        .flex_col()
        .rounded(px(RADIUS))
        .cursor_pointer()
        .child(
            div()
                .w(px(TILE_W))
                .h(px(THUMB_H))
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .rounded(px(RADIUS))
                .when(waiting, |this| this.bg(rgb(PLACEHOLDER)))
                .children(thumbnail.map(|texture| {
                    img(ImageSource::Render(texture))
                        .size_full()
                        .object_fit(ObjectFit::Fill)
                }))
                // egui painted its `rect_filled` wash after the texture, so it lit the thumbnail up
                // rather than hiding behind it: the overlay is what puts the white on top.
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(px(RADIUS))
                        .hover(move |this| this.bg(rgb(0xff_ff_ff).opacity(HOVER_WASH))),
                ),
        )
        // egui drew the folder glyph at the centre of the whole 120x114 cell, not of the 90pt thumb
        // box, so it sits over the caption line as an overlay rather than as one of its children.
        .when(directory, |this| {
            this.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .font_family(fonts::ICONS)
                            .text_size(px(85.))
                            .text_color(palette.text)
                            .child(icons::FOLDERFILL),
                    ),
            )
        })
        .when(waiting, |this| {
            this.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(px(TILE_W))
                    .h(px(THUMB_H))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(25.))
                    .text_color(rgb(0xff_ff_ff))
                    .child(
                        entry
                            .extension()
                            .map(|ext| ext.to_string_lossy().to_string().to_uppercase())
                            .unwrap_or_default(),
                    ),
            )
        })
        .child(
            div()
                .w(px(TILE_W))
                .h(px(CAPTION_H))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .max_w(px(TILE_W))
                        .text_size(px(13.))
                        .text_color(palette.text)
                        .truncate()
                        .text_ellipsis()
                        .child(name.clone()),
                ),
        )
        .tooltip(move |window, app| Tooltip::new(name.clone()).build(window, app))
        .on_click(move |_, window, app| on_event(event.clone(), window, app))
        .into_any()
}

/// egui's save footer: the filename field, one button per offered encoder, "Save file", and under all
/// of that the options row of whichever encoder owns the current extension (`fe.ui(ui)`).
fn footer<F>(
    browser: &State,
    encoders: &[FileEncoder],
    palette: theme::Palette,
    filename_field: &Entity<InputState>,
    quality: &Entity<SliderState>,
    compression: &Entity<SliderState>,
    on_event: F,
) -> Div
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    let current = browser.extension();
    let filter = browser.str_filter();
    // egui walked the encoder list itself and skipped whatever the filter did not name, so the
    // buttons come in `FileEncoder`'s order, and the selected one is the filename's own extension
    // compared case-sensitively against the encoder's lowercase name.
    let buttons = FileEncoder::iter()
        .map(|encoder| encoder.ext())
        .filter(|ext| filter.contains(&ext.as_str()))
        .collect::<Vec<String>>();
    let row = encoder(&current, encoders);

    div()
        .flex()
        .flex_col()
        .gap(px(GAP_Y))
        .px(px(GAP_X))
        .child(div().text_size(px(15.)).child("Filename"))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(GAP_X))
                .child(
                    div()
                        .w(px(240.))
                        .child(Input::new(filename_field).h(px(28.))),
                )
                .children(buttons.into_iter().map(|ext| {
                    let chosen = ext == current;
                    ext_button(ext, chosen, palette, on_event.clone())
                }))
                .child(
                    div()
                        .id("browser-save")
                        .px(px(10.))
                        .h(px(NAV))
                        .flex()
                        .items_center()
                        .rounded(px(RADIUS))
                        .cursor_pointer()
                        .bg(palette.backdrop)
                        .child("   Save file   ")
                        .on_click(move |_, window, app| on_event(Event::Save, window, app)),
                ),
        )
        .children(row.map(|row| {
            let slider = match row {
                EncoderRow::Quality { .. } => quality,
                EncoderRow::Compression { .. } => compression,
            };
            div()
                .flex()
                .flex_col()
                .gap(px(GAP_Y))
                .child(div().text_size(px(15.)).child(match row {
                    EncoderRow::Quality { .. } => "Quality",
                    EncoderRow::Compression { .. } => "Compression Level",
                }))
                .child(slider_line(slider, row, palette))
        }))
}

/// egui's `styled_slider`: the rail, then the value right-aligned in the 40pt box it reserved for it,
/// then the caption egui's compression row put after the whole widget.
fn slider_line(slider: &Entity<SliderState>, row: EncoderRow, palette: theme::Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(GAP_X))
        .child(
            // egui's `styled_slider` sets `trailing_fill(true)`, so the solid fill sits to the right of
            // the handle; `.reverse()` is gpui-component's same convention.
            Slider::new(slider)
                .flex_1()
                .reverse()
                .bg(palette.accent)
                .text_color(palette.accent),
        )
        .child(
            div()
                .w(px(READOUT))
                .flex_shrink_0()
                .text_right()
                .child(format!("{}", row.value())),
        )
        .children(
            row.caption()
                .map(|label| div().text_size(px(15.)).child(label)),
        )
}

/// egui's extension buttons, which were `Button::new(ext).selected(ext == current)`.
fn ext_button<F>(ext: String, chosen: bool, palette: theme::Palette, on_event: F) -> AnyElement
where
    F: Fn(Event, &mut Window, &mut App) + Clone + 'static,
{
    let event = Event::Extension(ext.clone());
    div()
        .id(format!("browser-ext {ext}"))
        .px(px(10.))
        .h(px(NAV))
        .flex()
        .items_center()
        .rounded(px(RADIUS))
        .cursor_pointer()
        .bg(if chosen {
            palette.accent
        } else {
            palette.backdrop
        })
        .text_color(if chosen {
            palette.accent_text
        } else {
            palette.text
        })
        .child(ext)
        .on_click(move |_, window, app| on_event(event.clone(), window, app))
        .into_any()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use crate::file_encoder::{CompressionLevel, FileEncoder};
    use crate::filebrowser::BrowserDir;
    use crate::gpui_app::browser::{
        CRUMB_CHAR, CRUMB_WIDTH, EncoderRow, Purpose, State, encoder, encoder_values, set_encoder,
    };

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oculante-browser-{tag}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(tiles: &[&PathBuf]) -> Vec<String> {
        tiles
            .iter()
            .map(|entry| entry.file_name().unwrap().to_string_lossy().to_string())
            .collect()
    }

    /// egui's `read_dir_entries` skipped dotfiles and anything the filter did not name, and its grid
    /// then drew all directories before all files.
    #[test]
    fn listing_puts_directories_first_and_drops_other_files() {
        let dir = temp_dir("listing");
        fs::create_dir(dir.join("zeta")).unwrap();
        fs::create_dir(dir.join("alpha")).unwrap();
        fs::write(dir.join("b.png"), b"x").unwrap();
        fs::write(dir.join("a.png"), b"x").unwrap();
        fs::write(dir.join("skipped.txt"), b"x").unwrap();
        fs::write(dir.join(".hidden.png"), b"x").unwrap();

        let browser = State::new(dir.clone(), Purpose::Open, vec!["png".to_string()]);
        assert_eq!(names(&browser.tiles()), ["alpha", "zeta", "a.png", "b.png"]);
        let _ = fs::remove_dir_all(&dir);
    }

    /// The search filter egui applied every frame, through the same `BrowserState` helper.
    #[test]
    fn search_narrows_the_listing() {
        let dir = temp_dir("search");
        fs::write(dir.join("cat.png"), b"x").unwrap();
        fs::write(dir.join("dog.png"), b"x").unwrap();

        let mut browser = State::new(dir.clone(), Purpose::Open, vec!["png".to_string()]);
        browser.browser.search_term = "DO".into();
        assert_eq!(names(&browser.tiles()), ["dog.png"]);
        let _ = fs::remove_dir_all(&dir);
    }

    /// egui's breadcrumb walk counted ancestors from the deepest until the width ran out, then
    /// reversed what fit. Nothing ever fits out of a very short budget, and `max(1)` still shows the
    /// deepest folder alone.
    #[test]
    fn breadcrumb_fits_from_the_deepest_folder() {
        let root = temp_dir("crumbs");
        let dir = root.join("a").join("bb").join("ccc");
        fs::create_dir_all(&dir).unwrap();
        let browser = State::new(dir.clone(), Purpose::Open, vec!["png".to_string()]);

        let taken = |stem: &str| stem.len() as f32 * CRUMB_CHAR + 4. * 2. + 8. * 2.;

        let all = browser.breadcrumb(4000.);
        assert_eq!(all.last(), Some(&dir));
        assert!(all.len() >= 4, "got {:?}", all.len());

        let two = browser.breadcrumb(taken("ccc") + taken("bb") + 1.);
        assert_eq!(two, [dir.parent().unwrap().to_path_buf(), dir.clone()]);

        let one = browser.breadcrumb(taken("ccc") + taken("bb") - 1.);
        assert_eq!(one, [dir.clone()]);

        assert_eq!(browser.breadcrumb(0.), [dir.clone()]);
        let _ = fs::remove_dir_all(&root);
    }

    /// The default card width is what egui measured its crumbs against: the nav bar's three buttons
    /// and their spacing are already gone from it.
    #[test]
    fn crumb_budget_leaves_room_for_the_nav_buttons() {
        assert!(CRUMB_WIDTH > 500., "got {CRUMB_WIDTH}");
        assert!(CRUMB_WIDTH < 822.);
    }

    /// egui's footer rewrote a name whose extension the filter does not offer, taking the filter's
    /// first item; the LUT and ASE popups relied on that.
    #[test]
    fn filename_keeps_an_extension_the_filter_offers() {
        let dir = temp_dir("filename");
        let mut browser = State::new(dir.clone(), Purpose::Palette, vec!["ase".to_string()]);
        browser.set_filename("palette.png");
        assert_eq!(browser.browser.filename, "palette.ase");
        assert_eq!(browser.extension(), "ase");

        browser.set_extension("png");
        assert_eq!(browser.browser.filename, "palette.ase");
        browser.filter = vec!["png".to_string()];
        browser.set_extension("png");
        assert_eq!(browser.browser.filename, "palette.png");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Opening scans the path it was handed; a path that cannot be read leaves no listing rather
    /// than a stale one, which is what egui's "Empty directory" row then showed.
    #[test]
    fn opening_scans_the_path_it_was_given() {
        let dir = temp_dir("scanned");
        fs::write(dir.join("one.png"), b"x").unwrap();
        let browser = State::new(dir.clone(), Purpose::Open, vec!["png".to_string()]);
        assert_eq!(browser.browser.entries.as_ref().unwrap().len(), 1);

        let browser = State::new(dir.join("nowhere"), Purpose::Open, vec!["png".to_string()]);
        assert!(browser.browser.entries.is_none());
        assert!(browser.tiles().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    /// `go_up` is egui's `path.parent()`: the listing follows it, and a directory that only holds
    /// subdirectories still shows those.
    #[test]
    fn going_up_replaces_the_listing() {
        let dir = temp_dir("up");
        let child = dir.join("child");
        fs::create_dir_all(&child).unwrap();
        fs::write(child.join("in.png"), b"x").unwrap();
        fs::write(dir.join("outer.png"), b"x").unwrap();

        let mut browser = State::new(child.clone(), Purpose::Open, vec!["png".to_string()]);
        assert_eq!(names(&browser.tiles()), ["in.png"]);
        assert_eq!(browser.current_dir(), child);
        browser.go_up();
        assert_eq!(browser.path, dir);
        assert_eq!(names(&browser.tiles()), ["child", "outer.png"]);
        let _ = fs::remove_dir_all(&dir);
    }

    /// egui's browser-button reset dropped the parked listing wholesale — search term, filename,
    /// nav flags and generated thumbnails went with it, and only the directory mode was carried
    /// over. With no entries left, the directory was read again on the frame that followed.
    #[test]
    fn resetting_the_listing_drops_everything_but_the_mode() {
        let dir = temp_dir("reset");
        fs::write(dir.join("one.png"), b"x").unwrap();

        let mut browser = State::new(dir.clone(), Purpose::Open, vec!["png".to_string()]);
        browser.browser.search_term = "one".into();
        browser.browser.search_active = true;
        browser.browser.path_active = true;
        browser.set_filename("renamed.png");
        browser.reset_listing(BrowserDir::CurrentImageDir);

        assert_eq!(browser.browser.last_dir, BrowserDir::CurrentImageDir);
        assert!(browser.browser.entries.is_none());
        assert!(browser.browser.search_term.is_empty());
        assert!(!browser.browser.search_active);
        assert!(!browser.browser.path_active);
        assert_eq!(browser.browser.filename, "unnamed.png");
        assert!(browser.browser.drives.is_some());

        browser.set_path(dir.clone());
        assert_eq!(names(&browser.tiles()), ["one.png"]);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A path that is a file rather than a directory reports its parent, which is what egui's
    /// `current_dir` did before it compared the crumbs against it.
    #[test]
    fn a_file_path_reports_its_parent() {
        let dir = temp_dir("file-path");
        let file = dir.join("x.png");
        fs::write(&file, b"x").unwrap();
        let browser = State::new(file, Purpose::Open, vec!["png".to_string()]);
        assert_eq!(browser.current_dir(), dir);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Tiles are looked up in the thumbnail map by the path the listing holds, so a path that is not
    /// there simply renders the placeholder.
    #[test]
    fn thumbnails_are_keyed_by_the_listing_path() {
        let dir = temp_dir("thumbs");
        fs::write(dir.join("x.png"), b"x").unwrap();
        let browser = State::new(dir.clone(), Purpose::Open, vec!["png".to_string()]);
        assert!(browser.thumbs.is_empty());
        assert!(browser.thumbs.get(browser.tiles()[0].as_path()).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    /// The four popups egui had, and which of them drew the filename footer.
    #[test]
    fn only_the_savers_get_a_filename_footer() {
        assert!(Purpose::Save.saves());
        assert!(Purpose::Palette.saves());
        assert!(!Purpose::Open.saves());
        assert!(!Purpose::Lut(crate::gpui_app::edit_panel::Stack::Image, 0).saves());
        assert_eq!(Purpose::Open.title(), "Open");
        assert_eq!(Purpose::Save.title(), "Save");
        assert_eq!(Purpose::Palette.title(), "Save");
        // egui's `Window::new(if save { "Save" } else { "Open" })` keyed its title on the `save`
        // flag alone, so the LUT popup it opened with `save: false` read "Open" like the opener.
        assert_eq!(
            Purpose::Lut(crate::gpui_app::edit_panel::Stack::Pixel, 3).title(),
            "Open"
        );
    }

    /// A missing directory has nothing to page through.
    #[test]
    fn a_bogus_path_has_no_tiles() {
        let browser = State::new(
            Path::new("z:/definitely-not-here").to_path_buf(),
            Purpose::Open,
            vec!["png".to_string()],
        );
        assert!(browser.tiles().is_empty());
    }

    /// egui's `fe.ui(ui)` ran for the encoder whose lowercase name the filename's extension matched, and
    /// drew nothing at all for the three whose arms were empty.
    #[test]
    fn only_the_encoders_with_options_draw_a_row() {
        let encoders = vec![
            FileEncoder::Jpg { quality: 75 },
            FileEncoder::WebP,
            FileEncoder::Png {
                compressionlevel: CompressionLevel::Best,
            },
            FileEncoder::Bmp,
            FileEncoder::Avif,
        ];
        assert_eq!(
            encoder("jpg", &encoders),
            Some(EncoderRow::Quality { value: 75 })
        );
        assert_eq!(
            encoder("JPG", &encoders),
            Some(EncoderRow::Quality { value: 75 })
        );
        assert_eq!(
            encoder("png", &encoders),
            Some(EncoderRow::Compression { level: 2 })
        );
        for ext in ["webp", "bmp", "avif", "ase", ""] {
            assert_eq!(encoder(ext, &encoders), None, "{ext}");
        }
    }

    /// egui's slider held the `&mut` it wrote on every frame the pointer moved, so a drag landed on the
    /// encoder that owned the extension and nowhere else. Each row clamps to its own range.
    #[test]
    fn a_drag_lands_on_the_encoder_that_owns_the_extension() {
        let mut encoders = vec![
            FileEncoder::Jpg { quality: 75 },
            FileEncoder::Png {
                compressionlevel: CompressionLevel::Default,
            },
        ];
        assert!(set_encoder("jpg", &mut encoders, 12));
        assert!(!set_encoder("jpg", &mut encoders, 12));
        assert!(set_encoder("PNG", &mut encoders, 2));
        assert_eq!(
            encoder("jpg", &encoders),
            Some(EncoderRow::Quality { value: 12 })
        );
        assert_eq!(
            encoder("png", &encoders),
            Some(EncoderRow::Compression { level: 2 })
        );
        assert!(!set_encoder("webp", &mut encoders, 1));
        assert!(!set_encoder("cube", &mut encoders, 1));
        assert!(set_encoder("jpg", &mut encoders, 900));
        assert_eq!(
            encoder("jpg", &encoders),
            Some(EncoderRow::Quality { value: 100 })
        );
        // Both rails clamp to their own range, so a value past the end that lands on what the encoder
        // already holds reports no change.
        assert!(!set_encoder("png", &mut encoders, 9));
        assert_eq!(
            encoder("png", &encoders),
            Some(EncoderRow::Compression { level: 2 })
        );
        assert!(set_encoder("png", &mut encoders, 0));
        assert_eq!(
            encoder("png", &encoders),
            Some(EncoderRow::Compression { level: 0 })
        );
    }

    /// The seeds the footer's two slider entities are built from, so swapping the extension shows the
    /// value that encoder holds instead of the previous row's.
    #[test]
    fn the_seeds_follow_each_encoder_not_the_row_order() {
        let encoders = vec![
            FileEncoder::Jpg { quality: 3 },
            FileEncoder::Png {
                compressionlevel: CompressionLevel::Fast,
            },
        ];
        assert_eq!(encoder_values(&encoders), (3, 0));
        assert_eq!(encoder_values(&[]), (0, 1));
    }
}
