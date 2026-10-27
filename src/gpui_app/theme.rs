use gpui_kit::{App, Hsla, Rgba, WindowAppearance, rgb};

use crate::appstate::OculanteState;
use crate::settings::{ColorTheme, PersistentSettings};

/// The colours oculante derives from its own settings.
///
/// The egui shell reached these through `egui::Visuals`; GPUI has no equivalent global, so
/// the same values are resolved here from `PersistentSettings` on every frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    /// Behind the image.
    pub backdrop: Rgba,
    /// Panels and bars.
    pub panel: Rgba,
    pub text: Rgba,
    pub accent: Rgba,
    /// Text drawn on top of `accent`.
    pub accent_text: Rgba,
}

/// egui's `Color32::gamma_multiply`, which scales the sRGB bytes rather than linear light:
/// `(channel * factor + 0.5) as u8`. The `+ 0.5` is truncation rounding, and alpha is scaled too
/// in egui, but every colour the port tints here is opaque.
pub fn gamma_multiply(color: Rgba, factor: f32) -> Rgba {
    let scale = |channel: f32| {
        let scaled = channel * 255.0 * factor + 0.5;
        (scaled.clamp(0., 255.)) as u32
    };
    rgb((scale(color.r) << 16) | (scale(color.g) << 8) | scale(color.b))
}

fn gray(v: u8) -> Rgba {
    rgb(u32::from(v) * 0x00_01_01_01)
}

fn rgb3([r, g, b]: [u8; 3]) -> Rgba {
    rgb((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b))
}

pub fn resolve(state: &OculanteState, appearance: WindowAppearance) -> Palette {
    let dark = match state.persistent_settings.theme {
        ColorTheme::Light => false,
        ColorTheme::Dark => true,
        ColorTheme::System => {
            matches!(
                appearance,
                WindowAppearance::Dark | WindowAppearance::VibrantDark
            )
        }
    };

    let defaults = PersistentSettings::default();
    let settings = &state.persistent_settings;
    let mut background = settings.background_color;
    let mut accent = settings.accent_color;

    // A palette that was saved for the other theme is not a user choice; restore this
    // theme's default for it. Mirrors the egui shell, except nothing is written back.
    if dark {
        if !settings.background_color_is_custom && background == [200, 200, 200] {
            background = defaults.background_color;
        }
        if !settings.accent_color_is_custom && accent == [0, 170, 255] {
            accent = defaults.accent_color;
        }
    } else {
        if !settings.background_color_is_custom && background == defaults.background_color {
            background = [200, 200, 200];
        }
        if !settings.accent_color_is_custom && accent == defaults.accent_color {
            accent = [0, 170, 255];
        }
    }

    let luma = (f32::from(accent[0]) * 0.299
        + f32::from(accent[1]) * 0.587
        + f32::from(accent[2]) * 0.114) as u8;

    Palette {
        dark,
        backdrop: rgb3(background),
        panel: gray(if dark { 25 } else { 230 }),
        text: rgb3(if dark { [204, 204, 204] } else { [51, 51, 51] }),
        accent: rgb3(accent),
        accent_text: gray(if luma < 80 { 220 } else { 80 }),
    }
}

/// gpui-component's widgets (checkboxes, radios, selects, inputs) take their colours from its own
/// global theme, which is what egui's `Visuals` used to cover. Left at gpui's defaults a checked
/// box paints the default primary — near-black on oculante's dark panel — and an input paints
/// white, so the palette is copied across whenever it changes.
pub fn apply(palette: Palette, cx: &mut App) {
    use gpui_kit::component::theme::{Theme, ThemeMode};

    let border: Hsla = palette.text.opacity(0.12).into();
    let input: Hsla = palette.text.opacity(0.35).into();
    let accent: Hsla = palette.accent.into();
    let accent_text: Hsla = palette.accent_text.into();
    let panel: Hsla = palette.panel.into();
    let text: Hsla = palette.text.into();

    Theme::update(cx, |theme| {
        theme.mode = if palette.dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        };
    });
    // A mode change makes gpui-component reload that mode's registered config over the colours,
    // so the palette goes in on its own update, once the reload has happened.
    Theme::update(cx, |theme| {
        let colors = &mut theme.colors;
        colors.primary = accent;
        colors.primary_hover = accent;
        colors.primary_active = accent;
        colors.primary_foreground = accent_text;
        colors.accent = accent;
        colors.accent_foreground = accent_text;
        colors.ring = accent;
        colors.link = accent;
        colors.link_hover = accent;
        colors.link_active = accent;
        colors.background = panel;
        colors.popover = panel;
        colors.popover_foreground = text;
        colors.foreground = text;
        colors.muted = panel;
        colors.muted_foreground = text;
        colors.secondary = border;
        colors.secondary_hover = border;
        colors.secondary_active = border;
        colors.secondary_foreground = text;
        colors.border = border;
        colors.input = input;
    });
}

#[cfg(test)]
mod tests {
    use super::{gamma_multiply, rgb3};
    use gpui_kit::Rgba;

    fn bytes(color: Rgba) -> [u8; 3] {
        [
            (color.r * 255.0).round() as u8,
            (color.g * 255.0).round() as u8,
            (color.b * 255.0).round() as u8,
        ]
    }

    /// The numbers the egui shell painted its `{CLOCK} Recent` chips with, for the default dark
    /// accent: `gamma_multiply` is a byte-space scale, so these are exact rather than approximate.
    #[test]
    fn gamma_multiply_scales_bytes_the_way_egui_does() {
        let accent = rgb3([255, 0, 75]);
        assert_eq!(bytes(gamma_multiply(accent, 1.0)), [255, 0, 75]);
        assert_eq!(bytes(gamma_multiply(accent, 0.1)), [26, 0, 8]);
        assert_eq!(bytes(gamma_multiply(accent, 0.8)), [204, 0, 60]);
    }
}
