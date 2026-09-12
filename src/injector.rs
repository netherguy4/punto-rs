//! Прерываемый инжект с обязательной попыткой отпустить синтетические клавиши.

use std::{io, time::Duration};

use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
use evdev::{AttributeSet, EventType, InputEvent, Key};

use crate::{config::Config, keys, state::Stroke};

pub trait KeyOutput {
    fn emit_key(&mut self, code: u16, value: i32) -> io::Result<()>;
}

impl KeyOutput for VirtualDevice {
    fn emit_key(&mut self, code: u16, value: i32) -> io::Result<()> {
        self.emit(&[InputEvent::new(EventType::KEY, code, value)])
    }
}

pub struct Injector<T: KeyOutput = VirtualDevice> {
    output: T,
    pressed: Vec<u16>,
}

impl Injector {
    pub fn new(name: &str) -> io::Result<Self> {
        let mut set = AttributeSet::<Key>::new();
        for code in 1..=255u16 {
            set.insert(Key::new(code));
        }
        let device = VirtualDeviceBuilder::new()?
            .name(name)
            .with_keys(&set)?
            .build()?;
        Ok(Self::with_output(device))
    }
}

impl<T: KeyOutput> Injector<T> {
    pub fn with_output(output: T) -> Self {
        Self {
            output,
            pressed: Vec::new(),
        }
    }

    fn emit(&mut self, code: u16, value: i32) -> io::Result<()> {
        // Даже неуспешная запись могла частично дойти до uinput.
        if value == 1 && !self.pressed.contains(&code) {
            self.pressed.push(code);
        }
        self.output.emit_key(code, value)?;
        if value == 0 {
            self.pressed.retain(|held| *held != code);
        }
        Ok(())
    }

    fn release_all(&mut self) -> io::Result<()> {
        let mut error = None;
        for code in self.pressed.clone().into_iter().rev() {
            if let Err(err) = self.emit(code, 0) {
                error.get_or_insert(err);
            }
        }
        error.map_or(Ok(()), Err)
    }

    fn key(
        &mut self,
        code: u16,
        value: i32,
        delay: Duration,
        wait: &mut impl FnMut(Duration) -> io::Result<()>,
    ) -> io::Result<()> {
        wait(Duration::ZERO)?;
        self.emit(code, value)?;
        wait(delay)
    }

    fn tap(
        &mut self,
        code: u16,
        delay: Duration,
        wait: &mut impl FnMut(Duration) -> io::Result<()>,
    ) -> io::Result<()> {
        self.key(code, 1, delay, wait)?;
        self.key(code, 0, delay, wait)
    }

    pub fn fix(
        &mut self,
        strokes: &[Stroke],
        cfg: &Config,
        mut wait: impl FnMut(Duration) -> io::Result<()>,
    ) -> io::Result<()> {
        if strokes.is_empty() {
            return Ok(());
        }
        let delay = Duration::from_millis(cfg.key_delay_ms);
        let result = (|| {
            for _ in strokes {
                self.tap(keys::KEY_BACKSPACE, delay, &mut wait)?;
            }
            wait(Duration::from_millis(cfg.post_backspace_ms))?;
            for &code in &cfg.layout_switch {
                self.key(code, 1, delay, &mut wait)?;
            }
            for &code in cfg.layout_switch.iter().rev() {
                self.key(code, 0, delay, &mut wait)?;
            }
            wait(Duration::from_millis(cfg.switch_delay_ms))?;
            for stroke in strokes {
                if stroke.shift {
                    self.key(keys::KEY_LEFTSHIFT, 1, delay, &mut wait)?;
                }
                self.tap(stroke.code, delay, &mut wait)?;
                if stroke.shift {
                    self.key(keys::KEY_LEFTSHIFT, 0, delay, &mut wait)?;
                }
            }
            Ok(())
        })();
        if let Err(err) = self.release_all() {
            return Err(io::Error::other(format!(
                "не удалось отпустить синтетические клавиши: {err}"
            )));
        }
        result
    }
}

impl<T: KeyOutput> Drop for Injector<T> {
    fn drop(&mut self) {
        let _ = self.release_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[derive(Default)]
    struct Output {
        events: Vec<(u16, i32)>,
        fail_at: Option<usize>,
        calls: usize,
    }
    impl KeyOutput for Output {
        fn emit_key(&mut self, code: u16, value: i32) -> io::Result<()> {
            self.calls += 1;
            self.events.push((code, value));
            if self.fail_at == Some(self.calls) {
                Err(io::Error::other("partial write"))
            } else {
                Ok(())
            }
        }
    }
    fn word() -> [Stroke; 2] {
        [
            Stroke {
                code: 30,
                shift: true,
            },
            Stroke {
                code: 48,
                shift: false,
            },
        ]
    }
    fn assert_released(events: &[(u16, i32)]) {
        let mut held = HashSet::new();
        for &(code, value) in events {
            if value == 1 {
                held.insert(code);
            } else {
                held.remove(&code);
            }
        }
        assert!(held.is_empty(), "stuck keys: {held:?}; events: {events:?}");
    }
    #[test]
    fn emits_exact_sequence_including_releases() {
        let mut injector = Injector::with_output(Output::default());
        injector
            .fix(&word(), &Config::default(), |_| Ok(()))
            .unwrap();
        assert_eq!(
            injector.output.events,
            vec![
                (14, 1),
                (14, 0),
                (14, 1),
                (14, 0),
                (125, 1),
                (57, 1),
                (57, 0),
                (125, 0),
                (42, 1),
                (30, 1),
                (30, 0),
                (42, 0),
                (48, 1),
                (48, 0),
            ]
        );
    }
    #[test]
    fn releases_keys_after_every_possible_partial_write_failure() {
        for fail_at in 1..=14 {
            let mut injector = Injector::with_output(Output {
                fail_at: Some(fail_at),
                ..Output::default()
            });
            assert!(injector
                .fix(&word(), &Config::default(), |_| Ok(()))
                .is_err());
            assert_released(&injector.output.events);
            assert!(injector.output.events[fail_at..]
                .iter()
                .all(|(_, value)| *value == 0));
        }
    }
    #[test]
    fn cancels_at_every_wait_without_leaving_keys_pressed() {
        let mut waits = 0;
        Injector::with_output(Output::default())
            .fix(&word(), &Config::default(), |_| {
                waits += 1;
                Ok(())
            })
            .unwrap();
        for cancel_at in 1..=waits {
            let mut count = 0;
            let mut injector = Injector::with_output(Output::default());
            let result = injector.fix(&word(), &Config::default(), |_| {
                count += 1;
                if count == cancel_at {
                    Err(io::Error::new(io::ErrorKind::Interrupted, "new input"))
                } else {
                    Ok(())
                }
            });
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
            assert_released(&injector.output.events);
            assert_eq!(count, cancel_at);
        }
    }
    #[test]
    fn cleanup_failure_is_fatal_even_when_correction_was_cancelled() {
        let mut injector = Injector::with_output(Output {
            fail_at: Some(2),
            ..Output::default()
        });
        let mut count = 0;
        let err = injector
            .fix(&word(), &Config::default(), |_| {
                count += 1;
                if count == 2 {
                    Err(io::Error::new(io::ErrorKind::Interrupted, "cancel"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Other);
    }
    #[test]
    fn empty_buffer_does_not_switch_layout() {
        let mut injector = Injector::with_output(Output::default());
        injector
            .fix(&[], &Config::default(), |_| panic!("unexpected wait"))
            .unwrap();
        assert!(injector.output.events.is_empty());
    }
}
