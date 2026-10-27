//! egui's `edit_ui`: the right-hand filter panel.
//!
//! The panel only owns its widgets and the stack rows. Everything that actually touches pixels
//! runs in [`crate::gpui_app::viewer::Viewer`], which owns the edit buffers.

use std::collections::{HashMap, HashSet};
use std::ops::RangeInclusive;
use std::path::Path;
use std::sync::Arc;

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::color_picker::{ColorPicker, ColorPickerState};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::radio::{Radio, RadioGroup};
use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use image::ColorType;
use strum::IntoEnumIterator;

use crate::appstate::OculanteState;
use crate::gpui_app::{fonts, panel, texture, theme};
use crate::icons;
use crate::image_editing::{
    Channel, ColorTypeExt, EditState, GradientStop, ImageOperation, ImgOpItem, ScaleFilter,
    builtin_luts, closest_pt, interpolate_u8,
};
use crate::paint::PaintStroke;

/// egui's `ScrollArea::vertical().max_height(300.)` over the filter list.
const LIST_HEIGHT: f32 = 300.;
/// egui's caret size in the stack rows; the remove glyph was 1.5x that.
const CARET: f32 = 10.;
/// The width egui's `styled_slider` reserved for its monospace value readout.
const READOUT: f32 = 40.;
/// egui's `InputSettings::max_click_dist`, in points: how far a `DragValue`'s pointer may travel
/// between press and release before the widget reads the gesture as a drag rather than the click
/// that opens its text field. The port is px-sized throughout, so points and device pixels meet.
const CLICK_DIST: f32 = 6.;
/// The width of that text field. egui sized the field to the readout it replaced; this fits the
/// widest readout the rows here can show.
const NUM_EDIT_WIDTH: f32 = 68.;
/// egui drew each brush preview as `Vec2::splat(ui.available_height())` — a square as tall as its
/// row — beside the combo trigger and beside every option. The row's height here is set by its tallest
/// widget, the colour swatch, which measures 30 points (45 device px at 1.5x), so that is the dab's
/// side. Pinned rather than read from the layout because gpui gives an element no height of its own.
const BRUSH_PREVIEW: f32 = 30.;

/// Which of egui's two operator stacks a row belongs to. The image stack runs first and may
/// change the image's geometry, the pixel stack then works over the result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stack {
    Image,
    Pixel,
}

/// What the panel asks the viewer to do. The stacks and their buffers live on the viewer, so no
/// event carries more than what it takes to name the row.
#[derive(Clone)]
pub enum Event {
    ToggleFilters,
    /// egui cloned the template out of its per-frame `ops` array and pushed it onto whichever
    /// stack `is_per_pixel` said it belonged to.
    Add(ImageOperation),
    Move(Stack, usize, usize),
    Bypass(Stack, usize),
    Remove(Stack, usize),
    /// egui's checkboxes, radios and selectable buttons wrote the operator in place and marked the
    /// response changed; the value set is fixed, so the event carries it.
    SetParam(Stack, usize, Param),
    /// egui's combo boxes opened a popup; here the same choices unfold under their trigger, so the
    /// panel needs to know which one is open.
    ToggleCombo(Stack, usize, &'static str),
    /// A `DragValue` field grabbed the pointer and tracked it until release; the panel reports the
    /// grab and the viewer follows the pointer from there.
    NumDragStart(Stack, usize, NumGrab),
    /// egui's GradientMap editor: its bar and its per-stop buttons.
    Gradient(Stack, usize, GradientEdit),
    /// egui's Resize "Reset" button, which needed the geometry to restore.
    ResetResize(Stack, usize),
    /// egui's perspective-crop "✔ Apply": it set the `"crop"` temp, which hid the overlay and
    /// swapped this button for Reset, and marked its response changed, which is what re-ran the
    /// pipeline with the dragged points. The operator keeps its points either way.
    CropApply(Stack),
    /// egui's "⤴ Reset": cleared that temp and rebuilt this operator's corners from its own
    /// `original_size`.
    CropReset(Stack, usize),
    /// egui's LUT "Load from disk", which put a Hald CLUT of the operator's own choosing into the row.
    LoadLut(Stack, usize),
    /// egui's LUT link to the collection it suggested the builtin list came from.
    MoreLuts,
    SkipProcessing(bool),
    /// egui's "Paint mode" / "Stop painting" pair, which flipped `edit_state.painting`.
    PaintMode(bool),
    /// egui's "📜 Keep history" checkbox: `non_destructive_painting`, which decides whether a
    /// finished stroke stays a live overlay or gets baked into the pixel buffer.
    KeepHistory(bool),
    /// One of a stroke row's discrete widgets. Its colour is a `Params` picker, like an operator's.
    Stroke(usize, StrokeParam),
    /// The stroke row's `DragValue` grabbed the pointer.
    StrokeDrag(usize, NumGrab),
    /// egui's "↩", which popped the empty in-progress stroke *and* the finished one underneath it.
    StrokeUndo,
    StrokeClear,
    /// egui's per-row "⊗".
    StrokeRemove(usize),
    /// The brush combo of that row unfolded; one at a time, like the operator combos.
    StrokeCombo(usize),
    ApplyAll,
    RemoveAll,
    Reload,
    /// egui's save row: the native dialog, the encoder save of the current path, and the two
    /// ".oculante" metafiles.
    SaveAs,
    Save,
    SaveEdits,
    SaveDirEdits,
    /// egui's `not(file_open)` twin of "Save as...", for an image that arrived without a path.
    CreateOutputFile,
    /// egui's resize strip on the panel's canvas-facing edge had the pointer pressed on it.
    ResizeStart,
    /// egui's `jpg_lossless_ui` collapsing header.
    #[cfg(feature = "turbo")]
    ToggleLossless,
    #[cfg(feature = "turbo")]
    Lossless(Lossless),
}

/// The discrete values a stroke row's widgets own. `Hover` is egui's `stroke_ui` writing
/// `stroke.highlight = combined_response.hovered()`, which `PaintStroke::render` turns into a 2.5×
/// brighter brush on the canvas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StrokeParam {
    Fade(bool),
    Flip(bool),
    Brush(usize),
    Hover(bool),
}

/// libjpeg-turbo's lossless transforms, one button each.
#[cfg(feature = "turbo")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lossless {
    Rot90,
    Rot270,
    Rot180,
    Hflip,
    Vflip,
    /// egui built this transform from the operator stack at click time, so the event carries no
    /// geometry.
    Crop,
}

/// The crop amounts egui's lossless crop used: the first *active* Crop in the image stack.
#[cfg(feature = "turbo")]
pub fn first_active_crop(stack: &[ImgOpItem]) -> Option<[u32; 4]> {
    stack
        .iter()
        .filter(|item| item.active)
        .find_map(|item| match &item.operation {
            ImageOperation::Crop(amounts) => Some(*amounts),
            _ => None,
        })
}

/// egui's `jpg_lossless_ui` gate: the section only ever drew for a current path ending in jpg or
/// jpeg.
#[cfg(feature = "turbo")]
pub fn is_jpeg(path: Option<&std::path::Path>) -> bool {
    path.and_then(|path| path.extension())
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            let ext = ext.to_ascii_lowercase();
            ext == "jpg" || ext == "jpeg"
        })
}

/// One of the fixed values a discrete widget can write into an operator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Param {
    /// Slice's mode checkbox, Noise's "Grey", Flip's axis, Resize's aspect lock.
    Flag(&'static str, bool),
    /// Rotate's three angles.
    Angle(i16),
    /// ColorConverter's target format.
    ColorType(ColorTypeExt),
    /// One of ChannelSwap's two source channels.
    Channel(usize, Channel),
    /// Resize's resampling filter.
    ScaleFilter(ScaleFilter),
    /// Filter3x3's preset kernels.
    Kernel([i32; 9]),
    /// LUT's hald CLUT: one of the builtin names, or a path picked from disk.
    Lut(String),
}

/// Which value of an operator a stateful widget addresses: one of its named fields, or one of a
/// GradientMap's stops.
///
/// Stops are keyed by their random id rather than by index because egui sorted that list by
/// position at the end of every frame it drew it; an index would hand a widget's value to a
/// different stop under the user's cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    Field(&'static str),
    Stop(usize),
}

/// egui's GradientMap editor, as messages. The bar's carry the pointer's position across it in the
/// ramp's own 0..=255 units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GradientEdit {
    /// A press inside the bar: egui grabbed the nearest stop and moved it under the pointer in the
    /// same frame.
    Press(u8),
    /// The pointer moved inside the bar, holding a stop or merely hovering.
    Move(u8),
    /// The button came up, which is what cleared egui's `"gradient"` temp.
    Release,
    /// The pointer left the bar, which only ended the hover highlight.
    Leave,
    /// egui's "Add point", a black stop in the middle of the ramp.
    Add,
    /// egui's trash button, which its `len > 2` gate kept out of reach for the last two stops.
    Delete(usize),
}

/// What the bar decided about its pointer after one of those messages.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarState {
    /// The stop under the button, egui's temp data.
    pub grab: Option<usize>,
    /// The stop whose handle egui drew four pixels wide.
    pub hover: Option<usize>,
    pub changed: bool,
}

/// What a grabbed `DragValue` field needs the viewer to track until the pointer is released.
#[derive(Clone, Copy, Debug)]
pub struct NumGrab {
    pub slot: Slot,
    pub x: f32,
    pub y: f32,
    pub value: f64,
    pub speed: f64,
    pub lo: f64,
    pub hi: f64,
    pub integer: bool,
}

impl NumGrab {
    /// egui's release test: a press whose pointer stayed within `max_click_dist` of where it
    /// began was a click, which opened the text field instead of dragging.
    pub fn clicked(&self, x: f32, y: f32) -> bool {
        (x - self.x).hypot(y - self.y) < CLICK_DIST
    }

    /// The field's text as egui read it: parsed, then clamped into the row's range and rounded when
    /// the row holds an integer. Text that is not a number yet ("", "-") keeps the last value.
    pub fn parse(&self, text: &str) -> Option<f64> {
        let value = text.trim().parse::<f64>().ok()?;
        let value = value.clamp(self.lo, self.hi);
        Some(if self.integer { value.round() } else { value })
    }
}

/// The write half of the discrete widgets; only a field the operator actually has counts.
pub fn set_param(op: &mut ImageOperation, param: Param) -> bool {
    match (op, param) {
        (ImageOperation::Slice(_, _, hard), Param::Flag("hard", value)) => *hard = value,
        (ImageOperation::Noise { mono, .. }, Param::Flag("mono", value)) => *mono = value,
        (ImageOperation::Flip(horizontal), Param::Flag("horizontal", value)) => *horizontal = value,
        (ImageOperation::Resize { aspect, .. }, Param::Flag("aspect", value)) => *aspect = value,
        (ImageOperation::Rotate(angle), Param::Angle(value)) => *angle = value,
        (ImageOperation::ColorConverter(slot), Param::ColorType(value)) => *slot = value,
        (ImageOperation::ChannelSwap(slot), Param::Channel(0, value)) => slot.0 = value,
        (ImageOperation::ChannelSwap(slot), Param::Channel(_, value)) => slot.1 = value,
        (ImageOperation::Resize { filter, .. }, Param::ScaleFilter(value)) => *filter = value,
        (ImageOperation::Filter3x3(slot), Param::Kernel(value)) => *slot = value,
        (ImageOperation::LUT(slot), Param::Lut(value)) => *slot = value,
        _ => return false,
    }
    true
}

/// egui's `PANEL_WIDTH` right panel, resizable between 100 and 500 like it was, gated the same way
/// its left twin is.
pub fn render<F>(
    state: &OculanteState,
    palette: theme::Palette,
    top: f32,
    bottom: f32,
    width: f32,
    dragging: bool,
    filters_open: bool,
    lossless_open: bool,
    crop_applied: bool,
    query: String,
    search: Entity<InputState>,
    params: &mut Params,
    window: &mut Window,
    cx: &mut App,
    open_combo: Option<(u64, &'static str)>,
    stroke_combo: Option<usize>,
    on_panel: F,
) -> Option<Div>
where
    F: Fn(Event, &mut App) + Clone + 'static,
{
    let settings = &state.persistent_settings;
    if !settings.edit_enabled
        || state.settings_enabled
        || settings.zen_mode
        || state.current_image.is_none()
    {
        return None;
    }

    let on_resize = {
        let on_panel = on_panel.clone();
        move |app: &mut App| on_panel(Event::ResizeStart, app)
    };
    Some(panel::side_panel(
        "edit-scroll",
        panel::Side::Right,
        palette,
        top,
        bottom,
        width,
        dragging,
        on_resize,
        {
            let on_panel = on_panel.clone();
            move |column| {
                let stack = &state.edit_state;
                column
                    .child(filters(
                        state,
                        palette,
                        filters_open,
                        &query,
                        &search,
                        on_panel.clone(),
                    ))
                    .children(rows(
                        Stack::Image,
                        &stack.image_op_stack,
                        palette,
                        crop_applied,
                        params,
                        window,
                        cx,
                        open_combo,
                        on_panel.clone(),
                    ))
                    .when(
                        !stack.image_op_stack.is_empty() && !stack.pixel_op_stack.is_empty(),
                        |this| this.child(panel::separator()),
                    )
                    .children(rows(
                        Stack::Pixel,
                        &stack.pixel_op_stack,
                        palette,
                        crop_applied,
                        params,
                        window,
                        cx,
                        open_combo,
                        on_panel.clone(),
                    ))
                    .children(paint(
                        state,
                        palette,
                        stroke_combo,
                        params,
                        window,
                        cx,
                        on_panel.clone(),
                    ))
                    .child(skip_processing(stack.skip_processing, on_panel.clone()))
                    .children(egui_tail(state, palette, lossless_open, on_panel.clone()))
                    .children(notice(state))
                    .children(debug_info(state).into_iter().flatten())
            }
        },
    ))
}

/// egui's "Filters" header: a search field over the list of operations, each of which is a
/// template that gets cloned into a stack when it is clicked.
fn filters<F>(
    state: &OculanteState,
    palette: theme::Palette,
    open: bool,
    query: &str,
    search: &Entity<InputState>,
    on_event: F,
) -> Div
where
    F: Fn(Event, &mut App) + Clone + 'static,
{
    let query = query.to_lowercase();
    let rows = templates(state)
        .into_iter()
        .filter(|op| format!("{op}").to_lowercase().contains(&query))
        .map(|op| {
            let name = format!("{op}");
            panel::chrome(
                name.clone(),
                name,
                Event::Add(op.operation),
                palette,
                on_event.clone(),
            )
            .w_full()
            .justify_center()
            .into_any()
        })
        .collect::<Vec<AnyElement>>();

    panel::section(
        "Filters",
        open,
        palette,
        Event::ToggleFilters,
        on_event,
        vec![panel::dark_panel(
            palette,
            vec![
                div().child(Input::new(search)).into_any(),
                div()
                    .id("filter-list")
                    .max_h(px(LIST_HEIGHT))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(rows)
                    .into_any(),
            ],
        )],
    )
}

/// One row per operator: egui's borderless carets for reorder, bypass and remove, then the
/// operation's own widgets underneath. The widgets are egui's `ImageOperation::ui` match, ported
/// group by group; operators that are not covered yet render as their label alone.
fn rows<F>(
    stack: Stack,
    ops: &[ImgOpItem],
    palette: theme::Palette,
    crop_applied: bool,
    params: &mut Params,
    window: &mut Window,
    cx: &mut App,
    open_combo: Option<(u64, &'static str)>,
    on_event: F,
) -> Vec<AnyElement>
where
    F: Fn(Event, &mut App) + Clone + 'static,
{
    let frame = if palette.dark {
        rgb(0x21_21_21)
    } else {
        rgb(0xF2_F2_F2)
    };
    let len = ops.len();

    ops.iter()
        .enumerate()
        .map(|(i, op)| {
            let name = format!("{op}");
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .p(px(6.))
                .rounded(px(4.))
                .bg(frame)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(CARET / 2.))
                        .child(caret(
                            format!("{stack:?} {i} up"),
                            icons::CHEVRON_UP,
                            CARET,
                            i != 0,
                            "Move up",
                            // The caret is inert at the head of the stack, but the payload is still
                            // built there, so it must not underflow.
                            Event::Move(stack, i, i.saturating_sub(1)),
                            on_event.clone(),
                        ))
                        .child(caret(
                            format!("{stack:?} {i} down"),
                            icons::CHEVRON_DOWN,
                            CARET,
                            i + 1 < len,
                            "Move down",
                            Event::Move(stack, i, i + 1),
                            on_event.clone(),
                        ))
                        .child(caret(
                            format!("{stack:?} {i} bypass"),
                            if op.active { icons::EYE } else { icons::EYEOFF },
                            CARET,
                            true,
                            "Bypass",
                            Event::Bypass(stack, i),
                            on_event.clone(),
                        ))
                        .child(caret(
                            format!("{stack:?} {i} remove"),
                            icons::X,
                            CARET * 1.5,
                            true,
                            "Remove operator",
                            Event::Remove(stack, i),
                            on_event.clone(),
                        ))
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .when(!op.active, |this| this.opacity(0.5))
                                .child(name),
                        ),
                )
                .children(params.widgets(stack, i, op, palette, on_event.clone(), window, cx))
                .children(num_widgets(
                    params,
                    stack,
                    i,
                    &op.operation,
                    palette,
                    on_event.clone(),
                ))
                .children(discrete(
                    stack,
                    i,
                    op,
                    crop_applied,
                    open_combo,
                    palette,
                    on_event.clone(),
                ))
                .into_any()
        })
        .collect()
}

/// egui's `add_enabled_ui` carets: a frameless glyph that greys out instead of going missing.
fn caret(
    id: String,
    glyph: &'static str,
    size: f32,
    enabled: bool,
    tip: &'static str,
    event: Event,
    on_event: impl Fn(Event, &mut App) + 'static,
) -> AnyElement {
    let button = div()
        .id(id)
        .flex_shrink_0()
        .text_size(px(size))
        .font_family(fonts::ICONS)
        .child(glyph);
    if enabled {
        button
            .cursor_pointer()
            .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
            .on_click(move |_, _, app| on_event(event.clone(), app))
            .into_any()
    } else {
        button.opacity(0.3).into_any()
    }
}

/// One of egui's `styled_slider` rows inside an operator's widgets.
struct SliderRow {
    /// egui keyed its widget memory by the operator's id plus this, which is what keeps two
    /// sliders in the same operator apart.
    field: &'static str,
    /// egui only labelled a slider when the operator had more than one, e.g. Slice's "Position".
    label: Option<&'static str>,
    range: RangeInclusive<i32>,
    value: i32,
}

/// The slider rows of an operator: exactly the `styled_slider` sites of egui's
/// `ImageOperation::ui`, ranges included. Operators whose widgets are not ported yet have none,
/// so their row stays its label alone.
fn slider_rows(op: &ImageOperation) -> Vec<SliderRow> {
    let single = |range: RangeInclusive<i32>, value: i32| {
        vec![SliderRow {
            field: "value",
            label: None,
            range,
            value,
        }]
    };
    match op {
        ImageOperation::Brightness(value) => single(-255..=255, *value),
        ImageOperation::Contrast(value) => single(-128..=128, *value),
        ImageOperation::Exposure(value) => single(-100..=100, *value),
        ImageOperation::Desaturate(value) => single(0..=100, i32::from(*value)),
        ImageOperation::Blur(value) => single(0..=254, i32::from(*value)),
        ImageOperation::ChromaticAberration(value) => single(0..=255, i32::from(*value)),
        ImageOperation::Posterize(value) => single(1..=255, i32::from(*value)),
        ImageOperation::Noise { amt, .. } => single(0..=100, i32::from(*amt)),
        ImageOperation::Slice(position, range, _) => vec![
            SliderRow {
                field: "position",
                label: Some("Position"),
                range: 0..=255,
                value: i32::from(*position),
            },
            SliderRow {
                field: "range",
                label: Some("Range"),
                range: 0..=255,
                value: i32::from(*range),
            },
        ],
        _ => Vec::new(),
    }
}

/// egui's `ImageOperation::ui` ended with `_ => ui.label("Filter has no options.")` (HEAD
/// src/image_editing.rs:1233), which is what the operators with no widget arm at all showed. The four
/// unit variants are exactly that set; every other variant has at least one arm.
fn no_options(op: &ImageOperation) -> bool {
    matches!(
        op,
        ImageOperation::Invert
            | ImageOperation::MMult
            | ImageOperation::MDiv
            | ImageOperation::ScaleImageMinMax
    )
}

/// The write half of [`slider_rows`]: a drag lands here, and only a field the operator actually
/// has counts as a change.
fn set_slider(op: &mut ImageOperation, field: &'static str, value: i32) -> bool {
    match (op, field) {
        (ImageOperation::Brightness(slot), "value") => *slot = value,
        (ImageOperation::Contrast(slot), "value") => *slot = value,
        (ImageOperation::Exposure(slot), "value") => *slot = value,
        (ImageOperation::Desaturate(slot), "value") => *slot = value as u8,
        (ImageOperation::Blur(slot), "value") => *slot = value as u8,
        (ImageOperation::ChromaticAberration(slot), "value") => *slot = value as u8,
        (ImageOperation::Posterize(slot), "value") => *slot = value as u8,
        (ImageOperation::Noise { amt, .. }, "value") => *amt = value as u8,
        (ImageOperation::Slice(position, _, _), "position") => *position = value as u8,
        (ImageOperation::Slice(_, range, _), "range") => *range = value as u8,
        _ => return false,
    }
    true
}

/// egui's `text_edit_singleline` sites: for now the Expression operator's formula alone.
fn text_rows(op: &ImageOperation) -> Vec<(&'static str, String)> {
    match op {
        ImageOperation::Expression(expr) => vec![("expr", expr.clone())],
        _ => Vec::new(),
    }
}

/// The write half of [`text_rows`].
fn set_text(op: &mut ImageOperation, field: &'static str, value: &str) -> bool {
    match (op, field) {
        (ImageOperation::Expression(slot), "expr") => {
            *slot = value.to_string();
            true
        }
        _ => false,
    }
}

/// egui's `color_edit_button_*` sites: Add and Mult got an RGB button and Fill an RGBA one, each
/// bound straight to the operator's array, and every GradientMap stop got an RGB one in its row.
fn color_rows(op: &ImageOperation) -> Vec<(Slot, [u8; 4])> {
    let field = |value: [u8; 4]| (Slot::Field("color"), value);
    match op {
        ImageOperation::Add(value) => vec![field([value[0], value[1], value[2], 255])],
        ImageOperation::Mult(value) => vec![field([value[0], value[1], value[2], 255])],
        ImageOperation::Fill(value) => vec![field(*value)],
        ImageOperation::GradientMap(stops) => stops
            .iter()
            .map(|stop| {
                (
                    Slot::Stop(stop.id),
                    [stop.col[0], stop.col[1], stop.col[2], 255],
                )
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The write half of [`color_rows`]: Add, Mult and the gradient stops keep their three bytes, as
/// in egui, where only Fill's button was the RGBA one.
fn set_color(op: &mut ImageOperation, slot: Slot, value: [u8; 4]) -> bool {
    if let Slot::Stop(id) = slot {
        let ImageOperation::GradientMap(stops) = op else {
            return false;
        };
        let Some(stop) = stops.iter_mut().find(|stop| stop.id == id) else {
            return false;
        };
        stop.col.copy_from_slice(&value[..3]);
        return true;
    }
    match (op, slot) {
        (ImageOperation::Add(slot), Slot::Field("color")) => slot.copy_from_slice(&value[..3]),
        (ImageOperation::Mult(slot), Slot::Field("color")) => slot.copy_from_slice(&value[..3]),
        (ImageOperation::Fill(slot), Slot::Field("color")) => *slot = value,
        _ => return false,
    }
    true
}

/// The picker's own space is HSLA, so an operator's bytes enter through `Rgba`.
pub(crate) fn hsla_of(value: [u8; 4]) -> Hsla {
    Rgba {
        r: value[0] as f32 / 255.,
        g: value[1] as f32 / 255.,
        b: value[2] as f32 / 255.,
        a: value[3] as f32 / 255.,
    }
    .into()
}

/// The reverse, rounded rather than truncated so a colour the user did not touch survives the
/// byte -> HSLA -> byte round trip.
pub(crate) fn bytes_of(color: Hsla) -> [u8; 4] {
    let rgba = Rgba::from(color);
    let byte = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
    [byte(rgba.r), byte(rgba.g), byte(rgba.b), byte(rgba.a)]
}

/// One of egui's `DragValue` fields: dragged sideways, with the range and speed egui gave it.
pub struct NumRow {
    pub slot: Slot,
    /// egui's prefix, e.g. Equalize's "dark "; empty when it had none.
    pub label: &'static str,
    pub lo: f64,
    pub hi: f64,
    pub speed: f64,
    pub value: f64,
    pub integer: bool,
    /// egui's `custom_formatter` plus its `suffix`, which the GradientMap stops use to show their
    /// position as a fraction of 256. `None` formats the number itself.
    pub shown: Option<String>,
}

impl NumRow {
    /// What the row shows: egui's formatter output, which is also the text its `DragValue` put in
    /// the field when a click turned the readout into an editor.
    pub fn shown(&self) -> String {
        self.shown.clone().unwrap_or_else(|| {
            if self.integer {
                format!("{}", self.value.round())
            } else {
                format!("{:.3}", self.value)
            }
        })
    }
}

/// egui's `DragValue` sites, ranges and speeds included. Crop stores 0..=1 as u32/10000, so its
/// rows carry the fraction and the write half scales it back.
pub fn num_rows(op: &ImageOperation) -> Vec<NumRow> {
    let int =
        |field: &'static str, label: &'static str, lo: f64, hi: f64, speed: f64, value: f64| {
            NumRow {
                slot: Slot::Field(field),
                label,
                lo,
                hi,
                speed,
                value,
                integer: true,
                shown: None,
            }
        };
    match op {
        ImageOperation::HSV((h, s, v)) => vec![
            int("h", "", 0., 360., 1., f64::from(*h)),
            int("s", "", 0., 200., 1., f64::from(*s)),
            int("v", "", 0., 200., 1., f64::from(*v)),
        ],
        ImageOperation::Equalize((dark, bright)) => vec![
            int("dark", "dark ", -128., 128., 1., f64::from(*dark)),
            int("bright", "bright ", 64., 2000., 2., f64::from(*bright)),
        ],
        // egui laid these out as X, W, Y, H with `⏵ ⏴ ⏷ ⏶` prefixes; the body font has no glyphs for
        // those, so the axis names are written out in their place.
        ImageOperation::Crop(bounds) => [
            (0usize, "x", "x "),
            (2, "w", "w "),
            (1, "y", "y "),
            (3, "h", "h "),
        ]
        .into_iter()
        .map(|(i, field, label)| NumRow {
            slot: Slot::Field(field),
            label,
            lo: 0.,
            hi: 1.,
            speed: 0.004,
            value: f64::from(bounds[i]) / 10000.,
            integer: false,
            shown: None,
        })
        .collect(),
        ImageOperation::Filter3x3(kernel) => kernel
            .iter()
            .enumerate()
            .map(|(i, k)| int(BOXES[i], "", -255., 255., 1., f64::from(*k)))
            .collect(),
        ImageOperation::Resize { dimensions, .. } => vec![
            int("w", "X ", 10., 10000., 0.5, f64::from(dimensions.0)),
            int("h", "Y ", 10., 10000., 0.5, f64::from(dimensions.1)),
        ],
        _ => Vec::new(),
    }
}

/// Field keys for the nine Filter3x3 kernel boxes, in row-major order.
const BOXES: [&str; 9] = ["k0", "k1", "k2", "k3", "k4", "k5", "k6", "k7", "k8"];

/// The write half of [`num_rows`]. Resize keeps its aspect lock: writing one side derives the
/// other from the ratio the operator had before the write, which is what egui stored in temp.
pub fn set_num(op: &mut ImageOperation, slot: Slot, value: f64) -> bool {
    if let Slot::Stop(id) = slot {
        let ImageOperation::GradientMap(stops) = op else {
            return false;
        };
        let Some(stop) = stops.iter_mut().find(|stop| stop.id == id) else {
            return false;
        };
        stop.pos = value.round().clamp(0., f64::from(u8::MAX)) as u8;
        sort_stops(stops);
        return true;
    }
    match (op, slot) {
        (ImageOperation::HSV(slot), Slot::Field("h")) => slot.0 = value as u16,
        (ImageOperation::HSV(slot), Slot::Field("s")) => slot.1 = value as i32,
        (ImageOperation::HSV(slot), Slot::Field("v")) => slot.2 = value as i32,
        (ImageOperation::Equalize(slot), Slot::Field("dark")) => slot.0 = value as i32,
        (ImageOperation::Equalize(slot), Slot::Field("bright")) => slot.1 = value as i32,
        (ImageOperation::Crop(bounds), Slot::Field("x")) => bounds[0] = (value * 10000.) as u32,
        (ImageOperation::Crop(bounds), Slot::Field("y")) => bounds[1] = (value * 10000.) as u32,
        (ImageOperation::Crop(bounds), Slot::Field("w")) => bounds[2] = (value * 10000.) as u32,
        (ImageOperation::Crop(bounds), Slot::Field("h")) => bounds[3] = (value * 10000.) as u32,
        (ImageOperation::Filter3x3(kernel), Slot::Field(key)) => {
            let Some(i) = BOXES.iter().position(|b| *b == key) else {
                return false;
            };
            kernel[i] = value as i32;
        }
        (
            ImageOperation::Resize {
                dimensions, aspect, ..
            },
            Slot::Field("w"),
        ) => {
            let ratio = f64::from(dimensions.0) / f64::from(dimensions.1).max(1.);
            dimensions.0 = value as u32;
            if *aspect {
                dimensions.1 = (value / ratio).round().clamp(10., 10000.) as u32;
            }
        }
        (
            ImageOperation::Resize {
                dimensions, aspect, ..
            },
            Slot::Field("h"),
        ) => {
            let ratio = f64::from(dimensions.0) / f64::from(dimensions.1).max(1.);
            dimensions.1 = value as u32;
            if *aspect {
                dimensions.0 = (value * ratio).round().clamp(10., 10000.) as u32;
            }
        }
        _ => return false,
    }
    true
}

/// The ramp below the bar is interpolated in list order, so every write to a GradientMap leaves its
/// stops ascending by position — egui's `pts.sort_by_key(|a| a.pos)` ran at the end of every frame
/// its editor drew.
fn sort_stops(stops: &mut Vec<GradientStop>) {
    stops.sort_by_key(|stop| stop.pos);
}

/// egui's `closest_pt`: which stop's handle the pointer is over, in the ramp's 0..=255 units.
pub fn nearest_stop(op: &ImageOperation, position: u8) -> Option<usize> {
    let ImageOperation::GradientMap(stops) = op else {
        return None;
    };
    stops.get(closest_pt(stops, position)).map(|stop| stop.id)
}

/// What the bar made of one of its pointer messages. `grab` is the stop it was holding, which egui
/// kept in a single ctx temp shared by every gradient in the stack.
pub fn bar_drag(op: &mut ImageOperation, grab: Option<usize>, edit: GradientEdit) -> BarState {
    match edit {
        GradientEdit::Release => BarState {
            grab: None,
            hover: None,
            changed: false,
        },
        GradientEdit::Leave => BarState {
            grab,
            hover: None,
            changed: false,
        },
        GradientEdit::Press(position) => {
            let hover = nearest_stop(op, position);
            let changed = hover.is_some_and(|id| set_num(op, Slot::Stop(id), f64::from(position)));
            BarState {
                grab: hover,
                hover,
                changed,
            }
        }
        GradientEdit::Move(position) => {
            let changed = grab.is_some_and(|id| set_num(op, Slot::Stop(id), f64::from(position)));
            BarState {
                grab,
                hover: nearest_stop(op, position),
                changed,
            }
        }
        GradientEdit::Add | GradientEdit::Delete(_) => BarState {
            grab,
            hover: None,
            changed: false,
        },
    }
}

/// egui's two buttons under the bar.
pub fn gradient_edit(op: &mut ImageOperation, edit: GradientEdit) -> bool {
    let ImageOperation::GradientMap(stops) = op else {
        return false;
    };
    match edit {
        GradientEdit::Add => stops.push(GradientStop::new(128, [0, 0, 0])),
        GradientEdit::Delete(id) => {
            let Some(at) = stops.iter().position(|stop| stop.id == id) else {
                return false;
            };
            stops.remove(at);
        }
        _ => return false,
    }
    sort_stops(stops);
    true
}

/// egui's `DragValue` fields, drawn as framed readouts that drag sideways. Filter3x3 keeps egui's
/// three-per-row grid and its preset combo above it.
fn num_widgets<F>(
    params: &Params,
    stack: Stack,
    index: usize,
    op: &ImageOperation,
    palette: theme::Palette,
    on_event: F,
) -> Vec<AnyElement>
where
    F: Fn(Event, &mut App) + Clone + 'static,
{
    let mut widgets: Vec<AnyElement> = Vec::new();
    if let ImageOperation::Filter3x3(kernel) = op {
        let presets: [(&str, [i32; 9]); 3] = [
            ("Sharpen", [0, -100, 0, -100, 500, -100, 0, -100, 0]),
            ("Blur", [6, 12, 6, 12, 25, 12, 6, 12, 6]),
            ("Emboss", [-200, -100, 0, -100, 100, 100, 0, 100, 200]),
        ];
        widgets.push(
            dropdown(
                format!("{stack:?} {index} preset"),
                presets
                    .iter()
                    .find(|(_, k)| k == kernel)
                    .map(|(name, _)| (*name).to_string())
                    .unwrap_or_else(|| "Select".into()),
                false,
                presets
                    .iter()
                    .map(|(name, k)| ((*name).to_string(), Param::Kernel(*k), k == kernel))
                    .collect(),
                stack,
                index,
                "preset",
                palette,
                on_event.clone(),
            )
            .into_any(),
        );
    }
    let rows = num_rows(op);
    if rows.is_empty() {
        return widgets;
    }
    let frame = if palette.dark {
        rgb(0x2a_2a_2a)
    } else {
        rgb(0xE6_E6_E6)
    };
    let fields: Vec<AnyElement> = rows
        .into_iter()
        .map(|row| num_field(params, row, stack, index, frame, on_event.clone()))
        .collect();
    if matches!(op, ImageOperation::Filter3x3(_)) {
        // egui laid the kernel out as three rows of three.
        let mut row: Vec<AnyElement> = Vec::new();
        for (i, box_) in fields.into_iter().enumerate() {
            row.push(box_);
            if row.len() == 3 || i == 8 {
                widgets.push(
                    div()
                        .flex()
                        .gap(px(4.))
                        .children(std::mem::take(&mut row))
                        .into_any(),
                );
            }
        }
    } else {
        widgets.push(div().flex().gap(px(4.)).children(fields).into_any());
    }
    widgets
}

/// egui's gradient bar: `allocate_at_least(vec2(255., 50.))`, its stop handles drawn across the
/// lower half of that.
const BAR_HEIGHT: f32 = 50.;
const BAR_HANDLE: f32 = 25.;
/// The ramp's resolution: one stripe per position unit, so 0..=255 like a stop's `pos`.
const BAR_UNITS: f32 = 255.;

/// egui's gradient bar as a gpui element: the canvas paints the ramp and the handles, the pointer
/// handlers beside it report positions inside the bar.
///
/// egui mapped those positions by subtracting the bar's own left edge from the pointer, which a
/// handler that only ever sees window coordinates cannot do, so the canvas publishes its bounds for
/// them every frame it is laid out.
fn gradient_bar<F>(
    stack: Stack,
    index: usize,
    stops: &[GradientStop],
    hover: Option<usize>,
    grab: Option<usize>,
    on_event: F,
) -> AnyElement
where
    F: Fn(Event, &mut App) + Clone + 'static,
{
    let bounds = panel::ElementBounds::default();
    let painted = stops.to_vec();
    let unit = {
        let bounds = bounds.clone();
        move |x: Pixels| -> Option<u8> {
            bounds
                .fraction(x)
                .map(|fraction| (fraction * BAR_UNITS).round().clamp(0., BAR_UNITS) as u8)
        }
    };

    div()
        .id(format!("{stack:?} {index} gradient"))
        .child(
            canvas(
                {
                    let bounds = bounds.clone();
                    move |rect, _, _| bounds.capture(rect)
                },
                move |rect, (), window, _| {
                    let width = f32::from(rect.size.width);
                    let height = f32::from(rect.size.height);
                    let shade = |color: [u8; 3]| {
                        rgb((u32::from(color[0]) << 16)
                            | (u32::from(color[1]) << 8)
                            | u32::from(color[2]))
                    };
                    let mut fill = |bounds: Bounds<Pixels>, color: Rgba| {
                        window.paint_quad(quad(
                            bounds,
                            Corners::default(),
                            Hsla::from(color),
                            Edges::default(),
                            rgb(0),
                            BorderStyle::default(),
                        ))
                    };
                    // egui painted this ramp as a mesh between the stop vertices, clamping the first
                    // and last colour out to the ends of the bar. `interpolate_u8` is the same
                    // arithmetic and the very function a pixel of the image goes through, so the
                    // preview cannot disagree with the result.
                    for stripe in 0..BAR_UNITS as u32 {
                        let x = rect.origin.x + px(width * stripe as f32 / BAR_UNITS);
                        fill(
                            Bounds {
                                origin: point(x, rect.origin.y),
                                size: size(px(width / BAR_UNITS), px(height)),
                            },
                            shade(interpolate_u8(&painted, stripe as u8)),
                        );
                    }
                    for stop in &painted {
                        // egui's inverted grey, four pixels wide under the pointer and one otherwise.
                        let thick = Some(stop.id) == hover || Some(stop.id) == grab;
                        let line = if thick { 4. } else { 1. };
                        let centre = width * f32::from(stop.pos) / BAR_UNITS;
                        let grey = 255 - stop.col[0];
                        fill(
                            Bounds {
                                origin: point(
                                    rect.origin.x + px(centre - line / 2.),
                                    rect.origin.y + px(BAR_HANDLE),
                                ),
                                size: size(px(line), px(height - BAR_HANDLE)),
                            },
                            rgb(u32::from(grey) * 0x010101),
                        );
                    }
                },
            )
            .w_full()
            .h(px(BAR_HEIGHT)),
        )
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, {
            let on_event = on_event.clone();
            let unit = unit.clone();
            move |event: &MouseDownEvent, _, app| {
                if let Some(position) = unit(event.position.x) {
                    on_event(
                        Event::Gradient(stack, index, GradientEdit::Press(position)),
                        app,
                    );
                }
            }
        })
        .on_mouse_move({
            let on_event = on_event.clone();
            let unit = unit.clone();
            move |event: &MouseMoveEvent, _, app| {
                if let Some(position) = unit(event.position.x) {
                    on_event(
                        Event::Gradient(stack, index, GradientEdit::Move(position)),
                        app,
                    );
                }
            }
        })
        // egui cleared its grabbed stop on any release, wherever the pointer had gone to by then.
        .on_mouse_up(MouseButton::Left, {
            let on_event = on_event.clone();
            move |_: &MouseUpEvent, _, app| {
                on_event(Event::Gradient(stack, index, GradientEdit::Release), app);
            }
        })
        .on_mouse_up_out(MouseButton::Left, {
            let on_event = on_event.clone();
            move |_: &MouseUpEvent, _, app| {
                on_event(Event::Gradient(stack, index, GradientEdit::Release), app);
            }
        })
        .on_mouse_exit({
            let on_event = on_event.clone();
            move |_: &MouseExitEvent, _, app| {
                on_event(Event::Gradient(stack, index, GradientEdit::Leave), app);
            }
        })
        .into_any()
}

/// egui's `DragValue`: a framed readout that the pointer drags sideways. The GradientMap's stop
/// rows build their own [`NumRow`] and share this, which is why it takes the row rather than the
/// operator.
fn drag_value<F>(row: NumRow, stack: Stack, index: usize, frame: Rgba, on_event: F) -> Stateful<Div>
where
    F: Fn(Event, &mut App) + Clone + 'static,
{
    let shown = row.shown();
    let label = row.label;
    let grab = NumGrab {
        slot: row.slot,
        x: 0.,
        y: 0.,
        value: row.value,
        speed: row.speed,
        lo: row.lo,
        hi: row.hi,
        integer: row.integer,
    };
    div()
        .id(SharedString::from(format!(
            "num {stack:?} {index} {:?}",
            grab.slot
        )))
        .flex()
        .items_center()
        .gap(px(2.))
        .px(px(6.))
        .py(px(1.))
        .rounded(px(4.))
        .bg(frame)
        .cursor_ew_resize()
        .when(!label.is_empty(), |this| this.child(label))
        .child(shown)
        .on_mouse_down(MouseButton::Left, {
            let on_event = on_event.clone();
            move |event: &MouseDownEvent, _, app| {
                on_event(
                    Event::NumDragStart(
                        stack,
                        index,
                        NumGrab {
                            x: event.position.x.into(),
                            y: event.position.y.into(),
                            ..grab
                        },
                    ),
                    app,
                )
            }
        })
}

/// The row as egui drew it: the drag readout, or while it holds focus the text field a click
/// opened in its place.
fn num_field<F>(
    params: &Params,
    row: NumRow,
    stack: Stack,
    index: usize,
    frame: Rgba,
    on_event: F,
) -> AnyElement
where
    F: Fn(Event, &mut App) + Clone + 'static,
{
    let slot = row.slot;
    let Some(edit) = params
        .num_edit
        .as_ref()
        .filter(|edit| edit.stack == stack && edit.index == index)
    else {
        return drag_value(row, stack, index, frame, on_event).into_any();
    };
    if edit.grab.slot != slot {
        return drag_value(row, stack, index, frame, on_event).into_any();
    }
    div()
        .id(SharedString::from(format!(
            "num edit {stack:?} {index} {slot:?}"
        )))
        .w(px(NUM_EDIT_WIDTH))
        .child(Input::new(&edit.input).w_full().text_size(px(12.)))
        // egui's Escape stopped the field's focus without re-applying its text, which leaves the
        // value the live writes already made — so here it only has to end the edit, and blurring
        // the window is what ends it.
        .on_key_down(move |event: &KeyDownEvent, window, cx| {
            if event.keystroke.key == "escape" {
                window.blur(cx);
            }
        })
        .into_any()
}

/// egui's `DragValue` in its text-edit state: the row had focus, so it drew a field instead of its
/// readout and wrote the operator through on every change. The viewer opens it and owns its events.
pub struct NumEdit {
    pub stack: Stack,
    pub index: usize,
    /// The operator the row held when the field opened, which is what retires the field along with
    /// a row that moved, changed or went away.
    pub id: u64,
    pub grab: NumGrab,
    pub input: Entity<InputState>,
    /// The field's own change, focus and blur handlers, which live and die with the edit.
    pub subscription: Subscription,
}

/// egui's widgets wrote straight through the `&mut` they were handed, so a slider's value *was*
/// the operator's. gpui-component keeps drag state in an entity instead, so the panel holds one
/// per operator field and the viewer copies the values back before it runs the pipeline.
pub struct Params {
    sliders: HashMap<(u64, &'static str), Entity<SliderState>>,
    texts: HashMap<(u64, &'static str), Entity<InputState>>,
    colors: HashMap<(u64, Slot), Entity<ColorPickerState>>,
    /// egui bound each stroke row's colour button straight to `stroke.color`; these are those
    /// buttons, keyed by the stroke's place in the list.
    stroke_colors: HashMap<usize, Entity<ColorPickerState>>,
    /// The row whose colour the viewer wrote itself (egui's right-drag sample), so its picker gets
    /// re-seeded from the stroke rather than overwriting the sample.
    sampled_stroke: Option<usize>,
    /// egui uploaded each brush once and cached the texture by URI (HEAD `src/ui/edit_ui.rs:11-25`),
    /// so the combo could draw the dab beside every option. These are those uploads, seated the first
    /// time the paint rows render and reused as handles after that.
    brushes: Vec<Arc<RenderImage>>,
    /// The one `DragValue` field egui has open as text, its `mem.has_focus(id)` of this frame.
    pub(crate) num_edit: Option<NumEdit>,
    /// egui's GradientMap editor: its stop the pointer is holding, its single `"gradient"` ctx
    /// temp, and the one it is over. Both are one slot for the whole panel, exactly as egui's was.
    pub(crate) grab: Option<usize>,
    pub(crate) hover: Option<usize>,
}

impl Params {
    pub fn new() -> Self {
        Self {
            sliders: HashMap::new(),
            texts: HashMap::new(),
            colors: HashMap::new(),
            stroke_colors: HashMap::new(),
            sampled_stroke: None,
            brushes: Vec::new(),
            num_edit: None,
            grab: None,
            hover: None,
        }
    }

    /// An operator's widgets, in egui's order: the slider, then its value in a fixed-width
    /// readout, which is the layout `styled_slider` had.
    fn widgets(
        &mut self,
        stack: Stack,
        index: usize,
        op: &ImgOpItem,
        palette: theme::Palette,
        on_event: impl Fn(Event, &mut App) + Clone + 'static,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        let mut widgets: Vec<AnyElement> = slider_rows(&op.operation)
            .iter()
            .map(|row| {
                let slider = self.slider(op.id, row, cx);
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .children(row.label.map(|label| div().child(label)))
                    .child(
                        // egui's `styled_slider` sets `trailing_fill(true)`, so the solid fill sits
                        // to the right of the handle; `.reverse()` is gpui-component's same
                        // convention.
                        Slider::new(&slider)
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
                            .child(format!("{}", row.value)),
                    )
                    .into_any()
            })
            .collect();
        for (field, value) in text_rows(&op.operation) {
            let input = self.text(op.id, field, &value, window, cx);
            widgets.push(
                div()
                    .child(Input::new(&input).w_full().text_size(px(12.)))
                    .into_any(),
            );
        }
        if !matches!(op.operation, ImageOperation::GradientMap(_)) {
            for (slot, value) in color_rows(&op.operation) {
                let picker = self.color(op.id, slot, value, window, cx);
                widgets.push(
                    // egui's colour button opened its picker to the right of the panel; the popover
                    // here anchors to its own right edge so the widened card keeps it on screen.
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            ColorPicker::new(&picker)
                                .anchor(Anchor::TopRight)
                                .accessibility_label(format!("{} color", op.operation)),
                        )
                        .into_any(),
                );
            }
        }
        if let ImageOperation::GradientMap(stops) = &op.operation {
            widgets.extend(self.gradient(
                stack,
                index,
                op.id,
                stops.clone(),
                palette,
                on_event,
                window,
                cx,
            ));
        }
        if no_options(&op.operation) {
            widgets.push(div().child("Filter has no options.").into_any());
        }
        widgets
    }

    /// egui keyed its widget temp memory with `Id::new(...).with(operation.id)`, and a row is
    /// seeded from its own operator, so an operator restored from a project file starts where the
    /// file says rather than at the template's default.
    fn slider(&mut self, id: u64, row: &SliderRow, cx: &mut App) -> Entity<SliderState> {
        if let Some(existing) = self.sliders.get(&(id, row.field)) {
            return existing.clone();
        }
        let slider = cx.new(|_| {
            SliderState::new()
                .min(*row.range.start() as f32)
                .max(*row.range.end() as f32)
                .step(1.)
                .default_value(row.value as f32)
        });
        self.sliders.insert((id, row.field), slider.clone());
        slider
    }

    /// egui's `text_edit_singleline` bound the field straight to the operator's string; here the
    /// input owns its text and `apply` copies it across.
    fn text(
        &mut self,
        id: u64,
        field: &'static str,
        value: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<InputState> {
        if let Some(existing) = self.texts.get(&(id, field)) {
            return existing.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).default_value(value));
        self.texts.insert((id, field), input.clone());
        input
    }

    /// egui's colour button read the operator's array every frame; here the picker owns the colour
    /// from its first render on, and `apply` copies it back.
    fn color(
        &mut self,
        id: u64,
        slot: Slot,
        value: [u8; 4],
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<ColorPickerState> {
        if let Some(existing) = self.colors.get(&(id, slot)) {
            return existing.clone();
        }
        let picker = cx.new(|cx| ColorPickerState::new(window, cx).default_value(hsla_of(value)));
        self.colors.insert((id, slot), picker.clone());
        picker
    }

    /// The stroke rows' colour buttons, which egui bound to `stroke.color` in place.
    fn stroke_color(
        &mut self,
        index: usize,
        value: [u8; 4],
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<ColorPickerState> {
        if let Some(existing) = self.stroke_colors.get(&index) {
            return existing.clone();
        }
        let picker = cx.new(|cx| ColorPickerState::new(window, cx).default_value(hsla_of(value)));
        self.stroke_colors.insert(index, picker.clone());
        picker
    }

    /// egui's GradientMap editor: the bar, then a row per stop with its colour button, its position
    /// `DragValue` and its trash button, then "Add point".
    fn gradient<F>(
        &mut self,
        stack: Stack,
        index: usize,
        id: u64,
        stops: Vec<GradientStop>,
        palette: theme::Palette,
        on_event: F,
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<AnyElement>
    where
        F: Fn(Event, &mut App) + Clone + 'static,
    {
        let frame = if palette.dark {
            rgb(0x2a_2a_2a)
        } else {
            rgb(0xE6_E6_E6)
        };
        let mut widgets = vec![gradient_bar(
            stack,
            index,
            &stops,
            self.hover,
            self.grab,
            on_event.clone(),
        )];
        let len = stops.len();
        for stop in &stops {
            let picker = self.color(
                id,
                Slot::Stop(stop.id),
                [stop.col[0], stop.col[1], stop.col[2], 255],
                window,
                cx,
            );
            // egui bound the row's DragValue to `p.pos` with a formatter that showed it back as a
            // fraction of 256, suffix included.
            let position = NumRow {
                slot: Slot::Stop(stop.id),
                label: "",
                lo: 0.,
                hi: 255.,
                speed: 0.1,
                value: f64::from(stop.pos),
                integer: true,
                shown: Some(format!("{:.2} pos", f64::from(stop.pos) / 256.)),
            };
            widgets.push(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        ColorPicker::new(&picker)
                            .anchor(Anchor::TopRight)
                            .accessibility_label("Gradient stop color"),
                    )
                    .child(num_field(
                        self,
                        position,
                        stack,
                        index,
                        frame,
                        on_event.clone(),
                    ))
                    // egui's 🗑, which the same icon font stands in for as everywhere else here, and
                    // which it only drew while the ramp still had more than two stops.
                    .when(len > 2, |this| {
                        this.child(panel::chrome(
                            format!("{stack:?} {index} stop {}", stop.id),
                            panel::icon(icons::TRASH),
                            Event::Gradient(stack, index, GradientEdit::Delete(stop.id)),
                            palette,
                            on_event.clone(),
                        ))
                    })
                    .into_any(),
            );
        }
        widgets.push(
            panel::panel_button(
                "Add point",
                Event::Gradient(stack, index, GradientEdit::Add),
                palette,
                on_event,
            )
            .into_any(),
        );
        widgets
    }

    /// Copy every slider back into its operator, and drop the entities of operators that are gone.
    /// Returns whether the image stack and then the pixel stack changed, which is what decides
    /// which pipeline stage has to run again.
    pub fn apply(&mut self, edit: &mut EditState, cx: &App) -> (bool, bool) {
        let mut changed = (false, false);
        let mut live = HashSet::new();
        let mut live_colors = HashSet::new();
        for (stack, dirty) in [
            (&mut edit.image_op_stack, &mut changed.0),
            (&mut edit.pixel_op_stack, &mut changed.1),
        ] {
            for op in stack {
                for row in slider_rows(&op.operation) {
                    let key = (op.id, row.field);
                    live.insert(key);
                    let Some(slider) = self.sliders.get(&key) else {
                        continue;
                    };
                    let next = slider
                        .read(cx)
                        .value()
                        .end()
                        .round()
                        .clamp(*row.range.start() as f32, *row.range.end() as f32)
                        as i32;
                    if next != row.value && set_slider(&mut op.operation, row.field, next) {
                        *dirty = true;
                    }
                }
                for (field, current) in text_rows(&op.operation) {
                    let key = (op.id, field);
                    live.insert(key);
                    let Some(input) = self.texts.get(&key) else {
                        continue;
                    };
                    let next = input.read(cx).value();
                    if next.as_str() != current && set_text(&mut op.operation, field, next.as_str())
                    {
                        *dirty = true;
                    }
                }
                for (slot, current) in color_rows(&op.operation) {
                    let key = (op.id, slot);
                    live_colors.insert(key);
                    let Some(picker) = self.colors.get(&key) else {
                        continue;
                    };
                    let Some(color) = picker.read(cx).value() else {
                        continue;
                    };
                    let next = bytes_of(color);
                    if next != current && set_color(&mut op.operation, slot, next) {
                        *dirty = true;
                    }
                }
            }
        }
        // egui's right-drag colour sample wrote `stroke.color` itself, so that row's picker is
        // dropped *before* the read-back below — otherwise the picker's stale value would undo the
        // sample, and the next frame seeds a fresh picker from the sampled colour.
        if let Some(sampled) = self.sampled_stroke.take() {
            self.stroke_colors.remove(&sampled);
            changed.1 = true;
        }
        // egui bound a stroke row's colour button to `stroke.color` the same way it bound an
        // operator's, and every stroke edit marked the *pixel* stack changed.
        let mut live_strokes = HashSet::new();
        for (index, stroke) in edit.paint_strokes.iter_mut().enumerate() {
            live_strokes.insert(index);
            let Some(picker) = self.stroke_colors.get(&index) else {
                continue;
            };
            let Some(color) = picker.read(cx).value() else {
                continue;
            };
            let bytes = bytes_of(color);
            let next = [
                f32::from(bytes[0]) / 255.,
                f32::from(bytes[1]) / 255.,
                f32::from(bytes[2]) / 255.,
                f32::from(bytes[3]) / 255.,
            ];
            if next != stroke.color {
                stroke.color = next;
                changed.1 = true;
            }
        }
        // egui keyed its edit state by the widget id, so a readout that stopped being drawn stopped
        // being an editor; the field here is bound to a row, and the same row holding another
        // operator is another widget.
        let stale = self
            .num_edit
            .as_ref()
            .is_some_and(|field| match field.stack {
                Stack::Image => retire(field, &edit.image_op_stack),
                Stack::Pixel => retire(field, &edit.pixel_op_stack),
            });
        if stale {
            self.num_edit = None;
        }
        self.sliders.retain(|key, _| live.contains(key));
        self.texts.retain(|key, _| live.contains(key));
        self.colors.retain(|key, _| live_colors.contains(key));
        self.stroke_colors
            .retain(|index, _| live_strokes.contains(index));
        changed
    }

    /// The viewer sampled row `index`'s colour off the canvas, as egui's right-drag branch did.
    pub fn stroke_sampled(&mut self, index: usize) {
        self.sampled_stroke = Some(index);
    }
}

/// Whether the row a `DragValue` field was opened on no longer holds the operator it opened for.
fn retire(field: &NumEdit, stack: &[ImgOpItem]) -> bool {
    !stack.get(field.index).is_some_and(|op| op.id == field.id)
}

/// egui's checkboxes, radios and selectable buttons: widgets whose value set is fixed, so unlike
/// the sliders they need no state of their own and go straight to the operator through an event.
/// egui drew the Rotate arrows as emoji; the body font has no glyphs for them, so the labels here
/// are the angles alone.
fn discrete<F>(
    stack: Stack,
    index: usize,
    op: &ImgOpItem,
    crop_applied: bool,
    open_combo: Option<(u64, &'static str)>,
    palette: theme::Palette,
    on_event: F,
) -> Vec<AnyElement>
where
    F: Fn(Event, &mut App) + Clone + 'static,
{
    let mut widgets: Vec<AnyElement> = Vec::new();
    let id = op.id;
    let open = |field: &'static str| open_combo == Some((id, field));
    match &op.operation {
        ImageOperation::CropPerspective { .. } => widgets.push(
            panel::chrome(
                format!("{stack:?} {index} crop"),
                if crop_applied {
                    panel::labeled(icons::ARROW_U_UP_LEFT, "Reset")
                } else {
                    panel::labeled(icons::CHECK, "Apply")
                },
                if crop_applied {
                    Event::CropReset(stack, index)
                } else {
                    Event::CropApply(stack)
                },
                palette,
                on_event,
            )
            .tooltip(move |window, cx| {
                Tooltip::new("Apply the crop. You don't lose image data by this.").build(window, cx)
            })
            .into_any(),
        ),
        ImageOperation::Slice(_, _, hard) => widgets.push(
            flag_row(
                format!("{stack:?} {index} hard"),
                "Smooth",
                *hard,
                stack,
                index,
                "hard",
                on_event,
            )
            .into_any(),
        ),
        ImageOperation::Noise { mono, .. } => widgets.push(
            flag_row(
                format!("{stack:?} {index} mono"),
                "Grey",
                *mono,
                stack,
                index,
                "mono",
                on_event,
            )
            .into_any(),
        ),
        ImageOperation::Flip(horizontal) => widgets.push(
            RadioGroup::horizontal(format!("{stack:?} {index} flip"))
                .selected_index(Some(if *horizontal { 0 } else { 1 }))
                .child(Radio::new("v").label("V"))
                .child(Radio::new("h").label("H"))
                .on_click(move |chosen, _, app| {
                    on_event(
                        Event::SetParam(stack, index, Param::Flag("horizontal", *chosen == 0)),
                        app,
                    )
                })
                .into_any_element(),
        ),
        ImageOperation::Rotate(angle) => {
            let choices: [(i16, &'static str); 3] = [(90, "90°"), (270, "-90°"), (180, "180°")];
            widgets.push(
                div()
                    .flex()
                    .gap(px(2.))
                    .children(choices.iter().map(|(value, label)| {
                        panel::chrome(
                            format!("{stack:?} {index} rotate {value}"),
                            *label,
                            Event::SetParam(stack, index, Param::Angle(*value)),
                            palette,
                            on_event.clone(),
                        )
                        .when(angle == value, |this| {
                            this.bg(palette.accent).text_color(palette.accent_text)
                        })
                        .into_any()
                    }))
                    .into_any(),
            );
        }
        ImageOperation::ColorConverter(current) => widgets.push(
            dropdown(
                format!("{stack:?} {index} ct"),
                current.to_string(),
                open("ct"),
                ColorTypeExt::iter()
                    .map(|t| {
                        let selected = t == *current;
                        (t.to_string(), Param::ColorType(t), selected)
                    })
                    .collect(),
                stack,
                index,
                "ct",
                palette,
                on_event,
            )
            .into_any(),
        ),
        ImageOperation::ChannelSwap(channels) => widgets.push(
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(dropdown(
                    format!("{stack:?} {index} ch0"),
                    format!("{:?}", channels.0),
                    open("ch0"),
                    channel_items(0, channels.0),
                    stack,
                    index,
                    "ch0",
                    palette,
                    on_event.clone(),
                ))
                .child("=")
                .child(dropdown(
                    format!("{stack:?} {index} ch1"),
                    format!("{:?}", channels.1),
                    open("ch1"),
                    channel_items(1, channels.1),
                    stack,
                    index,
                    "ch1",
                    palette,
                    on_event,
                ))
                .into_any(),
        ),
        ImageOperation::Resize {
            dimensions,
            aspect,
            filter,
        } => {
            widgets.push(
                dropdown(
                    format!("{stack:?} {index} filter"),
                    format!("{filter:?}"),
                    open("filter"),
                    [
                        ScaleFilter::Box,
                        ScaleFilter::Bilinear,
                        ScaleFilter::Hamming,
                        ScaleFilter::CatmullRom,
                        ScaleFilter::Mitchell,
                        ScaleFilter::Lanczos3,
                    ]
                    .iter()
                    .map(|f| (format!("{f:?}"), Param::ScaleFilter(*f), f == filter))
                    .collect(),
                    stack,
                    index,
                    "filter",
                    palette,
                    on_event.clone(),
                )
                .into_any(),
            );
            let divisor = gcd(dimensions.0, dimensions.1);
            let ratio = f64::from(dimensions.0) / f64::from(dimensions.1).max(1.);
            widgets.push(
                div()
                    .child(format!(
                        "Aspect ratio: {}:{} ({:.5})",
                        dimensions.0 / divisor.max(1),
                        dimensions.1 / divisor.max(1),
                        ratio
                    ))
                    .into_any(),
            );
            // egui labelled this checkbox with a lock emoji; the body font has no glyph for it.
            widgets.push(
                flag_row(
                    format!("{stack:?} {index} aspect"),
                    "Lock",
                    *aspect,
                    stack,
                    index,
                    "aspect",
                    on_event.clone(),
                )
                .into_any(),
            );
            widgets.push(
                panel::panel_button("Reset", Event::ResetResize(stack, index), palette, on_event)
                    .w_full()
                    .justify_center()
                    .into_any(),
            );
        }
        ImageOperation::LUT(name) => widgets.push(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(dropdown(
                    format!("{stack:?} {index} lut"),
                    lut_shown(name),
                    open("lut"),
                    lut_items(name),
                    stack,
                    index,
                    "lut",
                    palette,
                    on_event.clone(),
                ))
                // egui's two builds of this row differed in both texts: the `file_open` one said
                // "Load from disk" over the platform dialog with a "Load Hald CLUT" hover, while its
                // default build said "Load lut", had no hover text at all, and opened the in-app
                // browser (HEAD src/image_editing.rs:423-472).
                .child({
                    let button = panel::panel_button(
                        if cfg!(feature = "file_open") {
                            "Load from disk"
                        } else {
                            "Load lut"
                        },
                        Event::LoadLut(stack, index),
                        palette,
                        on_event.clone(),
                    )
                    .w_full()
                    .justify_center();
                    if cfg!(feature = "file_open") {
                        button.tooltip(move |window, cx| {
                            Tooltip::new("Load Hald CLUT").build(window, cx)
                        })
                    } else {
                        button
                    }
                })
                .child("Find more LUTs here:")
                .child(
                    div()
                        .id(format!("{stack:?} {index} more luts"))
                        .px(px(10.))
                        .py(px(4.))
                        .flex()
                        .items_center()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .text_color(palette.accent)
                        .hover(move |this| this.bg(palette.text.opacity(0.08)))
                        .tooltip(move |window, cx| {
                            Tooltip::new("You can find more interesting LUTs here to use")
                                .build(window, cx)
                        })
                        .child("Cédric Eberhardt's collection")
                        .on_click(move |_, _, app| on_event(Event::MoreLuts, app)),
                )
                .into_any(),
        ),
        _ => {}
    }
    widgets
}

/// egui's LUT combo showed the file name alone, so a CLUT picked from disk reads as its name while
/// the row keeps the full path it was loaded from.
fn lut_shown(name: &str) -> String {
    Path::new(name)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

/// The builtins in the order egui sorted them into, each labelled the way its combo labelled it.
fn lut_items(current: &str) -> Vec<(String, Param, bool)> {
    let luts = builtin_luts();
    let mut names: Vec<&str> = luts.keys().map(String::as_str).collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            (
                lut_shown(name),
                Param::Lut(name.to_string()),
                name == current,
            )
        })
        .collect()
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// egui listed the four channels in both ChannelCopy combos, in this order.
fn channel_items(slot: usize, current: Channel) -> Vec<(String, Param, bool)> {
    [Channel::Red, Channel::Green, Channel::Blue, Channel::Alpha]
        .iter()
        .map(|channel| {
            (
                format!("{channel:?}"),
                Param::Channel(slot, *channel),
                *channel == current,
            )
        })
        .collect()
}

/// egui's `ComboBox`: a trigger carrying the current value, and its choices unfolded underneath
/// while it is open. gpui-component's `Select` needs a delegate of its own, so the panel keeps the
/// same choices with the chrome every other row button already uses.
fn dropdown<F>(
    id: String,
    label: String,
    open: bool,
    items: Vec<(String, Param, bool)>,
    stack: Stack,
    index: usize,
    field: &'static str,
    palette: theme::Palette,
    on_event: F,
) -> Div
where
    F: Fn(Event, &mut App) + Clone + 'static,
{
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            panel::chrome(
                id.clone(),
                label,
                Event::ToggleCombo(stack, index, field),
                palette,
                on_event.clone(),
            )
            .w_full()
            .justify_between()
            .child(panel::icon(if open {
                icons::CHEVRON_UP
            } else {
                icons::CHEVRON_DOWN
            })),
        )
        .when(open, |this| {
            this.children(items.into_iter().map(|(name, param, selected)| {
                panel::chrome(
                    format!("{id} {name}"),
                    name,
                    Event::SetParam(stack, index, param),
                    palette,
                    on_event.clone(),
                )
                .w_full()
                .when(selected, |item| {
                    item.bg(palette.accent).text_color(palette.accent_text)
                })
                .into_any()
            }))
        })
}

/// egui's `styled_checkbox`: the box carries the operator's bool and the label names the mode it
/// selects, which is not always the same thing (Slice's box is `hard` but reads "Smooth").
fn flag_row<F>(
    id: String,
    label: &'static str,
    checked: bool,
    stack: Stack,
    index: usize,
    field: &'static str,
    on_event: F,
) -> Div
where
    F: Fn(Event, &mut App) + 'static,
{
    div().child(
        Checkbox::new(id)
            .label(label)
            .checked(checked)
            .on_click(move |checked, _, app| {
                on_event(
                    Event::SetParam(stack, index, Param::Flag(field, *checked)),
                    app,
                )
            }),
    )
}

/// egui's per-frame paint block (HEAD `src/ui/edit_ui.rs:347-373`), as a pure transition: with the
/// primary button held and its pointer off egui's own widgets it pushed the cursor's UV onto the
/// last stroke *every frame*, and on a frame where the button was no longer held it cloned that
/// stroke's settings into a fresh empty one. Returns whether the pixel buffer needs re-rendering,
/// which is what egui's `pixels_changed = true` did.
pub fn paint_frame(
    strokes: &mut Vec<PaintStroke>,
    down: bool,
    over_ui: bool,
    uv: (f32, f32),
) -> bool {
    if strokes.is_empty() {
        strokes.push(PaintStroke::new());
    }
    let Some(current) = strokes.last_mut() else {
        return false;
    };
    if down && !over_ui {
        current.points.push(uv);
        return true;
    }
    if current.is_empty() {
        return false;
    }
    let next = current.without_points();
    strokes.push(next);
    false
}

/// egui's paint section (HEAD `src/ui/edit_ui.rs:206-344`): the mode toggle, its "Keep history"
/// checkbox, the row of the stroke the pointer is about to draw, and the list of finished ones.
/// egui drew the grid headers in `egui::Grid`; the rows here are flex, so the columns do not align
/// with those labels exactly. Its emoji-only labels ("📜 Keep history", "↩", "⊗") are ported as
/// icon-plus-text or plain text, as everywhere else in this panel.
fn paint<F>(
    state: &OculanteState,
    palette: theme::Palette,
    combo: Option<usize>,
    params: &mut Params,
    window: &mut Window,
    cx: &mut App,
    on_event: F,
) -> Vec<AnyElement>
where
    F: Fn(Event, &mut App) + Clone + 'static,
{
    let edit = &state.edit_state;
    let frame = if palette.dark {
        rgb(0x21_21_21)
    } else {
        rgb(0xF2_F2_F2)
    };
    let mut rows = vec![if edit.painting {
        panel::chrome(
            "stop-painting",
            "Stop painting",
            Event::PaintMode(false),
            palette,
            on_event.clone(),
        )
        .w_full()
        .bg(palette.accent)
        .text_color(palette.accent_text)
        .into_any()
    } else {
        panel::chrome(
            "paint-mode",
            "Paint mode",
            Event::PaintMode(true),
            palette,
            on_event.clone(),
        )
        .w_full()
        .into_any()
    }];
    if !edit.painting {
        return rows;
    }

    // The dabs are the same bitmaps every frame, so egui's one-upload-per-brush is this cache. Taken
    // out as handles rather than borrowed, because the rows below still need `params` mutably for
    // their colour pickers.
    if params.brushes.is_empty() {
        params.brushes = texture::brush_textures(&edit.brushes);
    }
    let previews = params.brushes.clone();

    let on_keep = on_event.clone();
    rows.push(
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .pt(px(6.))
            .child(
                Checkbox::new("keep-history")
                    .label("Keep history")
                    // egui's `styled_checkbox("").on_hover_text` (HEAD src/ui/edit_ui.rs:239).
                    .tooltip("Keeps all paint history and allows edits to it. Slower.")
                    .checked(edit.non_destructive_painting)
                    .on_click(move |checked, _, app| on_keep(Event::KeepHistory(*checked), app)),
            )
            .into_any(),
    );

    let last = edit.paint_strokes.len().checked_sub(1);
    if let (Some(index), Some(stroke)) = (last, edit.paint_strokes.last())
        && stroke.is_empty()
    {
        rows.push(stroke_header(false).into_any());
        rows.push(stroke_row(
            stroke,
            index,
            &previews,
            frame,
            combo == Some(index),
            false,
            palette,
            params,
            window,
            cx,
            on_event.clone(),
        ));
    }

    let finished: Vec<usize> = edit
        .paint_strokes
        .iter()
        .enumerate()
        .filter(|(_, stroke)| !stroke.is_empty() && !stroke.committed)
        .map(|(index, _)| index)
        .collect();
    // egui gated this section on "any stroke has points", not on "any row would be listed", so the
    // Undo/Clear row stayed reachable while every stroke was already committed into the buffer.
    let any_stroked = edit.paint_strokes.iter().any(|stroke| !stroke.is_empty());
    if any_stroked {
        rows.push(panel::separator().into_any());
        rows.push(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(div().child("Strokes"))
                .child(
                    panel::chrome(
                        "stroke-undo",
                        panel::labeled(icons::ARROW_U_UP_LEFT, "Undo"),
                        Event::StrokeUndo,
                        palette,
                        on_event.clone(),
                    )
                    .flex_1(),
                )
                .child(
                    panel::chrome(
                        "stroke-clear",
                        "Clear all",
                        Event::StrokeClear,
                        palette,
                        on_event.clone(),
                    )
                    .flex_1(),
                )
                .into_any(),
        );
        let list = edit
            .paint_strokes
            .iter()
            .enumerate()
            .filter_map(|(index, stroke)| {
                finished.contains(&index).then(|| {
                    stroke_row(
                        stroke,
                        index,
                        &previews,
                        frame,
                        combo == Some(index),
                        true,
                        palette,
                        params,
                        window,
                        cx,
                        on_event.clone(),
                    )
                })
            });
        rows.push(
            div()
                .id("stroke-list")
                .flex()
                .flex_col()
                .gap(px(4.))
                .min_h(px(64.))
                .max_h(px(300.))
                .overflow_y_scroll()
                .child(stroke_header(true))
                .children(list)
                .into_any(),
        );
    }
    rows
}

fn stroke_header(deletable: bool) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .text_sm()
        .children(
            ["Color", "Fade", "Flip", "Width", "Brush"]
                .into_iter()
                .map(|label| div().child(label)),
        )
        .when(deletable, |this| this.child(div().child("Del")))
}

/// egui's `stroke_ui` (HEAD `src/ui/edit_ui.rs:676-771`) as one row. Its hover response wrote
/// `stroke.highlight`, which `PaintStroke::render` turns into a 2.5x brighter brush, so the row
/// reports hover as an event rather than only styling itself.
#[allow(clippy::too_many_arguments)]
fn stroke_row<F>(
    stroke: &PaintStroke,
    index: usize,
    previews: &[Arc<RenderImage>],
    frame: Rgba,
    combo: bool,
    deletable: bool,
    palette: theme::Palette,
    params: &mut Params,
    window: &mut Window,
    cx: &mut App,
    on_event: F,
) -> AnyElement
where
    F: Fn(Event, &mut App) + Clone + 'static,
{
    let picker = params.stroke_color(index, bytes_of_stroke(stroke.color), window, cx);
    div()
        // `on_hover` is only available once the div has an id to keep its hover state in.
        .id(SharedString::from(format!("stroke row {index}")))
        .flex()
        .flex_col()
        .gap(px(2.))
        .w_full()
        .on_hover({
            let on_event = on_event.clone();
            move |&over, _, app| on_event(Event::Stroke(index, StrokeParam::Hover(over)), app)
        })
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(4.))
                .child(
                    ColorPicker::new(&picker)
                        .anchor(Anchor::TopRight)
                        .accessibility_label(format!("Stroke {index} color")),
                )
                .child(
                    Checkbox::new(format!("stroke fade {index}"))
                        .label("")
                        // egui HEAD src/ui/edit_ui.rs:681.
                        .tooltip("Fade out the stroke over its path")
                        .checked(stroke.fade)
                        .on_click({
                            let on_event = on_event.clone();
                            move |checked, _, app| {
                                on_event(Event::Stroke(index, StrokeParam::Fade(*checked)), app)
                            }
                        }),
                )
                .child(
                    Checkbox::new(format!("stroke flip {index}"))
                        .label("")
                        // egui HEAD src/ui/edit_ui.rs:699.
                        .tooltip("Flip brush X and Y randomly to make stroke less uniform")
                        .checked(stroke.flip_random)
                        .on_click({
                            let on_event = on_event.clone();
                            move |checked, _, app| {
                                on_event(Event::Stroke(index, StrokeParam::Flip(*checked)), app)
                            }
                        }),
                )
                .child(stroke_drag_value(
                    index,
                    stroke.width,
                    frame,
                    on_event.clone(),
                ))
                .child(
                    // egui's `ui.horizontal` put the current dab to the left of the combo, inside the
                    // same grid cell, so the pair is one cell here too.
                    div()
                        .flex()
                        .flex_1()
                        .items_center()
                        .gap(px(4.))
                        .children(brush_preview(previews, stroke.brush_index))
                        .child(
                            panel::chrome(
                                format!("stroke brush {index}"),
                                format!("Brush {}", stroke.brush_index),
                                Event::StrokeCombo(index),
                                palette,
                                on_event.clone(),
                            )
                            .flex_1()
                            .justify_between()
                            .child(panel::icon(if combo {
                                icons::CHEVRON_UP
                            } else {
                                icons::CHEVRON_DOWN
                            })),
                        ),
                )
                .when(deletable, |this| {
                    this.child(panel::chrome(
                        format!("stroke del {index}"),
                        "Del",
                        Event::StrokeRemove(index),
                        palette,
                        on_event.clone(),
                    ))
                }),
        )
        .when(combo, |this| {
            this.children(previews.iter().enumerate().map(|(brush, _)| {
                // egui's option row: `ui.horizontal(|ui| { image; selectable_value(..) })`, so the
                // accent fill of the selected one covers its label only, never the dab beside it.
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .w_full()
                    .children(brush_preview(previews, brush))
                    .child(
                        panel::chrome(
                            format!("stroke brush {index} {brush}"),
                            format!("Brush {brush}"),
                            Event::Stroke(index, StrokeParam::Brush(brush)),
                            palette,
                            on_event.clone(),
                        )
                        .flex_1()
                        .when(brush == stroke.brush_index, |item| {
                            item.bg(palette.accent).text_color(palette.accent_text)
                        }),
                    )
                    .into_any()
            }))
        })
        .into_any()
}

/// egui's brush preview (`src/ui/edit_ui.rs:725-731` and `:739-745`): the dab itself as a square of
/// the row's height, in front of the combo trigger and in front of every option, so a brush can be
/// told apart before it is picked. `fit_to_exact_size` stretched it into that square, which is
/// `ObjectFit::Fill` here. egui magnified it Nearest while gpui's atlas is always linear — that is
/// the shell-wide filtering divergence, not this widget's.
fn brush_preview(previews: &[Arc<RenderImage>], brush: usize) -> Option<Div> {
    let texture = previews.get(brush).cloned()?;
    Some(
        div().size(px(BRUSH_PREVIEW)).flex_shrink_0().child(
            img(ImageSource::Render(texture))
                .size_full()
                .object_fit(ObjectFit::Fill),
        ),
    )
}

/// egui's `DragValue` for a stroke's width: `0.0..=0.3` at `speed(0.001)`. Drag-only, like the
/// Preferences rows — its click-to-edit field is wired per operator, not per stroke.
fn stroke_drag_value<F>(index: usize, width: f32, frame: Rgba, on_event: F) -> Stateful<Div>
where
    F: Fn(Event, &mut App) + Clone + 'static,
{
    let grab = NumGrab {
        slot: Slot::Field("width"),
        x: 0.,
        y: 0.,
        value: f64::from(width),
        speed: 0.001,
        lo: 0.,
        hi: 0.3,
        integer: false,
    };
    div()
        .id(SharedString::from(format!("stroke num {index}")))
        .flex()
        .items_center()
        .gap(px(2.))
        .px(px(6.))
        .py(px(1.))
        .rounded(px(4.))
        .bg(frame)
        .cursor_ew_resize()
        .child(format!("{width:.3}"))
        .on_mouse_down(MouseButton::Left, {
            let on_event = on_event.clone();
            move |event: &MouseDownEvent, _, app| {
                on_event(
                    Event::StrokeDrag(
                        index,
                        NumGrab {
                            x: event.position.x.into(),
                            y: event.position.y.into(),
                            ..grab
                        },
                    ),
                    app,
                )
            }
        })
}

/// egui bound its colour button to the 0..=1 floats directly; gpui-component's picker holds bytes,
/// so a row seeds it with its stroke's colour quantised to 8 bits.
fn bytes_of_stroke(color: [f32; 4]) -> [u8; 4] {
    color.map(|c| (c.clamp(0., 1.) * 255.).round() as u8)
}

/// egui's "Disable all edits", which sat outside the centered button groups.
fn skip_processing<F>(skip: bool, on_event: F) -> Div
where
    F: Fn(Event, &mut App) + 'static,
{
    div().flex().items_center().pt(px(6.)).child(
        Checkbox::new("disable-edits")
            .label("Disable all edits")
            .checked(skip)
            .on_click(move |checked, _, app| on_event(Event::SkipProcessing(*checked), app)),
    )
}

/// The buttons egui stacked under the stacks: apply, remove all and the reload of the file.
fn egui_tail<F>(
    state: &OculanteState,
    palette: theme::Palette,
    lossless_open: bool,
    on_event: F,
) -> Vec<Div>
where
    F: Fn(Event, &mut App) + Clone + 'static,
{
    #[cfg(not(feature = "turbo"))]
    let _ = lossless_open;

    let mut tail = vec![
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                panel::panel_button(
                    "Apply all edits",
                    Event::ApplyAll,
                    palette,
                    on_event.clone(),
                )
                .w_full()
                .justify_center()
                .tooltip(move |window, cx| {
                    Tooltip::new("Apply all edits to the image and reset edit controls")
                        .build(window, cx)
                }),
            )
            .child(
                panel::panel_button(
                    "Remove all edits",
                    Event::RemoveAll,
                    palette,
                    on_event.clone(),
                )
                .w_full()
                .justify_center(),
            ),
    ];

    if state.current_path.is_some() {
        tail.push(
            div().flex().flex_col().gap(px(2.)).pt(px(6.)).child(
                panel::panel_button("Reload & Restore", Event::Reload, palette, on_event.clone())
                    .w_full()
                    .justify_center()
                    .tooltip(move |window, cx| {
                        Tooltip::new("Completely reloads the current image, destroying all edits.")
                            .build(window, cx)
                    }),
            ),
        );
    }

    // egui drew the JPEG-only lossless section right below "Reload & Restore".
    #[cfg(feature = "turbo")]
    tail.extend(lossless(state, palette, lossless_open, on_event.clone()));

    // egui's build without a file dialog: an image that has no path yet first gets a default name.
    #[cfg(not(feature = "file_open"))]
    if state.current_image.is_some() && state.current_path.is_none() {
        tail.push(
            div()
                .flex()
                .pt(px(6.))
                .child(
                    panel::panel_button(
                        "Create output file",
                        Event::CreateOutputFile,
                        palette,
                        on_event.clone(),
                    )
                    .w_full()
                    .justify_center()
                    .tooltip(move |window, cx| {
                        Tooltip::new(
                            "This image does not have any file associated with it. Click to create a default one.",
                        )
                        .build(window, cx)
                    }),
                ),
        );
    }

    // egui drew this row in both builds — a native dialog in `file_open`, the in-app browser in the
    // default one — so it is gated only on having an image.
    if state.current_image.is_some() {
        tail.push(
            div().flex().pt(px(6.)).child(
                panel::panel_button("Save as...", Event::SaveAs, palette, on_event.clone())
                    .w_full()
                    .justify_center(),
            ),
        );
    }

    if let Some(path) = &state.current_path {
        let label = if path.exists() { "Overwrite" } else { "Save" };
        tail.push(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .pt(px(6.))
                .child(
                    panel::panel_button(label, Event::Save, palette, on_event.clone())
                        .w_full()
                        .justify_center()
                        .tooltip(move |window, cx| {
                            Tooltip::new(
                                "Saves the image. This will create a new file or overwrite an existing one.",
                            )
                            .build(window, cx)
                        }),
                )
                .child(
                    panel::panel_button("Save edits", Event::SaveEdits, palette, on_event.clone())
                        .w_full()
                        .justify_center()
                        .tooltip(move |window, cx| {
                            Tooltip::new(
                                "Saves an .oculante metafile in the same directory as the image. This file will contain all edits and will be restored automatically if you open the image again. This leaves the original image unmodified and allows you to continue editing later.",
                            )
                            .build(window, cx)
                        }),
                )
                .child(
                    panel::panel_button(
                        "Save directory edits",
                        Event::SaveDirEdits,
                        palette,
                        on_event,
                    )
                    .w_full()
                    .justify_center()
                    .tooltip(move |window, cx| {
                        Tooltip::new(
                            "Saves an .oculante metafile in the same directory as all applicable images. This file will contain all edits and will be restored automatically if you open the image(s) again. This leaves the original image(s) unmodified and allows you to continue editing later.",
                        )
                        .build(window, cx)
                    }),
                ),
        );
    }
    tail
}

/// egui's `jpg_lossless_ui`: the collapsing section whose buttons write through libjpeg-turbo
/// straight to disk. Upstream drew it inside the save block and returned early unless the current
/// path ended in jpg or jpeg.
#[cfg(feature = "turbo")]
fn lossless<F>(
    state: &OculanteState,
    palette: theme::Palette,
    open: bool,
    on_event: F,
) -> Option<Div>
where
    F: Fn(Event, &mut App) + Clone + 'static,
{
    if !is_jpeg(state.current_path.as_deref()) {
        return None;
    }

    let cropped = first_active_crop(&state.edit_state.image_op_stack)
        .is_some_and(|amounts| amounts != [0, 0, 0, 0]);

    let body = vec![
        div()
            .text_size(px(11.))
            .child(panel::labeled(
                icons::WARNING_CIRCLE,
                "These operations will immediately write changes to disk.",
            ))
            .into_any(),
        // egui's ui.columns(3) and ui.columns(2): equal-width rows.
        div()
            .flex()
            .gap(px(4.))
            .child(lossless_button(
                "Rotate 90°",
                Lossless::Rot90,
                palette,
                on_event.clone(),
            ))
            .child(lossless_button(
                "Rotate -90°",
                Lossless::Rot270,
                palette,
                on_event.clone(),
            ))
            .child(lossless_button(
                "Rotate 180°",
                Lossless::Rot180,
                palette,
                on_event.clone(),
            ))
            .into_any(),
        div()
            .flex()
            .gap(px(4.))
            .child(lossless_button(
                "Flip H",
                Lossless::Hflip,
                palette,
                on_event.clone(),
            ))
            .child(lossless_button(
                "Flip V",
                Lossless::Vflip,
                palette,
                on_event.clone(),
            ))
            .into_any(),
        crop_button(cropped, palette, on_event.clone()).into_any(),
    ];

    Some(panel::section(
        "Lossless JPEG edits",
        open,
        palette,
        Event::ToggleLossless,
        on_event,
        body,
    ))
}

#[cfg(feature = "turbo")]
fn lossless_button(
    label: &'static str,
    edit: Lossless,
    palette: theme::Palette,
    on_event: impl Fn(Event, &mut App) + 'static,
) -> AnyElement {
    panel::panel_button(label, Event::Lossless(edit), palette, on_event)
        .flex_1()
        .justify_center()
        .into_any()
}

/// egui's "Crop Losslessly": full width and centred, greyed out with its own hover text until the
/// stack's crop operator held amounts.
#[cfg(feature = "turbo")]
fn crop_button(
    cropped: bool,
    palette: theme::Palette,
    on_event: impl Fn(Event, &mut App) + 'static,
) -> AnyElement {
    if cropped {
        panel::panel_button(
            "Crop Losslessly",
            Event::Lossless(Lossless::Crop),
            palette,
            on_event,
        )
        .w_full()
        .justify_center()
        .tooltip(move |window, cx| {
            Tooltip::new("Crop according to values defined in the operator stack above")
                .build(window, cx)
        })
        .into_any()
    } else {
        div()
            .id("edit-lossless-crop")
            .w_full()
            .px(px(10.))
            .py(px(4.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .bg(palette.backdrop)
            .opacity(0.3)
            .child("Crop Losslessly")
            .tooltip(move |window, cx| {
                Tooltip::new(
                    "Please modify crop values above before cropping. You would be cropping nothing right now.",
                )
                .build(window, cx)
            })
            .into_any()
    }
}

/// egui's warning that the loaded image is not 8-bit RGBA, which is what makes it insert a
/// conversion operator of its own accord.
fn notice(state: &OculanteState) -> Option<Div> {
    let rgba = state
        .current_image
        .as_ref()
        .is_some_and(|img| img.color() == ColorType::Rgba8);
    (!rgba).then(|| {
        div()
            .pt(px(10.))
            .text_size(px(11.))
            .child(panel::labeled(
                icons::INFO,
                "Your image is not 8 bit RGBA. For full editing support a conversion operator was added.",
            ))
    })
}

/// egui printed the two result buffers' colour types under a debug build, which is what made a
/// stuck pipeline obvious.
fn debug_info(state: &OculanteState) -> Option<Vec<Div>> {
    cfg!(debug_assertions).then(|| {
        vec![
            div().text_color(rgb(0xAD_D8_E6)).child("Debug info"),
            div().child(format!(
                "image op: {:?}",
                state.edit_state.result_image_op.color()
            )),
            div().child(format!(
                "pixel op: {:?}",
                state.edit_state.result_pixel_op.color()
            )),
        ]
    })
}

/// egui rebuilt these from scratch every frame, so the defaults below are what a freshly added
/// operator carries; its per-operation widgets are what changed them afterwards.
fn templates(state: &OculanteState) -> Vec<ImgOpItem> {
    let dimensions = state.image_geometry.dimensions;
    vec![
        ImgOpItem::new(ImageOperation::Brightness(0)),
        ImgOpItem::new(ImageOperation::Contrast(0)),
        ImgOpItem::new(ImageOperation::Exposure(0)),
        ImgOpItem::new(ImageOperation::Desaturate(0)),
        ImgOpItem::new(ImageOperation::Invert),
        ImgOpItem::new(ImageOperation::ChannelSwap((Channel::Red, Channel::Red))),
        ImgOpItem::new(ImageOperation::Equalize((0, 255))),
        ImgOpItem::new(ImageOperation::HSV((0, 100, 100))),
        ImgOpItem::new(ImageOperation::Add([0, 0, 0])),
        ImgOpItem::new(ImageOperation::Mult([255, 255, 255])),
        ImgOpItem::new(ImageOperation::Fill([255, 255, 255, 255])),
        ImgOpItem::new(ImageOperation::Slice(128, 20, false)),
        ImgOpItem::new(ImageOperation::LUT("Lomography Redscale 100".into())),
        ImgOpItem::new(ImageOperation::GradientMap(vec![
            GradientStop::new(0, [155, 33, 180]),
            GradientStop::new(128, [255, 83, 0]),
            GradientStop::new(255, [224, 255, 0]),
        ])),
        ImgOpItem::new(ImageOperation::Posterize(8)),
        ImgOpItem::new(ImageOperation::Filter3x3([
            0, -100, 0, -100, 500, -100, 0, -100, 0,
        ])),
        ImgOpItem::new(ImageOperation::ColorConverter(ColorTypeExt::Rgba8)),
        ImgOpItem::new(ImageOperation::MMult),
        ImgOpItem::new(ImageOperation::MDiv),
        ImgOpItem::new(ImageOperation::Expression("r = 1.0".into())),
        ImgOpItem::new(ImageOperation::ScaleImageMinMax),
        ImgOpItem::new(ImageOperation::Blur(0)),
        ImgOpItem::new(ImageOperation::Noise {
            amt: 50,
            mono: false,
        }),
        ImgOpItem::new(ImageOperation::ChromaticAberration(15)),
        ImgOpItem::new(ImageOperation::Flip(false)),
        ImgOpItem::new(ImageOperation::Rotate(90)),
        ImgOpItem::new(ImageOperation::Resize {
            dimensions,
            aspect: true,
            filter: ScaleFilter::Hamming,
        }),
        ImgOpItem::new(ImageOperation::Crop([0, 0, 0, 0])),
        ImgOpItem::new(ImageOperation::CropPerspective {
            points: [
                (0, 0),
                (dimensions.0, 0),
                (0, dimensions.1),
                (dimensions.0, dimensions.1),
            ],
            original_size: dimensions,
        }),
    ]
}

#[cfg(test)]
mod tests {
    // The panel's glob import of `gpui_kit` carries gpui's own `test` attribute macro, which
    // expands to `#[test]` and so recurses; name the items instead.
    #[cfg(feature = "turbo")]
    use super::first_active_crop;
    use super::{
        BarState, GradientEdit, NumGrab, NumRow, Param, Slot, bar_drag, bytes_of, bytes_of_stroke,
        color_rows, gradient_edit, hsla_of, lut_items, lut_shown, nearest_stop, no_options,
        num_rows, paint_frame, set_color, set_num, set_param, slider_rows, text_rows,
    };
    use crate::image_editing::{GradientStop, ImageOperation, builtin_luts};
    use crate::paint::PaintStroke;

    /// egui's crop row read X, W, Y, H off the bounds array, not its memory order.
    #[test]
    fn crop_rows_keep_eguis_axis_order() {
        let rows = num_rows(&ImageOperation::Crop([1000, 2000, 3000, 4000]));
        assert_eq!(
            rows.iter().map(|r| (r.slot, r.label)).collect::<Vec<_>>(),
            vec![
                (Slot::Field("x"), "x "),
                (Slot::Field("w"), "w "),
                (Slot::Field("y"), "y "),
                (Slot::Field("h"), "h "),
            ]
        );
        assert_eq!(
            rows.iter().map(|r| r.value).collect::<Vec<_>>(),
            vec![0.1, 0.3, 0.2, 0.4]
        );
    }

    #[test]
    fn colour_rows_follow_eguis_button_arms() {
        assert_eq!(
            color_rows(&ImageOperation::Add([1, 2, 3])),
            vec![(Slot::Field("color"), [1, 2, 3, 255])]
        );
        assert_eq!(
            color_rows(&ImageOperation::Mult([4, 5, 6])),
            vec![(Slot::Field("color"), [4, 5, 6, 255])]
        );
        assert_eq!(
            color_rows(&ImageOperation::Fill([7, 8, 9, 10])),
            vec![(Slot::Field("color"), [7, 8, 9, 10])]
        );
        assert!(color_rows(&ImageOperation::Crop([0, 0, 0, 0])).is_empty());

        // egui drew one sRGB button per stop, in its horizontal row.
        let stops = vec![
            GradientStop::new(0, [1, 2, 3]),
            GradientStop::new(128, [4, 5, 6]),
            GradientStop::new(255, [7, 8, 9]),
        ];
        let rows = color_rows(&ImageOperation::GradientMap(stops.clone()));
        assert_eq!(
            rows.iter().map(|(_, value)| *value).collect::<Vec<_>>(),
            vec![[1, 2, 3, 255], [4, 5, 6, 255], [7, 8, 9, 255]]
        );
        assert_eq!(
            rows.iter().map(|(slot, _)| *slot).collect::<Vec<_>>(),
            stops
                .iter()
                .map(|stop| Slot::Stop(stop.id))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_picked_colour_lands_in_the_operator() {
        let mut add = ImageOperation::Add([0, 0, 0]);
        assert!(set_color(&mut add, Slot::Field("color"), [10, 20, 30, 40]));
        assert_eq!(
            color_rows(&add),
            vec![(Slot::Field("color"), [10, 20, 30, 255])]
        );

        let mut mult = ImageOperation::Mult([0, 0, 0]);
        assert!(set_color(&mut mult, Slot::Field("color"), [10, 20, 30, 40]));
        assert_eq!(
            color_rows(&mult),
            vec![(Slot::Field("color"), [10, 20, 30, 255])]
        );

        let mut fill = ImageOperation::Fill([0, 0, 0, 0]);
        assert!(set_color(&mut fill, Slot::Field("color"), [10, 20, 30, 40]));
        assert_eq!(
            color_rows(&fill),
            vec![(Slot::Field("color"), [10, 20, 30, 40])]
        );

        assert!(!set_color(
            &mut fill,
            Slot::Field("amount"),
            [10, 20, 30, 40]
        ));
    }

    /// egui's LUT combo sorted the builtins and labelled each one by its file name, while a CLUT
    /// picked from disk kept its full path in the operator and showed only its name.
    #[test]
    fn lut_combo_lists_eguis_builtins_by_file_name() {
        let mut names: Vec<String> = builtin_luts().keys().cloned().collect();
        names.sort();
        let current = names[0].clone();

        let items = lut_items(&current);
        assert_eq!(
            items
                .iter()
                .map(|(label, _, _)| label.clone())
                .collect::<Vec<String>>(),
            names
                .iter()
                .map(|name| lut_shown(name))
                .collect::<Vec<String>>()
        );
        assert!(items[0].2);
        assert_eq!(items.iter().filter(|item| item.2).count(), 1);

        let mut op = ImageOperation::LUT(current.clone());
        let other = items
            .iter()
            .find(|item| !item.2)
            .map(|(_, param, _)| param.clone())
            .unwrap();
        assert!(set_param(&mut op, other));
        let ImageOperation::LUT(chosen) = op.clone() else {
            unreachable!()
        };
        assert!(names.contains(&chosen));
        assert!(!chosen.eq(&current));

        assert!(set_param(&mut op, Param::Lut("/luts/kodak.png".into())));
        let ImageOperation::LUT(path) = op else {
            unreachable!()
        };
        assert_eq!(lut_shown(&path), "kodak.png");
        // A path is neither of the builtins, so nothing in the list reads as selected.
        assert!(lut_items(&path).iter().all(|item| !item.2));
    }

    /// egui bound its colour button straight to the operator's bytes; this port goes through the
    /// picker's HSLA space, so an untouched colour must survive the trip unchanged. A loss here
    /// would both shift the user's colour and mark the stack dirty on every frame.
    #[test]
    fn an_untouched_colour_survives_the_picker_round_trip() {
        for channel in 0..=255u8 {
            for value in [
                [channel, 0, 0, 255],
                [0, channel, 0, 255],
                [0, 0, channel, 255],
                [channel, channel, channel, 255],
                [255, channel, channel / 2, channel],
                [channel, 7, 200, 33],
            ] {
                assert_eq!(bytes_of(hsla_of(value)), value, "of {value:?}");
            }
        }
    }

    /// A stop's colour is written by id, and so is the position a grab drags: egui sorted its list
    /// at the end of every frame, so a stop moves past its neighbours mid-drag and must still be the
    /// one its widget owns.
    #[test]
    fn a_stop_write_finds_the_stop_the_widget_was_built_for() {
        let stops = vec![
            GradientStop::new(200, [1, 1, 1]),
            GradientStop::new(10, [2, 2, 2]),
            GradientStop::new(120, [3, 3, 3]),
        ];
        let first = stops[0].id;
        let second = stops[1].id;
        let third = stops[2].id;
        let mut op = ImageOperation::GradientMap(stops);

        assert!(set_color(&mut op, Slot::Stop(second), [9, 9, 9, 255]));
        assert!(set_num(&mut op, Slot::Stop(first), 30.7));

        let ImageOperation::GradientMap(stops) = op.clone() else {
            unreachable!()
        };
        assert_eq!(
            stops
                .iter()
                .map(|stop| (stop.pos, stop.id, stop.col))
                .collect::<Vec<_>>(),
            vec![
                (10, second, [9, 9, 9]),
                (31, first, [1, 1, 1]),
                (120, third, [3, 3, 3]),
            ],
            "the list must leave ascending, with both writes on their own stop"
        );

        // A stop that is gone is not written, and neither is a field of another operator.
        assert!(!set_color(&mut op, Slot::Stop(7777), [0, 0, 0, 255]));
        assert!(!set_num(&mut op, Slot::Stop(7777), 1.));
        let mut crop = ImageOperation::Crop([0, 0, 0, 0]);
        assert!(!set_num(&mut crop, Slot::Stop(first), 1.));
    }

    /// The bar's state machine, as egui's temp data made it: a press grabs the nearest stop and
    /// moves it under the pointer, a move drags that stop and only highlights while held, and the
    /// release and the pointer leaving the bar are different messages.
    #[test]
    fn the_bar_grabs_the_nearest_stop_and_holds_it() {
        let stops = vec![
            GradientStop::new(0, [0, 0, 0]),
            GradientStop::new(100, [128, 128, 128]),
            GradientStop::new(255, [255, 255, 255]),
        ];
        let middle = stops[1].id;
        let low = stops[0].id;
        let mut op = ImageOperation::GradientMap(stops);

        assert_eq!(nearest_stop(&op, 90), Some(middle));
        assert_eq!(nearest_stop(&op, 130), Some(middle));
        assert_eq!(nearest_stop(&op, 40), Some(low));
        assert_eq!(nearest_stop(&ImageOperation::Crop([0, 0, 0, 0]), 90), None);

        // Hovering alone marks a stop and changes nothing.
        assert_eq!(
            bar_drag(&mut op, None, GradientEdit::Move(130)),
            BarState {
                grab: None,
                hover: Some(middle),
                changed: false
            }
        );
        // A press at 70 grabs the middle stop and yanks it under the pointer, egui's same-frame
        // jump.
        assert_eq!(
            bar_drag(&mut op, None, GradientEdit::Press(70)),
            BarState {
                grab: Some(middle),
                hover: Some(middle),
                changed: true
            }
        );
        assert_eq!(position_of(&op, middle), 70);
        // Held, it follows the pointer across the ramp.
        assert_eq!(
            bar_drag(&mut op, Some(middle), GradientEdit::Move(200)),
            BarState {
                grab: Some(middle),
                hover: Some(middle),
                changed: true
            }
        );
        assert_eq!(position_of(&op, middle), 200);
        // Leaving the bar keeps the grab, as egui's temp did; release drops it.
        assert_eq!(
            bar_drag(&mut op, Some(middle), GradientEdit::Leave),
            BarState {
                grab: Some(middle),
                hover: None,
                changed: false
            }
        );
        assert_eq!(
            bar_drag(&mut op, Some(middle), GradientEdit::Release),
            BarState {
                grab: None,
                hover: None,
                changed: false
            }
        );
        // A drag past its neighbour moves the same stop, not the one that has taken its row.
        assert_eq!(
            position_of(&op, low),
            0,
            "the stop that was not grabbed stayed put"
        );
    }

    #[test]
    fn the_buttons_add_and_drop_stops_and_keep_the_ramp_ordered() {
        let mut op = ImageOperation::GradientMap(vec![
            GradientStop::new(200, [0, 0, 0]),
            GradientStop::new(10, [255, 255, 255]),
        ]);
        assert!(gradient_edit(&mut op, GradientEdit::Add));
        let positions = match &op {
            ImageOperation::GradientMap(stops) => {
                assert_eq!(stops.len(), 3);
                assert!(
                    stops
                        .iter()
                        .any(|stop| stop.pos == 128 && stop.col == [0, 0, 0])
                );
                stops.iter().map(|stop| stop.pos).collect::<Vec<_>>()
            }
            _ => unreachable!(),
        };
        assert_eq!(positions, vec![10, 128, 200]);

        let added = match &op {
            ImageOperation::GradientMap(stops) => stops
                .iter()
                .find(|stop| stop.pos == 128)
                .map(|stop| stop.id)
                .unwrap(),
            _ => unreachable!(),
        };
        assert!(gradient_edit(&mut op, GradientEdit::Delete(added)));
        assert!(!gradient_edit(&mut op, GradientEdit::Delete(added)));
        // Only the bar's own messages go through this half.
        assert!(!gradient_edit(&mut op, GradientEdit::Press(1)));
        assert!(!gradient_edit(
            &mut ImageOperation::Crop([0, 0, 0, 0]),
            GradientEdit::Add
        ));
    }

    fn position_of(op: &ImageOperation, id: usize) -> u8 {
        match op {
            ImageOperation::GradientMap(stops) => stops
                .iter()
                .find(|stop| stop.id == id)
                .map(|stop| stop.pos)
                .unwrap(),
            _ => unreachable!(),
        }
    }

    /// egui's lossless crop button read the first *active* Crop of the image stack, and only one
    /// holding non-zero amounts enabled it.
    #[cfg(feature = "turbo")]
    #[test]
    fn only_an_active_non_zero_crop_enables_the_lossless_crop() {
        use crate::image_editing::ImgOpItem;

        let stack = |items: &[(bool, [u32; 4])]| {
            items
                .iter()
                .map(|(active, amounts)| {
                    let mut item = ImgOpItem::new(ImageOperation::Crop(*amounts));
                    item.active = *active;
                    item
                })
                .collect::<Vec<_>>()
        };
        let enabled = |items: &[(bool, [u32; 4])]| {
            first_active_crop(&stack(items)).is_some_and(|amounts| amounts != [0, 0, 0, 0])
        };

        assert_eq!(first_active_crop(&stack(&[])), None);
        assert!(!enabled(&[]));
        assert_eq!(
            first_active_crop(&stack(&[(true, [0, 0, 0, 0])])),
            Some([0, 0, 0, 0])
        );
        assert!(!enabled(&[(true, [0, 0, 0, 0])]));
        assert!(!enabled(&[(false, [9, 9, 9, 9])]));
        assert_eq!(
            first_active_crop(&stack(&[(false, [9, 9, 9, 9]), (true, [1, 2, 3, 4])])),
            Some([1, 2, 3, 4])
        );
        assert_eq!(
            first_active_crop(&stack(&[(true, [1, 1, 1, 1]), (true, [2, 2, 2, 2])])),
            Some([1, 1, 1, 1])
        );
    }

    #[test]
    fn a_press_that_stayed_put_is_the_click_that_opens_the_field() {
        let grab = |lo: f64, hi: f64, integer: bool| NumGrab {
            slot: Slot::Field("x"),
            x: 100.,
            y: 200.,
            value: 0.,
            speed: 0.004,
            lo,
            hi,
            integer,
        };
        let crop = grab(0., 1., false);
        // egui compared the squared distance against `max_click_dist`, so the slop is a radius.
        assert!(crop.clicked(100., 200.));
        assert!(crop.clicked(104., 204.));
        assert!(crop.clicked(105.9, 200.));
        assert!(!crop.clicked(106., 200.));
        assert!(!crop.clicked(105., 205.));

        assert_eq!(crop.parse("0.25"), Some(0.25));
        assert_eq!(crop.parse(" 2 "), Some(1.));
        assert_eq!(crop.parse("-1"), Some(0.));
        // "0." is a number to Rust's parser, as it is to egui's; an empty or half-typed "-" is not,
        // and leaves the value the live writes already put there.
        assert_eq!(crop.parse("0."), Some(0.));
        assert_eq!(crop.parse(""), None);
        assert_eq!(crop.parse("-"), None);
        assert_eq!(grab(-255., 255., true).parse("2.6"), Some(3.));
    }

    #[test]
    fn the_field_opens_holding_the_rows_readout() {
        let row = |value: f64, integer: bool, shown: Option<&str>| NumRow {
            slot: Slot::Field("x"),
            label: "",
            lo: 0.,
            hi: 1.,
            speed: 0.004,
            value,
            integer,
            shown: shown.map(str::to_string),
        };
        assert_eq!(row(0.25, false, None).shown(), "0.250");
        assert_eq!(row(3.6, true, None).shown(), "4");
        assert_eq!(row(0.5, false, Some("50%")).shown(), "50%");
    }

    #[test]
    fn a_paint_frame_appends_while_held_and_clones_the_settings_on_release() {
        // egui created the working stroke on the first frame that had none, and an idle frame must
        // not add a second one.
        let mut strokes: Vec<PaintStroke> = vec![];
        assert!(!paint_frame(&mut strokes, false, false, (0.5, 0.5)));
        assert_eq!(strokes.len(), 1);
        assert!(strokes[0].is_empty());

        // Held over the canvas it pushed the cursor's UV every frame, not every move event.
        assert!(paint_frame(&mut strokes, true, false, (0.1, 0.2)));
        assert!(paint_frame(&mut strokes, true, false, (0.3, 0.4)));
        assert_eq!(strokes[0].points, vec![(0.1, 0.2), (0.3, 0.4)]);

        // The first frame where it is no longer held finishes the stroke and clones it forward, so
        // the next stroke inherits colour, width, brush and the flags.
        strokes[0].fade = true;
        strokes[0].width = 0.25;
        strokes[0].brush_index = 3;
        strokes[0].flip_random = true;
        assert!(!paint_frame(&mut strokes, false, false, (0.3, 0.4)));
        assert_eq!(strokes.len(), 2);
        assert!(strokes[1].is_empty());
        assert_eq!(strokes[1].fade, true);
        assert_eq!(strokes[1].width, 0.25);
        assert_eq!(strokes[1].brush_index, 3);
        assert_eq!(strokes[1].flip_random, true);

        // A freshly cloned, still-empty stroke is not duplicated by another idle frame.
        assert!(!paint_frame(&mut strokes, false, false, (0.3, 0.4)));
        assert_eq!(strokes.len(), 2);

        // egui's `else if` fired for *any* frame that was not "held off-UI", including one where
        // the button was held over its own widgets: that pushed a clone of a non-empty stroke too.
        // Mirrored, not fixed.
        let mut over = vec![PaintStroke {
            points: vec![(0.1, 0.1), (0.2, 0.2)],
            ..PaintStroke::new()
        }];
        assert!(!paint_frame(&mut over, true, true, (0.9, 0.9)));
        assert_eq!(over.len(), 2);
        assert!(over[1].is_empty());
        assert_eq!(over[0].points.len(), 2);
    }

    #[test]
    fn a_stroke_colour_quantises_to_the_picker_and_back() {
        // gpui-component's picker holds 8-bit bytes while egui bound the 0..=1 floats directly, so
        // the seeding is lossy by a half step at most.
        assert_eq!(bytes_of_stroke([1., 0.5, 0., 1.]), [255, 128, 0, 255]);
        assert_eq!(bytes_of_stroke([0.2, 0.4, 0.6, 0.8]), [51, 102, 153, 204]);
        // Out-of-range components are clamped rather than cast into wrapped bytes.
        assert_eq!(bytes_of_stroke([-0.5, 1., 1.5, 2.]), [0, 255, 255, 255]);
        // Reading the picker back into a stroke keeps each channel within a half 8-bit step of
        // what egui had bound to the float.
        let seeded = bytes_of_stroke([0.2, 0.4, 0.6, 0.8]);
        for (before, byte) in [0.2_f32, 0.4, 0.6, 0.8].into_iter().zip(seeded) {
            let after = byte as f32 / 255.;
            assert!(
                (after - before).abs() <= 0.5 / 255. + 1e-6,
                "{before} -> {after}"
            );
        }
    }

    /// egui's `ui()` catch-all printed "Filter has no options." for exactly the operators with no
    /// widget arm. Those are the four unit variants, and each of them also yields no row from any
    /// row source this port has, so the label never competes with a widget.
    #[test]
    fn the_no_options_label_replaces_only_a_missing_widget() {
        for op in [
            ImageOperation::Invert,
            ImageOperation::MMult,
            ImageOperation::MDiv,
            ImageOperation::ScaleImageMinMax,
        ] {
            assert!(no_options(&op), "{op}");
            assert!(slider_rows(&op).is_empty(), "{op} sliders");
            assert!(text_rows(&op).is_empty(), "{op} text");
            assert!(color_rows(&op).is_empty(), "{op} colours");
            assert!(num_rows(&op).is_empty(), "{op} drag values");
        }
        for op in [
            ImageOperation::Brightness(0),
            ImageOperation::Desaturate(5),
            ImageOperation::Add([1, 2, 3]),
            ImageOperation::Expression("x".into()),
            ImageOperation::Crop([0, 0, 0, 0]),
            ImageOperation::LUT("x".into()),
            ImageOperation::GradientMap(vec![]),
        ] {
            assert!(!no_options(&op), "{op}");
        }
    }
}
