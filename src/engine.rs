//! Состояние набора и горячих клавиш без доступа к устройствам и часам ОС.

use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

use crate::{
    config::Config,
    keys,
    state::{Buffer, Stroke},
};

#[derive(Debug)]
pub struct KeyEvent {
    pub device_id: u64,
    pub code: u16,
    pub value: i32,
}

#[derive(Debug)]
pub enum DeviceEvent {
    Resynced { device_id: u64, held_keys: Vec<u16> },
    Key(KeyEvent),
    Click,
    Disconnected(u64),
    Session(Option<String>),
    LostEvents(u64),
}

#[derive(Default)]
struct HeldKeys {
    keys: HashSet<(u64, u16)>,
}

impl HeldKeys {
    fn observe(&mut self, event: &DeviceEvent) {
        match event {
            DeviceEvent::Resynced {
                device_id,
                held_keys,
            } => {
                self.keys.retain(|(id, _)| id != device_id);
                self.keys
                    .extend(held_keys.iter().map(|code| (*device_id, *code)));
            }
            DeviceEvent::Key(event) => match event.value {
                0 => {
                    self.keys.remove(&(event.device_id, event.code));
                }
                1 => {
                    self.keys.insert((event.device_id, event.code));
                }
                _ => {}
            },
            DeviceEvent::Disconnected(device_id) => self.keys.retain(|(id, _)| id != device_id),
            _ => {}
        }
    }
    fn matches(&self, hotkey: &[u16]) -> bool {
        hotkey
            .iter()
            .all(|required| self.keys.iter().any(|(_, code)| code == required))
            && self.keys.iter().all(|(_, code)| hotkey.contains(code))
    }
    fn shift(&self) -> bool {
        self.keys.iter().any(|(_, code)| keys::is_shift(*code))
    }
    fn command(&self) -> bool {
        self.keys
            .iter()
            .any(|(_, code)| keys::is_command_modifier(*code))
    }
}

pub struct PendingFix {
    pub strokes: Vec<Stroke>,
    pub phrase: bool,
    trigger: u16,
    ready_at: Option<Instant>,
}

pub struct Engine {
    buffer: Buffer,
    held: HeldKeys,
    pending: Option<PendingFix>,
    pub paused: bool,
    session: Option<String>,
    last_input: Instant,
    unsynced: HashSet<u64>,
}

impl Engine {
    pub fn new(cfg: &Config, now: Instant) -> Self {
        Self {
            buffer: Buffer::new(cfg.max_strokes),
            held: HeldKeys::default(),
            pending: None,
            paused: false,
            session: (!cfg.session_guard).then(|| "unguarded".to_string()),
            last_input: now,
            unsynced: HashSet::new(),
        }
    }

    fn observe_device_state(&mut self, event: &DeviceEvent) {
        self.held.observe(event);
        match event {
            DeviceEvent::LostEvents(id) => {
                self.unsynced.insert(*id);
            }
            DeviceEvent::Resynced { device_id, .. } | DeviceEvent::Disconnected(device_id) => {
                self.unsynced.remove(device_id);
            }
            _ => {}
        }
    }

    pub fn invalidate(&mut self) {
        self.buffer.clear();
        self.pending = None;
    }

    pub fn expire(&mut self, cfg: &Config, now: Instant) {
        if now.duration_since(self.last_input) >= Duration::from_millis(cfg.buffer_timeout_ms) {
            self.invalidate();
        }
    }

    pub fn observe(&mut self, event: DeviceEvent, cfg: &Config, now: Instant) {
        self.expire(cfg, now);
        self.observe_device_state(&event);
        if let DeviceEvent::Session(session) = event {
            self.session = session;
            self.invalidate();
            return;
        }
        let DeviceEvent::Key(event) = event else {
            self.invalidate();
            return;
        };
        if event.value == 1
            && event.code == *cfg.pause_hotkey.last().unwrap()
            && self.held.matches(&cfg.pause_hotkey)
        {
            self.paused = !self.paused;
            self.invalidate();
            return;
        }
        if self.paused || self.session.is_none() || !self.unsynced.is_empty() {
            self.invalidate();
            return;
        }
        if let Some(pending) = &mut self.pending {
            if event.value == 1 || (event.value == 2 && event.code != pending.trigger) {
                self.invalidate();
            } else if self.held.keys.is_empty() {
                pending.ready_at = Some(now + Duration::from_millis(30));
            }
            return;
        }
        if event.value == 0 {
            return;
        }
        self.last_input = now;
        // Повтор на уровне evdev не гарантирует столько же символов в Wayland.
        if event.value != 1 {
            self.invalidate();
            return;
        }
        let phrase = if event.code == *cfg.phrase_hotkey.last().unwrap()
            && self.held.matches(&cfg.phrase_hotkey)
        {
            Some(true)
        } else if event.code == *cfg.hotkey.last().unwrap() && self.held.matches(&cfg.hotkey) {
            Some(false)
        } else {
            None
        };
        if let Some(phrase) = phrase {
            let strokes = if phrase {
                self.buffer.phrase()
            } else {
                self.buffer.last_word()
            };
            if !strokes.is_empty() {
                self.pending = Some(PendingFix {
                    strokes: strokes.to_vec(),
                    phrase,
                    trigger: event.code,
                    ready_at: None,
                });
            }
            return;
        }
        if self.held.matches(&cfg.layout_switch) {
            self.invalidate();
            return;
        }
        if keys::is_shift(event.code) || keys::is_command_modifier(event.code) {
            return;
        }
        if self.held.command() {
            self.invalidate();
        } else if event.code == keys::KEY_BACKSPACE {
            self.buffer.backspace();
        } else if event.code == keys::KEY_TAB || keys::is_phrase_end(event.code) {
            self.invalidate();
        } else if keys::is_char(event.code) || keys::is_separator(event.code) {
            self.buffer.push(event.code, self.held.shift());
        } else {
            self.invalidate();
        }
    }

    pub fn take_ready(&mut self, cfg: &Config, now: Instant) -> Option<PendingFix> {
        self.expire(cfg, now);
        if self
            .pending
            .as_ref()
            .is_some_and(|fix| fix.ready_at.is_some_and(|at| at <= now))
            && self.held.keys.is_empty()
        {
            self.pending.take()
        } else {
            None
        }
    }

    pub fn discard(&mut self, event: DeviceEvent) {
        self.observe_device_state(&event);
        self.invalidate();
    }

    pub fn during_fix(&mut self, event: DeviceEvent, cfg: &Config, now: Instant) {
        self.invalidate();
        self.observe(event, cfg, now);
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Harness {
        engine: Engine,
        cfg: Config,
        now: Instant,
    }
    impl Harness {
        fn new() -> Self {
            let cfg = Config {
                session_guard: false,
                ..Config::default()
            };
            let now = Instant::now();
            Self {
                engine: Engine::new(&cfg, now),
                cfg,
                now,
            }
        }
        fn event(&mut self, event: DeviceEvent) {
            self.engine.observe(event, &self.cfg, self.now);
        }
        fn key(&mut self, device_id: u64, code: u16, value: i32) {
            self.event(DeviceEvent::Key(KeyEvent {
                device_id,
                code,
                value,
            }));
        }
        fn tap(&mut self, code: u16) {
            self.key(1, code, 1);
            self.key(1, code, 0);
        }
        fn ready(&mut self) -> Option<PendingFix> {
            self.now += Duration::from_millis(31);
            self.engine.take_ready(&self.cfg, self.now)
        }
        fn fix(&mut self) -> Option<PendingFix> {
            self.tap(keys::KEY_INSERT);
            self.ready()
        }
    }

    #[test]
    fn word_and_phrase_preserve_shift_and_trailing_space() {
        let mut h = Harness::new();
        h.tap(16);
        h.tap(keys::KEY_SPACE);
        h.key(1, 42, 1);
        h.tap(17);
        h.key(1, 42, 0);
        h.tap(57);
        let fix = h.fix().unwrap();
        assert!(!fix.phrase);
        assert_eq!(
            fix.strokes,
            vec![
                Stroke {
                    code: 17,
                    shift: true
                },
                Stroke {
                    code: 57,
                    shift: false
                }
            ]
        );
        h.key(1, 125, 1);
        h.tap(110);
        h.key(1, 125, 0);
        let fix = h.ready().unwrap();
        assert!(fix.phrase);
        assert_eq!(fix.strokes.len(), 4);
    }

    #[test]
    fn navigation_tab_shift_tab_and_click_forget_previous_field() {
        for code in [
            keys::KEY_TAB,
            keys::KEY_ENTER,
            keys::KEY_KPENTER,
            105,
            106,
            102,
            107,
            111,
        ] {
            for shift in [false, true] {
                let mut h = Harness::new();
                h.tap(30);
                if shift {
                    h.key(1, 42, 1);
                }
                h.tap(code);
                if shift {
                    h.key(1, 42, 0);
                }
                assert!(h.fix().is_none(), "code={code}, shift={shift}");
            }
        }
        let mut h = Harness::new();
        h.tap(30);
        h.event(DeviceEvent::Click);
        assert!(h.fix().is_none());
    }

    #[test]
    fn shift_insert_does_not_correct() {
        let mut h = Harness::new();
        h.tap(30);
        h.key(1, 42, 1);
        h.tap(110);
        h.key(1, 42, 0);
        assert!(h.ready().is_none());
        assert!(h.fix().is_none());
    }

    #[test]
    fn waits_for_release_and_cancels_on_new_input() {
        let mut h = Harness::new();
        h.tap(30);
        h.key(1, 110, 1);
        assert!(h.ready().is_none());
        h.key(1, 110, 0);
        assert!(h.engine.take_ready(&h.cfg, h.now).is_none());
        h.tap(48);
        assert!(h.ready().is_none());
        assert!(h.fix().is_none());
    }

    #[test]
    fn same_modifier_on_two_keyboards_is_not_released_early() {
        let mut h = Harness::new();
        h.tap(30);
        h.key(1, 125, 1);
        h.key(2, 125, 1);
        h.tap(110);
        h.key(1, 125, 0);
        assert!(h.ready().is_none());
        h.key(2, 125, 0);
        assert!(h.ready().unwrap().phrase);
    }

    #[test]
    fn loss_and_device_reconnect_cancel_pending_and_replace_held_state() {
        let mut h = Harness::new();
        h.tap(30);
        h.tap(110);
        h.event(DeviceEvent::LostEvents(1));
        assert!(h.ready().is_none());
        h.key(1, 29, 1);
        h.event(DeviceEvent::Resynced {
            device_id: 1,
            held_keys: vec![],
        });
        h.tap(30);
        assert!(h.fix().is_some());
        h.event(DeviceEvent::Disconnected(1));
        assert!(h.fix().is_none());
    }

    #[test]
    fn no_correction_until_lost_device_is_resynchronized() {
        let mut h = Harness::new();
        h.event(DeviceEvent::LostEvents(2));
        h.tap(30);
        assert!(h.fix().is_none());
        h.event(DeviceEvent::Resynced {
            device_id: 2,
            held_keys: vec![],
        });
        h.tap(30);
        assert!(h.fix().is_some());
    }

    #[test]
    fn pause_and_session_changes_discard_sensitive_history() {
        let mut h = Harness::new();
        h.tap(30);
        h.key(1, 125, 1);
        h.tap(119);
        h.key(1, 125, 0);
        assert!(h.engine.paused);
        h.tap(48);
        assert!(h.fix().is_none());
        h.key(1, 125, 1);
        h.tap(119);
        h.key(1, 125, 0);
        assert!(!h.engine.paused);
        assert!(h.fix().is_none());
        h.tap(30);
        h.event(DeviceEvent::Session(None));
        h.tap(48);
        assert!(h.fix().is_none());
        h.event(DeviceEvent::Session(Some("new-session".into())));
        assert!(h.fix().is_none());
        h.tap(30);
        assert!(h.fix().is_some());
    }

    #[test]
    fn idle_and_repeat_invalidate_history() {
        let mut h = Harness::new();
        h.tap(30);
        h.now += Duration::from_millis(h.cfg.buffer_timeout_ms);
        assert!(h.fix().is_none());
        h.tap(30);
        h.key(1, 30, 2);
        assert!(h.fix().is_none());
    }

    #[test]
    fn abort_does_not_retain_old_or_interleaved_text() {
        let mut h = Harness::new();
        h.tap(30);
        assert!(h.fix().is_some());
        h.engine.during_fix(
            DeviceEvent::Key(KeyEvent {
                device_id: 1,
                code: 48,
                value: 1,
            }),
            &h.cfg,
            h.now,
        );
        h.key(1, 48, 0);
        assert!(h.fix().is_none());
    }
}
