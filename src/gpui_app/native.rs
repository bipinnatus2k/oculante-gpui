//! Window-level calls gpui-pre does not expose — or exposes without them reaching the window.
//!
//! egui put the viewport above every other window with `egui::ViewportCommand::WindowLevel`.
//! gpui-pre 0.3.8 has no such API — its only `WS_EX_TOPMOST` is baked into popup creation, and
//! `PlatformWindow::get_raw_handle` is unreachable from `Window` — so this reaches for user32
//! directly. Everywhere else the level cannot be changed and the call reports `false`.
//!
//! Fullscreen lives here too, for a different reason: `Window::toggle_fullscreen` does exist, but its
//! Windows backend hands the style and rect change to a `ForegroundExecutor` future that never runs
//! in this app, so calling it leaves the window exactly as it was. The same two user32 calls, made
//! inline, are what egui's double-click and `F` chord need.

/// A top-level window as the OS describes it, gathered while enumerating.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowInfo {
    pub handle: isize,
    pub own_process: bool,
    pub visible: bool,
    /// `WS_EX_APPWINDOW`: the window is advertised in the taskbar.
    pub app_window: bool,
    /// `WS_EX_TOOLWINDOW`: a utility window, which is what gpui creates for popups.
    pub tool_window: bool,
}

/// The one window whose level egui's toggle applied to: ours, on screen, and the taskbar window
/// rather than one of the utility popups.
fn is_main_window(info: &WindowInfo) -> bool {
    info.own_process && info.visible && info.app_window && !info.tool_window
}

/// Ask the OS to keep the main window above the others. Returns whether it did, so the caller can
/// leave its own flag alone when the level could not be changed.
pub fn set_always_on_top(above: bool) -> bool {
    #[cfg(windows)]
    return win32::set(above);
    #[cfg(not(windows))]
    {
        let _ = above;
        false
    }
}

/// Show or hide the pointer. egui hid it with `CursorIcon::None` for the frames a zen-mode canvas
/// stayed untouched; gpui-pre 0.3.8's `CursorStyle` has no hidden variant, so the only way to get
/// the same effect is user32. Anywhere else the pointer cannot be hidden and the call does nothing.
pub fn set_cursor_visible(visible: bool) {
    #[cfg(windows)]
    win32::set_cursor_visible(visible);
    #[cfg(not(windows))]
    {
        let _ = visible;
    }
}

/// Whether the window is currently the chromeless, whole-monitor window this module made it.
pub fn is_fullscreen() -> bool {
    #[cfg(windows)]
    return win32::is_fullscreen();
    #[cfg(not(windows))]
    false
}

/// Whether `toggle_fullscreen` here is the one that moves the window, as opposed to always answering
/// `None` and leaving it to gpui. Lets a caller decide up front, without running the toggle.
pub const HANDLES_FULLSCREEN: bool = cfg!(windows);

/// Cover the monitor with the window, or put back the style and rect saved on the way in, and
/// report the new state. `None` says there was no window to restyle — which on every platform but
/// Windows is always, so the caller knows to use gpui's own toggle instead.
pub fn toggle_fullscreen() -> Option<bool> {
    #[cfg(windows)]
    return win32::toggle_fullscreen();
    #[cfg(not(windows))]
    None
}

#[cfg(windows)]
mod win32 {
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicUsize, Ordering};

    use log::{debug, error};

    use super::{WindowInfo, is_main_window};

    type Bool = i32;
    type DWord = u32;
    type LPARAM = isize;

    const GWL_EXSTYLE: i32 = -20;
    const GWL_STYLE: i32 = -16;
    const WS_EX_APPWINDOW: usize = 0x0004_0000;
    const WS_EX_TOOLWINDOW: usize = 0x0000_0080;
    const HWND_TOPMOST: isize = -1;
    const HWND_NOTOPMOST: isize = -2;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_FRAMECHANGED: u32 = 0x0020;
    const MONITOR_DEFAULTTONEAREST: DWord = 0x0000_0002;
    const WM_SIZE: DWord = 0x0005;
    const SIZE_RESTORED: usize = 0;

    // The five bits `gpui-pre-windows` clears on the way into fullscreen (window.rs:333-338), which
    // together are the whole of the standard chrome; the clip and visible bits are left alone so the
    // restyled window is still drawn.
    const WS_CAPTION: usize = 0x00C0_0000;
    const WS_SYSMENU: usize = 0x0008_0000;
    const WS_THICKFRAME: usize = 0x0004_0000;
    const WS_MINIMIZEBOX: usize = 0x0002_0000;
    const WS_MAXIMIZEBOX: usize = 0x0001_0000;
    const CHROME: usize = WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX;

    #[repr(C)]
    #[derive(Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    /// `MONITORINFO`, whose first field has to carry the struct's size for the call to work.
    #[repr(C)]
    struct MonitorInfo {
        size: DWord,
        monitor: Rect,
        work_area: Rect,
        flags: DWord,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcessId() -> DWord;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(
            callback: Option<unsafe extern "system" fn(isize, LPARAM) -> Bool>,
            param: LPARAM,
        ) -> Bool;
        fn GetWindowThreadProcessId(handle: isize, process_id: *mut DWord) -> DWord;
        fn GetClientRect(handle: isize, rect: *mut Rect) -> Bool;
        fn GetWindowRect(handle: isize, rect: *mut Rect) -> Bool;
        fn IsWindowVisible(handle: isize) -> Bool;
        fn MonitorFromWindow(handle: isize, flags: DWord) -> isize;
        fn GetMonitorInfoW(monitor: isize, info: *mut MonitorInfo) -> Bool;
        fn PostMessageW(handle: isize, message: DWord, wparam: usize, lparam: LPARAM) -> Bool;
        fn ShowCursor(show: Bool) -> i32;
        fn SetWindowPos(
            handle: isize,
            insert_after: isize,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> Bool;
    }

    // Only 64-bit user32 exports `GetWindowLongPtrW`; the 32-bit call is `GetWindowLongW`, which
    // still returns the whole extended style since every `WS_EX_*` bit fits in 24 of them.
    #[cfg(target_pointer_width = "64")]
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetWindowLongPtrW(handle: isize, index: i32) -> isize;
        fn SetWindowLongPtrW(handle: isize, index: i32, value: isize) -> isize;
    }

    #[cfg(not(target_pointer_width = "64"))]
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetWindowLongW(handle: isize, index: i32) -> i32;
        fn SetWindowLongW(handle: isize, index: i32, value: i32) -> i32;
    }

    fn ex_style(handle: isize) -> usize {
        #[cfg(target_pointer_width = "64")]
        let style = unsafe { GetWindowLongPtrW(handle, GWL_EXSTYLE) };
        #[cfg(not(target_pointer_width = "64"))]
        let style = unsafe { GetWindowLongW(handle, GWL_EXSTYLE) };
        style as usize
    }

    fn style(handle: isize) -> usize {
        #[cfg(target_pointer_width = "64")]
        let style = unsafe { GetWindowLongPtrW(handle, GWL_STYLE) };
        #[cfg(not(target_pointer_width = "64"))]
        let style = unsafe { GetWindowLongW(handle, GWL_STYLE) };
        style as usize
    }

    fn set_style(handle: isize, new_style: usize) {
        #[cfg(target_pointer_width = "64")]
        unsafe {
            SetWindowLongPtrW(handle, GWL_STYLE, new_style as isize)
        };
        #[cfg(not(target_pointer_width = "64"))]
        unsafe {
            SetWindowLongW(handle, GWL_STYLE, new_style as i32)
        };
    }

    struct Scan {
        process_id: DWord,
        windows: Vec<WindowInfo>,
    }

    /// Keeps enumerating: the whole desktop has to be walked before the taskbar window can be
    /// picked, since window order says nothing about which one is ours.
    unsafe extern "system" fn collect(handle: isize, param: LPARAM) -> Bool {
        // SAFETY: `param` is the address of the caller's `Scan`, and that frame stays alive for
        // the whole enumeration because `EnumWindows` only returns once the walk finished.
        let scan = unsafe { &mut *(param as *mut Scan) };
        let mut process_id = 0;
        unsafe { GetWindowThreadProcessId(handle, &mut process_id) };
        let style = ex_style(handle);
        scan.windows.push(WindowInfo {
            handle,
            own_process: process_id == scan.process_id,
            visible: unsafe { IsWindowVisible(handle) } != 0,
            app_window: style & WS_EX_APPWINDOW != 0,
            tool_window: style & WS_EX_TOOLWINDOW != 0,
        });
        1
    }

    /// The handle stays valid for the window's life, so it is looked up once and remembered.
    static MAIN: AtomicIsize = AtomicIsize::new(0);

    fn main_window() -> isize {
        let cached = MAIN.load(Ordering::Relaxed);
        if cached != 0 {
            return cached;
        }
        let mut scan = Scan {
            // SAFETY: takes no arguments and only reports the current process's id.
            process_id: unsafe { GetCurrentProcessId() },
            windows: Vec::new(),
        };
        // SAFETY: `collect` only writes through the `Scan` it is handed, which lives in this frame.
        unsafe { EnumWindows(Some(collect), &mut scan as *mut Scan as LPARAM) };
        let found = scan
            .windows
            .iter()
            .find(|info| is_main_window(info))
            .map_or(0, |info| info.handle);
        if found != 0 {
            MAIN.store(found, Ordering::Relaxed);
        }
        found
    }

    pub fn set(above: bool) -> bool {
        let handle = main_window();
        if handle == 0 {
            return false;
        }
        let insert_after = if above { HWND_TOPMOST } else { HWND_NOTOPMOST };
        // SAFETY: the handle came from the enumeration above and the flags ask Windows to move
        // neither the position nor the size and not to activate the window.
        let changed = unsafe {
            SetWindowPos(
                handle,
                insert_after,
                0,
                0,
                0,
                0,
                SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE,
            )
        } != 0;
        if !changed {
            MAIN.store(0, Ordering::Relaxed);
        }
        changed
    }

    /// `ShowCursor` moves a per-thread counter rather than setting a state: one `false` hides the
    /// pointer and a second one would have to be undone by two `true` calls, so the transitions are
    /// watched here and the counter is only touched when the wanted visibility changes.
    static CURSOR_VISIBLE: AtomicBool = AtomicBool::new(true);

    pub fn set_cursor_visible(visible: bool) {
        if CURSOR_VISIBLE.swap(visible, Ordering::Relaxed) == visible {
            return;
        }
        // SAFETY: `ShowCursor` only takes a boolean and reports the thread's new display count.
        unsafe { ShowCursor(visible as Bool) };
    }

    /// The style and outer rect the window had before it went fullscreen. Only the window's own
    /// thread reads or writes them, so atomics are enough, as they are for `MAIN` above.
    static FULLSCREEN: AtomicBool = AtomicBool::new(false);
    static SAVED_STYLE: AtomicUsize = AtomicUsize::new(0);
    static SAVED_RECT: [AtomicI32; 4] = [
        AtomicI32::new(0),
        AtomicI32::new(0),
        AtomicI32::new(0),
        AtomicI32::new(0),
    ];

    pub fn is_fullscreen() -> bool {
        FULLSCREEN.load(Ordering::Relaxed)
    }

    pub fn toggle_fullscreen() -> Option<bool> {
        let handle = main_window();
        if handle == 0 {
            error!("fullscreen: no taskbar window to restyle");
            return None;
        }
        let entering = !FULLSCREEN.load(Ordering::Relaxed);
        let (new_style, x, y, width, height) = if entering {
            let mut rect = Rect::default();
            // SAFETY: `handle` is the window found above and `rect` is this frame's own storage.
            if unsafe { GetWindowRect(handle, &mut rect) } == 0 {
                error!("fullscreen: no rect for handle {handle}");
                return None;
            }
            let mut info = MonitorInfo {
                size: size_of::<MonitorInfo>() as DWord,
                monitor: Rect::default(),
                work_area: Rect::default(),
                flags: 0,
            };
            // SAFETY: the monitor handle comes straight from `MonitorFromWindow`, where 0 means
            // there is none, and `info` declares its own size in `cbSize`.
            let monitor = unsafe { MonitorFromWindow(handle, MONITOR_DEFAULTTONEAREST) };
            if monitor == 0 || unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
                error!("fullscreen: no monitor for handle {handle}");
                return None;
            }
            let style = style(handle);
            SAVED_STYLE.store(style, Ordering::Relaxed);
            SAVED_RECT[0].store(rect.left, Ordering::Relaxed);
            SAVED_RECT[1].store(rect.top, Ordering::Relaxed);
            SAVED_RECT[2].store(rect.right - rect.left, Ordering::Relaxed);
            SAVED_RECT[3].store(rect.bottom - rect.top, Ordering::Relaxed);
            (
                style & !CHROME,
                info.monitor.left,
                info.monitor.top,
                info.monitor.right - info.monitor.left,
                info.monitor.bottom - info.monitor.top,
            )
        } else {
            (
                SAVED_STYLE.load(Ordering::Relaxed),
                SAVED_RECT[0].load(Ordering::Relaxed),
                SAVED_RECT[1].load(Ordering::Relaxed),
                SAVED_RECT[2].load(Ordering::Relaxed),
                SAVED_RECT[3].load(Ordering::Relaxed),
            )
        };

        set_style(handle, new_style);
        // SAFETY: the handle is the window found by the enumeration, `insert_after` is ignored under
        // `SWP_NOZORDER`, and the flags keep the restyled window from stealing activation.
        let moved = unsafe {
            SetWindowPos(
                handle,
                0,
                x,
                y,
                width,
                height,
                SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOZORDER,
            )
        } != 0;
        // The re-read is the whole point of this line: `SetWindowPos` reports success even when
        // something else puts the window back, and gpui's own toggle was already silently losing the
        // change once.
        let mut after = Rect::default();
        // SAFETY: same handle and same frame-local storage as the call above.
        if unsafe { GetWindowRect(handle, &mut after) } != 0 {
            debug!(
                "fullscreen: {handle} asked for {x} {y} {width} {height}, got {} {} {} {}",
                after.left,
                after.top,
                after.right - after.left,
                after.bottom - after.top
            );
        }
        if !moved {
            error!("fullscreen: SetWindowPos failed on {handle}");
            return None;
        }
        // gpui learns a window changed size only from the `WM_SIZE` its own message loop services.
        // The one the call above triggers is delivered on this stack, where gpui's app state is
        // already borrowed, and is lost to a `RefCell already borrowed` panic — two of them per
        // toggle in the log, with and without this module, so it is not the restyling that causes it.
        // The window then covers the monitor while the surface keeps its old size: a fullscreened
        // 1920-wide window painted 1777 columns (`D:\tmp\oculante-gpui\fsbar.ps1`, 2026-10-15).
        // Posting the same message lets the loop pick it up at a point where nothing is borrowed.
        let mut client = Rect::default();
        // SAFETY: same handle and frame-local storage as the reads above.
        if unsafe { GetClientRect(handle, &mut client) } != 0 {
            let lparam = (((client.bottom & 0xffff) << 16) | (client.right & 0xffff)) as LPARAM;
            // SAFETY: `PostMessageW` only queues to this window's own loop, and `WM_SIZE` carries its
            // geometry in the parameters rather than reading anything from the pointer.
            unsafe { PostMessageW(handle, WM_SIZE, SIZE_RESTORED, lparam) };
        }
        FULLSCREEN.store(entering, Ordering::Relaxed);
        Some(entering)
    }

    #[cfg(test)]
    mod tests {
        use super::CHROME;

        /// The style the live oculante window was measured carrying before a toggle
        /// (`D:\tmp\oculante-gpui\fsbar.ps1`, 2026-10-15): `WS_VISIBLE | WS_CLIPSIBLINGS` over the
        /// standard chrome.
        const WINDOW: usize = 0x14CF_0000;

        /// What that same window was measured carrying while fullscreened, on the same run.
        const FULLSCREENED: usize = 0x1400_0000;

        /// Taking the chrome off has to leave every other bit alone, or the window that comes back
        /// is not the one that went in: hidden, unclipped, or both.
        #[test]
        fn the_fullscreen_style_drops_the_chrome_and_nothing_else() {
            let stripped = WINDOW & !CHROME;
            assert_eq!(stripped | CHROME, WINDOW);
            assert_eq!(stripped, FULLSCREENED);
            assert_eq!(stripped & 0x1400_0000, 0x1400_0000);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{WindowInfo, is_main_window};

    const fn info(
        own_process: bool,
        visible: bool,
        app_window: bool,
        tool_window: bool,
    ) -> WindowInfo {
        WindowInfo {
            handle: 1,
            own_process,
            visible,
            app_window,
            tool_window,
        }
    }

    /// The taskbar window of this process is the only one the level may be applied to: another
    /// process's window, a hidden helper, a gpui popup or a minimized window all have to be passed
    /// over, or the toggle would silently restyle something the user never sees.
    #[test]
    fn only_the_taskbar_window_of_this_process_is_selected() {
        assert!(is_main_window(&info(true, true, true, false)));

        // A popup gpui opened for a menu: visible and ours, but a utility window.
        assert!(!is_main_window(&info(true, true, true, true)));
        // The taskbar flag is what separates the main window from an unstyled helper window.
        assert!(!is_main_window(&info(true, true, false, false)));
        // Minimized and hidden windows still carry WS_EX_APPWINDOW, but are not on screen.
        assert!(!is_main_window(&info(true, false, true, false)));
        // Zed's own window looks exactly like ours otherwise.
        assert!(!is_main_window(&info(false, true, true, false)));
    }
}
