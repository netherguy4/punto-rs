//! Виртуальная клавиатура uinput: стирает набранное и печатает заново.

use std::io;
use std::thread;
use std::time::Duration;

use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
use evdev::{AttributeSet, EventType, InputEvent, Key};

use crate::config::Config;
use crate::keys;
use crate::state::Stroke;

pub struct Injector {
    device: VirtualDevice,
}

impl Injector {
    pub fn new(name: &str) -> io::Result<Self> {
        let mut set = AttributeSet::<Key>::new();
        // Весь основной блок клавиатуры — заявляем сразу, чтобы не пересоздавать
        // устройство под каждую раскладку.
        for code in 1..=255u16 {
            set.insert(Key::new(code));
        }
        let device = VirtualDeviceBuilder::new()?
            .name(name)
            .with_keys(&set)?
            .build()?;
        Ok(Injector { device })
    }

    fn emit(&mut self, code: u16, value: i32) -> io::Result<()> {
        self.device
            .emit(&[InputEvent::new(EventType::KEY, code, value)])
    }

    fn tap(&mut self, code: u16, delay: Duration) -> io::Result<()> {
        self.emit(code, 1)?;
        thread::sleep(delay);
        self.emit(code, 0)?;
        thread::sleep(delay);
        Ok(())
    }

    /// Снимает модификаторы, которые пользователь может держать физически
    /// (например Shift при Shift+Insert) — иначе они исказят набор.
    fn release_modifiers(&mut self) -> io::Result<()> {
        for code in [
            keys::KEY_LEFTSHIFT,
            keys::KEY_RIGHTSHIFT,
            keys::KEY_LEFTCTRL,
            keys::KEY_RIGHTCTRL,
            keys::KEY_LEFTALT,
            keys::KEY_RIGHTALT,
            keys::KEY_LEFTMETA,
            keys::KEY_RIGHTMETA,
        ] {
            self.emit(code, 0)?;
        }
        Ok(())
    }

    fn combo(&mut self, codes: &[u16], delay: Duration) -> io::Result<()> {
        for &code in codes {
            self.emit(code, 1)?;
            thread::sleep(delay);
        }
        for &code in codes.iter().rev() {
            self.emit(code, 0)?;
            thread::sleep(delay);
        }
        Ok(())
    }

    /// Стереть `strokes`, переключить раскладку и набрать те же клавиши заново.
    pub fn fix(&mut self, strokes: &[Stroke], cfg: &Config) -> io::Result<()> {
        let delay = Duration::from_millis(cfg.key_delay_ms);

        self.release_modifiers()?;
        thread::sleep(delay);

        for _ in 0..strokes.len() {
            self.tap(keys::KEY_BACKSPACE, delay)?;
        }
        thread::sleep(Duration::from_millis(cfg.post_backspace_ms));

        self.combo(&cfg.layout_switch, delay)?;
        thread::sleep(Duration::from_millis(cfg.switch_delay_ms));

        for stroke in strokes {
            if stroke.shift {
                self.emit(keys::KEY_LEFTSHIFT, 1)?;
                thread::sleep(delay);
            }
            self.tap(stroke.code, delay)?;
            if stroke.shift {
                self.emit(keys::KEY_LEFTSHIFT, 0)?;
                thread::sleep(delay);
            }
        }
        Ok(())
    }
}
