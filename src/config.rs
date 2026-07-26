//! Конфиг в формате `ключ=значение` — тот же стиль, что был у Easy Switcher.

use std::fs;
use std::path::Path;

use crate::keys;

pub struct Config {
    /// Скан-код клавиши исправления.
    pub hotkey: u16,
    /// Комбинация переключения раскладки в системе (по умолчанию Super+Space).
    pub layout_switch: Vec<u16>,
    /// Пауза между отдельными нажатиями при переигрывании.
    pub key_delay_ms: u64,
    /// Пауза после стирания слова, до переключения раскладки.
    pub post_backspace_ms: u64,
    /// Пауза после переключения раскладки — GNOME применяет её не мгновенно.
    pub switch_delay_ms: u64,
    /// Явный список клавиатур по имени; пустой — автоопределение.
    pub devices: Vec<String>,
    /// Предел буфера в нажатиях.
    pub max_strokes: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hotkey: keys::KEY_INSERT,
            layout_switch: vec![keys::KEY_LEFTMETA, keys::KEY_SPACE],
            key_delay_ms: 6,
            post_backspace_ms: 20,
            switch_delay_ms: 120,
            devices: Vec::new(),
            max_strokes: 512,
        }
    }
}

impl Config {
    /// Читает конфиг, подставляя значения по умолчанию для всего,
    /// что не задано. Отсутствующий файл — не ошибка.
    pub fn load(path: &Path) -> (Config, Vec<String>) {
        let mut cfg = Config::default();
        let mut warnings = Vec::new();

        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) => {
                warnings.push(format!(
                    "конфиг {} не прочитан ({err}), используются значения по умолчанию",
                    path.display()
                ));
                return (cfg, warnings);
            }
        };

        for (lineno, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                warnings.push(format!("строка {}: нет '=', пропущена", lineno + 1));
                continue;
            };
            let key = key.trim();
            let value = value.trim().trim_matches('"').trim();

            let applied = match key {
                "hotkey" => value.parse::<u16>().map(|v| cfg.hotkey = v).is_ok(),
                "layout-switch" => match keys::parse_combo(value) {
                    Some(combo) => {
                        cfg.layout_switch = combo;
                        true
                    }
                    None => false,
                },
                "key-delay" => value.parse::<u64>().map(|v| cfg.key_delay_ms = v).is_ok(),
                "post-backspace-delay" => value
                    .parse::<u64>()
                    .map(|v| cfg.post_backspace_ms = v)
                    .is_ok(),
                "switch-delay" => value
                    .parse::<u64>()
                    .map(|v| cfg.switch_delay_ms = v)
                    .is_ok(),
                "max-strokes" => value.parse::<usize>().map(|v| cfg.max_strokes = v).is_ok(),
                "devices" => {
                    cfg.devices = value
                        .split(',')
                        .map(|d| d.trim().to_string())
                        .filter(|d| !d.is_empty())
                        .collect();
                    true
                }
                _ => {
                    warnings.push(format!("строка {}: неизвестный ключ '{key}'", lineno + 1));
                    continue;
                }
            };

            if !applied {
                warnings.push(format!(
                    "строка {}: не разобрано значение '{value}' для '{key}'",
                    lineno + 1
                ));
            }
        }

        (cfg, warnings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// `tag` делает имя файла уникальным: тесты идут параллельно в одном
    /// процессе и на общем пути затирали конфиг друг другу.
    fn parse(tag: &str, body: &str) -> (Config, Vec<String>) {
        let mut file = std::env::temp_dir();
        file.push(format!("punto-rs-test-{}-{tag}.conf", std::process::id()));
        let mut fh = fs::File::create(&file).unwrap();
        fh.write_all(body.as_bytes()).unwrap();
        drop(fh);
        let out = Config::load(&file);
        let _ = fs::remove_file(&file);
        out
    }

    #[test]
    fn parses_values_and_strips_comments() {
        let (cfg, warnings) = parse("basic", "hotkey=119 # Pause\nswitch-delay=200\n");
        assert_eq!(cfg.hotkey, 119);
        assert_eq!(cfg.switch_delay_ms, 200);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn parses_combo_and_device_list() {
        let (cfg, _) = parse(
            "combo",
            "layout-switch=29+42\ndevices=\"AT Translated Set 2 keyboard, Foo\"\n",
        );
        assert_eq!(cfg.layout_switch, vec![29, 42]);
        assert_eq!(cfg.devices, vec!["AT Translated Set 2 keyboard", "Foo"]);
    }

    #[test]
    fn missing_file_yields_defaults_with_warning() {
        let (cfg, warnings) = Config::load(Path::new("/nonexistent/punto-rs.conf"));
        assert_eq!(cfg.hotkey, keys::KEY_INSERT);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn bad_value_warns_and_keeps_default() {
        let (cfg, warnings) = parse("badvalue", "hotkey=абв\n");
        assert_eq!(cfg.hotkey, keys::KEY_INSERT);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn unknown_key_warns() {
        let (_, warnings) = parse("unknown", "reverse-mode=False\n");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("reverse-mode"));
    }
}
