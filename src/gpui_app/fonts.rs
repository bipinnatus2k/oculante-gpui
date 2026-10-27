use std::borrow::Cow;

use gpui_kit::App;
use log::debug;

use crate::{BOLD_FONT, FONT};

/// GPUI resolves fonts by the family name inside the file, so the bundled icon font is
/// `icomoon` here where egui knew it as its own registry key `icons`.
pub const ICONS: &str = "icomoon";

pub const BODY: &str = "Inter";

/// Registers the fonts oculante ships with. Must run before any view uses them.
pub fn load(cx: &mut App) {
    let text_system = cx.text_system();
    if let Err(e) = text_system.add_fonts(vec![
        Cow::Borrowed(FONT),
        Cow::Borrowed(BOLD_FONT),
        Cow::Borrowed(include_bytes!("../../res/fonts/icons.ttf")),
    ]) {
        debug!("bundled fonts failed to register: {e}");
    }
    let names = text_system.all_font_names();
    for family in [ICONS, BODY] {
        if !names.iter().any(|name| name == family) {
            debug!("font family {family:?} is not available");
        }
    }
}
