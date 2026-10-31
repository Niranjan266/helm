//! Key name ↔ Windows virtual-key (VK) code mapping.
//!
//! Codes are from the Microsoft "Virtual-Key Codes" reference. Names are case-insensitive;
//! for multi-character names `_`, `-` and spaces are ignored, so `Page_Up`, `page-up` and
//! `pageup` are the same key.

/// (canonical name, aliases, vk). The first name is what [`vk_to_name`] returns.
const TABLE: &[(&str, &[&str], u16)] = &[
    ("backspace", &["back", "bksp"], 0x08),
    ("tab", &[], 0x09),
    ("clear", &[], 0x0C),
    ("enter", &["return"], 0x0D),
    ("shift", &[], 0x10),
    ("ctrl", &["control", "ctl"], 0x11),
    ("alt", &["menu", "option"], 0x12),
    ("pause", &["break"], 0x13),
    ("capslock", &["caps"], 0x14),
    ("esc", &["escape"], 0x1B),
    ("space", &["spacebar"], 0x20),
    ("pageup", &["pgup", "prior"], 0x21),
    ("pagedown", &["pgdn", "pgdown", "next"], 0x22),
    ("end", &[], 0x23),
    ("home", &[], 0x24),
    ("left", &["arrowleft"], 0x25),
    ("up", &["arrowup"], 0x26),
    ("right", &["arrowright"], 0x27),
    ("down", &["arrowdown"], 0x28),
    (
        "printscreen",
        &["prtsc", "prtscn", "snapshot", "print"],
        0x2C,
    ),
    ("insert", &["ins"], 0x2D),
    ("delete", &["del"], 0x2E),
    ("win", &["lwin", "super", "meta", "cmd", "windows"], 0x5B),
    ("rwin", &[], 0x5C),
    ("apps", &["contextmenu", "application"], 0x5D),
    ("sleep", &[], 0x5F),
    ("num0", &["numpad0"], 0x60),
    ("num1", &["numpad1"], 0x61),
    ("num2", &["numpad2"], 0x62),
    ("num3", &["numpad3"], 0x63),
    ("num4", &["numpad4"], 0x64),
    ("num5", &["numpad5"], 0x65),
    ("num6", &["numpad6"], 0x66),
    ("num7", &["numpad7"], 0x67),
    ("num8", &["numpad8"], 0x68),
    ("num9", &["numpad9"], 0x69),
    ("multiply", &["nummultiply", "numpadmultiply"], 0x6A),
    ("add", &["numadd", "numpadadd"], 0x6B),
    ("separator", &[], 0x6C),
    ("subtract", &["numsubtract", "numpadsubtract"], 0x6D),
    ("decimal", &["numdecimal", "numpaddecimal"], 0x6E),
    ("divide", &["numdivide", "numpaddivide"], 0x6F),
    ("numlock", &[], 0x90),
    ("scrolllock", &["scroll"], 0x91),
    ("lshift", &["leftshift"], 0xA0),
    ("rshift", &["rightshift"], 0xA1),
    ("lctrl", &["leftctrl", "lcontrol"], 0xA2),
    ("rctrl", &["rightctrl", "rcontrol"], 0xA3),
    ("lalt", &["leftalt", "lmenu"], 0xA4),
    ("ralt", &["rightalt", "rmenu", "altgr"], 0xA5),
    ("browserback", &[], 0xA6),
    ("browserforward", &[], 0xA7),
    ("browserrefresh", &[], 0xA8),
    ("browserstop", &[], 0xA9),
    ("browsersearch", &[], 0xAA),
    ("browserfavorites", &[], 0xAB),
    ("browserhome", &[], 0xAC),
    ("volumemute", &["mute"], 0xAD),
    ("volumedown", &["voldown"], 0xAE),
    ("volumeup", &["volup"], 0xAF),
    ("medianext", &["nexttrack", "medianexttrack"], 0xB0),
    (
        "mediaprev",
        &["prevtrack", "mediaprevtrack", "mediaprevious"],
        0xB1,
    ),
    ("mediastop", &[], 0xB2),
    ("mediaplaypause", &["playpause"], 0xB3),
    ("launchmail", &["mail"], 0xB4),
    ("launchmediaselect", &["mediaselect"], 0xB5),
    ("launchapp1", &[], 0xB6),
    ("launchapp2", &[], 0xB7),
    (";", &["semicolon"], 0xBA),
    ("=", &["equals", "equal", "plus"], 0xBB),
    (",", &["comma"], 0xBC),
    ("-", &["minus"], 0xBD),
    (".", &["period", "dot"], 0xBE),
    ("/", &["slash"], 0xBF),
    ("`", &["backquote", "backtick", "grave", "tilde"], 0xC0),
    ("[", &["bracketleft", "lbracket"], 0xDB),
    ("\\", &["backslash"], 0xDC),
    ("]", &["bracketright", "rbracket"], 0xDD),
    ("'", &["quote", "apostrophe"], 0xDE),
];

fn normalize(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.chars().count() <= 1 {
        return trimmed.to_ascii_lowercase();
    }
    trimmed
        .chars()
        .filter(|c| !matches!(c, '_' | '-' | ' '))
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Looks up a key name such as `"ctrl"`, `"f5"`, `"pageup"`, `"a"`, `"7"` or
/// `"mediaplaypause"`. Returns `None` for unknown names.
pub fn name_to_vk(name: &str) -> Option<u16> {
    let n = normalize(name);
    let mut chars = n.chars();
    if let (Some(c), None) = (chars.next(), chars.clone().next()) {
        if c.is_ascii_lowercase() {
            return Some(c.to_ascii_uppercase() as u16);
        }
        if c.is_ascii_digit() {
            return Some(c as u16);
        }
    }
    if let Some(num) = n.strip_prefix('f') {
        if !num.is_empty() && num.len() <= 2 && num.bytes().all(|b| b.is_ascii_digit()) {
            let i: u16 = num.parse().ok()?;
            return (1..=24).contains(&i).then(|| 0x70 + i - 1);
        }
    }
    TABLE
        .iter()
        .find(|(canon, aliases, _)| *canon == n || aliases.contains(&n.as_str()))
        .map(|&(_, _, vk)| vk)
}

/// Returns the canonical name of a virtual-key code (inverse of [`name_to_vk`]).
pub fn vk_to_name(vk: u16) -> Option<String> {
    match vk {
        0x30..=0x39 => Some(((vk as u8) as char).to_string()),
        0x41..=0x5A => Some(((vk as u8 + 32) as char).to_string()),
        0x70..=0x87 => Some(format!("f{}", vk - 0x70 + 1)),
        _ => TABLE.iter().find(|e| e.2 == vk).map(|e| e.0.to_string()),
    }
}

/// Parses a combination such as `"ctrl+shift+esc"` or `"ctrl++"` into VK codes, in order.
pub fn parse_shortcut(combo: &str) -> Option<Vec<u16>> {
    let combo = combo.trim();
    if combo.is_empty() {
        return None;
    }
    let mut keys = Vec::new();
    let mut rest = combo;
    loop {
        // A '+' directly at a separator position means the plus/equals key itself.
        let (part, tail) = match rest.find('+') {
            Some(0) => ("+", rest[1..].strip_prefix('+')),
            Some(i) => (&rest[..i], Some(&rest[i + 1..])),
            None => (rest, None),
        };
        let vk = if part == "+" { 0xBB } else { name_to_vk(part)? };
        keys.push(vk);
        match tail {
            Some(t) if !t.is_empty() => rest = t,
            Some(_) => return None, // trailing separator like "ctrl+"
            None => break,
        }
    }
    Some(keys)
}

/// Keys that need `KEYEVENTF_EXTENDEDKEY` (their scan code has the `E0` prefix).
pub fn is_extended_vk(vk: u16) -> bool {
    matches!(
        vk,
        0x21..=0x28      // page up/down, end, home, arrows
            | 0x2C       // print screen
            | 0x2D | 0x2E // insert, delete
            | 0x5B..=0x5D // windows keys, apps
            | 0x6F       // numpad divide
            | 0x90       // num lock
            | 0xA3 | 0xA5 // right ctrl, right alt
            | 0xA6..=0xB7 // browser, volume, media, launch keys
    )
}

/// True for modifier keys (shift/ctrl/alt/win, either side).
pub fn is_modifier_vk(vk: u16) -> bool {
    matches!(vk, 0x10..=0x12 | 0x5B | 0x5C | 0xA0..=0xA5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_names() {
        let cases = [
            ("ctrl", 0x11),
            ("Control", 0x11),
            ("alt", 0x12),
            ("shift", 0x10),
            ("win", 0x5B),
            ("enter", 0x0D),
            ("Return", 0x0D),
            ("esc", 0x1B),
            ("tab", 0x09),
            ("left", 0x25),
            ("pageup", 0x21),
            ("Page_Up", 0x21),
            ("page-down", 0x22),
            ("volumeup", 0xAF),
            ("volumedown", 0xAE),
            ("volumemute", 0xAD),
            ("mediaplaypause", 0xB3),
            ("medianext", 0xB0),
            ("space", 0x20),
            ("backspace", 0x08),
            ("delete", 0x2E),
            ("printscreen", 0x2C),
            ("-", 0xBD),
            ("\\", 0xDC),
        ];
        for (name, vk) in cases {
            assert_eq!(name_to_vk(name), Some(vk), "{name}");
        }
    }

    #[test]
    fn letters_digits_function_keys() {
        assert_eq!(name_to_vk("a"), Some(0x41));
        assert_eq!(name_to_vk("Z"), Some(0x5A));
        assert_eq!(name_to_vk("0"), Some(0x30));
        assert_eq!(name_to_vk("9"), Some(0x39));
        assert_eq!(name_to_vk("f1"), Some(0x70));
        assert_eq!(name_to_vk("F12"), Some(0x7B));
        assert_eq!(name_to_vk("f24"), Some(0x87));
        assert_eq!(name_to_vk("f0"), None);
        assert_eq!(name_to_vk("f25"), None);
        assert_eq!(name_to_vk("f"), Some(0x46));
    }

    #[test]
    fn unknown_names() {
        assert_eq!(name_to_vk(""), None);
        assert_eq!(name_to_vk("hyper"), None);
        assert_eq!(name_to_vk("é"), None);
        assert_eq!(name_to_vk("f1x"), None);
    }

    #[test]
    fn round_trip_every_known_vk() {
        let mut vks: Vec<u16> = TABLE.iter().map(|e| e.2).collect();
        vks.extend(0x30..=0x39);
        vks.extend(0x41..=0x5A);
        vks.extend(0x70..=0x87);
        for vk in vks {
            let name = vk_to_name(vk).unwrap_or_else(|| panic!("no name for {vk:#x}"));
            assert_eq!(name_to_vk(&name), Some(vk), "{name}");
        }
        assert_eq!(vk_to_name(0xFF), None);
    }

    #[test]
    fn table_has_no_duplicate_names() {
        let mut seen = std::collections::HashSet::new();
        for (canon, aliases, _) in TABLE {
            assert!(seen.insert(*canon), "{canon}");
            for a in *aliases {
                assert!(seen.insert(*a), "{a}");
                assert_eq!(normalize(a), *a, "aliases must be stored normalized: {a}");
            }
        }
    }

    #[test]
    fn shortcuts() {
        assert_eq!(
            parse_shortcut("ctrl+shift+esc"),
            Some(vec![0x11, 0x10, 0x1B])
        );
        assert_eq!(parse_shortcut("Win + D"), Some(vec![0x5B, 0x44]));
        assert_eq!(parse_shortcut("ctrl++"), Some(vec![0x11, 0xBB]));
        assert_eq!(parse_shortcut("alt+f4"), Some(vec![0x12, 0x73]));
        assert_eq!(parse_shortcut("ctrl+"), None);
        assert_eq!(parse_shortcut("ctrl+nope"), None);
        assert_eq!(parse_shortcut(""), None);
    }

    #[test]
    fn extended_and_modifiers() {
        assert!(is_extended_vk(0x25));
        assert!(is_extended_vk(0x2E));
        assert!(is_extended_vk(0xA3));
        assert!(is_extended_vk(0xB3));
        assert!(!is_extended_vk(0x41));
        assert!(!is_extended_vk(0x0D));
        assert!(!is_extended_vk(0xA2));
        assert!(is_modifier_vk(0x11));
        assert!(is_modifier_vk(0x5B));
        assert!(!is_modifier_vk(0x41));
    }
}
