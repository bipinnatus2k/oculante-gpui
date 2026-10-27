use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::slider::{Slider, SliderState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::appstate::OculanteState;
use crate::gpui_app::{fonts, panel, theme, tiles};
use crate::icons;
use crate::utils;
use panel::{chrome, dark_panel, icon, labeled, panel_button, separator};

/// Width of the egui panel's label column.
const LABEL: f32 = 92.;
/// egui's default `Plot` height.
const HIST_HEIGHT: f32 = 200.;
/// egui allocated 32 logical pixels for every sampled swatch.
const SWATCH: f32 = 32.;
/// egui's `desired_width` for the panel's own widgets, `PANEL_WIDTH - PANEL_WIDGET_OFFSET - 20.`
/// (src/ui/info_ui.rs:62). It is built from the *default* width constant, so the preview square
/// keeps its size even though the panel is now resizable — the same upstream quirk the info grid
/// shares.
const PREVIEW: f32 = 240.;
/// Half the source-pixel neighbourhood egui magnifies: the preview spans `2*16+1` = 33 pixels.
const PREVIEW_RADIUS: f32 = 16.;
/// egui's crosshair `Stroke::new(7.0, from_rgba_unmultiplied(128,128,128,128))`.
const CROSSHAIR: f32 = 7.;

/// One of the collapsing headers egui put at the bottom of the panel.
#[derive(Clone, Copy)]
pub enum Section {
    Alpha,
    Palette,
    Measure,
    Exif,
    Dicom,
    Compare,
}

/// Which headers are open. gpui rebuilds the element tree every frame, so this cannot live in
/// the panel itself.
#[derive(Clone, Copy, Default)]
pub struct Sections {
    pub alpha: bool,
    pub palette: bool,
    pub measure: bool,
    pub exif: bool,
    pub dicom: bool,
    pub compare: bool,
}

impl Sections {
    pub fn toggle(&mut self, section: Section) {
        match section {
            Section::Alpha => self.alpha = !self.alpha,
            Section::Palette => self.palette = !self.palette,
            Section::Measure => self.measure = !self.measure,
            Section::Exif => self.exif = !self.exif,
            Section::Dicom => self.dicom = !self.dicom,
            Section::Compare => self.compare = !self.compare,
        }
    }
}

/// What the panel asks the viewer to do. egui kept the swatches in its `"picker"` temp memory and
/// the debug render in `edit_state`, neither of which has a gpui equivalent, so both live on the
/// viewer.
#[derive(Clone)]
pub enum Event {
    /// egui kept each header's open flag in its own memory; gpui has to be told to flip it.
    Toggle(Section),
    AlphaBleed,
    AlphaSemitrans,
    AlphaReset,
    Clear,
    Sort,
    Save,
    FromImage,
    Copy([u8; 4]),
    Remove([u8; 4]),
    /// egui's `{FOLDER} Open another image...`: browse, then add whatever loads to the list.
    CompareOpen,
    CompareKeepView(bool),
    CompareLoad(PathBuf),
    CompareRemove(PathBuf),
    CompareAdd,
    CompareUpdate,
    CompareClear,
    /// egui's resize strip on the panel's canvas-facing edge had the pointer pressed on it.
    ResizeStart,
}

/// egui's info panel: the per-pixel grid, the zoomed pixel preview, the alpha debug buttons, the
/// sampled palette and the image statistics.
pub fn render<F>(
    state: &OculanteState,
    palette: theme::Palette,
    top: f32,
    bottom: f32,
    width: f32,
    dragging: bool,
    sections: Sections,
    swatches: &[[u8; 4]],
    tiling_slider: &Entity<SliderState>,
    frame: Option<Arc<tiles::Frame>>,
    on_panel: F,
) -> Option<Div>
where
    F: Fn(Event, &mut App) + Clone + 'static,
{
    let settings = &state.persistent_settings;
    if !settings.info_enabled || settings.zen_mode || state.current_image.is_none() {
        return None;
    }

    let on_resize = {
        let on_panel = on_panel.clone();
        move |app: &mut App| on_panel(Event::ResizeStart, app)
    };
    Some(panel::side_panel(
        "info-scroll",
        panel::Side::Left,
        palette,
        top,
        bottom,
        width,
        dragging,
        on_resize,
        {
            let on_panel = on_panel.clone();
            move |column| {
                column
                    .children(image_grid(state))
                    .children(zoom_preview(frame, state))
                    .child(compare(state, palette, sections.compare, on_panel.clone()))
                    .child(alpha_tools(palette, sections.alpha, on_panel.clone()))
                    .child(color_palette(
                        state,
                        palette,
                        sections.palette,
                        swatches,
                        on_panel.clone(),
                    ))
                    .children(measure(state, palette, sections.measure, on_panel.clone()))
                    .child(tiling(state, palette, tiling_slider))
                    .children(stats(state).unwrap_or_default())
                    .children(advanced(state, palette, sections, on_panel.clone()))
                    .children(state.image_metadata.as_ref().map(histogram))
            }
        },
    ))
}

fn image_grid(state: &OculanteState) -> Vec<Div> {
    let (width, height) = state.image_geometry.dimensions;
    let sampled = state.sampled_color;
    // egui read the colour type off the loaded image, not off the edit result.
    let color_type = state
        .current_image
        .as_ref()
        .map_or(String::new(), |image| format!("{:?}", image.color()));
    let uv = (
        state.cursor_relative.x / width as f32,
        state.cursor_relative.y / height as f32,
    );

    let mut rows = vec![row(
        icons::ARROWS_OUT,
        "Size",
        value(format!("{width}x{height}")),
    )];

    if let Some(path) = &state.current_path {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let full = path.to_string_lossy().into_owned();
        rows.push(row(
            icons::IMAGE,
            "File",
            div()
                .id("info-file")
                .flex_1()
                .truncate()
                .text_right()
                .tooltip(move |window, cx| {
                    let full = full.clone();
                    Tooltip::element(move |_, _| div().child(full.clone())).build(window, cx)
                })
                .child(name),
        ));
    }

    rows.push(row(icons::PALETTE, "RGBA", value(utils::disp_col(sampled))));
    rows.push(row(
        icons::PALETTE,
        "RGBA",
        value(utils::disp_col_norm(sampled, 255.)),
    ));
    rows.push(row(icons::PALETTE, "HEX", value(hex(to_bytes(sampled)))));
    rows.push(row(icons::PALETTE, "Color", value(color_type)));
    rows.push(row(
        icons::MOVE,
        "Pos",
        value(format!(
            "{:.0},{:.0}",
            state.cursor_relative.x.floor(),
            state.cursor_relative.y.floor()
        )),
    ));
    rows.push(row(
        icons::INTERSECT,
        "UV",
        value(format!("{:.3},{:.3}", uv.0, 1.0 - uv.1)),
    ));
    rows
}

/// The preview's UV crop, as offsets from the square's own top-left so it can be tested without a
/// window: `(px_per_src, slot)` where `slot` is `[x, y, width, height]`.
///
/// `slot` is where the *whole* frame lands, at `px_per_src` per source pixel, shifted so the
/// neighbourhood around the cursor starts at the square's origin. Painting the sprite into the
/// square then shows exactly the `2*16+1` source pixels egui magnified, and wherever the
/// neighbourhood leaves the image the square's own filler shows through — which is what egui's
/// per-tile overlap loop produced by only drawing tiles that intersected it.
fn preview_crop(cursor: (f32, f32), dims: (u32, u32)) -> (f32, [f32; 4]) {
    let px_per_src = PREVIEW / (PREVIEW_RADIUS * 2. + 1.);
    let src_left = cursor.0.floor() - PREVIEW_RADIUS;
    let src_top = cursor.1.floor() - PREVIEW_RADIUS;
    (
        px_per_src,
        [
            -src_left * px_per_src,
            -src_top * px_per_src,
            dims.0 as f32 * px_per_src,
            dims.1 as f32 * px_per_src,
        ],
    )
}

/// egui's SECTION 2 zoom preview (src/ui/info_ui.rs:408-517): the 33x33 source-pixel neighbourhood
/// around the cursor, magnified into a 240-logical square, its centre pixel crosshaired and boxed.
///
/// egui drew one UV-cropped quad per GPU tile the frame had been subdivided into; the port's frame is
/// tiled the same way, so the crop is the same loop: `preview_sprites` lays each tile out on the
/// square's shared grid and `bounds` clips the lot back to the square, with `paint_image` skipping any
/// tile that misses it. GPUI rounds that crop to whole atlas texels (gpui-pre window.rs:4973-4976),
/// which is exact here because the cursor is floored, as egui floored it.
///
/// Divergence: gpui's atlas always samples linearly, so a magnified source pixel reads as a short
/// ramp rather than egui's default `Nearest` flat block. The canvas itself differs the same way.
fn zoom_preview(frame: Option<Arc<tiles::Frame>>, state: &OculanteState) -> Option<Div> {
    let dims = state.image_geometry.dimensions;
    let frame = frame.filter(|_| dims.0 > 0 && dims.1 > 0)?;
    let (px_per_src, slot) = preview_crop((state.cursor_relative.x, state.cursor_relative.y), dims);
    // The middle of the neighbourhood: egui's `center_x = rect.left() + radius * px_per_src`, with
    // the lines through `+ px_per_src / 2.`.
    let center = PREVIEW_RADIUS * px_per_src;
    let mid = center + px_per_src / 2.;
    Some(
        div().flex_shrink_0().pt(px(10.)).child(
            canvas(
                move |rect, _window, _cx| {
                    let (left, top) = (rect.origin.x.as_f32(), rect.origin.y.as_f32());
                    let (w, h) = (rect.size.width.as_f32(), rect.size.height.as_f32());
                    let sprites = preview_sprites(&frame, px_per_src, slot)
                        .into_iter()
                        .map(|(tile, image)| {
                            (
                                Bounds {
                                    origin: point(
                                        px(left + tile.origin.x),
                                        px(top + tile.origin.y),
                                    ),
                                    size: size(px(tile.size.width), px(tile.size.height)),
                                },
                                image,
                            )
                        })
                        .collect::<Vec<_>>();
                    (
                        rect,
                        sprites,
                        Bounds::from_corners(
                            point(px(left + mid - CROSSHAIR / 2.), px(top)),
                            point(px(left + mid + CROSSHAIR / 2.), px(top + h)),
                        ),
                        Bounds::from_corners(
                            point(px(left), px(top + mid - CROSSHAIR / 2.)),
                            point(px(left + w), px(top + mid + CROSSHAIR / 2.)),
                        ),
                        Bounds {
                            origin: point(px(left + center), px(top + center)),
                            size: size(px(px_per_src), px(px_per_src)),
                        },
                    )
                },
                move |_, (square, sprites, vertical, horizontal, cell), window, _cx| {
                    // egui's `Color32::from_gray(30)` filler, then the crop, then the 7-pt
                    // half-transparent crosshair, then the 1-pt gray-40 box inside the centre
                    // pixel's rect. `gpui_kit::fill` is spelled out because this module owns a
                    // `fill` for swatch colours.
                    window.paint_quad(gpui_kit::fill(square, rgb(0x1e_1e_1e)));
                    for (bounds, image) in sprites {
                        let _ =
                            window.paint_image(square, bounds, Corners::default(), image, 0, false);
                    }
                    let crosshair = rgb(0x80_80_80).opacity(128. / 255.);
                    window.paint_quad(gpui_kit::fill(vertical, crosshair));
                    window.paint_quad(gpui_kit::fill(horizontal, crosshair));
                    window.paint_quad(outline(cell, rgb(0x28_28_28), BorderStyle::default()));
                },
            )
            .size(px(PREVIEW)),
        ),
    )
}

/// Where each atlas tile of the frame lands inside the preview square, in logical px relative to the
/// square's origin. One tile of a 512x512 frame at `px_per_src` 7.05 occupies the whole square; a
/// frame split across tiles contributes one rect per split, laid out on the same source-pixel grid,
/// which is what makes the crop identical however the frame was subdivided.
fn preview_sprites(
    frame: &tiles::Frame,
    px_per_src: f32,
    slot: [f32; 4],
) -> Vec<(Bounds<f32>, Arc<RenderImage>)> {
    frame
        .iter()
        .map(|tile| {
            (
                Bounds {
                    origin: point(
                        slot[0] + tile.x as f32 * px_per_src,
                        slot[1] + tile.y as f32 * px_per_src,
                    ),
                    size: size(
                        tile.width as f32 * px_per_src,
                        tile.height as f32 * px_per_src,
                    ),
                },
                tile.image,
            )
        })
        .collect()
}

fn stats(state: &OculanteState) -> Option<Vec<Div>> {
    let info = state.image_metadata.as_ref()?;
    let transparent = if info.num_pixels == 0 {
        0.
    } else {
        info.num_transparent_pixels as f32 / info.num_pixels as f32 * 100.
    };
    Some(vec![
        separator(),
        row("", "Number of colors", value(info.num_colors.to_string())),
        row("", "Fully transparent", value(format!("{transparent:.2}%"))),
        row("", "Pixels", value(info.num_pixels.to_string())),
    ])
}

/// egui's `advanced_ui` tail: the metadata behind two collapsing headers.
fn advanced<F>(
    state: &OculanteState,
    palette: theme::Palette,
    sections: Sections,
    on_event: F,
) -> Vec<Div>
where
    F: Fn(Event, &mut App) + Clone + 'static,
{
    let Some(info) = &state.image_metadata else {
        return Vec::new();
    };
    let mut headers = Vec::new();
    if !info.exif.is_empty() {
        headers.push(panel::section(
            "EXIF",
            sections.exif,
            palette,
            Event::Toggle(Section::Exif),
            on_event.clone(),
            fields(&info.exif),
        ));
    }
    if let Some(dicom) = &info.dicom {
        headers.push(panel::section(
            "DICOM",
            sections.dicom,
            palette,
            Event::Toggle(Section::Dicom),
            on_event,
            fields(&dicom.dicom_data),
        ));
    }
    headers
}

/// egui's "Compare": every entry remembers the zoom and pan it was added with, so jumping between
/// images in the list restores that view instead of refitting.
fn compare<F>(state: &OculanteState, palette: theme::Palette, open: bool, on_event: F) -> Div
where
    F: Fn(Event, &mut App) + Clone + 'static,
{
    let mut body: Vec<AnyElement> = Vec::new();

    if state.persistent_settings.max_cache == 0 {
        body.push(
            div()
                .child("Warning! Set your cache to more than 0 in settings for this to be fast.")
                .into_any(),
        );
    }

    let keep_view = state.persistent_settings.compare_keep_view;
    let handler = on_event.clone();
    body.push(
        div()
            .id("persistent-zoom")
            .flex()
            .items_center()
            .child(
                Checkbox::new("persistent-zoom")
                    .label("Persistent zoom")
                    .checked(keep_view)
                    .on_click(move |checked, _, app| {
                        handler(Event::CompareKeepView(*checked), app)
                    }),
            )
            .tooltip(move |window, cx| {
                Tooltip::new(
                    "Keep the current zoom and pan when switching between images in this list.",
                )
                .build(window, cx)
            })
            .into_any(),
    );

    let mut rows: Vec<AnyElement> = vec![
        chrome(
            "compare-open",
            labeled(icons::FOLDER, "Open another image..."),
            Event::CompareOpen,
            palette,
            on_event.clone(),
        )
        .w_full()
        .justify_center()
        .into_any(),
    ];

    for item in state.compare_list.iter() {
        let name = item
            .path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let selected = state.current_path.as_ref() == Some(&item.path);
        rows.push(
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(
                    chrome(
                        format!("{name} remove"),
                        icon(icons::X),
                        Event::CompareRemove(item.path.clone()),
                        palette,
                        on_event.clone(),
                    )
                    .flex_shrink_0(),
                )
                .child(
                    chrome(
                        name.clone(),
                        name,
                        Event::CompareLoad(item.path.clone()),
                        palette,
                        on_event.clone(),
                    )
                    .flex_1()
                    .when(selected, |this| {
                        this.bg(palette.accent).text_color(palette.accent_text)
                    }),
                )
                .into_any(),
        );
    }

    // egui offered "Add current image" for a path that is not in the list yet, and only replaced
    // it with "Update position" once the stored view had drifted from the live one.
    let action = state
        .current_path
        .as_ref()
        .and_then(|path| match state.compare_list.get(path) {
            Some(geo) if geo != state.image_geometry => Some((
                "compare-update",
                labeled(icons::LOCATION_PIN, "Update position"),
                Event::CompareUpdate,
                true,
            )),
            Some(_) => None,
            None => Some((
                "compare-add",
                labeled(icons::PLUS, "Add current image"),
                Event::CompareAdd,
                false,
            )),
        });
    if let Some((id, label, event, yellow)) = action {
        rows.push(
            chrome(id, label, event, palette, on_event.clone())
                .w_full()
                .justify_center()
                .when(yellow, |this| this.text_color(rgb(0xFF_FF_00)))
                .into_any(),
        );
    }

    if !state.compare_list.is_empty() {
        rows.push(
            chrome(
                "compare-clear",
                labeled(icons::TRASH, "Clear all"),
                Event::CompareClear,
                palette,
                on_event.clone(),
            )
            .w_full()
            .justify_center()
            .into_any(),
        );
    }

    body.push(dark_panel(palette, rows).into_any());

    panel::section(
        "Compare",
        open,
        palette,
        Event::Toggle(Section::Compare),
        on_event,
        body,
    )
}

/// egui's "Alpha tools": each button swaps `edit_state.result_pixel_op` for a debug render of the
/// alpha channel, which the shell then uploads in preference to the untouched image.
fn alpha_tools(
    palette: theme::Palette,
    open: bool,
    on_event: impl Fn(Event, &mut App) + Clone + 'static,
) -> Div {
    let body = vec![dark_panel(
        palette,
        vec![
            alpha_button(
                "Show alpha bleed",
                Event::AlphaBleed,
                Some("Highlight pixels with zero alpha and color information"),
                palette,
                on_event.clone(),
            ),
            alpha_button(
                "Show semi-transparent pixels",
                Event::AlphaSemitrans,
                Some("Highlight pixels that are neither fully opaque nor fully transparent"),
                palette,
                on_event.clone(),
            ),
            alpha_button(
                "Reset image",
                Event::AlphaReset,
                None,
                palette,
                on_event.clone(),
            ),
        ],
    )];
    panel::section(
        "Alpha tools",
        open,
        palette,
        Event::Toggle(Section::Alpha),
        on_event,
        body,
    )
}

/// egui stacked these full width, unlike the palette's inline row.
fn alpha_button(
    label: &'static str,
    event: Event,
    tip: Option<&'static str>,
    palette: theme::Palette,
    on_event: impl Fn(Event, &mut App) + 'static,
) -> AnyElement {
    let button = panel_button(label, event, palette, on_event)
        .w_full()
        .justify_center();
    match tip {
        Some(tip) => button
            .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
            .into_any(),
        None => button.into_any(),
    }
}

/// egui's `palette_ui`: the swatches sampled off the image. Its widgets only ran while the
/// header was open, which is also what gated egui's right-click sampling.
fn color_palette<F>(
    state: &OculanteState,
    palette: theme::Palette,
    open: bool,
    swatches: &[[u8; 4]],
    on_event: F,
) -> Div
where
    F: Fn(Event, &mut App) + Clone + 'static,
{
    let sampled = to_bytes(state.sampled_color);
    let mut body: Vec<AnyElement> = Vec::new();

    if swatches.is_empty() {
        body.push(div().child("Right click to sample color").into_any());
    } else {
        body.push(
            div()
                .flex()
                .flex_wrap()
                .gap(px(6.))
                .children(
                    swatches
                        .iter()
                        .map(|color| swatch(*color, *color == sampled, palette, on_event.clone())),
                )
                .into_any(),
        );

        // egui drew the button in both builds — a native dialog in `file_open`, the in-app browser
        // in the default one — so it is gated on nothing but a non-empty palette.
        const SAVE: Option<(Event, &'static str)> = Some((Event::Save, "Save ASE"));

        body.push(
            div()
                .flex()
                .gap(px(4.))
                .child(panel_button(
                    "Clear",
                    Event::Clear,
                    palette,
                    on_event.clone(),
                ))
                .child(panel_button("Sort", Event::Sort, palette, on_event.clone()))
                .children(
                    SAVE.map(|(event, label)| {
                        panel_button(label, event, palette, on_event.clone())
                    }),
                )
                .into_any(),
        );
    }

    if state.current_image.is_some() {
        body.push(
            div()
                .flex()
                .child(panel_button(
                    "From image",
                    Event::FromImage,
                    palette,
                    on_event.clone(),
                ))
                .into_any(),
        );
    }

    panel::section(
        "Palette",
        open,
        palette,
        Event::Toggle(Section::Palette),
        on_event,
        vec![dark_panel(palette, body)],
    )
}

/// egui's `measure_ui`: a header whose body is one empty dark panel. The tool it hosts is not
/// driven from any widget — the viewport's right button moves the rect and `measure::Mark` draws it
/// over the image — and egui's body only really *did* two things: keep exactly one
/// `ImageOperation::Measure` in the stack (src/ui/mod.rs:585-598) and re-run `op.operation.ui(...)`
/// for every operator, which is how its Measure arm got painted.
///
/// The second half is not mirrored: egui reused one `ui` method for both the edit panel's parameter
/// widgets and this overlay hook, so opening the section drew a second copy of every filter's widget
/// in the info panel. The port's widgets are `Params` entities the edit panel owns, so it has no
/// second copy to draw; the overlay is painted from the viewport instead, from the same gate.
///
/// Upstream's own gate is `experimental_features`, so the header is absent until that is ticked.
fn measure<F>(
    state: &OculanteState,
    palette: theme::Palette,
    open: bool,
    on_event: F,
) -> Option<Div>
where
    F: Fn(Event, &mut App) + Clone + 'static,
{
    if !state.persistent_settings.experimental_features {
        return None;
    }
    Some(panel::section(
        "Measure",
        open,
        palette,
        Event::Toggle(Section::Measure),
        on_event,
        vec![dark_panel(palette, Vec::new())],
    ))
}

/// egui's tiling row: a label, an accent slider over 1..=10, and the value right-aligned in a/// 40pt box. The viewer repeats the image `n x n` times off `state.tiling`.
fn tiling(state: &OculanteState, palette: theme::Palette, slider: &Entity<SliderState>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .pt(px(8.))
        .child("Tiling")
        .child(
            // egui's `styled_slider` sets `trailing_fill(true)`, so the solid fill sits to the
            // right of the handle; `.reverse()` is gpui-component's same convention.
            Slider::new(slider)
                .flex_1()
                .reverse()
                .bg(palette.accent)
                .text_color(palette.accent),
        )
        .child(
            div()
                .w(px(40.))
                .text_right()
                .child(format!("{}", state.tiling)),
        )
}

/// egui painted a 32px filled rect per swatch, ringed in the selection colour when it matched
/// the pixel under the cursor, and took the click/hover off the same response.
fn swatch(
    color: [u8; 4],
    selected: bool,
    palette: theme::Palette,
    on_event: impl Fn(Event, &mut App) + Clone + 'static,
) -> AnyElement {
    let hex = hex(color);
    let tooltip = format!(
        "HEX: {hex}\nRGBA: {}\nLeft click to copy Hex, right click to remove.",
        utils::disp_col(to_f32(color))
    );
    div()
        .id(ElementId::Integer(u64::from(u32::from_be_bytes(color))))
        .w(px(SWATCH))
        .h(px(SWATCH))
        .flex_shrink_0()
        .rounded(px(1.))
        .cursor_pointer()
        .bg(fill(color))
        .when(selected, |this| {
            this.border_2().border_color(palette.accent)
        })
        .tooltip(move |window, cx| {
            let tooltip = tooltip.clone();
            Tooltip::element(move |_, _| div().whitespace_nowrap().child(tooltip.clone()))
                .build(window, cx)
        })
        .on_mouse_down(MouseButton::Right, {
            let on_event = on_event.clone();
            move |_, _, app| on_event(Event::Remove(color), app)
        })
        .on_click(move |_, _, app| on_event(Event::Copy(color), app))
        .into_any()
}

/// The bytes widened back to what `disp_col` expects, like egui did.
fn to_f32([r, g, b, a]: [u8; 4]) -> [f32; 4] {
    [f32::from(r), f32::from(g), f32::from(b), f32::from(a)]
}

/// egui cast the f32 readout back to bytes whenever it needed a `Color32`.
pub fn to_bytes([r, g, b, a]: [f32; 4]) -> [u8; 4] {
    [r as u8, g as u8, b as u8, a as u8]
}

fn fill(color: [u8; 4]) -> Rgba {
    rgba(u32::from_be_bytes(color))
}

/// egui's RGB histogram: three filled lines scaled to the busiest channel, with a legend.
fn histogram(info: &utils::ExtendedImageInfo) -> Div {
    let red = info.red_histogram.clone();
    let green = info.green_histogram.clone();
    let blue = info.blue_histogram.clone();

    div()
        .w_full()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .pb(px(2.))
                .child(legend_key("red", rgb(0xFF0000)))
                .child(legend_key("green", rgb(0x00FF00)))
                .child(legend_key("blue", rgb(0x0000FF))),
        )
        .child(
            canvas(
                move |_, _, _| (),
                move |bounds, (), window, _| {
                    let max = red
                        .iter()
                        .chain(&green)
                        .chain(&blue)
                        .map(|(_, count)| *count)
                        .max()
                        .unwrap_or(1)
                        .max(1) as f32;
                    for (values, color) in [
                        (&red, rgb(0xFF0000)),
                        (&green, rgb(0x00FF00)),
                        (&blue, rgb(0x0000FF)),
                    ] {
                        let line = line_points(values, bounds, max);
                        let mut area =
                            vec![point(bounds.origin.x, bounds.origin.y + bounds.size.height)];
                        area.extend(line.iter().copied());
                        area.push(point(
                            bounds.origin.x + bounds.size.width,
                            bounds.origin.y + bounds.size.height,
                        ));
                        if let Some(path) = filled(&area) {
                            window.paint_path(path, color.opacity(0.25));
                        }
                        if let Some(path) = outlined(&line) {
                            window.paint_path(path, color);
                        }
                    }
                },
            )
            .w_full()
            .h(px(HIST_HEIGHT)),
        )
}

fn line_points(values: &[(i32, u64)], bounds: Bounds<Pixels>, max: f32) -> Vec<Point<Pixels>> {
    let last = (values.len().saturating_sub(1)).max(1) as f32;
    let (left, width) = (bounds.origin.x.as_f32(), bounds.size.width.as_f32());
    let (top, height) = (bounds.origin.y.as_f32(), bounds.size.height.as_f32());
    values
        .iter()
        .enumerate()
        .map(|(i, (_, count))| {
            point(
                px(left + width * (i as f32 / last)),
                px(top + height * (1. - *count as f32 / max)),
            )
        })
        .collect()
}

fn filled(area: &[Point<Pixels>]) -> Option<Path<Pixels>> {
    let mut path = PathBuilder::fill();
    path.add_polygon(area, true);
    path.build().ok()
}

fn outlined(line: &[Point<Pixels>]) -> Option<Path<Pixels>> {
    let mut path = PathBuilder::stroke(px(1.5));
    path.add_polygon(line, false);
    path.build().ok()
}

fn legend_key(name: &'static str, color: Rgba) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .text_size(px(11.))
        .child(
            div()
                .w(px(8.))
                .h(px(8.))
                .flex_shrink_0()
                .rounded(px(2.))
                .bg(color),
        )
        .child(name)
}

/// egui listed every key above its value with a rule between them. Its `HashMap` gave an
/// arbitrary order per run, so this sorts to keep the panel from reshuffling.
fn fields(entries: &HashMap<String, String>) -> Vec<Div> {
    let mut sorted: Vec<(&String, &String)> = entries.iter().collect();
    sorted.sort();
    sorted
        .into_iter()
        .flat_map(|(key, value)| vec![field(key, value), separator()])
        .collect()
}

fn field(key: &str, value: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(1.))
        .child(div().font_weight(FontWeight(700.)).child(key.to_string()))
        .child(div().child(value.to_string()))
}

/// egui's `Color32::to_hex`, which keeps the alpha byte.
pub fn hex([r, g, b, a]: [u8; 4]) -> String {
    format!("#{r:02X}{g:02X}{b:02X}{a:02X}")
}

fn row(glyph: &'static str, label: &'static str, value: impl IntoElement) -> Div {
    div()
        .w_full()
        .flex()
        .items_center()
        .child(label_cell(glyph, label))
        .child(value)
}

fn label_cell(glyph: &'static str, label: &'static str) -> Div {
    div()
        .w(px(LABEL))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(5.))
        .when(!glyph.is_empty(), |this| {
            this.child(
                div()
                    .w(px(15.))
                    .flex_shrink_0()
                    .text_center()
                    .text_size(px(13.))
                    .font_family(fonts::ICONS)
                    .child(glyph),
            )
        })
        .child(label)
}

fn value(text: String) -> Div {
    div().flex_1().truncate().text_right().child(text)
}

#[cfg(test)]
mod tests {
    // The panel's glob import of `gpui_kit` carries gpui's own `test` attribute macro, which
    // expands to `#[test]` and so recurses; name the items instead.
    use super::{PREVIEW, PREVIEW_RADIUS, preview_crop};

    /// egui scaled the preview by `rect.width() / (2*radius+1)` and anchored the neighbourhood's
    /// floored top-left source pixel to the square's top-left corner, whatever the image size.
    #[test]
    fn the_preview_anchors_the_neighbourhood_on_the_square() {
        let (px_per_src, slot) = preview_crop((100.7, 50.2), (512, 512));
        assert_eq!(PREVIEW_RADIUS * 2. + 1., 33.);
        assert!((px_per_src - PREVIEW / 33.).abs() < 1e-6);
        // src_left = 100 - 16 = 84, so the sprite is pushed left by 84 magnified pixels.
        assert!(
            (slot[0] + 84. * px_per_src).abs() < 1e-4,
            "slot[0] = {}",
            slot[0]
        );
        assert!((slot[1] + 34. * px_per_src).abs() < 1e-4);
        assert!((slot[2] - 512. * px_per_src).abs() < 1e-3);
        assert!((slot[3] - 512. * px_per_src).abs() < 1e-3);

        // Sitting on the neighbourhood's own edge, the crop starts at the image origin.
        let (_, flush) = preview_crop((16., 16.), (512, 512));
        assert_eq!(flush[0], 0.);
        assert_eq!(flush[1], 0.);

        // Near a corner the neighbourhood leaves the image, so the sprite starts inside the square
        // and egui's gray-30 filler keeps the margin.
        let (_, corner) = preview_crop((5., 5.), (512, 512));
        assert!(corner[0] > 0. && corner[1] > 0.);

        // A frame narrower than the neighbourhood does not stretch to fill it: px_per_src is fixed,
        // so the whole image lands as a band and the rest of the square stays filler.
        let (pps, tiny) = preview_crop((4., 4.), (8, 8));
        assert!((tiny[2] - 8. * pps).abs() < 1e-4);
        assert!(tiny[2] < PREVIEW);
    }

    /// A tiled frame has to land on the preview's source-pixel grid tile for tile, or the zoomed
    /// neighbourhood would show seams and misaligned pixels once the frame is split.
    #[test]
    fn a_tiled_frame_reconstructs_the_preview_grid() {
        use super::preview_sprites;
        use crate::gpui_app::tiles;
        use crate::utils::ColorChannel;
        use image::{DynamicImage, RgbaImage};

        let width = tiles::MAX_SPRITE * 2 + 7232;
        let frame = tiles::Frame::upload(
            &DynamicImage::ImageRgba8(RgbaImage::from_pixel(width, 4, image::Rgba([1, 2, 3, 4]))),
            ColorChannel::Rgba,
        );
        assert_eq!(frame.tile_count(), 3);

        let (px_per_src, slot) = preview_crop((16., 16.), (width, 4));
        let sprites = preview_sprites(&frame, px_per_src, slot);
        assert_eq!(sprites.len(), 3);

        let mut covered = 0.;
        for (index, (bounds, _)) in sprites.iter().enumerate() {
            let expected = index as f32 * tiles::MAX_SPRITE as f32 * px_per_src;
            assert!(
                (bounds.origin.x - expected).abs() < 1e-3,
                "tile {index} starts at {} instead of {expected}",
                bounds.origin.x
            );
            assert_eq!(bounds.origin.y, slot[1]);
            assert_eq!(bounds.size.height, 4. * px_per_src);
            covered += bounds.size.width;
        }
        // Contiguous: the union of the tiles is exactly the one-sprite rect the preview used to paint.
        assert!(
            (covered - slot[2]).abs() < 1e-2,
            "tiles cover {covered} of {} px",
            slot[2]
        );
        assert_eq!(
            sprites[0].1.as_bytes(0).map(<[u8]>::len),
            Some(tiles::MAX_SPRITE as usize * 4 * 4)
        );
    }
}
