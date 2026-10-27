use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{InputEvent as TextEvent, InputState};
use gpui_kit::component::slider::{SliderEvent, SliderState, SliderValue};
use gpui_kit::*;
use image::{ColorType, DynamicImage};
use log::{debug, error};
use nalgebra::Vector2;
use quantette::{ColorSpace, PalettePipeline};

use crate::appstate::{Message, OculanteState};
use crate::comparelist::CompareItem;
use crate::file_encoder;
use crate::filebrowser::BrowserDir;
#[cfg(not(feature = "file_open"))]
use crate::gpui_app::browser;
use crate::gpui_app::{
    checker, crop, edit_panel, info_panel, keys, measure, modal, native, panel, settings, texture,
    theme, tiles, toasts, top_bar,
};
use crate::image_editing::{self, ColorTypeExt, ImageOperation, ImgOpItem, MeasureShape};
use crate::shortcuts::{self, InputEvent, KeyEvent, Shortcut};
use crate::utils::{self, ColorChannel, Frame, ImageExt as _};

const POLL_INTERVAL: Duration = Duration::from_millis(16);

/// Geometry of the bottom scrub bar, matching the egui shell's 22pt bottom panel.
const SCRUB_HEIGHT: f32 = 22.;
const SCRUB_INSET: f32 = 12.;
/// Gutter reserved at the right end of the bar for the "index/total" readout.
const SCRUB_COUNTER: f32 = 80.;
const SCRUB_THUMB: f32 = 14.;

/// egui's `styled_slider(&mut state.tiling, 1..=10)` upper bound.
const TILING_MAX: f32 = 10.;

/// Commands the gpui shell answers already; panel-dependent ones arrive with the UI phases.
/// `CompareNext` is missing because egui never bound it either.
const HANDLED: [InputEvent; 37] = [
    InputEvent::Quit,
    InputEvent::Browse,
    InputEvent::Fullscreen,
    InputEvent::AlwaysOnTop,
    InputEvent::NextImage,
    InputEvent::PreviousImage,
    InputEvent::FirstImage,
    InputEvent::LastImage,
    InputEvent::ResetView,
    InputEvent::ZoomIn,
    InputEvent::ZoomOut,
    InputEvent::ZoomActualSize,
    InputEvent::ZoomDouble,
    InputEvent::ZoomThree,
    InputEvent::ZoomFour,
    InputEvent::ZoomFive,
    InputEvent::PanLeft,
    InputEvent::PanRight,
    InputEvent::PanUp,
    InputEvent::PanDown,
    InputEvent::ClearImage,
    InputEvent::DeleteFile,
    InputEvent::Copy,
    InputEvent::CopyPath,
    InputEvent::Paste,
    InputEvent::ScrubBar,
    InputEvent::ZenMode,
    InputEvent::InfoMode,
    InputEvent::EditMode,
    InputEvent::RedChannel,
    InputEvent::GreenChannel,
    InputEvent::BlueChannel,
    InputEvent::AlphaChannel,
    InputEvent::RGBChannel,
    InputEvent::RGBAChannel,
    InputEvent::LosslessRotateRight,
    InputEvent::LosslessRotateLeft,
];

/// egui's arrow-key panning step.
const PAN_STEP: f32 = 40.;

pub struct Viewer {
    pub state: OculanteState,
    /// The frame as gpui can hold it: `cols x rows` sprites, each side at most 16384 px.
    image: Option<Arc<tiles::Frame>>,
    /// egui's checker texture, loaded once in `on_start` and repeated behind the image. Kept with
    /// the DPI it was baked for, since the tile has to stay 1:1 with the device grid.
    checker: Option<(f32, Arc<RenderImage>)>,
    last_cursor: Point<Pixels>,
    last_window_size: Size<Pixels>,
    /// egui's `egui_started_press` for a middle-button press: which chrome owned the pointer when
    /// the button went down, held for the whole press so a drag leaving it never pans.
    middle_claimed: bool,
    /// Whether the viewport itself saw the button that is currently held.
    ///
    /// egui's pan branches both read a button state it latched from a press event *it* received
    /// (`primary_down`, `button_down(Middle)`), so a press that began on the OS caption — where
    /// Windows' `SC_MOVE` loop keeps forwarding client `WM_MOUSEMOVE`s with `MK_*BUTTON` set — never
    /// moved its canvas. gpui derives `pressed_button` (and therefore `dragging()`, which is just
    /// `pressed_button == Left`) from the wparam, i.e. from the physical button state, so the port has
    /// to record the press itself or a title-bar drag slides the image along with the window.
    press_seen: bool,
    /// Whole client area, including the bars. `state.window_size` only covers the content
    /// between them, so bottom-anchored hit tests need this.
    raw_size: Vector2<f32>,
    /// Dropdown the top bar is showing, if any.
    popup: Option<top_bar::Popup>,
    /// Which of the info panel's metadata headers are expanded.
    info_sections: info_panel::Sections,
    /// egui's `"filter_open"` temp tag: the Filters header of the edit panel.
    filters_open: bool,
    /// egui's collapsing state for the "Lossless JPEG edits" header, which only the `turbo`
    /// feature draws.
    lossless_open: bool,
    /// egui kept the filter search text in temp data; gpui's input owns its own.
    filter_search: Entity<InputState>,
    /// The edit panel's per-operator widgets, which gpui keeps in entities rather than binding
    /// them to the operators the way egui did.
    params: edit_panel::Params,
    /// The swatches the Palette section shows. egui kept them in its `"picker"` temp memory,
    /// which has no gpui equivalent.
    palette: Vec<[u8; 4]>,
    /// Owner of the info panel's tiling slider. gpui-component keeps slider drag state in an
    /// entity, so the viewer has to outlive each render for the drag to survive a repaint.
    tiling: Entity<SliderState>,
    /// Kept so a quantette pass is not cancelled by its task being dropped.
    palette_task: Option<Task<()>>,
    /// egui gated every delete behind a confirmation modal.
    confirm_delete: bool,
    /// Same for overwriting the file on disk.
    confirm_overwrite: bool,
    /// Card the pointer is over; its countdown is paused while it stays hovered.
    hovered_toast: Option<u64>,
    /// Close box the pointer is over, which only decides how the glyph is painted.
    hovered_close: Option<u64>,
    last_tick: Instant,
    /// egui's `pointer.time_since_last_movement()` clock (HEAD `src/app.rs:1104`), which the zen-mode
    /// idle hide reads.
    last_pointer_move: Instant,
    /// Whether the OS pointer is currently hidden on this shell's behalf, so the counter is only
    /// stepped on a real transition.
    cursor_hidden: bool,
    /// egui's debug frame overlay read three things gpui exposes no getter for, so this view keeps
    /// its own: the interval between two renders (`stable_dt`), how many frames have been drawn
    /// (`cumulative_pass_nr`, which egui documents as one pass per rendered frame), and whether a
    /// repaint is pending (`has_requested_repaint`). The last is set wherever the pump schedules a
    /// frame, which is this shell's counterpart of egui's channel-driven repaints.
    #[cfg(debug_assertions)]
    debug_last_frame: Option<Instant>,
    #[cfg(debug_assertions)]
    debug_fps: f32,
    #[cfg(debug_assertions)]
    debug_passes: u64,
    #[cfg(debug_assertions)]
    debug_repaint: bool,
    title: String,
    reset_after_upload: bool,
    last_frame_was_compared_image: bool,
    /// Kept so the pending path prompt is not cancelled by its task being dropped.
    #[cfg(feature = "file_open")]
    browse_task: Option<Task<()>>,
    /// Same, for the LUT row's picker, which writes the row it was opened from rather than the view.
    /// Only the `file_open` build has a picker to keep alive; the default build's LUT row uses the
    /// in-app browser, whose completion runs through `browser_event`.
    #[cfg(feature = "file_open")]
    lut_task: Option<Task<()>>,
    /// egui's `FBSTATE` temp key: the browser's listing, which its popup memory parked rather than
    /// dropped, so a reopen came back to the same directory — and, since nothing rescanned it, to
    /// the same stale file list.
    #[cfg(not(feature = "file_open"))]
    browser: Option<browser::State>,
    /// The flag egui's `is_popup_open(Id)` stood for.
    #[cfg(not(feature = "file_open"))]
    browser_shown: bool,
    /// egui's `FBPATH`: the directory on show, which outlived the window and was written back to
    /// `volatile_settings.last_open_directory` only when a file was picked.
    #[cfg(not(feature = "file_open"))]
    browser_path: PathBuf,
    /// egui's three text edits, which were bound straight to the browser state. gpui's inputs own
    /// their text, so these are re-seated whenever the listing moves under them.
    #[cfg(not(feature = "file_open"))]
    browser_search: Entity<InputState>,
    #[cfg(not(feature = "file_open"))]
    browser_path_field: Entity<InputState>,
    #[cfg(not(feature = "file_open"))]
    browser_filename: Entity<InputState>,
    /// egui's `prev_path == PathBuf::default()` first-frame test, which seated the fields and asked
    /// for the filename field's focus. The request lands in the next render, the only place that
    /// has a window to make it in — which is also when egui's `request_focus` took effect. Set on
    /// every open rather than only the first one, because egui's edits were bound to the model and
    /// could not lag behind it.
    #[cfg(not(feature = "file_open"))]
    browser_seating: bool,
    /// The half of that same test that egui's save footer alone answered: the filename field took
    /// the keyboard on the frames that started without a listing, which is the frame after an open
    /// that had to read the directory again.
    #[cfg(not(feature = "file_open"))]
    browser_refocus: bool,
    #[cfg(not(feature = "file_open"))]
    _browser_inputs: Vec<Subscription>,
    /// egui's save footer drew its encoder options as sliders bound straight to
    /// `settings.encoding_options`. gpui bakes a slider's range into the entity, so the two rows keep
    /// one each and the footer shows whichever extension is on show.
    #[cfg(not(feature = "file_open"))]
    browser_quality: Entity<SliderState>,
    #[cfg(not(feature = "file_open"))]
    browser_compression: Entity<SliderState>,
    /// The extension those two were last seated for: a different extension is the only thing that makes
    /// a row read the settings again, and it arrives from the footer's buttons or from a typed name.
    #[cfg(not(feature = "file_open"))]
    browser_encoder_ext: String,
    #[cfg(not(feature = "file_open"))]
    _browser_encoders: Vec<Subscription>,
    /// egui streamed thumbnails on every frame its window was drawn; this timer does the same
    /// while it is open and stops asking when it is not.
    #[cfg(not(feature = "file_open"))]
    _browser_poll: Option<Task<()>>,
    /// egui's `"compare"` temp tag: whatever finishes loading next joins the compare list.
    pending_compare: bool,
    /// egui's `"crop"` temp tag: whether the perspective crop has been applied, which is what hid
    /// its overlay and swapped "Apply" for "Reset". One global flag, not one per operator.
    crop_applied: bool,
    /// egui's `"pt"` temp tag: the handle index the pointer owns.
    crop_grab: Option<usize>,
    /// The palette gpui-component's global theme was last synced to; its widgets would otherwise
    /// keep gpui's own defaults.
    applied_palette: Option<theme::Palette>,
    /// Which operator's combo box is unfolded, by operator id and field.
    open_combo: Option<(u64, &'static str)>,
    _keystrokes: Option<Subscription>,
    /// Keeps the view dirty when the window itself changes size: gpui updates its own model from
    /// `WM_SIZE` but leaves an unchanged view painted at the old bounds, and egui re-ran the whole
    /// UI on eframe's resize event.
    _bounds: Option<Subscription>,
    /// A dropped Subscription unsubscribes, so the tiling listener needs a home.
    _tiling: Option<Subscription>,
    /// Same for the filter search field, whose changes repaint the filter list.
    _filter_search: Option<Subscription>,
    _poll: Option<Task<()>>,
    /// The `DragValue` field under the pointer, if any; its value follows the pointer until
    /// release, the way egui's drag wrote the field on every frame it moved.
    num_drag: Option<NumDrag>,
    /// egui kept each side panel's `PanelState` rect in its own memory; these widths are that.
    info_width: f32,
    edit_width: f32,
    /// The panel whose resize strip currently has the pointer, tracked like `num_drag`.
    panel_drag: Option<panel::Side>,
    /// The Preferences window's own widgets, which gpui keeps in entities rather than binding to
    /// the settings the way egui bound its `SettingsUiState`.
    settings_form: settings::Form,
    /// A settings `DragValue` under the pointer, tracked the same way as `num_drag`.
    settings_drag: Option<settings::Drag>,
    /// A stroke row's width readout under the pointer, tracked the same way again.
    stroke_drag: Option<(usize, edit_panel::NumGrab)>,
    /// Which stroke row's brush combo is unfolded.
    stroke_combo: Option<usize>,
    /// egui read `primary_down()` / `secondary_down()` off live input state every frame; these are
    /// gpui's reported button state, kept fresh by `mouse_moved`.
    left_down: bool,
    right_down: bool,
    /// egui's keybinding editor read the keys it held down off the input state; this is the chord
    /// the Assign buttons offer. It sticks until one is clicked, because gpui reports presses only.
    pending_shortcut: Option<Shortcut>,
    /// Watches on the settings widgets, so a typed or picked value repaints and is then copied into
    /// the settings.
    settings_watch: HashSet<EntityId>,
    settings_subs: Vec<Subscription>,
}

/// A grabbed numeric field: which row, and the grab the panel reported.
#[derive(Clone, Copy)]
struct NumDrag {
    stack: edit_panel::Stack,
    index: usize,
    grab: edit_panel::NumGrab,
}

impl Viewer {
    pub fn new(state: OculanteState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tiling = cx.new(|_| {
            SliderState::new()
                .min(1.)
                .max(TILING_MAX)
                .step(1.)
                .default_value(state.tiling as f32)
        });
        let filter_search = cx.new(|cx| InputState::new(window, cx).placeholder("Search filters"));
        // egui's `FBPATH` started out as the last opened directory, and its three text edits
        // carried whatever the parked listing said.
        #[cfg(not(feature = "file_open"))]
        let browser_path = state.volatile_settings.last_open_directory.clone();
        #[cfg(not(feature = "file_open"))]
        let browser_search = cx.new(|cx| InputState::new(window, cx));
        #[cfg(not(feature = "file_open"))]
        let browser_path_field = cx.new(|cx| {
            InputState::new(window, cx).default_value(browser_path.to_string_lossy().to_string())
        });
        #[cfg(not(feature = "file_open"))]
        let browser_filename = cx.new(|cx| InputState::new(window, cx));
        // Seated from the settings by `seat_encoders` on the first frame the footer is drawn, the same
        // way the three text edits are seated from the listing.
        #[cfg(not(feature = "file_open"))]
        let browser_quality = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(browser::QUALITY_MAX as f32)
                .step(1.)
        });
        #[cfg(not(feature = "file_open"))]
        let browser_compression = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(browser::COMPRESSION_MAX as f32)
                .step(1.)
        });
        let mut viewer = Self {
            state,
            image: None,
            checker: None,
            last_cursor: window.mouse_position(),
            last_window_size: window.bounds().size,
            middle_claimed: false,
            press_seen: false,
            raw_size: Vector2::new(
                window.bounds().size.width.as_f32(),
                window.bounds().size.height.as_f32(),
            ),
            popup: None,
            info_sections: Default::default(),
            filters_open: false,
            lossless_open: false,
            filter_search,
            params: edit_panel::Params::new(),
            palette: Vec::new(),
            palette_task: None,
            tiling,
            confirm_delete: false,
            confirm_overwrite: false,
            hovered_toast: None,
            hovered_close: None,
            last_tick: Instant::now(),
            // egui's clock started with the app, so a pointer that never moved still ages out.
            last_pointer_move: Instant::now(),
            cursor_hidden: false,
            #[cfg(debug_assertions)]
            debug_last_frame: None,
            #[cfg(debug_assertions)]
            debug_fps: 0.,
            #[cfg(debug_assertions)]
            debug_passes: 0,
            #[cfg(debug_assertions)]
            debug_repaint: false,
            title: String::new(),
            reset_after_upload: false,
            last_frame_was_compared_image: false,
            #[cfg(feature = "file_open")]
            browse_task: None,
            #[cfg(feature = "file_open")]
            lut_task: None,
            #[cfg(not(feature = "file_open"))]
            browser: None,
            #[cfg(not(feature = "file_open"))]
            browser_shown: false,
            #[cfg(not(feature = "file_open"))]
            browser_path,
            #[cfg(not(feature = "file_open"))]
            browser_search,
            #[cfg(not(feature = "file_open"))]
            browser_path_field,
            #[cfg(not(feature = "file_open"))]
            browser_filename,
            #[cfg(not(feature = "file_open"))]
            browser_quality,
            #[cfg(not(feature = "file_open"))]
            browser_compression,
            #[cfg(not(feature = "file_open"))]
            browser_encoder_ext: String::new(),
            #[cfg(not(feature = "file_open"))]
            _browser_encoders: Vec::new(),
            #[cfg(not(feature = "file_open"))]
            browser_seating: false,
            #[cfg(not(feature = "file_open"))]
            browser_refocus: false,
            #[cfg(not(feature = "file_open"))]
            _browser_inputs: Vec::new(),
            #[cfg(not(feature = "file_open"))]
            _browser_poll: None,
            pending_compare: false,
            crop_applied: false,
            crop_grab: None,
            applied_palette: None,
            open_combo: None,
            num_drag: None,
            info_width: panel::WIDTH,
            edit_width: panel::WIDTH,
            panel_drag: None,
            settings_form: settings::Form::new(),
            settings_drag: None,
            stroke_drag: None,
            stroke_combo: None,
            left_down: false,
            right_down: false,
            pending_shortcut: None,
            settings_watch: HashSet::new(),
            settings_subs: Vec::new(),
            _keystrokes: None,
            _bounds: None,
            _tiling: None,
            _filter_search: None,
            _poll: None,
        };

        // egui scaled everything with `ctx.set_pixels_per_point(dpi * ui_scale)` (src/app.rs:305).
        // gpui has no way to do that: `Window::set_scale_factor` is `#[cfg(any(test, feature =
        // "test-support"))]` and gpui-kit only enables that feature for its own dev-dependencies,
        // while `Window::bounds_changed` re-reads `platform_window.scale_factor()` on every resize.
        // `set_rem_size` is public but only resizes rem-based styles, and this UI is absolute-px,
        // so `ui_scale` is stored, shown and persisted without ever being applied.

        // Observe every keystroke: this app registers no gpui keymap actions, so filtering on
        // `event.action` could only ever lose chords that gpui's default bindings happen to name.
        // Text fields are handled the egui way, by setting `state.key_grab` while focused.
        viewer._keystrokes = Some(cx.observe_keystrokes(|this, event, window, cx| {
            this.handle_keystroke(&event.keystroke, window, cx);
        }));

        // eframe handed egui a resize event and the whole UI re-ran at the new size. gpui's resize
        // hook updates its own model but repaints nothing while the view is unchanged, which leaves
        // a maximised or dragged window showing the old layout, so the bounds dirty the view.
        viewer._bounds = Some(cx.observe_window_bounds(window, |_, _, cx| cx.notify()));

        // egui bound the tiling slider straight to `state.tiling`; gpui keeps the drag in its own
        // entity, so the value is copied back out on every change.
        viewer._tiling = Some(
            cx.subscribe(&viewer.tiling, |this, _, event, cx| match event {
                SliderEvent::Change(value) | SliderEvent::Release(value) => {
                    let raw = match value {
                        SliderValue::Single(v) => v,
                        SliderValue::Range(_, end) => end,
                    };
                    let next = raw.round().clamp(1., TILING_MAX) as usize;
                    if this.state.tiling != next {
                        this.state.tiling = next;
                        cx.notify();
                    }
                }
            }),
        );

        // egui's footer sliders held a `&mut` into `settings.encoding_options` and wrote it on every
        // frame the pointer moved, so the drag is copied out the same way: onto whichever encoder owns
        // the filename's extension, which is also the one the save will use.
        #[cfg(not(feature = "file_open"))]
        {
            let quality = cx.subscribe(&viewer.browser_quality, |this, _, event, cx| {
                this.set_encoder_value(event, cx);
            });
            let compression = cx.subscribe(&viewer.browser_compression, |this, _, event, cx| {
                this.set_encoder_value(event, cx);
            });
            viewer._browser_encoders = vec![quality, compression];
        }

        // The filter list is rebuilt off the field's text, and the window's keystrokes are read by
        // the app itself, so a focused field has to claim them the way egui's `key_grab` did.
        viewer._filter_search =
            Some(
                cx.subscribe(&viewer.filter_search, |this, _, event, cx| match event {
                    TextEvent::Change => cx.notify(),
                    TextEvent::Focus => this.state.key_grab = true,
                    TextEvent::Blur => this.state.key_grab = false,
                    TextEvent::PressEnter { .. } => {}
                }),
            );

        let weak = cx.entity().downgrade();
        // A dropped Task is cancelled, so the handle must stay owned by the entity.
        viewer._poll = Some(cx.spawn(async move |_, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                cx.update(|app| {
                    let _ = weak.update(app, |viewer, cx| viewer.process_channels(cx));
                });
            }
        }));

        // egui bound the browser's text edits straight to `FBSTATE`; these copy the same values
        // across as they are typed, and claim the window's shortcuts while a field is focused.
        #[cfg(not(feature = "file_open"))]
        {
            let mut inputs = Vec::new();
            inputs.push(
                cx.subscribe(&viewer.browser_search, |this, _, event, cx| match event {
                    TextEvent::Change => {
                        let term = this.browser_search.read(cx).value().to_string();
                        if let Some(browser) = this.browser.as_mut() {
                            browser.browser.search_term = term;
                        }
                        cx.notify();
                    }
                    TextEvent::Focus => this.state.key_grab = true,
                    TextEvent::Blur => this.state.key_grab = false,
                    TextEvent::PressEnter { .. } => {}
                }),
            );
            // egui wrote the typed text into `*path` on every change, so the listing followed the
            // field as it was typed.
            inputs.push(cx.subscribe(
                &viewer.browser_path_field,
                |this, _, event, cx| match event {
                    TextEvent::Change => {
                        let typed = this.browser_path_field.read(cx).value().to_string();
                        if let Some(browser) = this.browser.as_mut() {
                            browser.set_path(PathBuf::from(typed));
                        }
                        cx.notify();
                    }
                    TextEvent::Focus => this.state.key_grab = true,
                    TextEvent::Blur => this.state.key_grab = false,
                    TextEvent::PressEnter { .. } => {}
                },
            ));
            inputs.push(
                cx.subscribe(&viewer.browser_filename, |this, _, event, cx| match event {
                    TextEvent::Change => {
                        let typed = this.browser_filename.read(cx).value().to_string();
                        if let Some(browser) = this.browser.as_mut() {
                            browser.set_filename(&typed);
                        }
                        cx.notify();
                    }
                    TextEvent::Focus => this.state.key_grab = true,
                    TextEvent::Blur => this.state.key_grab = false,
                    TextEvent::PressEnter { .. } => {}
                }),
            );
            viewer._browser_inputs = inputs;

            let weak = cx.entity().downgrade();
            viewer._browser_poll = Some(cx.spawn(async move |_, cx| {
                loop {
                    cx.background_executor().timer(browser::POLL_INTERVAL).await;
                    let _ = cx.update(|app| {
                        let _ = weak.update(app, |viewer, cx| viewer.poll_browser(cx));
                    });
                }
            }));
        }

        viewer
    }

    fn handle_keystroke(
        &mut self,
        stroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // egui's `keybinding_ui` set `state.key_grab` for every frame the window was open, so no
        // shortcut fired behind it, and it built the held chord from the input state. GPUI only
        // reports presses, so the chord here is the last key down and stays offered until an Assign
        // button takes it.
        if self.state.settings_enabled {
            if let Some(key) = keys::to_key(&stroke.key, stroke.key_char.as_deref()) {
                self.pending_shortcut = Some(Shortcut {
                    keys: BTreeSet::from([key]),
                    modifiers: keys::to_modifiers(&stroke.modifiers),
                });
                cx.notify();
            }
            return;
        }
        // egui checked Escape from inside the browser window's own closure, and its input state kept
        // reporting the press while one of the browser's text fields had focus — so the close runs
        // ahead of the keyboard grab, and only while the window is on screen.
        #[cfg(not(feature = "file_open"))]
        if self.browser_shown && stroke.key == "escape" {
            self.close_browser(window, cx);
            return;
        }
        // egui's second bookmark delete route (HEAD src/filebrowser.rs:432-437): `d` released while a
        // bookmark row was hovered and no text field had the keyboard. Its `key_released` ignored
        // modifiers, so this does too, and the row still has to be hovered — a stray `d` in the
        // listing does nothing, exactly as the tooltip promises.
        #[cfg(not(feature = "file_open"))]
        if self.browser_shown && !self.state.key_grab && stroke.key == "d" {
            let hovered = self
                .browser
                .as_ref()
                .and_then(|state| state.hovered_bookmark.clone());
            if let Some(path) = hovered {
                self.state.volatile_settings.folder_bookmarks.remove(&path);
                if let Some(state) = self.browser.as_mut() {
                    state.hovered_bookmark = None;
                }
                cx.notify();
                return;
            }
        }
        if self.state.key_grab {
            return;
        }
        let Some(key) = keys::to_key(&stroke.key, stroke.key_char.as_deref()) else {
            return;
        };
        let event = KeyEvent {
            key,
            modifiers: keys::to_modifiers(&stroke.modifiers),
        };

        let triggered: Vec<InputEvent> = HANDLED
            .iter()
            .filter(|command| shortcuts::key_pressed(&mut self.state, &[event], **command))
            .cloned()
            .collect();

        for command in triggered {
            self.dispatch(command, window, cx);
        }
    }

    fn dispatch(&mut self, command: InputEvent, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            InputEvent::Quit => {
                let _ = self.state.persistent_settings.save_blocking();
                let _ = self.state.volatile_settings.save_blocking();
                cx.quit();
            }
            InputEvent::Browse => self.browse(window.modifiers().shift, false, cx),
            InputEvent::Fullscreen => {
                let origin = window.bounds().origin;
                let origin = (origin.x.as_f32(), origin.y.as_f32());
                if native::HANDLES_FULLSCREEN {
                    // gpui's own toggle is inert on Windows: it queues the style and rect change on a
                    // foreground executor whose future never runs here, so four seconds after the chord
                    // neither the window rect nor `GWL_STYLE` had moved and nothing had panicked
                    // (`D:\tmp\oculante-gpui\fsproof.ps1`, 2026-10-15). `native` makes the same calls
                    // inline and keeps the state on that platform.
                    let was_fullscreen = native::is_fullscreen();
                    if native::toggle_fullscreen().is_some() {
                        self.apply_fullscreen(was_fullscreen, origin, cx);
                    }
                } else {
                    let was_fullscreen = window.is_fullscreen();
                    window.toggle_fullscreen();
                    self.apply_fullscreen(was_fullscreen, origin, cx);
                }
            }
            InputEvent::AlwaysOnTop => self.always_on_top(cx),
            InputEvent::NextImage => utils::next_image(&mut self.state),
            InputEvent::PreviousImage => utils::prev_image(&mut self.state),
            InputEvent::FirstImage => utils::first_image(&mut self.state),
            InputEvent::LastImage => utils::last_image(&mut self.state),
            InputEvent::ResetView => self.state.reset_image = true,
            InputEvent::ZoomIn => zoom_key(&mut self.state, 3.5, self.raw_size / 2.0),
            InputEvent::ZoomOut => zoom_key(&mut self.state, -3.5, self.raw_size / 2.0),
            InputEvent::ZoomActualSize => utils::set_zoom(1.0, None, &mut self.state),
            InputEvent::ZoomDouble => utils::set_zoom(2.0, None, &mut self.state),
            InputEvent::ZoomThree => utils::set_zoom(3.0, None, &mut self.state),
            InputEvent::ZoomFour => utils::set_zoom(4.0, None, &mut self.state),
            InputEvent::ZoomFive => utils::set_zoom(5.0, None, &mut self.state),
            InputEvent::PanRight => self.state.image_geometry.offset.x -= PAN_STEP,
            InputEvent::PanLeft => self.state.image_geometry.offset.x += PAN_STEP,
            InputEvent::PanUp => self.state.image_geometry.offset.y += PAN_STEP,
            InputEvent::PanDown => self.state.image_geometry.offset.y -= PAN_STEP,
            InputEvent::ClearImage => {
                utils::clear_image(&mut self.state);
                self.image = None;
            }
            InputEvent::DeleteFile => {
                self.popup = None;
                // egui only trusted the user to delete a file they could see; a pasted image
                // without a path is cleared straight away.
                if self.state.current_path.is_some() {
                    self.confirm_delete = true;
                } else {
                    utils::delete_file(&mut self.state);
                    self.image = None;
                }
            }
            InputEvent::Copy => {
                if let Some(img) = utils::effective_image(&mut self.state).cloned() {
                    utils::clipboard_copy(&img);
                    self.state.send_message_info("Image copied");
                }
            }
            InputEvent::CopyPath => {
                if let Some(path) = self.state.current_path.clone() {
                    utils::clipboard_copy_path(&path);
                    self.state.send_message_info("Path copied");
                }
            }
            InputEvent::Paste => match utils::clipboard_to_image() {
                Ok(img) => {
                    self.state.current_path = None;
                    self.state.player.stop();
                    let _ = self.state.player.image_sender.send(Frame::new_still(img));
                    self.state.send_message_info("Image pasted");
                }
                Err(e) => self.state.send_message_err(&e.to_string()),
            },
            InputEvent::ScrubBar => {
                self.state.persistent_settings.show_scrub_bar =
                    !self.state.persistent_settings.show_scrub_bar;
            }
            InputEvent::ZenMode => utils::toggle_zen_mode(&mut self.state),
            InputEvent::InfoMode => {
                self.state.persistent_settings.info_enabled =
                    !self.state.persistent_settings.info_enabled;
            }
            InputEvent::EditMode => {
                self.state.persistent_settings.edit_enabled =
                    !self.state.persistent_settings.edit_enabled;
                // egui's top bar overwrote the channel with RGBA every frame the panel was open, so
                // entering edit mode drops a red/green/blue/alpha view — and leaving it keeps RGBA.
                let forced = texture::shown_channel(
                    self.state.persistent_settings.edit_enabled,
                    self.state.persistent_settings.current_channel,
                );
                if forced != self.state.persistent_settings.current_channel {
                    self.set_channel(forced);
                }
            }
            InputEvent::RedChannel => self.set_channel(ColorChannel::Red),
            InputEvent::GreenChannel => self.set_channel(ColorChannel::Green),
            InputEvent::BlueChannel => self.set_channel(ColorChannel::Blue),
            InputEvent::AlphaChannel => self.set_channel(ColorChannel::Alpha),
            InputEvent::RGBChannel => self.set_channel(ColorChannel::Rgb),
            InputEvent::RGBAChannel => self.set_channel(ColorChannel::Rgba),
            #[cfg(feature = "turbo")]
            InputEvent::LosslessRotateRight => self.lossless_rotate(turbojpeg::TransformOp::Rot90),
            #[cfg(feature = "turbo")]
            InputEvent::LosslessRotateLeft => self.lossless_rotate(turbojpeg::TransformOp::Rot270),
            _ => {}
        }
        cx.notify();
    }

    /// Channel masking is baked into the uploaded texture, so switching means re-uploading.
    fn set_channel(&mut self, channel: ColorChannel) {
        self.state.persistent_settings.current_channel =
            texture::shown_channel(self.state.persistent_settings.edit_enabled, channel);
        self.upload_texture();
    }

    #[cfg(feature = "turbo")]
    fn lossless_rotate(&mut self, op: turbojpeg::TransformOp) {
        let Some(path) = self.state.current_path.clone() else {
            return;
        };
        if crate::image_editing::lossless_tx(&path, turbojpeg::Transform::op(op)).is_ok() {
            self.state.is_loaded = false;
            self.state.player.cache.clear();
            self.state.player.load(&path);
        }
    }

    /// Mirrors the previous shell: the channels are drained and the newest frame wins.
    fn process_channels(&mut self, cx: &mut Context<Self>) {
        let mut received = false;

        let latest = self
            .state
            .texture_channel
            .1
            .try_iter()
            .inspect(|frame| {
                if matches!(frame, Frame::AnimationStart(_)) {
                    self.reset_after_upload = true;
                }
            })
            .last();

        if let Some(frame) = latest {
            self.apply_frame(frame);
            received = true;
        }

        while let Ok(message) = self.state.message_channel.1.try_recv() {
            match message {
                Message::LoadError(e) => {
                    self.state.current_image = None;
                    self.image = None;
                    self.state.is_loaded = true;
                    self.state.toasts.error(e);
                }
                Message::Info(m) => {
                    self.state
                        .toasts
                        .info(m)
                        .duration(Some(Duration::from_secs(1)));
                }
                Message::Warning(m) => {
                    self.state.toasts.warning(m);
                }
                Message::Error(m) => {
                    self.state.toasts.error(m);
                }
                Message::Saved(_) => {
                    self.state.toasts.info("Saved");
                }
            }
            received = true;
        }

        while let Ok(info) = self.state.extended_info_channel.1.try_recv() {
            self.state.image_metadata = Some(info);
            received = true;
        }

        // GPUI repaints dirty entities only, so the toast animations are advanced here rather
        // than by egui's continuous redraw loop.
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f32();
        self.last_tick = now;
        if self.state.toasts.tick(dt, self.hovered_toast) {
            received = true;
        }

        if received {
            // egui's dot lit on `has_requested_repaint()`; the closest thing this shell can observe
            // is its own request for the next frame, which is what this notify is.
            #[cfg(debug_assertions)]
            {
                self.debug_repaint = true;
            }
            cx.notify();
        }
        // egui re-ran its idle-cursor branch on every frame and asked for a repaint
        // `hide_delay - idle_time` later (HEAD `src/app.rs:1099-1111`); this poll already beats at
        // 16 ms, so the deadline is met to within one beat without scheduling anything.
        self.tick_idle_cursor();
    }

    /// egui hid the pointer with `CursorIcon::None` while a zen-mode canvas stayed untouched, and the
    /// restore was simply the next frame asking for the default icon again — so the wanted visibility
    /// is read from the same condition on both sides here.
    fn tick_idle_cursor(&mut self) {
        let settings = &self.state.persistent_settings;
        // egui's `is_pointer_over_egui` answers for every foreground layer, and each card the port
        // draws over the canvas is one of those: it occludes the root, so no move reaches
        // `mouse_moved` while the pointer sits on it and `pointer_over_ui` would be left holding the
        // last canvas answer. The browser is tested by its flag rather than its rect for the same
        // reason — the rect is only known once the card has been drawn. The settings window is
        // already excluded by its own clause, and with `file_open` the picker is a native dialog on
        // another thread, whose pointer is not ours.
        #[cfg(not(feature = "file_open"))]
        let over_chrome = self.state.pointer_over_ui
            || self.confirm_overwrite
            || self.browser_shown
            || self.hovered_toast.is_some();
        #[cfg(feature = "file_open")]
        let over_chrome =
            self.state.pointer_over_ui || self.confirm_overwrite || self.hovered_toast.is_some();
        let hide = idle_cursor_should_hide(
            settings.zen_mode,
            self.state.settings_enabled,
            settings.zen_mode_cursor_timeout,
            self.last_pointer_move.elapsed().as_secs_f32(),
            over_chrome,
        );
        if hide != self.cursor_hidden {
            self.cursor_hidden = hide;
            native::set_cursor_visible(!hide);
        }
    }

    fn apply_frame(&mut self, frame: Frame) {
        self.state.is_loaded = true;
        self.last_frame_was_compared_image = matches!(frame, Frame::CompareResult(_, _));

        if matches!(
            frame,
            Frame::AnimationStart(_) | Frame::Still(_) | Frame::ImageCollectionMember(_)
        ) || (self.reset_after_upload && matches!(frame, Frame::Animation(_, _)))
        {
            self.sync_scrubber();
            self.track_recent();
        }

        if !matches!(frame, Frame::Animation(_, _)) {
            self.state.image_metadata = None;
        }
        if !matches!(
            frame,
            Frame::Animation(_, _) | Frame::EditResult(_) | Frame::UpdateTexture
        ) {
            self.state.edit_state.result_pixel_op = Default::default();
            self.state.edit_state.result_image_op = Default::default();
            if !self.state.persistent_settings.keep_edits {
                self.state.edit_state = Default::default();
            }
            // egui's lossless section pushed its crop operator back on every frame it was drawn,
            // so a reload with the section open left a fresh zero crop in the wiped stack.
            #[cfg(feature = "turbo")]
            if self.lossless_open && edit_panel::is_jpeg(self.state.current_path.as_deref()) {
                self.ensure_crop_op();
            }
        }

        match frame {
            Frame::Still(img)
            | Frame::CompareResult(img, _)
            | Frame::ImageCollectionMember(img) => {
                debug!("Received image {}x{}", img.width(), img.height());
                if self.state.persistent_settings.max_cache != 0
                    && let Some(path) = self.state.current_path.clone()
                {
                    self.state.player.cache.insert(&path, img.clone());
                }
                self.state.new_image_loaded = true;
                self.reset_after_upload = true;
                self.state.current_image = Some(img);
            }
            Frame::AnimationStart(img) => {
                debug!("Animation start {}x{}", img.width(), img.height());
                self.state.new_image_loaded = true;
                self.reset_after_upload = true;
                self.state.current_image = Some(img);
            }
            Frame::EditResult(img) | Frame::Animation(img, _) => {
                self.state.current_image = Some(img);
            }
            Frame::UpdateTexture => {}
        }

        self.upload_texture();

        let keep_view = if self.last_frame_was_compared_image {
            self.state.persistent_settings.compare_keep_view
        } else {
            self.state.persistent_settings.keep_view
        };
        if !keep_view && self.reset_after_upload {
            self.state.reset_image = true;
        }
        self.reset_after_upload = false;

        utils::send_extended_info(
            &self.state.current_image,
            &self.state.current_path,
            &self.state.extended_info_channel,
        );
    }

    fn upload_texture(&mut self) {
        let channel = self.state.persistent_settings.current_channel;
        // egui uploaded the edit result whenever one was set, which is what made the alpha debug
        // views and the pixel ops show up without disturbing `current_image`.
        let edited = self.state.edit_state.result_pixel_op.width() > 0;
        let source = if edited {
            Some(&self.state.edit_state.result_pixel_op)
        } else {
            self.state.current_image.as_ref()
        };
        // egui sized the geometry off whatever it uploaded, so an operator that changes the
        // image's extent - rotate, crop, resize - moves the canvas with it.
        let image = source.map(|img| tiles::Frame::upload(img, channel));
        let dimensions = source
            .map(|img| (img.width(), img.height()))
            .unwrap_or((0, 0));
        self.image = image;
        self.state.image_geometry.dimensions = dimensions;
    }

    fn sync_scrubber(&mut self) {
        let Some(path) = self.state.current_path.clone() else {
            return;
        };
        let wrap = self.state.persistent_settings.wrap_folder;
        if self.state.scrubber.has_folder_changed(&path) && !self.state.scrubber.fixed_paths {
            self.state.scrubber = crate::scrubber::Scrubber::new(&path);
            self.state.scrubber.wrap = wrap;
        } else {
            let index = self
                .state
                .scrubber
                .entries
                .iter()
                .position(|p| *p == path)
                .unwrap_or_default();
            if index < self.state.scrubber.entries.len() {
                self.state.scrubber.index = index;
            }
        }
    }

    fn track_recent(&mut self) {
        let Some(path) = self.state.current_path.clone() else {
            return;
        };
        let max = self.state.persistent_settings.max_recents;
        if max > 0 && !self.state.volatile_settings.recent_images.contains(&path) {
            self.state.volatile_settings.recent_images.push_front(path);
            self.state
                .volatile_settings
                .recent_images
                .truncate(max.into());
        }
    }

    /// Persist both settings tables, which is what egui's `on_exit` hook did.
    pub fn save_settings(&self) {
        let _ = self.state.persistent_settings.save_blocking();
        let _ = self.state.volatile_settings.save_blocking();
    }

    /// The draggable part of the scrub bar: where it starts and how far the thumb travels.
    /// egui added the bottom panel before the side panels, so the bar keeps the full window
    /// width even while the info panel is open. Its panel was gated only on `show_scrub_bar`, so
    /// a single-image folder still drew the groove with the handle pinned to the left end.
    fn scrub_track(&self) -> Option<(f32, f32)> {
        if !self.state.persistent_settings.show_scrub_bar || self.state.scrubber.len() == 0 {
            return None;
        }
        let travel = (self.raw_size.x - SCRUB_INSET * 2. - SCRUB_COUNTER - SCRUB_THUMB).max(1.);
        Some((SCRUB_INSET, travel))
    }

    fn over_scrubber(&self, at: Point<Pixels>) -> bool {
        self.scrub_bar_height() > 0. && at.y.as_f32() > self.raw_size.y - SCRUB_HEIGHT
    }

    /// egui reserved the whole top strip for its panel, so nothing below the bar reacts to the
    /// pointer there either.
    fn over_top_bar(&self, at: Point<Pixels>) -> bool {
        self.top_bar_height() > 0. && at.y.as_f32() < self.top_bar_height()
    }

    /// How much of the window's top edge the menu bar claims.
    fn top_bar_height(&self) -> f32 {
        let settings = &self.state.persistent_settings;
        if settings.zen_mode {
            // egui still reserved its `menu_zen` strip when zen was combined with borderless.
            if settings.borderless {
                top_bar::ZEN_HEIGHT
            } else {
                0.
            }
        } else {
            top_bar::HEIGHT
        }
    }

    /// How much of the window's bottom edge the scrub bar claims; egui kept everything else
    /// out of that strip, including the toast stack. Reserved from the first frame even before
    /// the scrubber has entries, since the loader fills it asynchronously and the fit of the
    /// very first image depends on this height.
    fn scrub_bar_height(&self) -> f32 {
        if self.state.persistent_settings.show_scrub_bar {
            SCRUB_HEIGHT
        } else {
            0.
        }
    }

    /// egui's `{GEAR} Preferences` entry flipped the flag its window was bound to.
    fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        if self.state.settings_enabled {
            self.close_settings();
        } else {
            self.state.settings_enabled = true;
        }
        cx.notify();
    }

    /// egui's window closed through its own title bar box and kept its `SettingsUiState` for the
    /// next time. Here the widgets are dropped, since a stale one would otherwise be written back
    /// into the settings by `Form::apply` on the frame it reopened.
    fn close_settings(&mut self) {
        self.state.settings_enabled = false;
        self.pending_shortcut = None;
        self.settings_drag = None;
        self.settings_form.clear();
        self.settings_watch.clear();
        self.settings_subs.clear();
    }

    /// gpui repaints the widget that changed, not the view above it, so a typed field or a moved
    /// picker would never reach [`settings::Form::apply`] without being watched here.
    fn watch_settings_widgets(&mut self, cx: &mut Context<Self>) {
        for input in self.settings_form.inputs() {
            if self.settings_watch.insert(input.entity_id()) {
                let sub = cx.observe(&input, |_, _, cx| cx.notify());
                self.settings_subs.push(sub);
            }
        }
        for picker in self.settings_form.pickers() {
            if self.settings_watch.insert(picker.entity_id()) {
                let sub = cx.observe(&picker, |_, _, cx| cx.notify());
                self.settings_subs.push(sub);
            }
        }
    }

    /// egui handled each widget inside the window itself; here the window hands its events to the
    /// viewer, which owns both the settings and the widgets' cached state.
    fn settings_event(&mut self, event: settings::Event, cx: &mut Context<Self>) {
        match event {
            settings::Event::Close => self.close_settings(),
            settings::Event::Set(field, value) => {
                settings::apply(&mut self.state, field, &value);
                // egui's combo folded itself away once it had handed over a choice.
                self.settings_form.combo = None;
            }
            settings::Event::Drag(drag) => self.settings_drag = Some(drag),
            settings::Event::Combo(field) => self.settings_form.combo = field,
            settings::Event::Assign(command, shortcut) => {
                self.state
                    .persistent_settings
                    .shortcuts
                    .insert(command, shortcut);
                self.pending_shortcut = None;
            }
            settings::Event::Reset => {
                settings::reset(&mut self.state, &mut self.settings_form);
                self.settings_watch.clear();
                self.settings_subs.clear();
            }
            settings::Event::Repository => {
                let _ = webbrowser::open("https://github.com/woelper/oculante");
            }
            settings::Event::DeleteThumbnails => {
                let _ = crate::thumbnails::get_disk_cache_path().map(std::fs::remove_dir_all);
            }
            settings::Event::ThumbnailsDir => {
                // egui opened the directory off the main thread.
                std::thread::spawn(|| {
                    let _ = crate::thumbnails::get_disk_cache_path().map(open::that);
                });
            }
            settings::Event::TestMessage(kind) => match kind {
                0 => self.state.send_message_info("Test"),
                1 => self.state.send_message_warn("Test"),
                _ => self.state.send_message_err("Test"),
            },
        }
        cx.notify();
    }

    /// egui showed the info panel as a left panel that took the canvas's width.
    fn info_panel_width(&self) -> f32 {
        let settings = &self.state.persistent_settings;
        if settings.info_enabled
            && !self.state.settings_enabled
            && !settings.zen_mode
            && self.state.current_image.is_some()
        {
            self.info_width
        } else {
            0.
        }
    }

    /// egui's `"editing"` right panel, which took the canvas's width the same way.
    fn edit_panel_width(&self) -> f32 {
        let settings = &self.state.persistent_settings;
        if settings.edit_enabled
            && !self.state.settings_enabled
            && !settings.zen_mode
            && self.state.current_image.is_some()
        {
            self.edit_width
        } else {
            0.
        }
    }

    /// Whether one of the window-wide grabs (a panel edge, a grabbed `DragValue`) owns the pointer.
    /// Their overlay listener is what tracks those, but gpui bubbles the move back out to the root,
    /// so the viewport has to know the pointer is spoken for before it pans.
    fn grab_owned(&self) -> bool {
        self.num_drag.is_some()
            || self.stroke_drag.is_some()
            || self.settings_drag.is_some()
            || self.panel_drag.is_some()
    }

    /// egui's `prepare_resizable_panel`, whose resize response wrote the panel's width straight from
    /// the pointer: the distance from the window edge the panel is pinned to.
    fn resize_panel(&mut self, side: panel::Side, at: f32) {
        // egui added `"editing"` before `"info"`, so only the left panel saw the right one's claim —
        // and saw nothing at all while that panel was hidden.
        let available = match side {
            panel::Side::Left => (self.raw_size.x - self.edit_panel_width()).max(0.),
            panel::Side::Right => self.raw_size.x,
        };
        match side {
            panel::Side::Left => self.info_width = panel::resize_to(side, at, available),
            panel::Side::Right => self.edit_width = panel::resize_to(side, at, available),
        }
    }

    fn over_info_panel(&self, at: Point<Pixels>) -> bool {
        self.info_panel_width() > 0. && at.x.as_f32() < self.info_panel_width()
    }

    fn over_edit_panel(&self, at: Point<Pixels>) -> bool {
        let width = self.edit_panel_width();
        width > 0. && at.x.as_f32() > self.raw_size.x - width
    }

    /// egui sampled the pixel under the cursor for the info grid, and the edit panel's paint
    /// brush needs the same coordinate. Returns whether the readout moved, since gpui — unlike
    /// egui's continuous redraw — only repaints what it is told is dirty.
    fn track_cursor_pixel(&mut self) -> bool {
        if !self.state.persistent_settings.info_enabled && !self.state.edit_state.painting {
            return false;
        }
        let geometry = self.state.image_geometry;
        let relative = utils::pos_from_coord(
            geometry.offset,
            self.state.cursor,
            Vector2::new(geometry.dimensions.0 as f32, geometry.dimensions.1 as f32),
            geometry.scale,
        );
        let mut changed = relative != self.state.cursor_relative;
        self.state.cursor_relative = relative;
        let Some(image) = &self.state.current_image else {
            return changed;
        };
        let sampled = if self.state.edit_state.result_pixel_op.width() > 0 {
            &self.state.edit_state.result_pixel_op
        } else {
            image
        };
        let (x, y) = (
            self.state.cursor_relative.x as u32,
            self.state.cursor_relative.y as u32,
        );
        if let Some(pixel) = utils::get_pixel_checked(sampled, x, y) {
            let color = [
                f32::from(pixel[0]),
                f32::from(pixel[1]),
                f32::from(pixel[2]),
                f32::from(pixel[3]),
            ];
            if color != self.state.sampled_color {
                self.state.sampled_color = color;
                changed = true;
            }
        }
        changed
    }

    pub(crate) fn toggle_section(&mut self, section: info_panel::Section) {
        self.info_sections.toggle(section);
    }

    /// egui's `edit_ui` prologue: until the stacks have something to say about them, both result
    /// buffers are clones of the loaded image. Returns which stages that fills.
    fn fill_edit_buffers(&mut self) -> (bool, bool) {
        let mut image_changed = false;
        let mut pixels_changed = false;
        if let Some(image) = self.state.current_image.clone() {
            if self.state.edit_state.result_pixel_op.width() == 0 {
                debug!("Edit state pixel comp buffer is default, cloning from image");
                self.state.edit_state.result_pixel_op = image.clone();
                pixels_changed = true;
            }
            if self.state.edit_state.result_image_op.width() == 0 {
                debug!("Edit state image comp buffer is default, cloning from image");
                self.state.edit_state.result_image_op = image;
                image_changed = true;
            }
        }
        (image_changed, pixels_changed)
    }

    /// egui's paint block minus its widgets (HEAD `src/ui/edit_ui.rs:206-373`), run from the same
    /// per-frame edit pass: a held right button sampled the pixel under the cursor into the stroke
    /// being built, and a held left one wrote the cursor's UV into it. Returns whether the pixel
    /// buffer needs re-rendering, which is what egui's `pixels_changed` was.
    fn paint_pass(&mut self) -> bool {
        if !self.state.edit_state.painting {
            return false;
        }
        if self.right_down {
            let (x, y) = (
                self.state.cursor_relative.x as u32,
                self.state.cursor_relative.y as u32,
            );
            let sampled = utils::get_pixel_checked(&self.state.edit_state.result_pixel_op, x, y)
                .map(|pixel| {
                    [
                        f32::from(pixel[0]) / 255.,
                        f32::from(pixel[1]) / 255.,
                        f32::from(pixel[2]) / 255.,
                        f32::from(pixel[3]) / 255.,
                    ]
                });
            let last = self.state.edit_state.paint_strokes.len().checked_sub(1);
            if let (Some(index), Some(color)) = (last, sampled)
                && let Some(stroke) = self.state.edit_state.paint_strokes.last_mut()
                && stroke.color != color
            {
                stroke.color = color;
                // That row's colour lives in a picker entity, which would otherwise write its own
                // stale value back over the sample on the next frame.
                self.params.stroke_sampled(index);
            }
        }
        let geometry = self.state.image_geometry;
        let uv = (
            self.state.cursor_relative.x / geometry.dimensions.0 as f32,
            self.state.cursor_relative.y / geometry.dimensions.1 as f32,
        );
        edit_panel::paint_frame(
            &mut self.state.edit_state.paint_strokes,
            self.left_down,
            self.state.pointer_over_ui,
            uv,
        )
    }

    /// A stroke edit re-ran egui's pixel stack, which is the only way its result reaches the canvas.
    fn stroke_changed(&mut self, cx: &mut Context<Self>) {
        self.sync_edits((false, true), cx);
        cx.notify();
    }

    /// egui ran the whole panel on every frame it was open: the buffers, the colour conversion it
    /// adds of its own accord, and the processing tail. `changed` is what the panel's widgets
    /// wrote into the operators this frame, in `(image, pixel)` order.
    fn sync_edits(&mut self, changed: (bool, bool), cx: &mut Context<Self>) {
        let (mut image_changed, mut pixels_changed) = self.fill_edit_buffers();
        image_changed |= changed.0;
        pixels_changed |= changed.1;
        pixels_changed |= self.paint_pass();

        if self.state.edit_state.result_image_op.color() != ColorType::Rgba8 {
            let present = self
                .state
                .edit_state
                .image_op_stack
                .first()
                .is_some_and(|op| matches!(op.operation, ImageOperation::ColorConverter(_)));
            if !present {
                self.state.edit_state.image_op_stack.insert(
                    0,
                    ImgOpItem::new(ImageOperation::ColorConverter(ColorTypeExt::Rgba8)),
                );
                image_changed = true;
                pixels_changed = true;
                self.state
                    .send_message_info("Color conversion operator added.");
            }
        }

        self.run_edit_stack(image_changed, pixels_changed, cx);
    }

    /// egui's tail: the image stack rebuilds `result_image_op` from the untouched image, the pixel
    /// stack works over that, and the result is what the canvas uploads.
    fn run_edit_stack(
        &mut self,
        image_changed: bool,
        mut pixels_changed: bool,
        cx: &mut Context<Self>,
    ) {
        if self.state.edit_state.skip_processing {
            return;
        }

        if image_changed {
            if let Some(image) = self.state.current_image.clone() {
                let stamp = Instant::now();
                let mut result = image;
                for operation in &self.state.edit_state.image_op_stack {
                    if !operation.active {
                        continue;
                    }
                    if let Err(e) = operation.operation.process_image(&mut result) {
                        error!("{e}");
                        self.state.send_message_warn(&format!("{e}"));
                    }
                }
                debug!(
                    "Image changed. Finished evaluating in {}s",
                    stamp.elapsed().as_secs_f32()
                );
                self.state.edit_state.result_image_op = result;

                // Strokes are drawn over the pixel result, so a new base image needs them again.
                for stroke in &mut self.state.edit_state.paint_strokes {
                    stroke.committed = false;
                }
            }
            pixels_changed = true;
        }

        if pixels_changed {
            let stamp = Instant::now();
            let mut result = self.state.edit_state.result_image_op.clone();
            let ops: Vec<ImageOperation> = self
                .state
                .edit_state
                .pixel_op_stack
                .iter()
                .filter(|op| op.active)
                .map(|op| op.operation.clone())
                .collect();
            if !ops.is_empty()
                && let Err(e) = image_editing::process_pixels(&mut result, &ops)
            {
                self.state.send_message_warn(&format!("{e}"));
            }

            // egui drew the uncommitted strokes over the pixel stack's result (HEAD
            // `src/ui/edit_ui.rs:620-629`), which is why a stroke repaint re-ran the whole stack.
            if let Some(buffer) = result.as_mut_rgba8() {
                for stroke in &self.state.edit_state.paint_strokes {
                    if !stroke.committed {
                        stroke.render(buffer, &self.state.edit_state.brushes);
                    }
                }
            }

            self.state.edit_state.result_pixel_op = result;
            debug!(
                "Finished Pixel op stack in {} s",
                stamp.elapsed().as_secs_f32()
            );
            self.state.send_frame(Frame::UpdateTexture);
        }

        // egui then baked every *finished* stroke into the very buffer it had just handed to the
        // texture, so in destructive painting only the newest stroke stays a live overlay (HEAD
        // `src/ui/edit_ui.rs:638-668`). Those strokes end up blended twice in the uploaded frame —
        // upstream's own double render, mirrored rather than fixed.
        if self.state.edit_state.painting && !self.state.edit_state.non_destructive_painting {
            let painted = self
                .state
                .edit_state
                .paint_strokes
                .iter()
                .filter(|stroke| !stroke.points.is_empty())
                .count();
            let count = self.state.edit_state.paint_strokes.len();
            if painted > 1 {
                for (index, stroke) in self.state.edit_state.paint_strokes.iter_mut().enumerate() {
                    if index + 1 < count && !stroke.committed && !stroke.is_empty() {
                        if let Some(buffer) = self.state.edit_state.result_pixel_op.as_mut_rgba8() {
                            let brushes = &self.state.edit_state.brushes;
                            stroke.render(buffer, brushes);
                        }
                        stroke.committed = true;
                        debug!("Committed stroke {}", index);
                    }
                }
            }
        }

        let dimensions = {
            let result = &self.state.edit_state.result_pixel_op;
            (result.width(), result.height())
        };
        self.state.image_geometry.dimensions = dimensions;
        cx.notify();
    }

    fn stack_mut(&mut self, stack: edit_panel::Stack) -> &mut Vec<ImgOpItem> {
        match stack {
            edit_panel::Stack::Image => &mut self.state.edit_state.image_op_stack,
            edit_panel::Stack::Pixel => &mut self.state.edit_state.pixel_op_stack,
        }
    }

    /// egui routed a row change to the flag its own stack is processed under, then let the tail of
    /// the same frame do the work.
    fn edit_stack_changed(&mut self, stack: edit_panel::Stack, cx: &mut Context<Self>) {
        match stack {
            edit_panel::Stack::Image => self.run_edit_stack(true, false, cx),
            edit_panel::Stack::Pixel => self.run_edit_stack(false, true, cx),
        }
    }

    /// egui's `DragValue` turned a press that released without dragging into a text field holding
    /// its own readout with the text selected, and wrote the operator through on every change —
    /// its `update_while_editing`. Escape, Enter and a click away all end it, and since the live
    /// writes have landed by then, ending it is only the blur.
    fn begin_num_edit(
        &mut self,
        stack: edit_panel::Stack,
        index: usize,
        grab: edit_panel::NumGrab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(op) = self.stack_mut(stack).get(index) else {
            return;
        };
        let id = op.id;
        let Some(row) = edit_panel::num_rows(&op.operation)
            .into_iter()
            .find(|row| row.slot == grab.slot)
        else {
            return;
        };
        let slot = grab.slot;
        let input = cx.new(|cx| InputState::new(window, cx).default_value(row.shown()));
        let subscription =
            cx.subscribe_in(&input, window, move |this, input, event, window, cx| {
                match event {
                    TextEvent::Change => {
                        let Some(value) = grab.parse(&input.read(cx).value()) else {
                            return;
                        };
                        let changed = this
                            .stack_mut(stack)
                            .get_mut(index)
                            .filter(|op| op.id == id)
                            .is_some_and(|op| edit_panel::set_num(&mut op.operation, slot, value));
                        if changed {
                            this.edit_stack_changed(stack, cx);
                        }
                    }
                    // The field owns the window's keystrokes while it is open, as egui's `key_grab` did.
                    TextEvent::Focus => this.state.key_grab = true,
                    TextEvent::Blur => {
                        let open = this
                            .params
                            .num_edit
                            .as_ref()
                            .is_some_and(|edit| edit.input.entity_id() == input.entity_id());
                        if open {
                            // The readout is drawn again from here, and the keys go back to the filter
                            // field if that is where the focus landed.
                            this.params.num_edit = None;
                            this.state.key_grab = this
                                .filter_search
                                .read(cx)
                                .focus_handle(cx)
                                .is_focused(window);
                            cx.notify();
                        }
                    }
                    TextEvent::PressEnter { .. } => window.blur(cx),
                }
            });
        input.update(cx, |state, cx| state.select_all(window, cx));
        let focus = input.read(cx).focus_handle(cx);
        focus.focus(window, cx);
        self.params.num_edit = Some(edit_panel::NumEdit {
            stack,
            index,
            id,
            grab,
            input,
            subscription,
        });
        cx.notify();
    }

    fn add_filter(&mut self, operation: ImageOperation, cx: &mut Context<Self>) {
        let per_pixel = operation.is_per_pixel();
        let item = ImgOpItem::new(operation);
        if per_pixel {
            self.state.edit_state.pixel_op_stack.push(item);
        } else {
            self.state.edit_state.image_op_stack.push(item);
        }
        // egui folded the Filters header back up once it had handed over an operator.
        self.filters_open = false;
        self.edit_stack_changed(
            if per_pixel {
                edit_panel::Stack::Pixel
            } else {
                edit_panel::Stack::Image
            },
            cx,
        );
    }

    fn move_op(
        &mut self,
        stack: edit_panel::Stack,
        from: usize,
        to: usize,
        cx: &mut Context<Self>,
    ) {
        let ops = self.stack_mut(stack);
        if to < ops.len() {
            ops.swap(from, to);
        }
        self.edit_stack_changed(stack, cx);
    }

    fn bypass_op(&mut self, stack: edit_panel::Stack, index: usize, cx: &mut Context<Self>) {
        if let Some(op) = self.stack_mut(stack).get_mut(index) {
            op.active = !op.active;
        }
        self.edit_stack_changed(stack, cx);
    }

    fn remove_op(&mut self, stack: edit_panel::Stack, index: usize, cx: &mut Context<Self>) {
        if index < self.stack_mut(stack).len() {
            self.stack_mut(stack).remove(index);
        }
        self.edit_stack_changed(stack, cx);
    }

    /// egui's "Disable all edits": dropping the result is what puts the untouched image back on
    /// the canvas, since the upload prefers the result buffer while it holds an image.
    fn set_skip_processing(&mut self, skip: bool, cx: &mut Context<Self>) {
        self.state.edit_state.skip_processing = skip;
        if skip {
            self.state.edit_state.result_pixel_op = Default::default();
            self.state.send_frame(Frame::UpdateTexture);
            cx.notify();
        } else {
            self.run_edit_stack(false, true, cx);
        }
    }

    fn apply_all_edits(&mut self, cx: &mut Context<Self>) {
        let result = self.state.edit_state.result_pixel_op.clone();
        if result.width() == 0 {
            return;
        }
        self.state.current_image = Some(result);
        self.state.edit_state = Default::default();
        self.run_edit_stack(true, true, cx);
    }

    fn remove_all_edits(&mut self, cx: &mut Context<Self>) {
        self.state.edit_state = Default::default();
        self.run_edit_stack(false, true, cx);
    }

    fn reload_current(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.state.current_path.clone() else {
            return;
        };
        self.state.is_loaded = false;
        self.state.player.cache.clear();
        self.state.player.load(&path);
        cx.notify();
    }

    /// egui's "Create output file": a default name under the last opened directory, so an image
    /// that arrived without a path can still be saved.
    fn create_output_file(&mut self, cx: &mut Context<Self>) {
        let extension = self.state.edit_state.export_extension.clone();
        let dest = self
            .state
            .volatile_settings
            .last_open_directory
            .clone()
            .join("untitled")
            .with_extension(extension);
        self.state.current_path = Some(dest);
        cx.notify();
    }

    /// egui pushed a zero crop operator while drawing the lossless section, which is what gave its
    /// crop button something to be enabled by.
    #[cfg(feature = "turbo")]
    fn ensure_crop_op(&mut self) {
        if edit_panel::first_active_crop(&self.state.edit_state.image_op_stack).is_none() {
            log::info!("A missing crop operator was added.");
            self.state
                .edit_state
                .image_op_stack
                .push(ImgOpItem::new(ImageOperation::Crop([0, 0, 0, 0])));
        }
    }

    /// egui's lossless buttons: libjpeg-turbo rewrites the file in place, and a success gets the
    /// same reload "Reload & Restore" does. egui logged only a crop failure; the rest were silent.
    #[cfg(feature = "turbo")]
    fn lossless_edit(&mut self, edit: edit_panel::Lossless, cx: &mut Context<Self>) {
        use image::GenericImageView as _;
        use turbojpeg::TransformOp;

        let Some(path) = self.state.current_path.clone() else {
            return;
        };
        let transform = match edit {
            edit_panel::Lossless::Rot90 => turbojpeg::Transform::op(TransformOp::Rot90),
            edit_panel::Lossless::Rot270 => turbojpeg::Transform::op(TransformOp::Rot270),
            edit_panel::Lossless::Rot180 => turbojpeg::Transform::op(TransformOp::Rot180),
            edit_panel::Lossless::Hflip => turbojpeg::Transform::op(TransformOp::Hflip),
            edit_panel::Lossless::Vflip => turbojpeg::Transform::op(TransformOp::Vflip),
            edit_panel::Lossless::Crop => {
                let Some(amounts) =
                    edit_panel::first_active_crop(&self.state.edit_state.image_op_stack)
                else {
                    return;
                };
                let dimensions = self
                    .state
                    .current_image
                    .as_ref()
                    .map(|image| image.dimensions())
                    .unwrap_or_default();
                let range = image_editing::cropped_range(&amounts, &dimensions);
                let mut crop = turbojpeg::Transform::default();
                crop.crop = Some(turbojpeg::TransformCrop {
                    x: range[0] as usize,
                    y: range[1] as usize,
                    width: Some(range[2] as usize),
                    height: Some(range[3] as usize),
                });
                crop
            }
        };
        match image_editing::lossless_tx(&path, transform) {
            Ok(()) => self.reload_current(cx),
            // egui logged only a failed crop; the rotate/flip arms swallow their errors.
            Err(e) if matches!(edit, edit_panel::Lossless::Crop) => log::warn!("{e}"),
            Err(_) => {}
        }
    }

    /// egui's "Alpha tools" buttons: they replace the edit result the shell uploads, so the canvas
    /// shows a debug view of the alpha channel until "Reset image" puts the plain image back.
    fn alpha_preview(
        &mut self,
        highlight: fn(&DynamicImage) -> DynamicImage,
        cx: &mut Context<Self>,
    ) {
        let Some(image) = &self.state.current_image else {
            return;
        };
        self.state.edit_state.result_pixel_op = highlight(image);
        self.state.send_frame(Frame::UpdateTexture);
        cx.notify();
    }

    fn reset_alpha_preview(&mut self, cx: &mut Context<Self>) {
        self.state.edit_state.result_pixel_op = Default::default();
        self.state.send_frame(Frame::UpdateTexture);
        cx.notify();
    }

    /// egui built the Measure section inside the info panel, behind `experimental_features`, and put
    /// both its pointer reads and its overlay paint inside the collapsing body — so the tool is live
    /// exactly while that header is open in a panel that is itself visible.
    fn measure_active(&self) -> bool {
        let settings = &self.state.persistent_settings;
        self.info_sections.measure
            && settings.experimental_features
            && settings.info_enabled
            && !settings.zen_mode
            && self.state.current_image.is_some()
    }

    /// egui's `measure_ui` prologue: the stack has to hold a Measure operator, and it only ever
    /// *added* the missing one — a stack that somehow holds two was left alone, which is what
    /// `x.len() != 1` says.
    fn ensure_measure_op(&mut self) {
        if !self.measure_active() {
            return;
        }
        let count = self
            .state
            .edit_state
            .image_op_stack
            .iter()
            .filter(|op| matches!(op.operation, ImageOperation::Measure { .. }))
            .count();
        if count != 1 {
            self.state
                .edit_state
                .image_op_stack
                .push(ImgOpItem::new(ImageOperation::Measure {
                    shapes: vec![MeasureShape::new_rect(vec![(0, 0), (0, 0)])],
                }));
        }
    }

    /// The corner egui wrote: `cursor_relative` floored by its own `as u32` cast, already clamped to
    /// the image by `pos_from_coord`.
    fn measure_corner(&self) -> (u32, u32) {
        (
            self.state.cursor_relative.x as u32,
            self.state.cursor_relative.y as u32,
        )
    }

    /// egui wrote the rect's second corner on every frame the right button was held — including the
    /// press frame, where its `secondary_pressed` branch wrote the first corner too. That is why a
    /// click that never moves leaves a zero-size rect under the pointer.
    ///
    /// Only active items were written, and only the rect's points: egui's line shapes have no
    /// construction site in either shell.
    fn measure_write(&mut self, at: (u32, u32), press: bool, cx: &mut Context<Self>) {
        let mut wrote = false;
        for item in &mut self.state.edit_state.image_op_stack {
            if !item.active {
                continue;
            }
            if let ImageOperation::Measure { shapes } = &mut item.operation {
                for shape in shapes {
                    if let MeasureShape::Rect { points, .. } = shape {
                        points[1] = at;
                        if press {
                            points[0] = at;
                        }
                        wrote = true;
                    }
                }
            }
        }
        if wrote {
            cx.notify();
        }
    }

    /// The overlay egui painted from its Measure arm, which walked the whole stack without checking
    /// `active` — so a bypassed measure rect still draws.
    fn measure_marks(&self) -> Vec<measure::Mark> {
        let geometry = self.state.image_geometry;
        let origin = (
            geometry.offset.x + self.info_panel_width(),
            geometry.offset.y + self.top_bar_height(),
        );
        let mut marks = Vec::new();
        for item in &self.state.edit_state.image_op_stack {
            if let ImageOperation::Measure { shapes } = &item.operation {
                for shape in shapes {
                    if let MeasureShape::Rect {
                        points,
                        color,
                        width,
                    } = shape
                    {
                        marks.push(measure::rect(
                            [points[0], points[1]],
                            *color,
                            *width,
                            geometry.scale,
                            origin,
                        ));
                    }
                }
            }
        }
        marks
    }

    /// Every perspective-crop operator in either stack: egui drew the arm from `modifier_stack_ui`,
    /// which runs for both, and inside `add_enabled_ui(false)` its closure still runs, so a bypassed
    /// operator keeps both its overlay and its grab. Each one contributes an overlay of its own,
    /// which is why two of them dim the window twice. The arm lives in the `"editing"` panel body,
    /// so whatever hides that panel — `E`, zen mode, the settings window, no image — takes the scrim
    /// and the handles with it.
    fn crop_overlays(&self) -> Vec<crop::Overlay> {
        if self.crop_applied || self.edit_panel_width() <= 0. {
            return Vec::new();
        }
        let geometry = self.state.image_geometry;
        let origin = (
            geometry.offset.x + self.info_panel_width(),
            geometry.offset.y + self.top_bar_height(),
        );
        let cursor = [self.last_cursor.x.as_f32(), self.last_cursor.y.as_f32()];
        let scrim = [0., 0., self.raw_size.x, self.raw_size.y];
        [
            self.state.edit_state.image_op_stack.as_slice(),
            self.state.edit_state.pixel_op_stack.as_slice(),
        ]
        .into_iter()
        .flatten()
        .filter_map(|item| match &item.operation {
            ImageOperation::CropPerspective { points, .. } => Some(crop::project(
                *points,
                geometry.scale,
                origin,
                scrim,
                cursor,
            )),
            _ => None,
        })
        .collect()
    }

    /// egui grabbed a handle from `pointer.any_down()` while the cursor was within 20 points of it,
    /// and latched `block_panning` for the press. Any button counted, including the right one the
    /// Measure tool uses.
    fn crop_press(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let cursor = [at.x.as_f32(), at.y.as_f32()];
        let grabbed = self
            .crop_overlays()
            .iter()
            .filter_map(|overlay| crop::grabbed(&overlay.handles, cursor))
            .last();
        if grabbed.is_some() {
            self.crop_grab = grabbed;
            self.state.edit_state.block_panning = true;
            cx.notify();
        }
    }

    /// egui cleared both the latch and its `"pt"` temp from *inside* the `d < maxdist` branch, so a
    /// release with the cursor further than 20 points from every handle left them set and the handle
    /// went on following the pointer with no button held. Mirrored, not fixed.
    fn crop_release(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        if self.crop_grab.is_none() {
            return;
        }
        let cursor = [at.x.as_f32(), at.y.as_f32()];
        let near = self.crop_overlays().iter().any(|overlay| {
            overlay
                .handles
                .iter()
                .any(|handle| crop::near(handle, &cursor))
        });
        if near {
            self.crop_grab = None;
            self.state.edit_state.block_panning = false;
            cx.notify();
        }
    }

    /// The pointer in source-pixel coordinates. egui's crop arm ran `pos_from_coord` on its own
    /// hover position rather than reading the shared cursor, which matters because that one only
    /// tracks while the info panel is open.
    fn crop_cursor_relative(&self) -> Vector2<f32> {
        let geometry = self.state.image_geometry;
        utils::pos_from_coord(
            geometry.offset,
            self.state.cursor,
            Vector2::new(geometry.dimensions.0 as f32, geometry.dimensions.1 as f32),
            geometry.scale,
        )
    }

    /// egui wrote `cursor_relative` into the grabbed handle of every perspective-crop operator it
    /// drew, so with two in the stack both followed the cursor. It did that straight onto the
    /// operator without marking its response changed, so the pipeline did not re-run while the
    /// handle moved — the crop only lands once some other edit marks the stack dirty. Mirrored.
    fn crop_follow(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.crop_grab else { return };
        if self.crop_applied {
            return;
        }
        let cursor = self.crop_cursor_relative();
        let at = (cursor.x as u32, cursor.y as u32);
        let mut wrote = false;
        for stack in [edit_panel::Stack::Image, edit_panel::Stack::Pixel] {
            for item in self.stack_mut(stack) {
                if let ImageOperation::CropPerspective { points, .. } = &mut item.operation {
                    wrote |= points[index] != at;
                    points[index] = at;
                }
            }
        }
        if wrote {
            cx.notify();
        }
    }

    /// egui's "✔ Apply" set its temp flag *and* marked its response changed, so the pipeline
    /// re-ran with the dragged points — that is when the crop lands. The operator keeps its points
    /// either way, which is why the hover text promises no lost image data. Neither the `"pt"` temp
    /// nor `block_panning` is cleared here, because the only code that clears them lives in the
    /// branch Apply switches off: a handle left sticky keeps blocking panning until a release near a
    /// handle after a Reset. Mirrored.
    fn crop_apply(&mut self, stack: edit_panel::Stack, cx: &mut Context<Self>) {
        self.crop_applied = true;
        self.edit_stack_changed(stack, cx);
    }

    /// egui's "⤴ Reset" cleared the flag and rebuilt that operator's corners from its own
    /// `original_size`, the image's dimensions when the operator was added. It too leaves the grab
    /// alone, so a handle that was still following the pointer resumes the moment the overlay is back.
    fn crop_reset(&mut self, stack: edit_panel::Stack, index: usize, cx: &mut Context<Self>) {
        self.crop_applied = false;
        if let Some(item) = self.stack_mut(stack).get_mut(index) {
            if let ImageOperation::CropPerspective {
                points,
                original_size,
            } = &mut item.operation
            {
                *points = [
                    (0, 0),
                    (original_size.0, 0),
                    (0, original_size.1),
                    (original_size.0, original_size.1),
                ];
            }
        }
        self.edit_stack_changed(stack, cx);
    }

    /// egui sampled from the Palette section's own handler, so it only ran while that header was
    /// open and the pointer was over the canvas rather than any panel.
    fn right_click(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.right_down = true;
        self.crop_press(at, cx);
        if !self.state.pointer_over_ui && self.info_panel_width() > 0. && self.info_sections.palette
        {
            self.sample_color(cx);
        }
        // egui read the button off the global pointer state inside the Measure body, so a right-click
        // over either panel measured just the same as one over the image.
        if self.measure_active() {
            let at = self.measure_corner();
            self.measure_write(at, true, cx);
        }
    }

    /// egui's `secondary_released`: the rect keeps whatever corner the pointer was at when the button
    /// came up, which is the same point its `secondary_down` branch had already written.
    fn right_release(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.crop_release(at, cx);
        if self.measure_active() {
            let at = self.measure_corner();
            self.measure_write(at, false, cx);
        }
    }

    fn sample_color(&mut self, cx: &mut Context<Self>) {
        let color = info_panel::to_bytes(self.state.sampled_color);
        if self.palette.contains(&color) {
            self.state.send_message_info("Color already in palette");
        } else {
            self.palette.push(color);
        }
        cx.notify();
    }

    fn remove_color(&mut self, color: [u8; 4], cx: &mut Context<Self>) {
        if let Some(index) = self.palette.iter().position(|swatch| *swatch == color) {
            self.palette.remove(index);
            cx.notify();
        }
    }

    fn copy_color(&self, color: [u8; 4], cx: &mut Context<Self>) {
        let hex = info_panel::hex(color);
        cx.write_to_clipboard(ClipboardItem::new_string(hex.clone()));
        self.state
            .send_message_info(&format!("Copied color: {hex}"));
    }

    fn clear_palette(&mut self, cx: &mut Context<Self>) {
        self.palette.clear();
        self.palette_task = None;
        cx.notify();
    }

    fn sort_palette(&mut self, cx: &mut Context<Self>) {
        self.palette.sort();
        cx.notify();
    }

    /// egui's "From image": quantette's kmeans palette replaces the picked swatches. egui ran it
    /// on the UI thread, which froze the window for the whole pass.
    fn quantize_palette(&mut self, cx: &mut Context<Self>) {
        let Some(image) = self.state.current_image.clone() else {
            return;
        };
        self.palette.clear();
        let weak = cx.entity().downgrade();
        self.palette_task = Some(cx.spawn(async move |_, cx| {
            let swatches: Vec<[u8; 4]> = cx
                .background_executor()
                .spawn(async move {
                    let rgb8 = image.into_rgb8();
                    let Ok(mut pipeline) = PalettePipeline::try_from(&rgb8) else {
                        return Vec::new();
                    };
                    pipeline
                        .palette_size(32)
                        .colorspace(ColorSpace::Oklab)
                        .quantize_method(quantette::KmeansOptions::new())
                        .palette_par()
                        .into_iter()
                        .map(|color| [color.red, color.green, color.blue, 255])
                        .collect()
                })
                .await;
            let _ = cx.update(|app| {
                let _ = weak.update(app, |viewer, cx| {
                    viewer.palette.extend(swatches);
                    cx.notify();
                });
            });
        }));
        cx.notify();
    }

    /// egui's `file_open` branch of "Save ASE"; its other branch used the in-app browser.
    #[cfg(feature = "file_open")]
    fn save_ase(&self, cx: &mut Context<Self>) {
        let swatches = self.palette.clone();
        let directory = self.state.volatile_settings.last_open_directory.clone();
        cx.background_executor()
            .spawn(async move {
                let Some(path) = rfd::FileDialog::new().set_directory(directory).save_file() else {
                    return;
                };
                let colors = swatches
                    .iter()
                    .map(|color| ase_swatch::types::ObjectColor {
                        name: "".into(),
                        object_type: ase_swatch::types::ObjectColorType::Global,
                        data: ase_swatch::types::Color {
                            mode: ase_swatch::types::ColorMode::Rgb,
                            values: [
                                f32::from(color[0]) / 255.,
                                f32::from(color[1]) / 255.,
                                f32::from(color[2]) / 255.,
                            ]
                            .to_vec(),
                        },
                    })
                    .collect::<Vec<_>>();
                let bytes = ase_swatch::create_ase(&Vec::new(), &colors);
                if let Ok(mut file) = std::fs::File::create(path) {
                    _ = std::io::Write::write_all(&mut file, &bytes);
                }
            })
            .detach();
    }

    /// egui's default-build "Save ASE" picked the path in the in-app browser with the `["ase"]`
    /// filter, which is what forced the extension through `validated_filename`. Its button only
    /// opened the `"SAVEASE"` popup, so the parked listing and the directory it was left in both
    /// survive into it.
    #[cfg(not(feature = "file_open"))]
    fn save_ase(&mut self, cx: &mut Context<Self>) {
        self.open_browser(
            browser::Purpose::Palette,
            vec!["ase".to_string()],
            None,
            false,
            cx,
        );
    }

    fn scrub_to(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some((left, travel)) = self.scrub_track() else {
            return;
        };
        let last = self.state.scrubber.len() - 1;
        let fraction = ((at.x.as_f32() - left) / travel).clamp(0., 1.);
        let index = (fraction * last as f32).round() as usize;
        if index == self.state.scrubber.index {
            return;
        }
        let path = self.state.scrubber.set(index);
        if path.as_os_str().is_empty() {
            return;
        }
        self.state.is_loaded = false;
        self.state.current_path = Some(path.clone());
        self.state.player.load(&path);
        cx.notify();
    }

    fn scrubber_bar(&self, palette: theme::Palette) -> Option<Div> {
        let (left, travel) = self.scrub_track()?;
        let index = self.state.scrubber.index;
        let last = (self.state.scrubber.len() - 1).max(1) as f32;
        let thumb = left + travel * index as f32 / last;
        let track_y = (SCRUB_HEIGHT - 5.) / 2.;
        let thumb_y = (SCRUB_HEIGHT - 18.) / 2.;

        Some(
            div()
                .absolute()
                .left_0()
                .bottom_0()
                .w_full()
                .h(px(SCRUB_HEIGHT))
                .bg(palette.panel)
                .child(
                    div()
                        .absolute()
                        .top(px(track_y))
                        .left(px(left))
                        .right(px(SCRUB_INSET + SCRUB_COUNTER))
                        .h(px(5.))
                        .rounded(px(2.5))
                        .bg(palette.accent),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(thumb_y))
                        .left(px(thumb))
                        .w(px(SCRUB_THUMB))
                        .h(px(18.))
                        .rounded(px(4.))
                        .bg(palette.accent),
                )
                .child(
                    div()
                        .absolute()
                        .bottom_0()
                        .right(px(SCRUB_INSET))
                        .h(px(SCRUB_HEIGHT))
                        .w(px(SCRUB_COUNTER - SCRUB_INSET))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.))
                        .text_color(palette.text)
                        .child(format!("{}/{}", index + 1, self.state.scrubber.len())),
                ),
        )
    }

    fn fit_to_window(&mut self) {
        let Some(image) = &self.state.current_image else {
            return;
        };
        // egui laid the image out in `content_rect`, which excludes both bars.
        let window_size = self.state.window_size;
        let img_size = image.size_vec();
        let scaled_to_fit = (window_size.x / img_size.x).min(window_size.y / img_size.y);
        let scale = if self.state.persistent_settings.auto_scale {
            scaled_to_fit
        } else {
            scaled_to_fit.min(1.0)
        };
        self.state.image_geometry.scale = scale;
        self.state.image_geometry.offset = window_size / 2.0 - (img_size * scale) / 2.0;
        self.state.reset_image = false;
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        if self.state.pointer_over_ui {
            return;
        }
        let scroll = match event.delta {
            ScrollDelta::Pixels(delta) => delta.y.as_f32(),
            ScrollDelta::Lines(delta) => delta.y * 16.,
        };
        if scroll == 0. {
            return;
        }

        if event.modifiers.control || event.modifiers.platform {
            if scroll > 0. {
                utils::prev_image(&mut self.state);
            } else {
                utils::next_image(&mut self.state);
            }
        } else {
            let divisor = if cfg!(target_os = "macos") { 1.5 } else { 10. };
            let delta = utils::zoomratio(
                ((scroll / divisor) * self.state.persistent_settings.zoom_multiplier)
                    .clamp(-5., 5.),
                self.state.image_geometry.scale,
            );
            let new_scale = self.state.image_geometry.scale + delta;
            if new_scale > 0.01 && new_scale < 40. {
                self.state.image_geometry.offset -= utils::scale_pt(
                    self.state.image_geometry.offset,
                    self.state.cursor,
                    self.state.image_geometry.scale,
                    delta,
                );
                self.state.image_geometry.scale = new_scale;
                self.state.reset_image = false;
            }
        }
        cx.notify();
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.press_seen = true;
        self.left_down = true;
        if self.confirm_delete {
            return;
        }
        self.crop_press(event.position, cx);
        // Anything the open dropdown does not swallow closes it, like egui's popups.
        if self.popup.take().is_some() {
            cx.notify();
            return;
        }
        if self.over_scrubber(event.position) {
            self.scrub_to(event.position, cx);
            return;
        }
        // egui toggled fullscreen on a double click of the canvas, under the same gate as panning.
        if event.click_count >= 2 && !self.state.pointer_over_ui && !self.state.mouse_grab {
            self.dispatch(InputEvent::Fullscreen, window, cx);
        }
    }

    /// egui latched pointer ownership when a button went down (`egui_started_press`) and kept it
    /// for the whole press, so a middle drag begun on the chrome never pans even after it leaves.
    fn middle_press(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.crop_press(at, cx);
        self.press_seen = true;
        self.middle_claimed = self.state.mouse_grab || self.state.pointer_over_ui;
    }

    fn hover_toast(&mut self, id: u64, hovered: bool, cx: &mut Context<Self>) {
        let next = hovered.then_some(id);
        if self.hovered_toast != next {
            self.hovered_toast = next;
            cx.notify();
        }
    }

    fn hover_close(&mut self, id: u64, hovered: bool, cx: &mut Context<Self>) {
        let next = hovered.then_some(id);
        if self.hovered_close != next {
            self.hovered_close = next;
            cx.notify();
        }
    }

    fn dismiss_toast(&mut self, id: u64, cx: &mut Context<Self>) {
        self.state.toasts.dismiss(id);
        cx.notify();
    }

    fn mouse_moved(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let position = event.position;
        let delta = Vector2::new(
            position.x.as_f32() - self.last_cursor.x.as_f32(),
            position.y.as_f32() - self.last_cursor.y.as_f32(),
        );
        self.last_cursor = position;
        self.last_pointer_move = Instant::now();
        // Everything that positions the image works in content space, so the chrome the pointer
        // is not allowed to move the image under is subtracted here.
        self.state.cursor = Vector2::new(
            position.x.as_f32() - self.info_panel_width(),
            position.y.as_f32() - self.top_bar_height(),
        );
        self.state.mouse_delta = delta;

        let over_scrubber = self.over_scrubber(position);
        let over_bar = self.over_top_bar(position);
        let over_panel = self.over_info_panel(position) || self.over_edit_panel(position);
        // egui asked `ctx.layer_id_at(pointer)` and treated any layer that was not
        // `Order::Background` as UI (HEAD `src/app.rs:816-820`), so its floating windows — the file
        // browser, the modals, the popups — all held the pointer. gpui has no layer hit-test: every
        // one of these is a child of the root div, so its clicks bubble to the root's pan,
        // wheel-zoom and double-click-fullscreen handlers no matter where the pointer is. This gate
        // is the stand-in, and like the two popup flags below it is by *existence* rather than by
        // rect — so the canvas sliver around an open card is inert too, which egui left live.
        #[cfg(not(feature = "file_open"))]
        let overlay = self.popup.is_some() || self.confirm_delete || self.browser_shown;
        // The `rfd` build has no in-app card, so there is no floating layer of its own to hold the
        // pointer; the native dialog is a separate OS window.
        #[cfg(feature = "file_open")]
        let overlay = self.popup.is_some() || self.confirm_delete;
        // egui folded `block_panning` into `mouse_grab` (HEAD `src/app.rs:821`), which is what held
        // the canvas still while a perspective-crop handle was under the pointer.
        // egui kept these two apart (HEAD `src/app.rs:811-816`): `pointer_over_ui` is only "the
        // pointer left the canvas", while `mouse_grab` adds the perspective-crop latch and paint
        // mode. Paint mode is why a left drag while painting draws a line instead of sliding the
        // image out from under the brush.
        let blocked = self.state.edit_state.block_panning;
        self.state.pointer_over_ui = over_scrubber || over_bar || over_panel || overlay;
        self.state.mouse_grab =
            self.state.pointer_over_ui || blocked || self.state.edit_state.painting;
        // egui's paint branches read the live button state each frame, so the port keeps the same
        // reading off the move event's wparam — the authoritative, self-correcting source.
        self.left_down = event.pressed_button == Some(MouseButton::Left);
        self.right_down = event.pressed_button == Some(MouseButton::Right);
        // egui only forgot its press latch on a frame where it was *told* no button is held, so the
        // reset keys off the same reported state the pan branches read.
        if event.pressed_button.is_none() {
            self.press_seen = false;
        }
        // egui repainted on every pointer event, which is what made a handle light up as the cursor
        // approached; the port only repaints what it is told changed, so a live crop overlay has to
        // ask for the frame itself.
        let crop_live = !self.crop_overlays().is_empty();
        if self.track_cursor_pixel() || crop_live {
            cx.notify();
        }
        self.crop_follow(cx);

        // egui's Measure body read `pointer.secondary_down()` off the global pointer state, so a
        // right-drag moved the rect's second corner. It never panned under that button: gpui's
        // `dragging()` is left-only, the same as egui's primary-only drag sense.
        if event.pressed_button == Some(MouseButton::Right) && self.measure_active() {
            let at = self.measure_corner();
            self.measure_write(at, false, cx);
        }

        // egui's pan ran under `drag_enabled && !mouse_grab`, and its `mouse_grab` latched whenever
        // one of its widgets owned the pointer (`egui_is_using_pointer()`) — which is what kept the
        // canvas from sliding under a panel or DragValue drag once the pointer left the panel. Paint
        // mode is part of that same latch, so it holds the image still for the brush.
        if self.press_seen
            && event.dragging()
            && !overlay
            && !over_bar
            && !over_panel
            && !blocked
            && !self.state.edit_state.painting
            && !self.grab_owned()
        {
            if over_scrubber {
                self.scrub_to(position, cx);
            } else {
                self.pan_by(delta);
            }
            cx.notify();
        } else if self.press_seen
            && event.pressed_button == Some(MouseButton::Middle)
            && !self.middle_claimed
            && !blocked
        {
            // egui added the pointer delta twice per middle-drag frame, once in its middle-down
            // branch and again under `drag_enabled`, except over the chrome where the second add
            // is gated. Mirrored rather than fixed.
            self.pan_by(if self.state.mouse_grab {
                delta
            } else {
                delta * 2.0
            });
            cx.notify();
        }
    }

    fn pan_by(&mut self, delta: Vector2<f32>) {
        self.state.drag_enabled = true;
        self.state.image_geometry.offset += delta;
        self.state.reset_image = false;
    }

    fn resolve_delete(&mut self, yes: bool, cx: &mut Context<Self>) {
        self.confirm_delete = false;
        if yes {
            utils::delete_file(&mut self.state);
            self.image = None;
        }
        cx.notify();
    }

    fn dropped_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let images: Vec<PathBuf> = paths
            .iter()
            .filter(|p| utils::is_ext_compatible(p))
            .cloned()
            .collect();
        if images.is_empty() {
            return;
        }

        let first = images[0].clone();
        self.state.is_loaded = false;
        self.state.current_image = None;
        if images.len() == 1 && first.is_dir() {
            if let Ok(dir_first) = crate::scrubber::find_first_image_in_directory(&first) {
                self.state.current_path = Some(dir_first.clone());
                self.state.player.load(&dir_first);
            }
        } else if images.len() == 1 {
            self.state.current_path = Some(first.clone());
            self.state.player.load(&first);
        } else {
            self.state.current_path = Some(first.clone());
            self.state.player.load_advanced(
                &first,
                Some(Frame::ImageCollectionMember(Default::default())),
            );
            self.state.scrubber.fixed_paths = images.iter().all(|p| p.is_file());
            self.state.scrubber.entries = images;
            self.state.scrubber.wrap = self.state.persistent_settings.wrap_folder;
        }
        self.state.reset_image = true;
        cx.notify();
    }

    /// egui's Browse command. Its three entry points all set the directory mode from the shift
    /// chord and then opened either the native dialog or, in the default build, the in-app browser
    /// — but only the two buttons went one step further: they wrote `FBPATH` from
    /// `filebrowser_path()` and dropped the parked listing when `check_refresh_entries` said the
    /// mode or the current image's directory had changed. The `Browse` shortcut only set the popup
    /// flag, so it came back to the directory the browser had been left in, listing intact.
    /// `anchored` is that difference.
    fn browse(&mut self, shift: bool, anchored: bool, cx: &mut Context<Self>) {
        self.state.filebrowser_last_dir = if shift {
            BrowserDir::CurrentImageDir
        } else {
            BrowserDir::LastOpenDir
        };
        self.state.redraw = true;

        // The native dialog has no parked listing, so it cannot tell the two routes apart.
        #[cfg(feature = "file_open")]
        let _ = anchored;

        #[cfg(feature = "file_open")]
        {
            let receiver = cx.prompt_for_paths(PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: None,
            });
            let weak = cx.entity().downgrade();
            // The handle stays owned by the entity, or dropping it would cancel the pending dialog.
            self.browse_task = Some(cx.spawn(async move |_, cx| {
                let Ok(Ok(Some(paths))) = receiver.await else {
                    return;
                };
                let _ = cx.update(|app| {
                    let _ = weak.update(app, |viewer, cx| {
                        if paths.first().is_some_and(|p| !utils::is_ext_compatible(p)) {
                            viewer.state.send_message_err("Unsupported file type");
                        } else if let Some(dir) = paths.first().and_then(|p| p.parent()) {
                            // egui remembered the directory of anything opened through the browser.
                            viewer.state.volatile_settings.last_open_directory = dir.to_path_buf();
                        }
                        viewer.dropped_paths(&paths, cx);
                    });
                });
            }));
        }

        #[cfg(not(feature = "file_open"))]
        {
            let anchor = anchored.then(|| self.state.filebrowser_path());
            // egui called `check_refresh_entries` against whatever listing `FBSTATE` held, and a
            // missing one counted as a reset. The shortcut never asked, so its listing survived.
            let reset = match (anchored, self.browser.as_ref()) {
                (false, _) => false,
                (true, None) => true,
                (true, Some(state)) => state
                    .browser
                    .check_refresh_entries(self.state.filebrowser_last_dir, anchor.as_deref()),
            };
            self.open_browser(
                browser::Purpose::Open,
                utils::SUPPORTED_EXTENSIONS
                    .iter()
                    .map(|ext| ext.to_string())
                    .collect(),
                anchor,
                reset,
                cx,
            );
        }
    }

    /// egui's `browse_modal`: one window standing in for all of its popups, with a listing parked
    /// in `FBSTATE` between them. Parked rather than dropped, so a reopen came back to the same
    /// directory — and, as nothing rescanned a directory it had already shown, to the same file
    /// list it had left behind. Only the callers that mirror egui's browser buttons pass `reset`,
    /// which is the whole listing going with them.
    #[cfg(not(feature = "file_open"))]
    fn open_browser(
        &mut self,
        purpose: browser::Purpose,
        filter: Vec<String>,
        anchor: Option<PathBuf>,
        reset: bool,
        cx: &mut Context<Self>,
    ) {
        let path = anchor.unwrap_or_else(|| self.browser_path.clone());
        let last_dir = self.state.filebrowser_last_dir;
        let was_shown = self.browser_shown;
        let fresh = self.browser.is_none();
        match self.browser.as_mut() {
            Some(state) => {
                state.purpose = purpose;
                state.filter = filter;
                if reset {
                    state.reset_listing(last_dir);
                }
                state.set_path(path);
                // egui's footer ran its guard at the top of every draw, so an empty name came back
                // with the filter's first extension on the first frame.
                state.validate_filename();
            }
            None => {
                let mut state = browser::State::new(path, purpose, filter);
                // egui's first `FBSTATE` was the reset one, which carried the mode along.
                state.browser.last_dir = last_dir;
                state.validate_filename();
                self.browser = Some(state);
            }
        }
        self.browser_shown = true;
        // egui bound its three text edits straight to the model, so they could not lag behind it;
        // gpui's inputs own their text, so every open re-seats them. Only the opens that had to
        // read the directory again are the frames egui's save footer took the keyboard in.
        self.browser_seating = !was_shown || reset;
        self.browser_refocus = fresh || reset;
        cx.notify();
    }

    /// egui's `close_popup`. `FBPATH` was already current: its window wrote it back at the end of
    /// every draw, which is what the overlay does.
    ///
    /// The keyboard goes with the window. egui's edits died alongside it, so nothing on screen could
    /// take a keystroke after the close; a gpui input is an entity that outlives the frame that drew
    /// it, and left to itself the path field kept holding the focus through to the next open.
    #[cfg(not(feature = "file_open"))]
    fn close_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.browser_shown = false;
        window.blur(cx);
        cx.notify();
    }

    /// The fields take the listing's values, the way egui's text edits were bound to them. Only the
    /// frame after an open needs it: from then on the subscriptions keep both sides in step.
    #[cfg(not(feature = "file_open"))]
    fn seat_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.browser_seating = false;
        self.browser_refocus = false;
        let Some(state) = self.browser.as_ref() else {
            return;
        };
        let term = state.browser.search_term.clone();
        let path = state.path.to_string_lossy().to_string();
        let filename = state.browser.filename.clone();
        self.browser_search
            .update(cx, |input, cx| input.set_value(term, window, cx));
        self.browser_path_field
            .update(cx, |input, cx| input.set_value(path, window, cx));
        self.browser_filename
            .update(cx, |input, cx| input.set_value(filename, window, cx));
    }

    /// The footer's two encoder rows, seated from the settings. egui's sliders were bound to
    /// `encoding_options` directly, so the row a different extension brings up always started on that
    /// encoder's own value; here the extension is what says when to copy it across again.
    #[cfg(not(feature = "file_open"))]
    fn seat_encoders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ext) = self.browser.as_ref().map(|state| state.extension()) else {
            return;
        };
        if ext == self.browser_encoder_ext {
            return;
        }
        self.browser_encoder_ext = ext;
        let (quality, compression) =
            browser::encoder_values(&self.state.volatile_settings.encoding_options);
        self.browser_quality.update(cx, |slider, cx| {
            slider.set_value(quality as f32, window, cx)
        });
        self.browser_compression.update(cx, |slider, cx| {
            slider.set_value(compression as f32, window, cx)
        });
    }

    /// The write half of [`seat_encoders`]: a drag lands on the encoder that owns the filename's
    /// extension, the way egui's `&mut` did, and nowhere else.
    #[cfg(not(feature = "file_open"))]
    fn set_encoder_value(&mut self, event: &SliderEvent, cx: &mut Context<Self>) {
        let value = match event {
            SliderEvent::Change(value) | SliderEvent::Release(value) => value,
        };
        let raw = match value {
            SliderValue::Single(v) => *v,
            SliderValue::Range(_, end) => *end,
        };
        // egui re-read the extension off the filename every frame, after its footer guard had forced it
        // into the filter, so this is the same encoder the row on screen belongs to.
        let Some(ext) = self.browser.as_ref().map(|state| state.extension()) else {
            return;
        };
        let next = raw.max(0.).round() as u32;
        if browser::set_encoder(
            &ext,
            &mut self.state.volatile_settings.encoding_options,
            next,
        ) {
            cx.notify();
        }
    }

    #[cfg(not(feature = "file_open"))]
    fn poll_browser(&mut self, cx: &mut Context<Self>) {
        if !self.browser_shown {
            return;
        }
        if self
            .browser
            .as_mut()
            .is_some_and(|state| state.poll_thumbnails())
        {
            cx.notify();
        }
    }

    /// egui's `browse_modal` ran inside its own `ctx.available_rect()` pass, which painted the
    /// window over every panel; this is the same layer at the end of the element stack.
    #[cfg(not(feature = "file_open"))]
    fn browser_overlay(
        &mut self,
        palette: theme::Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.browser_seating {
            let refocus = self.browser_refocus;
            let saves = self
                .browser
                .as_ref()
                .is_some_and(|state| state.purpose.saves());
            self.seat_browser(window, cx);
            // egui's `request_focus` ran inside that same first frame, and only for the field the
            // save footer owns. It re-ran on every frame that started without a listing, which in
            // gpui would keep repainting, so here it is asked for once, on the open that had to
            // read the directory.
            if saves && refocus {
                let focus = self.browser_filename.read(cx).focus_handle(cx);
                focus.focus(window, cx);
            }
        }
        if !self.browser_shown {
            return None;
        }
        self.seat_encoders(window, cx);
        let state = self.browser.as_ref()?;
        // egui's window wrote `FBPATH` back at the end of every draw, so a reopen — or the next
        // popup, which reads the same key — started from the directory this one was left in.
        self.browser_path = state.path.clone();
        let viewport = cx.entity().downgrade();
        Some(browser::render(
            &self.state,
            state,
            palette,
            &self.browser_search,
            &self.browser_path_field,
            &self.browser_filename,
            &self.browser_quality,
            &self.browser_compression,
            cx,
            move |event, window, app| {
                let _ = viewport.update(app, |viewer, cx| viewer.browser_event(event, window, cx));
            },
        ))
    }

    /// The build with `rfd` has no in-app browser; its pickers are native dialogs.
    #[cfg(feature = "file_open")]
    fn browser_overlay(
        &mut self,
        _palette: theme::Palette,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        None
    }

    /// egui's browser answered its own clicks inside `browse()`. Everything that reaches past the
    /// listing comes here: the settings it wrote, and the four callbacks its `browse_modal` calls
    /// passed.
    #[cfg(not(feature = "file_open"))]
    fn browser_event(
        &mut self,
        event: browser::Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            browser::Event::Close => self.close_browser(window, cx),
            browser::Event::ToggleSearch => {
                let mut opened = false;
                if let Some(state) = self.browser.as_mut() {
                    state.browser.search_active = !state.browser.search_active;
                    if state.browser.search_active {
                        opened = true;
                    } else {
                        state.clear_search();
                    }
                }
                self.seat_browser(window, cx);
                if opened {
                    // egui's `lock_search_focus`: the field took the keyboard in the same click.
                    let focus = self.browser_search.read(cx).focus_handle(cx);
                    focus.focus(window, cx);
                }
            }
            browser::Event::Up => {
                if let Some(state) = self.browser.as_mut() {
                    state.go_up();
                }
                self.seat_browser(window, cx);
            }
            browser::Event::TogglePath => {
                if let Some(state) = self.browser.as_mut() {
                    state.browser.path_active = !state.browser.path_active;
                }
                self.seat_browser(window, cx);
            }
            browser::Event::Go(path) => {
                if let Some(state) = self.browser.as_mut() {
                    state.set_path(path);
                    // egui dropped the search term on the way into a new directory.
                    if state.browser.search_active {
                        state.clear_search();
                    }
                }
                self.seat_browser(window, cx);
            }
            browser::Event::Pick(path) => {
                // egui took the clicked file's canonical parent as the last opened directory,
                // whether it was opening or only borrowing the name.
                self.state.volatile_settings.last_open_directory = path
                    .parent()
                    .and_then(|parent| parent.canonicalize().ok())
                    .unwrap_or_default();
                let purpose = self.browser.as_ref().map(|state| state.purpose);
                let saves = purpose.is_some_and(|purpose| purpose.saves());
                if saves {
                    if let Some(state) = self.browser.as_mut() {
                        let name = path
                            .file_name()
                            .map(|name| name.to_string_lossy().to_string())
                            .unwrap_or_default();
                        state.set_filename(&name);
                    }
                    self.seat_browser(window, cx);
                } else {
                    if let Some(state) = self.browser.as_mut() {
                        state.browser.search_active = false;
                        state.clear_search();
                    }
                    self.browser_event_close_and_complete(purpose, path, window, cx);
                }
            }
            browser::Event::AddBookmark => {
                if let Some(state) = self.browser.as_ref() {
                    self.state
                        .volatile_settings
                        .folder_bookmarks
                        .insert(state.path.clone());
                }
                cx.notify();
            }
            browser::Event::RemoveBookmark(path) => {
                self.state.volatile_settings.folder_bookmarks.remove(&path);
                if let Some(state) = self.browser.as_mut() {
                    state.hovered_bookmark = None;
                }
                cx.notify();
            }
            browser::Event::HoverBookmark(path, hovered) => {
                if let Some(state) = self.browser.as_mut() {
                    // A row may only clear its own entry: gpui reports the transitions per element, so
                    // an ending hover can arrive after the next row has already claimed it. egui read
                    // the frame's own `res.hovered()` and could not see that ordering at all.
                    if hovered {
                        state.hovered_bookmark = Some(path);
                    } else if state.hovered_bookmark.as_ref() == Some(&path) {
                        state.hovered_bookmark = None;
                    }
                }
            }
            browser::Event::Extension(ext) => {
                if let Some(state) = self.browser.as_mut() {
                    state.set_extension(&ext);
                }
                self.seat_browser(window, cx);
            }
            browser::Event::Save => {
                let path = self.browser.as_ref().map(|state| state.path.clone());
                let filename = self
                    .browser
                    .as_ref()
                    .map(|state| state.browser.filename.clone());
                let purpose = self.browser.as_ref().map(|state| state.purpose);
                // egui's "Save file" cleared the search, closed the popup and handed the joined
                // path to the callback it had been given.
                if let Some(state) = self.browser.as_mut() {
                    state.browser.search_active = false;
                    state.clear_search();
                }
                if let (Some(dir), Some(name)) = (path, filename) {
                    self.browser_event_close_and_complete(purpose, dir.join(name), window, cx);
                } else {
                    self.close_browser(window, cx);
                }
            }
        }
    }

    /// Writes the file or loads the image, then closes the window the way egui's callback did.
    #[cfg(not(feature = "file_open"))]
    fn browser_event_close_and_complete(
        &mut self,
        purpose: Option<browser::Purpose>,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match purpose {
            Some(browser::Purpose::Open) => self.dropped_paths(&[path], cx),
            Some(browser::Purpose::Save) => {
                let result = self.state.edit_state.result_pixel_op.clone();
                let info = self.state.image_metadata.clone();
                let encoders = self.state.volatile_settings.encoding_options.clone();
                _ = file_encoder::save_with_encoding(&result, &path, &info, &encoders);
            }
            Some(browser::Purpose::Palette) => {
                let colors = self
                    .palette
                    .iter()
                    .map(|color| ase_swatch::types::ObjectColor {
                        name: "".into(),
                        object_type: ase_swatch::types::ObjectColorType::Global,
                        data: ase_swatch::types::Color {
                            mode: ase_swatch::types::ColorMode::Rgb,
                            values: [
                                f32::from(color[0]) / 255.,
                                f32::from(color[1]) / 255.,
                                f32::from(color[2]) / 255.,
                            ]
                            .to_vec(),
                        },
                    })
                    .collect::<Vec<_>>();
                let bytes = ase_swatch::create_ase(&Vec::new(), &colors);
                if let Ok(mut file) = std::fs::File::create(&path) {
                    _ = std::io::Write::write_all(&mut file, &bytes);
                }
            }
            Some(browser::Purpose::Lut(stack, index)) => {
                // egui's callback: `*lut_name = path.to_string_lossy()` plus `mark_changed()`, which
                // is the same write the `file_open` dialog's completion makes.
                if self.stack_mut(stack).get_mut(index).is_some_and(|op| {
                    edit_panel::set_param(
                        &mut op.operation,
                        edit_panel::Param::Lut(path.to_string_lossy().to_string()),
                    )
                }) {
                    self.open_combo = None;
                    self.edit_stack_changed(stack, cx);
                }
            }
            None => {}
        }
        self.close_browser(window, cx);
    }

    /// egui's `ViewportCommand::WindowLevel` toggle. gpui has no window-level API, so the request
    /// goes to the OS directly; the flag only flips when the level actually changed, or the bar's
    /// button would advertise a state the window does not have.
    /// The core half of a fullscreen toggle: egui's own helper, which slides the image by the window
    /// origin so the picture stays under the cursor as the chrome goes and comes back.
    fn apply_fullscreen(
        &mut self,
        was_fullscreen: bool,
        origin: (f32, f32),
        cx: &mut Context<Self>,
    ) {
        let fullscreen = utils::toggle_fullscreen(&mut self.state, was_fullscreen, origin);
        debug!("Fullscreen is now {fullscreen}");
        cx.notify();
    }

    fn always_on_top(&mut self, cx: &mut Context<Self>) {
        let above = !self.state.always_on_top;
        if native::set_always_on_top(above) {
            self.state.always_on_top = above;
            debug!("Always on top is now {above}");
            cx.notify();
        } else {
            error!("Could not set the window level to {above}");
        }
    }

    /// egui's LUT "Load from disk": the row keeps either a builtin name or the path it picked, and
    /// only the row that opened the prompt is written, the way egui's closure captured `lut_name`.
    #[cfg(feature = "file_open")]
    fn load_lut(&mut self, stack: edit_panel::Stack, index: usize, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        let weak = cx.entity().downgrade();
        // The handle stays owned by the entity, or dropping it would cancel the pending dialog.
        self.lut_task = Some(cx.spawn(async move |_, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let Some(path) = paths.first() else {
                return;
            };
            let path = path.to_string_lossy().to_string();
            let _ = cx.update(|app| {
                let _ = weak.update(app, move |viewer, cx| {
                    if viewer.stack_mut(stack).get_mut(index).is_some_and(|op| {
                        edit_panel::set_param(&mut op.operation, edit_panel::Param::Lut(path))
                    }) {
                        viewer.open_combo = None;
                        viewer.edit_stack_changed(stack, cx);
                    }
                });
            });
        }));
    }

    /// egui's default-build "Load lut" opened the in-app browser the same way its openers did: the
    /// `save: false` flag, the `SUPPORTED_EXTENSIONS` filter, and neither a directory write nor a
    /// listing reset, so it came up on the parked `FBPATH`. Only its callback differed, and
    /// `Purpose::Lut` carries the row that callback writes.
    #[cfg(not(feature = "file_open"))]
    fn load_lut(&mut self, stack: edit_panel::Stack, index: usize, cx: &mut Context<Self>) {
        self.open_browser(
            browser::Purpose::Lut(stack, index),
            utils::SUPPORTED_EXTENSIONS
                .iter()
                .map(|ext| ext.to_string())
                .collect(),
            None,
            false,
            cx,
        );
    }

    /// egui's compare list entry point: the image loads with the view it was added with, so the
    /// canvas jumps straight back to that zoom and pan.
    fn load_compare(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(geometry) = self.state.compare_list.get(&path) else {
            return;
        };
        self.state.current_path = Some(path.clone());
        self.state.player.load_advanced(
            &path,
            Some(Frame::CompareResult(Default::default(), geometry)),
        );
        cx.notify();
    }

    /// egui's `file_open` "Save as...": a native dialog on a background task, then the plain image
    /// save plus the EXIF re-application.
    #[cfg(feature = "file_open")]
    fn save_as(&self, cx: &mut Context<Self>) {
        let image = self.state.edit_state.result_pixel_op.clone();
        let info = self.state.image_metadata.clone();
        let directory = self.state.volatile_settings.last_open_directory.clone();
        let sender = self.state.message_channel.0.clone();
        cx.background_executor()
            .spawn(async move {
                let Some(path) = rfd::FileDialog::new().set_directory(directory).save_file() else {
                    return;
                };
                match image.save(&path) {
                    Ok(()) => {
                        if let Some(info) = &info
                            && let Err(e) = utils::fix_exif(&path, info.raw_exif.clone())
                        {
                            error!("{e}");
                        }
                        _ = sender.send(Message::Saved(path));
                    }
                    Err(e) => {
                        _ = sender.send(Message::err(&format!("Error: Could not save: {e}")));
                    }
                }
            })
            .detach();
    }

    /// egui's default-build "Save as..." picked the path in the in-app browser, whose filter came
    /// from the configured encoders' extensions, and whose Save callback ran the matching encoder —
    /// which is what `browser_event_close_and_complete` does for `Purpose::Save`. Like its palette
    /// twin the button only opened a popup: no `FBPATH` write, no listing reset, so the browser
    /// starts on the directory the last one was left in.
    #[cfg(not(feature = "file_open"))]
    fn save_as(&mut self, cx: &mut Context<Self>) {
        let filter = self
            .state
            .volatile_settings
            .encoding_options
            .iter()
            .map(|encoder| encoder.ext())
            .collect::<Vec<String>>();
        self.open_browser(browser::Purpose::Save, filter, None, false, cx);
    }

    /// egui's "Overwrite"/"Save": the result buffer through the matching encoder, EXIF and
    /// thumbnail included.
    fn save_to_current(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.state.current_path.clone() else {
            return;
        };
        let result = self.state.edit_state.result_pixel_op.clone();
        let info = self.state.image_metadata.clone();
        let encoders = self.state.volatile_settings.encoding_options.clone();
        match file_encoder::save_with_encoding(&result, &path, &info, &encoders) {
            Ok(()) => self.state.send_message_info("Saved"),
            Err(e) => self.state.send_message_err(&format!("Error: {e}")),
        }
        cx.notify();
    }

    /// egui's ".oculante" metafiles: the edit stack as JSON, beside the image or beside the whole
    /// directory, which is what makes the edits come back on the next open.
    fn save_metafile(&self, directory: bool) {
        let Some(path) = self.state.current_path.clone() else {
            return;
        };
        let target = if directory {
            path.parent().map(|parent| parent.join(".oculante"))
        } else {
            Some(path.with_extension("oculante"))
        };
        if let Some(target) = target
            && let Ok(file) = std::fs::File::create(target)
        {
            _ = serde_json::to_writer_pretty(&file, &self.state.edit_state);
        }
    }

    /// egui's "Add current image" and "Update position", which are the same insert.
    fn add_compare(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.state.current_path.clone() {
            let geometry = self.state.image_geometry;
            self.state
                .compare_list
                .insert(CompareItem::new(path, geometry));
        }
        cx.notify();
    }
}

/// egui's zen-mode idle hide (HEAD `src/app.rs:1100-1111`): only with the chrome closed and a
/// nonzero timeout, only once the pointer has aged out, and only while it sits over the canvas —
/// `is_pointer_over_egui` is false exactly there for egui's background order.
fn idle_cursor_should_hide(
    zen_mode: bool,
    chrome_open: bool,
    timeout: f32,
    idle: f32,
    over_chrome: bool,
) -> bool {
    zen_mode && !chrome_open && timeout > 0. && idle >= timeout && !over_chrome
}

/// egui's keyboard ZoomIn/ZoomOut (app.rs:906-933) anchored `scale_pt` on `state.window_size / 2.0`,
/// and that field held `ctx.content_rect()` — the whole window, not the canvas `image_geometry.offset`
/// is measured in. So egui's anchor sat half a panel-width right and half a bar-height below the
/// canvas centre, which the port has to reproduce rather than improve on: `center` is the whole
/// client centre, the port's counterpart to that field.
fn zoom_key(state: &mut OculanteState, amount: f32, center: Vector2<f32>) {
    let delta = utils::zoomratio(amount, state.image_geometry.scale);
    let new_scale = state.image_geometry.scale + delta;
    if new_scale <= 0.05 || new_scale >= 40. {
        return;
    }
    state.image_geometry.offset -= utils::scale_pt(
        state.image_geometry.offset,
        center,
        state.image_geometry.scale,
        delta,
    );
    state.image_geometry.scale = new_scale;
}

impl Render for Viewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bounds = window.bounds();
        let size = bounds.size;
        let raw = Vector2::new(size.width.as_f32(), size.height.as_f32());
        self.raw_size = raw;
        // egui's overlay used `ctx.input(|i| i.stable_dt)`, which it smooths; this is the plain
        // interval between two renders, which for a once-per-frame view is the same quantity.
        #[cfg(debug_assertions)]
        {
            let now = Instant::now();
            if let Some(previous) = self.debug_last_frame.replace(now) {
                let dt = now.duration_since(previous).as_secs_f32();
                self.debug_fps = if dt > 0. { 1. / dt } else { 0. };
            }
            self.debug_passes += 1;
        }
        let top = self.top_bar_height();
        let info = self.info_panel_width();
        let edit = self.edit_panel_width();
        let info_dragging = self.panel_drag == Some(panel::Side::Left);
        let edit_dragging = self.panel_drag == Some(panel::Side::Right);
        self.state.window_size = Vector2::new(
            (raw.x - info - edit).max(0.),
            (raw.y - top - self.scrub_bar_height()).max(0.),
        );

        if self.state.persistent_settings.fit_image_on_window_resize
            && size != self.last_window_size
        {
            self.state.reset_image = true;
        }
        self.last_window_size = size;

        // egui wrote the outer rect of every frame here; only the size half is ever read back, by
        // `build_window_settings` on the next launch (window_config.rs:63). gpui reports the client
        // bounds and restores them as client bounds, so the value round-trips without drifting, and
        // a window closed in fullscreen persists its fullscreen size like egui's did. The cast
        // saturates a negative origin to 0 the way egui's `as u32` did.
        self.state.volatile_settings.window_geometry = (
            (
                bounds.origin.x.as_f32() as u32,
                bounds.origin.y.as_f32() as u32,
            ),
            (size.width.as_f32() as u32, size.height.as_f32() as u32),
        );

        if self.state.reset_image {
            self.fit_to_window();
        }

        // egui ran the edit pipeline at the end of its panel, so only a visible panel costs work.
        // Its widgets wrote the operators they belonged to in place; here the sliders are entities,
        // so their values are copied across first and the same frame processes them.
        if edit > 0. {
            let changed = self.params.apply(&mut self.state.edit_state, cx);
            self.sync_edits(changed, cx);
        }

        // egui's `"compare"` temp tag: the entry is stored once the new image has settled into its
        // fitted view, because that view is what the entry later restores.
        if self.pending_compare
            && self.state.is_loaded
            && !self.state.reset_image
            && let Some(path) = self.state.current_path.clone()
        {
            let geometry = self.state.image_geometry;
            self.state
                .compare_list
                .insert(CompareItem::new(path, geometry));
            self.pending_compare = false;
        }

        let title = utils::window_title(&self.state);
        if title != self.title {
            self.title = title.clone();
            window.set_window_title(&title);
        }

        let geometry = self.state.image_geometry;
        let (image_w, image_h) = geometry.dimensions;
        let texture = self.image.clone();

        // egui's `show_checker_background` quad: one Repeat-wrapped texture per image copy, whose UV
        // ran `0..copy_size / 128` so the pattern kept a fixed screen size and stayed anchored to the
        // copy's corner. gpui's atlas has no wrap mode, so a canvas behind the tiles paints the same
        // pattern a cell at a time, over only the part of the copy the window can show.
        //
        // The tile is baked for the device grid, so a window that lands on another DPI rebuilds it.
        let scale_factor = window.scale_factor();
        let tile = self
            .checker
            .clone()
            .filter(|(baked, _)| *baked == scale_factor)
            .map(|(_, tile)| tile)
            .or_else(|| {
                let tile = checker::tile_texture(scale_factor)?;
                self.checker = Some((scale_factor, tile.clone()));
                Some(tile)
            });
        let checker = match (self.state.persistent_settings.show_checker_background, tile) {
            (true, Some(tile)) => Some(
                canvas(
                    {
                        let tiling = self.state.tiling.max(1) as u32;
                        let (w, h) = (
                            image_w as f32 * geometry.scale,
                            image_h as f32 * geometry.scale,
                        );
                        let origin = (geometry.offset.x + info, geometry.offset.y + top);
                        move |rect: Bounds<Pixels>, _window, _cx| {
                            let clip = [
                                rect.origin.x.as_f32(),
                                rect.origin.y.as_f32(),
                                rect.size.width.as_f32(),
                                rect.size.height.as_f32(),
                            ];
                            (0..tiling)
                                .flat_map(move |rep_y| {
                                    (0..tiling).map(move |rep_x| {
                                        checker::grid(
                                            [
                                                origin.0 + rep_x as f32 * w,
                                                origin.1 + rep_y as f32 * h,
                                                w,
                                                h,
                                            ],
                                            clip,
                                        )
                                    })
                                })
                                .flatten()
                                .collect::<Vec<_>>()
                        }
                    },
                    move |_rect, cells, window, _cx| {
                        for cell in cells {
                            let (bounds, image_bounds) = cell.paint();
                            let _ = window.paint_image(
                                bounds,
                                image_bounds,
                                Corners::default(),
                                tile.clone(),
                                0,
                                false,
                            );
                        }
                    },
                )
                .absolute()
                .inset_0(),
            ),
            (false, _) | (true, None) => None,
        };

        // egui read its `SettingsUiState` from inside the window's own widgets; gpui's fields and
        // pickers own their values, so they are copied into the settings before the palette is
        // resolved. A theme switch has to be seen in the same frame it was clicked in.
        if self.state.settings_enabled {
            self.watch_settings_widgets(cx);
            self.settings_form.apply(&mut self.state, &*cx);
        }

        let palette = theme::resolve(&self.state, cx.window_appearance());
        if self.applied_palette != Some(palette) {
            theme::apply(palette, cx);
            self.applied_palette = Some(palette);
        }

        // A grabbed numeric field or a grabbed panel edge follows the pointer until release; egui
        // re-ran its drag on every frame the pointer moved. gpui only accepts window-level mouse
        // listeners during paint, so the drag is tracked by an overlay element instead, which also
        // swallows the clicks that would otherwise land on the panel mid-drag.
        let drag_overlay = self.grab_owned().then(|| {
            let viewport = cx.entity().downgrade();
            div()
                .absolute()
                .inset_0()
                .on_mouse_move(move |event, _window, cx| {
                    let x: f32 = event.position.x.into();
                    let _ = viewport.update(cx, |viewer, cx| {
                        if let Some(side) = viewer.panel_drag {
                            let from_edge = match side {
                                panel::Side::Left => x,
                                panel::Side::Right => viewer.raw_size.x - x,
                            };
                            viewer.resize_panel(side, from_edge);
                            cx.notify();
                            return;
                        }
                        if let Some(drag) = viewer.settings_drag {
                            let value = (drag.value + f64::from(x - drag.x) * drag.speed)
                                .clamp(drag.lo, drag.hi);
                            let value = if drag.integer { value.round() } else { value };
                            settings::apply(
                                &mut viewer.state,
                                drag.field,
                                &settings::Value::Num(value),
                            );
                            cx.notify();
                            return;
                        }
                        if let Some((index, grab)) = viewer.stroke_drag {
                            let value = (grab.value + f64::from(x - grab.x) * grab.speed)
                                .clamp(grab.lo, grab.hi);
                            let painted = viewer
                                .state
                                .edit_state
                                .paint_strokes
                                .get_mut(index)
                                .is_some_and(|stroke| {
                                    let next = value as f32;
                                    if next == stroke.width {
                                        return false;
                                    }
                                    stroke.width = next;
                                    true
                                });
                            if painted {
                                viewer.stroke_changed(cx);
                            }
                            return;
                        }
                        let Some(drag) = viewer.num_drag else {
                            return;
                        };
                        let grab = drag.grab;
                        let value = (grab.value + f64::from(x - grab.x) * grab.speed)
                            .clamp(grab.lo, grab.hi);
                        let value = if grab.integer { value.round() } else { value };
                        let changed = viewer
                            .stack_mut(drag.stack)
                            .get_mut(drag.index)
                            .is_some_and(|op| {
                                edit_panel::set_num(&mut op.operation, grab.slot, value)
                            });
                        if changed {
                            viewer.edit_stack_changed(drag.stack, cx);
                        }
                    });
                })
                .on_mouse_up(MouseButton::Left, {
                    let viewport = cx.entity().downgrade();
                    move |event, window, cx| {
                        let _ = viewport.update(cx, |viewer, cx| {
                            // egui's `DragValue` read a press that never travelled further than
                            // `max_click_dist` as the click that opens its text field, so the grab
                            // answers that question before it goes.
                            let click = viewer.num_drag.take().filter(|drag| {
                                drag.grab
                                    .clicked(event.position.x.into(), event.position.y.into())
                            });
                            viewer.settings_drag = None;
                            viewer.stroke_drag = None;
                            viewer.panel_drag = None;
                            if let Some(drag) = click {
                                viewer
                                    .begin_num_edit(drag.stack, drag.index, drag.grab, window, cx);
                            }
                            // The overlay only exists while a drag is live, and it covers the whole
                            // window, so releasing has to dirty the view to drop it.
                            cx.notify();
                        });
                    }
                })
        });
        let scrub = self.scrub_bar_height();

        let viewport = cx.entity();
        let delete_open = self.confirm_delete.then(|| delete_prompt(&self.state));

        // egui's Measure section only ever drew a widget-less dark panel; what it was really doing was
        // keeping the operator in the stack and painting the rect from its `ui` arm. The port keeps
        // the operator here and paints on `Order::Middle`'s depth — over the image tiles, under the
        // side panels — which in this tree is a child between them.
        self.ensure_measure_op();
        let measure = self
            .measure_active()
            .then(|| self.measure_marks())
            .filter(|marks| !marks.is_empty())
            .map(|marks| {
                canvas(
                    move |_, _, _| marks.clone(),
                    move |_, marks, window, cx| {
                        for mark in marks.iter() {
                            mark.paint(window, cx);
                        }
                    },
                )
                .absolute()
                .inset_0()
            });

        // egui's crop scrim was `Rect::EVERYTHING` drawn through the edit panel's painter, which is
        // clipped to the window's content rect — so it washed over the chrome as well as the image.
        // That puts this canvas above the panels rather than in the Measure overlay's slot under
        // them, and below the toasts, which egui kept in a higher order.
        let crop = {
            let overlays = self.crop_overlays();
            (!overlays.is_empty()).then(|| {
                canvas(
                    move |_, _, _| overlays.clone(),
                    move |_, overlays, window, cx| {
                        for overlay in overlays.iter() {
                            overlay.paint(window, cx);
                        }
                    },
                )
                .absolute()
                .inset_0()
            })
        };

        // egui drew the overlay through an `Order::Foreground` layer painter, so it covered the
        // panels too — hence it is the viewport's last child.
        #[cfg(debug_assertions)]
        let debug = Some(top_bar::debug_overlay(
            top_bar::debug_anchor(raw.x, raw.y, info, edit, scrub),
            self.debug_fps,
            self.debug_passes,
            std::mem::take(&mut self.debug_repaint),
        ));
        #[cfg(not(debug_assertions))]
        let debug: Option<Div> = None;

        div()
            .id("viewport")
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(palette.backdrop)
            .on_mouse_down(MouseButton::Left, {
                let viewport = viewport.clone();
                move |event, window, cx| {
                    viewport.update(cx, |viewer, cx| viewer.mouse_down(event, window, cx));
                }
            })
            .on_mouse_down(MouseButton::Middle, {
                let viewport = viewport.clone();
                move |event, _window, cx| {
                    viewport.update(cx, |viewer, cx| viewer.middle_press(event.position, cx));
                }
            })
            .on_mouse_down(MouseButton::Right, {
                let viewport = viewport.clone();
                move |event, _window, cx| {
                    viewport.update(cx, |viewer, cx| viewer.right_click(event.position, cx));
                }
            })
            .on_mouse_up(MouseButton::Left, {
                let viewport = viewport.clone();
                move |event, _window, cx| {
                    let _ = viewport.update(cx, |viewer, cx| {
                        viewer.left_down = false;
                        viewer.crop_release(event.position, cx);
                        // egui repainted on every pointer event, so the frame that turned the just
                        // finished stroke into a clone of itself ran the moment the button came up.
                        if viewer.state.edit_state.painting {
                            viewer.stroke_changed(cx);
                        }
                    });
                }
            })
            .on_mouse_up(MouseButton::Right, {
                let viewport = viewport.clone();
                move |event, _window, cx| {
                    viewport.update(cx, |viewer, cx| viewer.right_release(event.position, cx));
                }
            })
            .on_mouse_move({
                let viewport = viewport.clone();
                move |event, _window, cx| {
                    viewport.update(cx, |viewer, cx| viewer.mouse_moved(event, cx));
                }
            })
            .on_scroll_wheel({
                let viewport = viewport.clone();
                move |event, _window, cx| {
                    viewport.update(cx, |viewer, cx| viewer.scroll(event, cx));
                }
            })
            .can_drop(|dragged, _, _| dragged.is::<ExternalPaths>())
            .on_drop({
                let viewport = viewport.clone();
                move |paths: &ExternalPaths, _, cx| {
                    let paths = paths.paths().to_vec();
                    viewport.update(cx, |viewer, cx| viewer.dropped_paths(&paths, cx));
                }
            })
            .children(checker)
            .children({
                // egui painted `tiling x tiling` copies of the image, each offset by one image
                // width/height in screen space.
                let tiling = self.state.tiling.max(1) as u32;
                let w = image_w as f32 * geometry.scale;
                let h = image_h as f32 * geometry.scale;
                let frame = self.state.persistent_settings.show_frame;
                let mut tiles = Vec::new();
                if let Some(texture) = texture {
                    for rep_y in 0..tiling {
                        for rep_x in 0..tiling {
                            let (left, above) = (
                                geometry.offset.x + info + rep_x as f32 * w,
                                geometry.offset.y + top + rep_y as f32 * h,
                            );
                            // egui drew one UV-cropped quad per GPU tile of a copy; the same split
                            // lives here as one positioned div per uploaded tile, so a frame wider
                            // than the atlas still paints every pixel of itself.
                            for tile in texture.iter() {
                                tiles.push(
                                    div()
                                        .absolute()
                                        .left(px(left + tile.x as f32 * geometry.scale))
                                        .top(px(above + tile.y as f32 * geometry.scale))
                                        .w(px(tile.width as f32 * geometry.scale))
                                        .h(px(tile.height as f32 * geometry.scale))
                                        .child(
                                            img(ImageSource::Render(tile.image))
                                                .size_full()
                                                .object_fit(ObjectFit::Fill),
                                        ),
                                );
                            }
                            // egui's `show_frame`: a 1pt `Color32::GRAY` stroke drawn inside the copy's
                            // rect, after that copy's tiles, so it covers the outermost image pixels
                            // rather than the backdrop beside them.
                            if frame {
                                tiles.push(
                                    div()
                                        .absolute()
                                        .left(px(left))
                                        .top(px(above))
                                        .w(px(w))
                                        .h(px(h))
                                        .border_1()
                                        .border_color(rgb(0x808080)),
                                );
                            }
                        }
                    }
                }
                tiles
            })
            .children(measure)
            .children({
                let viewport = viewport.downgrade();
                let handler = viewport.clone();
                info_panel::render(
                    &self.state,
                    palette,
                    top,
                    scrub,
                    info,
                    info_dragging,
                    self.info_sections,
                    &self.palette,
                    &self.tiling,
                    self.image.clone(),
                    move |event, app| {
                        let _ = handler.update(app, |viewer, cx| match event {
                            info_panel::Event::Toggle(section) => {
                                viewer.toggle_section(section);
                                cx.notify();
                            }
                            info_panel::Event::AlphaBleed => {
                                viewer.alpha_preview(utils::highlight_bleed, cx)
                            }
                            info_panel::Event::AlphaSemitrans => {
                                viewer.alpha_preview(utils::highlight_semitrans, cx)
                            }
                            info_panel::Event::AlphaReset => viewer.reset_alpha_preview(cx),
                            info_panel::Event::Clear => viewer.clear_palette(cx),
                            info_panel::Event::Sort => viewer.sort_palette(cx),
                            info_panel::Event::Save => viewer.save_ase(cx),
                            info_panel::Event::FromImage => viewer.quantize_palette(cx),
                            info_panel::Event::Copy(color) => viewer.copy_color(color, cx),
                            info_panel::Event::Remove(color) => viewer.remove_color(color, cx),
                            info_panel::Event::CompareOpen => {
                                // egui cleared `is_loaded` here too, so the tag below cannot match
                                // the image that is still on screen.
                                viewer.state.is_loaded = false;
                                viewer.pending_compare = true;
                                // egui's button read the shift chord off the input state to anchor
                                // the browser at the current image's directory. This panel's
                                // callback has no window to ask, so the button always anchors at
                                // the last opened directory, which is what the chord gives when it
                                // is held down.
                                viewer.browse(false, true, cx);
                            }
                            info_panel::Event::CompareKeepView(keep) => {
                                viewer.state.persistent_settings.compare_keep_view = keep;
                                cx.notify();
                            }
                            info_panel::Event::CompareLoad(path) => viewer.load_compare(path, cx),
                            info_panel::Event::CompareRemove(path) => {
                                viewer.state.compare_list.remove(&path);
                                cx.notify();
                            }
                            info_panel::Event::CompareAdd | info_panel::Event::CompareUpdate => {
                                viewer.add_compare(cx)
                            }
                            info_panel::Event::CompareClear => {
                                viewer.state.compare_list.clear();
                                cx.notify();
                            }
                            info_panel::Event::ResizeStart => {
                                viewer.panel_drag = Some(panel::Side::Left);
                                // The overlay that follows the pointer only exists once the view is
                                // dirty, and a press on a plain div does not dirty it by itself.
                                cx.notify();
                            }
                        });
                    },
                )
            })
            .children({
                let viewport = viewport.downgrade();
                let handler = viewport.clone();
                let query = self.filter_search.read(cx).value().to_lowercase();
                edit_panel::render(
                    &self.state,
                    palette,
                    top,
                    scrub,
                    edit,
                    edit_dragging,
                    self.filters_open,
                    self.lossless_open,
                    self.crop_applied,
                    query,
                    self.filter_search.clone(),
                    &mut self.params,
                    window,
                    cx,
                    self.open_combo,
                    self.stroke_combo,
                    move |event, app| {
                        let _ = handler.update(app, |viewer, cx| match event {
                            edit_panel::Event::ToggleFilters => {
                                viewer.filters_open = !viewer.filters_open;
                                cx.notify();
                            }
                            edit_panel::Event::CreateOutputFile => viewer.create_output_file(cx),
                            #[cfg(feature = "turbo")]
                            edit_panel::Event::ToggleLossless => {
                                viewer.lossless_open = !viewer.lossless_open;
                                // egui pushed the missing crop operator while drawing the section,
                                // which is the same user action as the click that opens it here.
                                if viewer.lossless_open {
                                    viewer.ensure_crop_op();
                                }
                                cx.notify();
                            }
                            #[cfg(feature = "turbo")]
                            edit_panel::Event::Lossless(edit) => viewer.lossless_edit(edit, cx),
                            edit_panel::Event::Add(operation) => viewer.add_filter(operation, cx),
                            edit_panel::Event::Move(stack, from, to) => {
                                viewer.move_op(stack, from, to, cx)
                            }
                            edit_panel::Event::Bypass(stack, index) => {
                                viewer.bypass_op(stack, index, cx)
                            }
                            edit_panel::Event::Remove(stack, index) => {
                                viewer.remove_op(stack, index, cx)
                            }
                            edit_panel::Event::CropApply(stack) => viewer.crop_apply(stack, cx),
                            edit_panel::Event::CropReset(stack, index) => {
                                viewer.crop_reset(stack, index, cx)
                            }
                            edit_panel::Event::SetParam(stack, index, param) => {
                                let changed =
                                    viewer.stack_mut(stack).get_mut(index).is_some_and(|op| {
                                        edit_panel::set_param(&mut op.operation, param)
                                    });
                                if changed {
                                    viewer.open_combo = None;
                                    viewer.edit_stack_changed(stack, cx);
                                }
                            }
                            edit_panel::Event::ToggleCombo(stack, index, field) => {
                                let id = viewer.stack_mut(stack).get(index).map(|op| op.id);
                                viewer.open_combo = match (id, viewer.open_combo) {
                                    (Some(id), Some(open)) if open == (id, field) => None,
                                    (Some(id), _) => Some((id, field)),
                                    (None, _) => None,
                                };
                                cx.notify();
                            }
                            edit_panel::Event::NumDragStart(stack, index, grab) => {
                                viewer.num_drag = Some(NumDrag { stack, index, grab });
                            }
                            edit_panel::Event::PaintMode(on) => {
                                viewer.state.edit_state.painting = on;
                                // egui's paint block created the first empty stroke in the very same
                                // pass it was enabled in, which is what `paint_pass` does here.
                                viewer.stroke_changed(cx);
                            }
                            edit_panel::Event::KeepHistory(keep) => {
                                viewer.state.edit_state.non_destructive_painting = keep;
                                cx.notify();
                            }
                            edit_panel::Event::Stroke(index, param) => {
                                let changed = viewer
                                    .state
                                    .edit_state
                                    .paint_strokes
                                    .get_mut(index)
                                    .is_some_and(|stroke| match param {
                                        edit_panel::StrokeParam::Fade(value) => {
                                            stroke.fade != value && {
                                                stroke.fade = value;
                                                true
                                            }
                                        }
                                        edit_panel::StrokeParam::Flip(value) => {
                                            stroke.flip_random != value && {
                                                stroke.flip_random = value;
                                                true
                                            }
                                        }
                                        edit_panel::StrokeParam::Brush(value) => {
                                            stroke.brush_index != value && {
                                                stroke.brush_index = value;
                                                true
                                            }
                                        }
                                        edit_panel::StrokeParam::Hover(value) => {
                                            stroke.highlight != value && {
                                                stroke.highlight = value;
                                                true
                                            }
                                        }
                                    });
                                if changed {
                                    if matches!(param, edit_panel::StrokeParam::Brush(_)) {
                                        // egui's ComboBox folded up on the pick that changed the value.
                                        viewer.stroke_combo = None;
                                    }
                                    viewer.stroke_changed(cx);
                                } else {
                                    cx.notify();
                                }
                            }
                            edit_panel::Event::StrokeDrag(index, grab) => {
                                viewer.stroke_drag = Some((index, grab));
                            }
                            edit_panel::Event::StrokeUndo => {
                                // egui popped twice: the empty stroke being built, and the finished
                                // one its settings were inherited from.
                                viewer.state.edit_state.paint_strokes.pop();
                                viewer.state.edit_state.paint_strokes.pop();
                                viewer.stroke_changed(cx);
                            }
                            edit_panel::Event::StrokeClear => {
                                viewer.state.edit_state.paint_strokes.clear();
                                viewer.stroke_changed(cx);
                            }
                            edit_panel::Event::StrokeRemove(index) => {
                                if index < viewer.state.edit_state.paint_strokes.len() {
                                    viewer.state.edit_state.paint_strokes.remove(index);
                                    viewer.stroke_changed(cx);
                                }
                            }
                            edit_panel::Event::StrokeCombo(index) => {
                                viewer.stroke_combo = if viewer.stroke_combo == Some(index) {
                                    None
                                } else {
                                    Some(index)
                                };
                                cx.notify();
                            }
                            edit_panel::Event::ResizeStart => {
                                viewer.panel_drag = Some(panel::Side::Right);
                                cx.notify();
                            }
                            edit_panel::Event::LoadLut(stack, index) => {
                                viewer.load_lut(stack, index, cx)
                            }
                            edit_panel::Event::MoreLuts => {
                                let _ = webbrowser::open("https://github.com/cedeber/hald-clut");
                            }
                            edit_panel::Event::Gradient(stack, index, edit) => {
                                // egui kept the bar's grabbed stop in one ctx temp slot that every
                                // gradient in the stack shared; both it and the hovered stop live on
                                // Params here, because they are what the bar draws its handles from.
                                let grab = viewer.params.grab;
                                let mut bar = None;
                                let mut changed = false;
                                if let Some(op) = viewer.stack_mut(stack).get_mut(index) {
                                    match edit {
                                        edit_panel::GradientEdit::Add
                                        | edit_panel::GradientEdit::Delete(_) => {
                                            changed =
                                                edit_panel::gradient_edit(&mut op.operation, edit);
                                        }
                                        _ => {
                                            bar = Some(edit_panel::bar_drag(
                                                &mut op.operation,
                                                grab,
                                                edit,
                                            ));
                                        }
                                    }
                                }
                                if let Some(bar) = bar {
                                    viewer.params.grab = bar.grab;
                                    viewer.params.hover = bar.hover;
                                    changed = bar.changed;
                                }
                                if changed {
                                    viewer.edit_stack_changed(stack, cx);
                                } else {
                                    cx.notify();
                                }
                            }
                            edit_panel::Event::ResetResize(stack, index) => {
                                let dimensions = viewer.state.image_geometry.dimensions;
                                if let Some(ImageOperation::Resize {
                                    dimensions: slot,
                                    aspect,
                                    ..
                                }) = viewer
                                    .stack_mut(stack)
                                    .get_mut(index)
                                    .map(|op| &mut op.operation)
                                {
                                    *slot = dimensions;
                                    *aspect = true;
                                    viewer.edit_stack_changed(stack, cx);
                                }
                            }
                            edit_panel::Event::SkipProcessing(skip) => {
                                viewer.set_skip_processing(skip, cx)
                            }
                            edit_panel::Event::ApplyAll => viewer.apply_all_edits(cx),
                            edit_panel::Event::RemoveAll => viewer.remove_all_edits(cx),
                            edit_panel::Event::Reload => viewer.reload_current(cx),
                            edit_panel::Event::SaveAs => viewer.save_as(cx),
                            edit_panel::Event::Save => {
                                if viewer
                                    .state
                                    .current_path
                                    .as_ref()
                                    .is_some_and(|path| path.exists())
                                {
                                    viewer.confirm_overwrite = true;
                                    cx.notify();
                                } else {
                                    viewer.save_to_current(cx);
                                }
                            }
                            edit_panel::Event::SaveEdits => viewer.save_metafile(false),
                            edit_panel::Event::SaveDirEdits => viewer.save_metafile(true),
                        });
                    },
                )
            })
            .children(top_bar::channel_notice(&self.state, palette, scrub))
            .children({
                let viewport = viewport.downgrade();
                top_bar::render(
                    &self.state,
                    // egui measured its bar widgets against `ctx.content_rect().width()`, the whole
                    // client width, which is what the absolute, `w_full` strip actually gets.
                    raw.x,
                    palette,
                    self.popup,
                    move |event, window, app| {
                        let _ = viewport.update(app, |viewer, cx| match event {
                            top_bar::Event::Command(InputEvent::Browse) => {
                                viewer.popup = None;
                                // egui's folder button is the anchored route: `FBPATH` from
                                // `filebrowser_path()`, listing reset when `check_refresh_entries`
                                // said so. The same `InputEvent` off the keyboard is not.
                                viewer.browse(window.modifiers().shift, true, cx);
                            }
                            top_bar::Event::Command(command) => {
                                viewer.popup = None;
                                viewer.dispatch(command, window, cx)
                            }
                            top_bar::Event::Channel(channel) => {
                                viewer.popup = None;
                                viewer.set_channel(channel);
                                cx.notify();
                            }
                            top_bar::Event::Popup(popup) => {
                                viewer.popup = popup;
                                cx.notify();
                            }
                            top_bar::Event::Recent(path) => {
                                viewer.popup = None;
                                utils::load_image_from_path(&path, &mut viewer.state);
                                cx.notify();
                            }
                            top_bar::Event::RecentClear => {
                                viewer.popup = None;
                                viewer.state.volatile_settings.recent_images.clear();
                                cx.notify();
                            }
                            top_bar::Event::Settings => {
                                viewer.popup = None;
                                viewer.toggle_settings(cx);
                            }
                            top_bar::Event::ViewOneToOne => {
                                viewer.popup = None;
                                // egui's menu route anchored on its `window_size`, the whole content
                                // rect; the port keeps that number in `raw_size`, because
                                // `state.window_size` holds the canvas here.
                                utils::set_zoom(
                                    1.0,
                                    Some(viewer.raw_size / 2.0),
                                    &mut viewer.state,
                                );
                                cx.notify();
                            }
                        });
                    },
                )
            })
            .children(self.scrubber_bar(palette))
            .children(crop)
            .children({
                let viewport = cx.entity().downgrade();
                let hover = toasts::Hover {
                    card: self.hovered_toast,
                    close: self.hovered_close,
                };
                toasts::render(
                    &self.state.toasts,
                    palette,
                    self.info_panel_width(),
                    scrub,
                    hover,
                    move |event, app| {
                        let _ = viewport.update(app, |viewer, cx| match event {
                            toasts::Event::Hover(id, hovered) => {
                                viewer.hover_toast(id, hovered, cx)
                            }
                            toasts::Event::HoverClose(id, hovered) => {
                                viewer.hover_close(id, hovered, cx)
                            }
                            toasts::Event::Dismiss(id) => viewer.dismiss_toast(id, cx),
                        });
                    },
                )
            })
            .children({
                let viewport = cx.entity().downgrade();
                let pending = self.pending_shortcut.clone();
                settings::render(
                    &self.state,
                    palette,
                    &mut self.settings_form,
                    pending.as_ref(),
                    window,
                    cx,
                    move |event, _window, app| {
                        let _ = viewport.update(app, |viewer, cx| viewer.settings_event(event, cx));
                    },
                )
            })
            .children(delete_open.map(|prompt| {
                let viewport = cx.entity().downgrade();
                modal::confirm(prompt, palette, move |yes, app| {
                    let _ = viewport.update(app, |viewer, cx| viewer.resolve_delete(yes, cx));
                })
            }))
            .children(self.confirm_overwrite.then(|| {
                let viewport = cx.entity().downgrade();
                modal::confirm("Overwrite?".to_string(), palette, move |yes, app| {
                    let _ = viewport.update(app, |viewer, cx| {
                        viewer.confirm_overwrite = false;
                        if yes {
                            viewer.save_to_current(cx);
                        } else {
                            cx.notify();
                        }
                    });
                })
            }))
            .children(self.browser_overlay(palette, window, cx))
            .children(drag_overlay)
            .children(debug)
    }
}

/// egui worded the delete prompt with just the file name.
fn delete_prompt(state: &OculanteState) -> String {
    let name = state
        .current_path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    format!("Move \"{name}\" to the trash?")
}

/// egui persisted the settings from eframe's `on_exit`, but gpui's Windows backend closes the last
/// window without ever reaching `App::shutdown()`, so `App::on_app_quit` does not run on that route.
/// The root view being dropped is the one hook that fires whatever the exit path was.
impl Drop for Viewer {
    fn drop(&mut self) {
        self.save_settings();
    }
}

#[cfg(test)]
mod tests {
    use super::idle_cursor_should_hide;

    /// The hide is egui's `zen_mode && !settings_enabled && timeout > 0.0 && idle >= timeout &&
    /// !is_pointer_over_egui()` (HEAD `src/app.rs:1100-1111`), and every one of those clauses has to
    /// hold: the chrome, a popup and the scrubber all keep the pointer, and a zero timeout means the
    /// feature is off.
    #[test]
    fn idle_cursor_hides_only_over_an_untouched_zen_canvas() {
        assert!(idle_cursor_should_hide(true, false, 1.5, 1.5, false));
        assert!(idle_cursor_should_hide(true, false, 1.5, 60., false));

        // Still moving, or not yet at the deadline.
        assert!(!idle_cursor_should_hide(true, false, 1.5, 1.49, false));
        // The chrome is open, in either of egui's two senses for it.
        assert!(!idle_cursor_should_hide(true, true, 1.5, 60., false));
        assert!(!idle_cursor_should_hide(true, false, 1.5, 60., true));
        // Not zen mode at all, or the timeout switched the feature off.
        assert!(!idle_cursor_should_hide(false, false, 1.5, 60., false));
        assert!(!idle_cursor_should_hide(true, false, 0., 60., false));
    }

    /// A negative timeout is egui's `hide_delay > 0.0` guard, which also keeps `idle >= timeout`
    /// from firing on a nonsensical setting.
    #[test]
    fn a_negative_timeout_never_hides() {
        assert!(!idle_cursor_should_hide(true, false, -1., 60., false));
    }
}
