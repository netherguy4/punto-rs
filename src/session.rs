//! logind — источник состояния локальной графической сессии seat0.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use dbus::{
    blocking::{stdintf::org_freedesktop_dbus::Properties, Connection},
    message::MatchRule,
    Path,
};

const LOGIN: &str = "org.freedesktop.login1";
const TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone, Debug)]
pub struct Context {
    pub generation: u64,
    pub session: Option<String>,
    valid_until: Option<std::time::Instant>,
}

#[derive(Clone)]
pub struct SessionGuard(Arc<Mutex<Context>>);

impl SessionGuard {
    pub fn new(enabled: bool) -> Self {
        Self(Arc::new(Mutex::new(Context {
            generation: 0,
            session: (!enabled).then(|| "unguarded".into()),
            valid_until: None,
        })))
    }
    pub fn context(&self) -> Context {
        let mut state = self.0.lock().unwrap();
        if state
            .valid_until
            .is_some_and(|deadline| deadline <= std::time::Instant::now())
            && state.session.is_some()
        {
            state.generation += 1;
            state.session = None;
        }
        state.clone()
    }
    pub fn set(&self, session: Option<String>) {
        let mut state = self.0.lock().unwrap();
        state.valid_until = Some(std::time::Instant::now() + Duration::from_secs(2));
        if state.session != session {
            state.generation += 1;
            state.session = session;
        }
    }
    fn invalidate(&self) {
        let mut state = self.0.lock().unwrap();
        state.generation += 1;
        state.session = None;
    }

    pub fn monitor(&self, stopped: &AtomicBool) {
        while !stopped.load(Ordering::Relaxed) {
            if let Err(err) = self.watch(stopped) {
                self.invalidate();
                eprintln!("punto-rs: проверка сессии недоступна, коррекция приостановлена: {err}");
                for _ in 0..30 {
                    if stopped.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }

    fn watch(&self, stopped: &AtomicBool) -> Result<(), dbus::Error> {
        let connection = Connection::new_system()?;
        let dirty = Arc::new(AtomicBool::new(true));
        let guard = self.clone();
        let changed = dirty.clone();
        let mut rule =
            MatchRule::new_signal("org.freedesktop.DBus.Properties", "PropertiesChanged");
        rule.sender = Some(LOGIN.into());
        connection.add_match(
            rule,
            move |(interface, values, invalidated): (String, dbus::arg::PropMap, Vec<String>),
                  _,
                  _| {
                if interface.starts_with(LOGIN)
                    && ["Active", "ActiveSession", "LockedHint", "State"]
                        .iter()
                        .any(|key| {
                            values.contains_key(*key) || invalidated.iter().any(|s| s == key)
                        })
                {
                    guard.invalidate();
                    changed.store(true, Ordering::Relaxed);
                }
                true
            },
        )?;
        for member in ["PrepareForSleep", "PrepareForShutdown"] {
            let guard = self.clone();
            let changed = dirty.clone();
            let mut rule = MatchRule::new_signal("org.freedesktop.login1.Manager", member);
            rule.sender = Some(LOGIN.into());
            connection.add_match(rule, move |_: (bool,), _, _| {
                guard.invalidate();
                changed.store(true, Ordering::Relaxed);
                true
            })?;
        }
        let mut last_check = std::time::Instant::now() - Duration::from_secs(2);
        while !stopped.load(Ordering::Relaxed) {
            connection.process(Duration::from_millis(50))?;
            if dirty.swap(false, Ordering::Relaxed)
                || last_check.elapsed() >= Duration::from_secs(1)
            {
                let snapshot = snapshot(&connection)?;
                // Обрабатываем накопившиеся сигналы до разрешения ввода: состояние
                // могло измениться между отдельными D-Bus запросами снимка.
                while connection.process(Duration::ZERO)? {}
                if !dirty.load(Ordering::Relaxed) {
                    self.set(snapshot);
                }
                last_check = std::time::Instant::now();
            }
        }
        self.invalidate();
        Ok(())
    }
}

fn snapshot(connection: &Connection) -> Result<Option<String>, dbus::Error> {
    let manager = connection.with_proxy(LOGIN, "/org/freedesktop/login1", TIMEOUT);
    let sleeping: bool = manager.get("org.freedesktop.login1.Manager", "PreparingForSleep")?;
    let shutdown: bool = manager.get("org.freedesktop.login1.Manager", "PreparingForShutdown")?;
    if sleeping || shutdown {
        return Ok(None);
    }
    let seat = connection.with_proxy(LOGIN, "/org/freedesktop/login1/seat/seat0", TIMEOUT);
    let (id, path): (String, Path<'static>) =
        seat.get("org.freedesktop.login1.Seat", "ActiveSession")?;
    if id.is_empty() {
        return Ok(None);
    }
    let session = connection.with_proxy(LOGIN, path, TIMEOUT);
    let properties = session.get_all("org.freedesktop.login1.Session")?;
    let string = |name: &str| properties.get(name).and_then(|v| v.0.as_str());
    let boolean = |name: &str| properties.get(name).and_then(|v| v.0.as_i64());
    let allowed = allowed_session(
        string("Type"),
        string("Class"),
        string("Desktop"),
        boolean("Active"),
        boolean("LockedHint"),
        boolean("Remote"),
    );
    let (current, _): (String, Path<'static>) =
        seat.get("org.freedesktop.login1.Seat", "ActiveSession")?;
    Ok((allowed && current == id).then_some(id))
}

pub fn check() -> Result<Option<String>, dbus::Error> {
    snapshot(&Connection::new_system()?)
}

fn allowed_session(
    kind: Option<&str>,
    class: Option<&str>,
    desktop: Option<&str>,
    active: Option<i64>,
    locked: Option<i64>,
    remote: Option<i64>,
) -> bool {
    // COSMIC 1.0.9 leaves LockedHint=false while locked. Revisit only after
    // its lock integration reliably reports every lock/unlock path to logind.
    let unsupported = desktop.is_some_and(|name| {
        name.split(':')
            .any(|part| part.eq_ignore_ascii_case("cosmic"))
    });
    !unsupported
        && matches!(kind, Some("wayland" | "x11"))
        && class == Some("user")
        && active == Some(1)
        && locked == Some(0)
        && remote == Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_known_unlocked_local_graphical_user_session_is_allowed() {
        assert!(allowed_session(
            Some("wayland"),
            Some("user"),
            Some("GNOME"),
            Some(1),
            Some(0),
            Some(0)
        ));
        assert!(allowed_session(
            Some("x11"),
            Some("user"),
            Some("GNOME"),
            Some(1),
            Some(0),
            Some(0)
        ));
        for tuple in [
            (Some("tty"), Some("user"), Some(1), Some(0), Some(0)),
            (Some("wayland"), Some("greeter"), Some(1), Some(0), Some(0)),
            (Some("wayland"), Some("user"), Some(0), Some(0), Some(0)),
            (Some("wayland"), Some("user"), Some(1), Some(1), Some(0)),
            (Some("wayland"), Some("user"), Some(1), None, Some(0)),
            (Some("wayland"), Some("user"), Some(1), Some(0), Some(1)),
        ] {
            assert!(!allowed_session(
                tuple.0,
                tuple.1,
                Some("KDE"),
                tuple.2,
                tuple.3,
                tuple.4
            ));
        }
    }
    #[test]
    fn cosmic_cannot_rely_on_an_unlocked_logind_hint() {
        for desktop in ["COSMIC", "cosmic", "COSMIC:Wayland", "test:Cosmic"] {
            assert!(!allowed_session(
                Some("wayland"),
                Some("user"),
                Some(desktop),
                Some(1),
                Some(0),
                Some(0)
            ));
        }
        for desktop in ["GNOME", "KDE", "GNOME:GNOME"] {
            assert!(allowed_session(
                Some("wayland"),
                Some("user"),
                Some(desktop),
                Some(1),
                Some(0),
                Some(0)
            ));
        }
    }

    #[test]
    fn stale_logind_snapshot_disables_correction() {
        let guard = SessionGuard::new(true);
        guard.set(Some("a".into()));
        let before = guard.context().generation;
        guard.0.lock().unwrap().valid_until = Some(std::time::Instant::now());
        assert!(guard.context().session.is_none());
        assert_ne!(guard.context().generation, before);
    }

    #[test]
    fn session_generations_invalidate_queued_input() {
        let guard = SessionGuard::new(true);
        assert!(guard.context().session.is_none());
        guard.set(Some("a".into()));
        let before = guard.context().generation;
        guard.set(Some("a".into()));
        assert_eq!(guard.context().generation, before);
        guard.invalidate();
        guard.set(Some("a".into()));
        assert_ne!(guard.context().generation, before);
    }
}
