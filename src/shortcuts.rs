use log::{debug, error, info, warn};
use std::collections::{BTreeMap, BTreeSet};

use crate::appstate::OculanteState;
use serde::{Deserialize, Serialize};

/// Keyboard keys.
///
/// Mirrors the key names that were persisted in `config.json` while the UI was egui-based,
/// so existing user shortcut mappings keep deserializing.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash, Serialize, Deserialize)]
pub enum Key {
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    Escape,
    Tab,
    Backspace,
    Enter,
    Space,
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Copy,
    Cut,
    Paste,
    Colon,
    Comma,
    Backslash,
    Slash,
    Pipe,
    Questionmark,
    Exclamationmark,
    OpenBracket,
    CloseBracket,
    OpenCurlyBracket,
    CloseCurlyBracket,
    Backtick,
    Minus,
    Period,
    Plus,
    Equals,
    Semicolon,
    Quote,
    Num0,
    Num1,
    Num2,
    Num3,
    Num4,
    Num5,
    Num6,
    Num7,
    Num8,
    Num9,
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    F13,
    F14,
    F15,
    F16,
    F17,
    F18,
    F19,
    F20,
    F21,
    F22,
    F23,
    F24,
    F25,
    F26,
    F27,
    F28,
    F29,
    F30,
    F31,
    F32,
    F33,
    F34,
    F35,
    BrowserBack,
    /// egui's `Key` had variants this build cannot represent (multimedia, `Menu`, `Select`, `Match`,
    /// `Undo`, `Redo`, lock keys, …). Without a catch-all one of those names in an existing
    /// `config.json` fails the whole `PersistentSettings` read, and `OculanteState::default()` turns
    /// that into `unwrap_or_default()` — every preference and the entire keymap silently reset.
    /// An unrepresentable binding now degrades to an unbound key instead.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct Modifiers {
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub mac_cmd: bool,
    /// Ctrl on non-Mac, Cmd on Mac.
    pub command: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        alt: false,
        ctrl: false,
        shift: false,
        mac_cmd: false,
        command: false,
    };
    pub const ALT: Self = Self {
        alt: true,
        ..Self::NONE
    };
    pub const CTRL: Self = Self {
        ctrl: true,
        command: true,
        ..Self::NONE
    };
    pub const SHIFT: Self = Self {
        shift: true,
        ..Self::NONE
    };
    pub const MAC_CMD: Self = Self {
        mac_cmd: true,
        command: true,
        ..Self::NONE
    };

    fn command_or_mac(&self) -> bool {
        self.mac_cmd || self.command
    }
}

/// A key press, independent of any UI toolkit.
///
/// GPUI only reports presses (there are no key-release keystroke events), so press
/// is the only state a shortcut can match on.
#[derive(Clone, Copy, Debug)]
pub struct KeyEvent {
    pub key: Key,
    pub modifiers: Modifiers,
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy, Serialize, Deserialize, PartialOrd, Ord)]
pub enum InputEvent {
    AlwaysOnTop,
    Fullscreen,
    InfoMode,
    EditMode,
    NextImage,
    FirstImage,
    LastImage,
    PreviousImage,
    RedChannel,
    GreenChannel,
    BlueChannel,
    AlphaChannel,
    RGBChannel,
    RGBAChannel,
    ResetView,
    ZoomOut,
    ZoomIn,
    ZoomActualSize,
    ZoomDouble,
    ZoomThree,
    ZoomFour,
    ZoomFive,
    CompareNext,
    PanLeft,
    PanRight,
    PanUp,
    PanDown,
    DeleteFile,
    ClearImage,
    LosslessRotateRight,
    LosslessRotateLeft,
    Copy,
    Paste,
    Browse,
    Quit,
    ZenMode,
    ScrubBar,
    CopyPath,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Shortcut {
    pub keys: BTreeSet<Key>,
    pub modifiers: Modifiers,
}

impl Shortcut {
    fn key(k: Key) -> Self {
        Self {
            keys: BTreeSet::from([k]),
            modifiers: Modifiers::NONE,
        }
    }

    fn ctrl_key(k: Key) -> Self {
        #[cfg(not(target_os = "macos"))]
        let modifiers = Modifiers::CTRL;
        #[cfg(target_os = "macos")]
        let modifiers = Modifiers::MAC_CMD;
        Self {
            keys: BTreeSet::from([k]),
            modifiers,
        }
    }

    fn ctrl_shift_key(k: Key) -> Self {
        #[cfg(not(target_os = "macos"))]
        let modifiers = Modifiers::CTRL;
        #[cfg(target_os = "macos")]
        let modifiers = Modifiers::MAC_CMD;
        Self {
            keys: BTreeSet::from([k]),
            modifiers: Modifiers {
                shift: true,
                ..modifiers
            },
        }
    }

    fn shift_key(k: Key) -> Self {
        Self {
            keys: BTreeSet::from([k]),
            modifiers: Modifiers::SHIFT,
        }
    }

    fn triggered_by(&self, event: &KeyEvent) -> bool {
        self.keys.iter().any(|key| *key == event.key)
            && event.modifiers.shift == self.modifiers.shift
            && event.modifiers.alt == self.modifiers.alt
            && event.modifiers.ctrl == self.modifiers.ctrl
            && event.modifiers.command_or_mac() == self.modifiers.command_or_mac()
    }

    /// Human-readable display string
    pub fn to_string_pretty(&self) -> String {
        let mut parts = Vec::new();
        if self.modifiers.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.modifiers.command_or_mac() {
            parts.push("Cmd".to_string());
        }
        if self.modifiers.alt {
            parts.push("Alt".to_string());
        }
        if self.modifiers.shift {
            parts.push("Shift".to_string());
        }
        for k in &self.keys {
            parts.push(format!("{k:?}"));
        }
        parts.join(" + ")
    }

    /// Markdown version for documentation
    pub fn to_markdown(&self) -> String {
        let mut parts = Vec::new();
        if self.modifiers.ctrl {
            parts.push("<kbd>Ctrl</kbd>".to_string());
        }
        if self.modifiers.command_or_mac() {
            parts.push("<kbd>Cmd</kbd>".to_string());
        }
        if self.modifiers.alt {
            parts.push("<kbd>Alt</kbd>".to_string());
        }
        if self.modifiers.shift {
            parts.push("<kbd>Shift</kbd>".to_string());
        }
        for k in &self.keys {
            parts.push(format!("<kbd>{k:?}</kbd>"));
        }
        parts.join(" + ")
    }
}

pub type Shortcuts = BTreeMap<InputEvent, Shortcut>;

pub fn default_shortcuts() -> Shortcuts {
    use Key::*;
    let mut s = Shortcuts::new();
    s.insert(InputEvent::AlwaysOnTop, Shortcut::key(T));
    s.insert(InputEvent::Fullscreen, Shortcut::key(F));
    s.insert(InputEvent::ResetView, Shortcut::key(V));
    s.insert(InputEvent::Quit, Shortcut::key(Q));
    s.insert(InputEvent::InfoMode, Shortcut::key(I));
    s.insert(InputEvent::EditMode, Shortcut::key(E));
    s.insert(InputEvent::RedChannel, Shortcut::key(R));
    s.insert(InputEvent::GreenChannel, Shortcut::key(G));
    s.insert(InputEvent::BlueChannel, Shortcut::key(B));
    s.insert(InputEvent::AlphaChannel, Shortcut::key(A));
    s.insert(InputEvent::RGBChannel, Shortcut::key(U));
    s.insert(InputEvent::RGBAChannel, Shortcut::key(C));
    s.insert(InputEvent::CompareNext, Shortcut::shift_key(C));
    s.insert(InputEvent::PreviousImage, Shortcut::key(ArrowLeft));
    s.insert(InputEvent::FirstImage, Shortcut::key(Home));
    s.insert(InputEvent::LastImage, Shortcut::key(End));
    s.insert(InputEvent::NextImage, Shortcut::key(ArrowRight));
    s.insert(InputEvent::ZoomIn, Shortcut::key(Equals));
    s.insert(InputEvent::ZoomOut, Shortcut::key(Minus));
    s.insert(InputEvent::ZoomActualSize, Shortcut::key(Num1));
    s.insert(InputEvent::ZoomDouble, Shortcut::key(Num2));
    s.insert(InputEvent::ZoomThree, Shortcut::key(Num3));
    s.insert(InputEvent::ZoomFour, Shortcut::key(Num4));
    s.insert(InputEvent::ZoomFive, Shortcut::key(Num5));
    s.insert(InputEvent::LosslessRotateLeft, Shortcut::key(OpenBracket));
    s.insert(InputEvent::LosslessRotateRight, Shortcut::key(CloseBracket));
    s.insert(InputEvent::ZenMode, Shortcut::key(Z));
    s.insert(InputEvent::ScrubBar, Shortcut::key(S));
    s.insert(InputEvent::DeleteFile, Shortcut::key(Delete));
    s.insert(InputEvent::ClearImage, Shortcut::shift_key(Delete));
    s.insert(InputEvent::Browse, Shortcut::ctrl_key(O));
    s.insert(InputEvent::PanRight, Shortcut::shift_key(ArrowRight));
    s.insert(InputEvent::PanLeft, Shortcut::shift_key(ArrowLeft));
    s.insert(InputEvent::PanDown, Shortcut::shift_key(ArrowDown));
    s.insert(InputEvent::PanUp, Shortcut::shift_key(ArrowUp));
    s.insert(InputEvent::Paste, Shortcut::ctrl_key(V));
    s.insert(InputEvent::Copy, Shortcut::ctrl_key(C));
    s.insert(InputEvent::CopyPath, Shortcut::ctrl_shift_key(C));
    s
}

/// Check whether a shortcut is triggered by the key events received this update.
/// Missing entries are repaired from the defaults, as with the previous egui-based matcher.
pub fn key_pressed(state: &mut OculanteState, events: &[KeyEvent], command: InputEvent) -> bool {
    if state.key_grab {
        return false;
    }

    let shortcut = match state.persistent_settings.shortcuts.get(&command) {
        Some(s) => s,
        None => {
            warn!("Command not registered: '{:?}'", command);
            if let Some(default) = default_shortcuts().get(&command) {
                info!("Inserted default shortcut for: {:?}", command);
                state
                    .persistent_settings
                    .shortcuts
                    .insert(command, default.clone());
            } else {
                error!(
                    "No default shortcut for {:?}. Please report this as a bug.",
                    command
                );
            }
            return false;
        }
    };

    let shortcut = shortcut.clone();

    let matched = shortcut.keys.iter().all(|key| {
        events
            .iter()
            .any(|e| e.key == *key && shortcut.triggered_by(e))
    });

    if !matched {
        return false;
    }

    debug!("Matched shortcut {:?}", command);
    true
}

pub fn lookup(shortcuts: &Shortcuts, command: &InputEvent) -> String {
    if let Some(shortcut) = shortcuts.get(command) {
        return shortcut.to_string_pretty();
    }
    "None".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// egui persisted key names this build cannot represent. Reading one used to fail the whole
    /// `PersistentSettings` deserialization, and `OculanteState::default()`'s `unwrap_or_default()`
    /// turned that error into a reset of every preference and the entire keymap.
    #[test]
    fn an_unrepresentable_egui_key_degrades_instead_of_failing() {
        assert_eq!(
            serde_json::from_str::<Key>("\"Menu\"").unwrap(),
            Key::Unknown
        );
        assert_eq!(
            serde_json::from_str::<Key>("\"MediaPlayPause\"").unwrap(),
            Key::Unknown
        );
        assert_eq!(
            serde_json::from_str::<Key>("\"ScrollLock\"").unwrap(),
            Key::Unknown
        );
        assert_eq!(serde_json::from_str::<Key>("\"Home\"").unwrap(), Key::Home);
        assert_eq!(
            serde_json::from_str::<Key>("\"BrowserBack\"").unwrap(),
            Key::BrowserBack
        );
    }

    #[test]
    fn pretty_hotkey_labels_are_just_the_key_name() {
        let shortcuts = default_shortcuts();
        assert_eq!(lookup(&shortcuts, &InputEvent::ResetView), "V");
        assert_eq!(lookup(&shortcuts, &InputEvent::InfoMode), "I");
        assert_eq!(lookup(&shortcuts, &InputEvent::RGBAChannel), "C");
        assert_eq!(lookup(&shortcuts, &InputEvent::PreviousImage), "ArrowLeft");
    }
}

pub fn keypresses_as_string(shortcut: &Shortcut) -> String {
    shortcut.to_string_pretty()
}

pub fn keypresses_as_markdown(shortcut: &Shortcut) -> String {
    shortcut.to_markdown()
}
