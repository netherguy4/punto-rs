//! Классификация скан-кодов клавиатуры (Linux input-event-codes.h).

pub const KEY_BACKSPACE: u16 = 14;
pub const KEY_TAB: u16 = 15;
pub const KEY_ENTER: u16 = 28;
pub const KEY_LEFTCTRL: u16 = 29;
pub const KEY_LEFTSHIFT: u16 = 42;
pub const KEY_RIGHTSHIFT: u16 = 54;
pub const KEY_LEFTALT: u16 = 56;
pub const KEY_SPACE: u16 = 57;
pub const KEY_KPENTER: u16 = 96;
pub const KEY_RIGHTCTRL: u16 = 97;
pub const KEY_RIGHTALT: u16 = 100;
pub const KEY_INSERT: u16 = 110;
pub const KEY_LEFTMETA: u16 = 125;
pub const KEY_RIGHTMETA: u16 = 126;

/// Клавиши, дающие печатный символ в обеих раскладках.
///
/// Диапазоны соответствуют основному блоку: цифровой ряд, три буквенных ряда
/// вместе с пунктуацией, которая в русской раскладке тоже отдаёт буквы
/// (`[` → `х`, `]` → `ъ`, `;` → `ж`, `'` → `э`, `` ` `` → `ё`, `,` → `б`, `.` → `ю`).
pub fn is_char(code: u16) -> bool {
    matches!(code, 2..=13 | 16..=27 | 30..=41 | 43 | 44..=53)
}

/// Разделители слов: их скан-коды одинаковы в любой раскладке,
/// поэтому при переигрывании они воспроизводятся как есть.
pub fn is_separator(code: u16) -> bool {
    matches!(code, KEY_SPACE | KEY_TAB)
}

/// Конец фразы — дальше буфер начинается заново.
pub fn is_phrase_end(code: u16) -> bool {
    matches!(code, KEY_ENTER | KEY_KPENTER)
}

pub fn is_shift(code: u16) -> bool {
    matches!(code, KEY_LEFTSHIFT | KEY_RIGHTSHIFT)
}

/// Ctrl/Alt/Meta — при зажатом любом из них нажатие считается сочетанием,
/// а не набором текста.
pub fn is_command_modifier(code: u16) -> bool {
    matches!(
        code,
        KEY_LEFTCTRL | KEY_RIGHTCTRL | KEY_LEFTALT | KEY_RIGHTALT | KEY_LEFTMETA | KEY_RIGHTMETA
    )
}

/// Клавиша из конфига: скан-код числом либо имя.
pub fn key_from_spec(spec: &str) -> Option<u16> {
    if let Ok(code) = spec.parse::<u16>() {
        return Some(code);
    }
    let code = match spec.to_ascii_lowercase().as_str() {
        "insert" | "ins" => KEY_INSERT,
        "pause" | "break" => 119,
        "scrolllock" => 70,
        "capslock" => 58,
        "menu" | "compose" => 127,
        "space" => KEY_SPACE,
        "tab" => KEY_TAB,
        "leftctrl" | "ctrl" => KEY_LEFTCTRL,
        "rightctrl" => KEY_RIGHTCTRL,
        "leftshift" | "shift" => KEY_LEFTSHIFT,
        "rightshift" => KEY_RIGHTSHIFT,
        "leftalt" | "alt" => KEY_LEFTALT,
        "rightalt" | "altgr" => KEY_RIGHTALT,
        "leftmeta" | "super" | "win" => KEY_LEFTMETA,
        "rightmeta" => KEY_RIGHTMETA,
        "f1" => 59,
        "f2" => 60,
        "f3" => 61,
        "f4" => 62,
        "f5" => 63,
        "f6" => 64,
        "f7" => 65,
        "f8" => 66,
        "f9" => 67,
        "f10" => 68,
        "f11" => 87,
        "f12" => 88,
        _ => return None,
    };
    Some(code)
}

/// Разбор комбинации вида `125+57` или `super+space` в список скан-кодов.
pub fn parse_combo(spec: &str) -> Option<Vec<u16>> {
    let codes: Vec<u16> = spec
        .split('+')
        .map(|part| key_from_spec(part.trim()))
        .collect::<Option<_>>()?;
    if codes.is_empty() {
        None
    } else {
        Some(codes)
    }
}
