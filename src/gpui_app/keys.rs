use crate::shortcuts::{Key, Modifiers};

const LETTERS: [(char, Key); 26] = [
    ('a', Key::A),
    ('b', Key::B),
    ('c', Key::C),
    ('d', Key::D),
    ('e', Key::E),
    ('f', Key::F),
    ('g', Key::G),
    ('h', Key::H),
    ('i', Key::I),
    ('j', Key::J),
    ('k', Key::K),
    ('l', Key::L),
    ('m', Key::M),
    ('n', Key::N),
    ('o', Key::O),
    ('p', Key::P),
    ('q', Key::Q),
    ('r', Key::R),
    ('s', Key::S),
    ('t', Key::T),
    ('u', Key::U),
    ('v', Key::V),
    ('w', Key::W),
    ('x', Key::X),
    ('y', Key::Y),
    ('z', Key::Z),
];

const DIGITS: [(char, Key); 10] = [
    ('0', Key::Num0),
    ('1', Key::Num1),
    ('2', Key::Num2),
    ('3', Key::Num3),
    ('4', Key::Num4),
    ('5', Key::Num5),
    ('6', Key::Num6),
    ('7', Key::Num7),
    ('8', Key::Num8),
    ('9', Key::Num9),
];

pub fn to_modifiers(mods: &gpui_kit::Modifiers) -> Modifiers {
    // oculante's `command` means "the primary accelerator modifier": Ctrl off macOS, Cmd on it.
    // gpui keeps those two apart, so folding in its `secondary` is what makes Ctrl-chords match.
    Modifiers {
        alt: mods.alt,
        ctrl: mods.control,
        shift: mods.shift,
        mac_cmd: cfg!(target_os = "macos") && mods.platform,
        command: mods.secondary(),
    }
}

/// Map GPUI's textual key representation onto the key names persisted in the config file.
pub fn to_key(key: &str, key_char: Option<&str>) -> Option<Key> {
    let mapped = match key {
        "escape" => Key::Escape,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "enter" | "return" => Key::Enter,
        "space" => Key::Space,
        "insert" => Key::Insert,
        "delete" | "backdelete" => Key::Delete,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "arrowleft" | "left" => Key::ArrowLeft,
        "arrowright" | "right" => Key::ArrowRight,
        "arrowup" | "up" => Key::ArrowUp,
        "arrowdown" | "down" => Key::ArrowDown,
        "colon" => Key::Colon,
        "semicolon" => Key::Semicolon,
        "comma" => Key::Comma,
        "period" | "full stop" => Key::Period,
        "backslash" => Key::Backslash,
        "slash" | "divide" => Key::Slash,
        "pipe" => Key::Pipe,
        "questionmark" | "question" => Key::Questionmark,
        "exclamationmark" | "exclamation" => Key::Exclamationmark,
        "openbracket" | "left bracket" | "[" => Key::OpenBracket,
        "closebracket" | "right bracket" | "]" => Key::CloseBracket,
        "opencurlybracket" | "{" => Key::OpenCurlyBracket,
        "closecurlybracket" | "}" => Key::CloseCurlyBracket,
        "backtick" => Key::Backtick,
        "minus" | "subtract" | "-" => Key::Minus,
        "equals" | "add" | "+" | "=" => Key::Equals,
        "quote" | "apostrophe" => Key::Quote,
        "copy" => Key::Copy,
        "cut" => Key::Cut,
        "paste" => Key::Paste,
        other => {
            if let Some(f) = other.strip_prefix('f')
                && let Ok(n) = f.parse::<u32>()
                && (1..=35).contains(&n)
            {
                return Some(f_key(n));
            } else {
                let character = key_char
                    .or(other.chars().next().map(|_| other))
                    .and_then(|c| c.chars().next())
                    .map(|c| c.to_ascii_lowercase());
                let Some(character) = character else {
                    return None;
                };
                if other.len() > 1 {
                    return None;
                }
                return character_to_key(character);
            }
        }
    };
    Some(mapped)
}

fn f_key(n: u32) -> Key {
    match n {
        1 => Key::F1,
        2 => Key::F2,
        3 => Key::F3,
        4 => Key::F4,
        5 => Key::F5,
        6 => Key::F6,
        7 => Key::F7,
        8 => Key::F8,
        9 => Key::F9,
        10 => Key::F10,
        11 => Key::F11,
        12 => Key::F12,
        13 => Key::F13,
        14 => Key::F14,
        15 => Key::F15,
        16 => Key::F16,
        17 => Key::F17,
        18 => Key::F18,
        19 => Key::F19,
        20 => Key::F20,
        21 => Key::F21,
        22 => Key::F22,
        23 => Key::F23,
        24 => Key::F24,
        25 => Key::F25,
        26 => Key::F26,
        27 => Key::F27,
        28 => Key::F28,
        29 => Key::F29,
        30 => Key::F30,
        31 => Key::F31,
        32 => Key::F32,
        33 => Key::F33,
        34 => Key::F34,
        _ => Key::F35,
    }
}

fn character_to_key(c: char) -> Option<Key> {
    LETTERS
        .iter()
        .chain(DIGITS.iter())
        .find(|(ch, _)| *ch == c)
        .map(|(_, key)| *key)
        .or_else(|| {
            Some(match c {
                '[' => Key::OpenBracket,
                ']' => Key::CloseBracket,
                '{' => Key::OpenCurlyBracket,
                '}' => Key::CloseCurlyBracket,
                ';' => Key::Semicolon,
                ',' => Key::Comma,
                '.' => Key::Period,
                '/' => Key::Slash,
                '\\' => Key::Backslash,
                '|' => Key::Pipe,
                ':' => Key::Colon,
                '?' => Key::Questionmark,
                '!' => Key::Exclamationmark,
                '`' => Key::Backtick,
                '\'' => Key::Quote,
                '-' => Key::Minus,
                '+' | '=' => Key::Equals,
                _ => return None,
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpui(control: bool, shift: bool, platform: bool) -> gpui_kit::Modifiers {
        gpui_kit::Modifiers {
            control,
            shift,
            platform,
            ..Default::default()
        }
    }

    /// The default bindings put Ctrl-chords under `command`, so a Ctrl press has to light it up
    /// off macOS — otherwise Copy, Paste and CopyPath silently never fire.
    #[test]
    fn ctrl_is_the_command_modifier_off_macos() {
        let mods = to_modifiers(&gpui(true, false, false));
        assert!(mods.ctrl);
        if cfg!(not(target_os = "macos")) {
            assert!(mods.command);
        } else {
            assert!(!mods.command);
        }
    }

    #[test]
    fn platform_key_is_command_on_macos_only() {
        let mods = to_modifiers(&gpui(false, false, true));
        assert_eq!(mods.command, cfg!(target_os = "macos"));
        assert_eq!(mods.mac_cmd, cfg!(target_os = "macos"));
        assert!(!mods.ctrl);
    }

    #[test]
    fn bare_press_sets_nothing() {
        let mods = to_modifiers(&gpui(false, false, false));
        assert_eq!(mods, Modifiers::default());
    }

    #[test]
    fn letters_and_named_keys_map() {
        assert_eq!(to_key("c", None), Some(Key::C));
        assert_eq!(to_key("C", None), Some(Key::C));
        assert_eq!(to_key("v", Some("V")), Some(Key::V));
        assert_eq!(to_key("left", None), Some(Key::ArrowLeft));
        assert_eq!(to_key("arrowleft", None), Some(Key::ArrowLeft));
        assert_eq!(to_key("closebracket", None), Some(Key::CloseBracket));
        assert_eq!(to_key("delete", None), Some(Key::Delete));
        assert_eq!(to_key("f12", None), Some(Key::F12));
    }

    #[test]
    fn multi_key_names_are_not_letters() {
        assert_eq!(to_key("ctrl-c", None), None);
    }
}
