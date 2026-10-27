/// egui's in-app file browser, and so the port: builds with `file_open` get the native dialog
/// instead and never compile this.
#[cfg(not(feature = "file_open"))]
pub mod browser;
pub mod checker;
pub mod crop;
pub mod edit_panel;
pub mod fonts;
pub mod info_panel;
pub mod keys;
pub mod measure;
pub mod modal;
pub mod native;
pub mod panel;
pub mod settings;
pub mod texture;
pub mod theme;
pub mod tiles;
pub mod toasts;
pub mod top_bar;
pub mod viewer;

use crate::appstate::OculanteState;
use crate::window_config::WindowSettings;
use gpui_kit::*;
use viewer::Viewer;

pub fn run(state: OculanteState, ws: WindowSettings) {
    application().run(move |cx: &mut App| {
        init(cx);
        fonts::load(cx);

        let bounds = Bounds::centered(None, size(px(ws.width as f32), px(ws.height as f32)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: ws.min_size.map(|(w, h)| size(px(w as f32), px(h as f32))),
            is_resizable: ws.resizable,
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from(ws.title.clone())),
                // Windows hides the OS title bar through this flag; `window_decorations` below is
                // only read on X11 and Wayland.
                appears_transparent: !ws.decorations,
                ..Default::default()
            }),
            window_decorations: Some(if ws.decorations {
                WindowDecorations::Server
            } else {
                WindowDecorations::Client
            }),
            ..Default::default()
        };

        // Both settings tables are persisted by `impl Drop for Viewer`, which is the one hook that
        // fires on every exit route: the Quit chord tears the root view down, and on Windows closing
        // the last window never reaches `App::shutdown()` at all, so an `App::on_app_quit` handler
        // would not run there.
        let (_handle, _viewer) = open_window(options, cx, move |window, cx| {
            cx.new(|cx| Viewer::new(state, window, cx))
        })
        .expect("failed to open the oculante window");
    });
}
