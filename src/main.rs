//! Пассивное чтение evdev и прерываемая коррекция через uinput.

mod config;
mod engine;
mod injector;
mod instance;
mod keys;
mod session;
mod state;

use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use evdev::{raw_stream::RawDevice, Device, EventType, InputEvent, Key, Synchronization};
use signal_hook::consts::{SIGINT, SIGTERM};

use config::Config;
use engine::{DeviceEvent, Engine, KeyEvent};
use injector::{Injector, KeyOutput};
use instance::InstanceLock;
use session::SessionGuard;

const VIRTUAL_NAME: &str = "punto-rs virtual keyboard";
const DEFAULT_CONFIG: &str = "/etc/punto-rs/config.conf";
const RESCAN_INTERVAL: Duration = Duration::from_secs(3);
const CONTROL_INTERVAL: Duration = Duration::from_millis(10);
static NEXT_DEVICE_ID: AtomicU64 = AtomicU64::new(1);

struct Message {
    generation: u64,
    event: DeviceEvent,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeviceKind {
    Keyboard,
    Pointer,
}

fn main() {
    let mut config_path = PathBuf::from(DEFAULT_CONFIG);
    let mut explicit_config = false;
    let mut verbose = false;
    let mut check_config = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-c" | "--config" => {
                config_path = args
                    .next()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| die("--config требует путь к файлу"));
                explicit_config = true;
            }
            "-v" | "--verbose" => verbose = true,
            "--check-config" => check_config = true,
            "--check-session" => {
                match session::check() {
                    Ok(Some(id)) => {
                        println!("punto-rs: локальная графическая сессия {id} доступна")
                    }
                    Ok(None) => die("локальная графическая сессия недоступна, заблокирована или не поддерживается"),
                    Err(err) => die(&format!("проверка logind: {err}")),
                }
                return;
            }
            "-l" | "--list-devices" => {
                list_devices();
                return;
            }
            "-V" | "--version" => {
                println!("punto-rs {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "-h" | "--help" => {
                print_help();
                return;
            }
            other => die(&format!("неизвестный аргумент: {other}")),
        }
    }
    let cfg = if !explicit_config && !check_config && matches!(config_path.try_exists(), Ok(false))
    {
        eprintln!("punto-rs: {DEFAULT_CONFIG} отсутствует, используются значения по умолчанию");
        Config::default()
    } else {
        Config::load(&config_path).unwrap_or_else(|err| die(&err))
    };
    if check_config {
        println!("punto-rs: конфиг корректен");
        return;
    }
    let _instance = InstanceLock::acquire(Path::new("/run/punto-rs"))
        .unwrap_or_else(|err| die(&format!("блокировка экземпляра: {err}")));
    // Старые версии ещё не брали файловую блокировку.
    if evdev::enumerate().any(|(_, device)| device.name() == Some(VIRTUAL_NAME)) {
        die("виртуальная клавиатура punto-rs уже существует; сначала остановите предыдущий экземпляр");
    }
    let stopped = Arc::new(AtomicBool::new(false));
    for signal in [SIGINT, SIGTERM] {
        signal_hook::flag::register(signal, stopped.clone())
            .unwrap_or_else(|err| die(&format!("обработчик завершения: {err}")));
    }
    let injector = Injector::new(VIRTUAL_NAME).unwrap_or_else(|err| {
        eprintln!("punto-rs: не удалось создать виртуальную клавиатуру (/dev/uinput): {err}");
        std::process::exit(1)
    });
    let guard = SessionGuard::new(cfg.session_guard);
    if cfg.session_guard {
        let guard = guard.clone();
        let stopped = stopped.clone();
        thread::spawn(move || guard.monitor(&stopped));
    } else {
        eprintln!("punto-rs: session-guard=no — блокировка экрана и смена сессии не отслеживаются");
    }
    let (tx, rx) = mpsc::sync_channel::<Message>(1024);
    let watched = Arc::new(Mutex::new(HashSet::new()));
    let devices_filter = cfg.devices.clone();
    let track_mouse = cfg.track_mouse;
    let device_guard = guard.clone();
    let device_stop = stopped.clone();
    thread::spawn(move || {
        while !device_stop.load(Ordering::Relaxed) {
            attach_devices(&tx, &watched, &devices_filter, track_mouse, &device_guard);
            thread::sleep(RESCAN_INTERVAL);
        }
    });
    eprintln!(
        "punto-rs {} запущен: слово {:?}, фраза {:?}, пауза {:?}",
        env!("CARGO_PKG_VERSION"),
        cfg.hotkey,
        cfg.phrase_hotkey,
        cfg.pause_hotkey
    );
    if let Err(err) = run(rx, injector, cfg, verbose, guard, &stopped) {
        eprintln!("punto-rs: инжект остановлен после ошибки: {err}");
        std::process::exit(1);
    }
}

fn run<T: KeyOutput>(
    rx: Receiver<Message>,
    mut injector: Injector<T>,
    cfg: Config,
    verbose: bool,
    guard: SessionGuard,
    stopped: &AtomicBool,
) -> io::Result<()> {
    let mut engine = Engine::new(&cfg, Instant::now());
    let mut generation = u64::MAX;
    loop {
        if stopped.load(Ordering::Relaxed) {
            return Ok(());
        }
        let context = guard.context();
        if generation != context.generation {
            generation = context.generation;
            eprintln!(
                "punto-rs: {}",
                if context.session.is_some() {
                    "локальная сессия доступна"
                } else {
                    "коррекция приостановлена: сессия недоступна или заблокирована"
                }
            );
            engine.observe(DeviceEvent::Session(context.session), &cfg, Instant::now());
        }
        match rx.recv_timeout(CONTROL_INTERVAL) {
            Ok(message) => {
                let paused = engine.paused;
                if message.generation == generation && guard.context().generation == generation {
                    engine.observe(message.event, &cfg, Instant::now());
                } else {
                    engine.discard(message.event);
                }
                if engine.paused != paused {
                    eprintln!(
                        "punto-rs: {}",
                        if engine.paused {
                            "пауза включена"
                        } else {
                            "пауза выключена"
                        }
                    );
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        if let Some(fix) = engine.take_ready(&cfg, Instant::now()) {
            if verbose {
                eprintln!(
                    "punto-rs: исправляю {} нажатий ({})",
                    fix.strokes.len(),
                    if fix.phrase {
                        "фраза"
                    } else {
                        "слово"
                    }
                );
            }
            let result = injector.fix(&fix.strokes, &cfg, |delay| {
                wait_for_input(&rx, &mut engine, &cfg, &guard, generation, stopped, delay)
            });
            if let Err(err) = result {
                engine.invalidate();
                if err.kind() != io::ErrorKind::Interrupted {
                    return Err(err);
                }
                eprintln!(
                    "punto-rs: коррекция прервана; буфер сброшен, текст мог быть изменён частично"
                );
            }
        }
    }
}

fn wait_for_input(
    rx: &Receiver<Message>,
    engine: &mut Engine,
    cfg: &Config,
    guard: &SessionGuard,
    generation: u64,
    stopped: &AtomicBool,
    delay: Duration,
) -> io::Result<()> {
    let deadline = Instant::now() + delay;
    loop {
        if stopped.load(Ordering::Relaxed) || guard.context().generation != generation {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "завершение или смена сессии",
            ));
        }
        let message = match rx.try_recv() {
            Ok(message) => Some(message),
            Err(TryRecvError::Disconnected) => {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "поток событий закрыт",
                ))
            }
            Err(TryRecvError::Empty) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Ok(());
                }
                match rx.recv_timeout(remaining.min(CONTROL_INTERVAL)) {
                    Ok(message) => Some(message),
                    Err(RecvTimeoutError::Disconnected) => {
                        return Err(io::Error::new(
                            io::ErrorKind::Interrupted,
                            "поток событий закрыт",
                        ))
                    }
                    Err(RecvTimeoutError::Timeout) => None,
                }
            }
        };
        if let Some(message) = message {
            if message.generation == generation {
                engine.during_fix(message.event, cfg, Instant::now());
            } else {
                engine.discard(message.event);
            }
            return Err(io::Error::new(io::ErrorKind::Interrupted, "новый ввод"));
        }
    }
}

fn is_keyboard(device: &Device) -> bool {
    device.supported_keys().is_some_and(|keys| {
        keys.contains(Key::KEY_A) && keys.contains(Key::KEY_Z) && keys.contains(Key::KEY_SPACE)
    })
}
fn is_pointer(device: &Device) -> bool {
    device
        .supported_keys()
        .is_some_and(|keys| keys.contains(Key::BTN_LEFT))
}

fn attach_devices(
    tx: &SyncSender<Message>,
    watched: &Arc<Mutex<HashSet<PathBuf>>>,
    filter: &[String],
    track_mouse: bool,
    guard: &SessionGuard,
) {
    for (path, device) in evdev::enumerate() {
        let name = device.name().unwrap_or_default().to_string();
        if name == VIRTUAL_NAME || !on_default_seat(&path) {
            continue;
        }
        let wanted_keyboard = if filter.is_empty() {
            is_keyboard(&device)
        } else {
            filter.iter().any(|allowed| allowed == &name)
        };
        let kind = if wanted_keyboard {
            DeviceKind::Keyboard
        } else if track_mouse && is_pointer(&device) {
            DeviceKind::Pointer
        } else {
            continue;
        };
        let mut set = watched.lock().unwrap();
        if set.contains(&path) {
            continue;
        }
        let raw = match RawDevice::open(&path) {
            Ok(raw) => raw,
            Err(err) => {
                eprintln!("punto-rs: не удалось открыть {}: {err}", path.display());
                continue;
            }
        };
        set.insert(path.clone());
        drop(set);
        eprintln!("punto-rs: слушаю «{name}» ({})", path.display());
        let device_id = NEXT_DEVICE_ID.fetch_add(1, Ordering::Relaxed);
        let tx = tx.clone();
        let watched = watched.clone();
        let guard = guard.clone();
        thread::spawn(move || {
            if let Err(err) = read_device(raw, &tx, device_id, kind, &guard) {
                eprintln!("punto-rs: чтение «{name}» остановлено: {err}");
            }
            let _ = tx.send(Message {
                generation: guard.context().generation,
                event: DeviceEvent::Disconnected(device_id),
            });
            watched.lock().unwrap().remove(&path);
        });
    }
}

fn on_default_seat(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    let id = metadata.rdev();
    let database = format!("/run/udev/data/c{}:{}", libc::major(id), libc::minor(id));
    match std::fs::read_to_string(database) {
        Ok(data) => data
            .lines()
            .find_map(|line| line.strip_prefix("E:ID_SEAT="))
            .is_none_or(|seat| seat == "seat0"),
        Err(err) => err.kind() == io::ErrorKind::NotFound,
    }
}

#[derive(Default)]
struct EventStream {
    dropped: bool,
}
#[derive(Debug, PartialEq)]
enum StreamAction {
    Ignore,
    Lost,
    Resync,
    Key,
}
impl EventStream {
    fn observe(&mut self, event: &InputEvent) -> StreamAction {
        if event.event_type() == EventType::SYNCHRONIZATION
            && event.code() == Synchronization::SYN_DROPPED.0
        {
            self.dropped = true;
            return StreamAction::Lost;
        }
        if self.dropped {
            if event.event_type() == EventType::SYNCHRONIZATION
                && event.code() == Synchronization::SYN_REPORT.0
            {
                self.dropped = false;
                return StreamAction::Resync;
            }
            return StreamAction::Ignore;
        }
        if event.event_type() == EventType::KEY {
            StreamAction::Key
        } else {
            StreamAction::Ignore
        }
    }
}

fn read_device(
    mut device: RawDevice,
    tx: &SyncSender<Message>,
    device_id: u64,
    kind: DeviceKind,
    guard: &SessionGuard,
) -> io::Result<()> {
    let send = |event, generation| {
        tx.send(Message { generation, event })
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "поток событий закрыт"))
    };
    let held_keys = if kind == DeviceKind::Keyboard {
        device.get_key_state()?.iter().map(Key::code).collect()
    } else {
        Vec::new()
    };
    send(
        DeviceEvent::Resynced {
            device_id,
            held_keys,
        },
        guard.context().generation,
    )?;
    let mut stream = EventStream::default();
    loop {
        let generation = guard.context().generation;
        let events: Vec<_> = device.fetch_events()?.collect();
        for event in events {
            match stream.observe(&event) {
                StreamAction::Ignore => continue,
                StreamAction::Lost => {
                    send(DeviceEvent::LostEvents(device_id), generation)?;
                    continue;
                }
                StreamAction::Resync => {
                    let held_keys = if kind == DeviceKind::Keyboard {
                        device.get_key_state()?.iter().map(Key::code).collect()
                    } else {
                        Vec::new()
                    };
                    send(
                        DeviceEvent::Resynced {
                            device_id,
                            held_keys,
                        },
                        generation,
                    )?;
                    // Снимок учитывает и оставшуюся часть уже прочитанного пакета.
                    break;
                }
                StreamAction::Key => {}
            }
            let message = if keys::is_pointer_button(event.code()) {
                if event.value() != 1 {
                    continue;
                }
                DeviceEvent::Click
            } else if kind == DeviceKind::Keyboard {
                DeviceEvent::Key(KeyEvent {
                    device_id,
                    code: event.code(),
                    value: event.value(),
                })
            } else {
                continue;
            };
            send(message, generation)?;
        }
    }
}

fn list_devices() {
    let mut found = false;
    for (path, device) in evdev::enumerate() {
        found = true;
        let name = device.name().unwrap_or("<без имени>");
        let tag = if name == VIRTUAL_NAME {
            "— своё виртуальное устройство"
        } else if is_keyboard(&device) {
            "— клавиатура"
        } else if is_pointer(&device) {
            "— указатель"
        } else {
            ""
        };
        println!("{:<20} {name:<45} {tag}", path.display());
    }
    if !found {
        eprintln!("punto-rs: устройства не видны — проверьте доступ к /dev/input/*");
    }
}
fn print_help() {
    println!("punto-rs — исправление раскладки набранного текста\n\nИспользование: punto-rs [опции]\n\n  -c, --config <файл>  конфиг (по умолчанию {DEFAULT_CONFIG})\n      --check-config   проверить конфиг без открытия устройств\n      --check-session  проверить доступность сессии через logind\n  -l, --list-devices   показать устройства ввода\n  -v, --verbose        подробный вывод\n  -V, --version        версия\n  -h, --help           справка\n\nПауза/возобновление: Super+Pause (pause-hotkey).\nДля запуска нужны права на /dev/input/*, /dev/uinput и /run/punto-rs.");
}
fn die(message: &str) -> ! {
    eprintln!("punto-rs: {message}");
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TestOutput {
        events: Arc<Mutex<Vec<(u16, i32)>>>,
        tx: SyncSender<Message>,
        cancel_after: Option<usize>,
        fail_after: Option<usize>,
        stopped: Arc<AtomicBool>,
    }
    impl KeyOutput for TestOutput {
        fn emit_key(&mut self, code: u16, value: i32) -> io::Result<()> {
            let mut events = self.events.lock().unwrap();
            events.push((code, value));
            let count = events.len();
            if self.cancel_after == Some(count) {
                self.tx
                    .send(Message {
                        generation: 0,
                        event: DeviceEvent::Click,
                    })
                    .unwrap();
                for value in [1, 0] {
                    self.tx
                        .send(Message {
                            generation: 0,
                            event: DeviceEvent::Key(KeyEvent {
                                device_id: 1,
                                code: keys::KEY_INSERT,
                                value,
                            }),
                        })
                        .unwrap();
                }
            }
            if self.fail_after == Some(count) {
                return Err(io::Error::other("simulated write failure"));
            }
            if count == 8 {
                self.stopped.store(true, Ordering::Relaxed);
            }
            Ok(())
        }
    }

    #[test]
    fn event_loop_cancels_replay_and_does_not_reuse_aborted_buffer() {
        let (events, result) = run_test(Some(1), None);
        assert!(result.is_ok());
        assert_eq!(events, vec![(14, 1), (14, 0)]);
    }

    #[test]
    fn event_loop_exits_on_write_failure_after_releasing_key() {
        let (events, result) = run_test(None, Some(1));
        assert!(result.is_err());
        assert_eq!(events, vec![(14, 1), (14, 0)]);
    }

    #[test]
    fn event_loop_completes_exact_correction() {
        let (events, result) = run_test(None, None);
        assert!(result.is_ok());
        assert_eq!(
            events,
            vec![
                (14, 1),
                (14, 0),
                (125, 1),
                (57, 1),
                (57, 0),
                (125, 0),
                (30, 1),
                (30, 0)
            ]
        );
    }

    fn run_test(
        cancel_after: Option<usize>,
        fail_after: Option<usize>,
    ) -> (Vec<(u16, i32)>, io::Result<()>) {
        let (tx, rx) = mpsc::sync_channel(32);
        for code in [30, keys::KEY_INSERT] {
            for value in [1, 0] {
                tx.send(Message {
                    generation: 0,
                    event: DeviceEvent::Key(KeyEvent {
                        device_id: 1,
                        code,
                        value,
                    }),
                })
                .unwrap();
            }
        }
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let (finished_tx, finished_rx) = mpsc::channel();
        let watchdog = thread::spawn(move || {
            if finished_rx.recv_timeout(Duration::from_secs(2)).is_err() {
                stop.store(true, Ordering::Relaxed);
            }
        });
        let events = Arc::new(Mutex::new(Vec::new()));
        let injector = Injector::with_output(TestOutput {
            events: events.clone(),
            tx,
            cancel_after,
            fail_after,
            stopped: stopped.clone(),
        });
        let cfg = Config {
            session_guard: false,
            key_delay_ms: 1,
            post_backspace_ms: 0,
            switch_delay_ms: 0,
            ..Config::default()
        };
        let result = run(rx, injector, cfg, false, SessionGuard::new(false), &stopped);
        let _ = finished_tx.send(());
        watchdog.join().unwrap();
        let events = events.lock().unwrap().clone();
        (events, result)
    }

    #[test]
    fn dropped_events_are_ignored_until_report() {
        let mut stream = EventStream::default();
        assert_eq!(
            stream.observe(&InputEvent::new(EventType::SYNCHRONIZATION, 3, 0)),
            StreamAction::Lost
        );
        assert_eq!(
            stream.observe(&InputEvent::new(EventType::KEY, 30, 1)),
            StreamAction::Ignore
        );
        assert_eq!(
            stream.observe(&InputEvent::new(EventType::SYNCHRONIZATION, 0, 0)),
            StreamAction::Resync
        );
        assert_eq!(
            stream.observe(&InputEvent::new(EventType::KEY, 30, 0)),
            StreamAction::Key
        );
    }
    #[test]
    fn new_input_interrupts_long_wait_immediately() {
        let (tx, rx) = mpsc::sync_channel(1);
        let cfg = Config {
            session_guard: false,
            ..Config::default()
        };
        let mut engine = Engine::new(&cfg, Instant::now());
        let guard = SessionGuard::new(false);
        tx.send(Message {
            generation: 0,
            event: DeviceEvent::Click,
        })
        .unwrap();
        let started = Instant::now();
        assert_eq!(
            wait_for_input(
                &rx,
                &mut engine,
                &cfg,
                &guard,
                0,
                &AtomicBool::new(false),
                Duration::from_secs(2)
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::Interrupted
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }
    #[test]
    fn session_change_and_shutdown_cancel_before_next_key() {
        let (_tx, rx) = mpsc::sync_channel(1);
        let cfg = Config::default();
        let guard = SessionGuard::new(true);
        let mut engine = Engine::new(&cfg, Instant::now());
        guard.set(Some("session".into()));
        assert!(wait_for_input(
            &rx,
            &mut engine,
            &cfg,
            &guard,
            0,
            &AtomicBool::new(false),
            Duration::ZERO
        )
        .is_err());
        assert!(wait_for_input(
            &rx,
            &mut engine,
            &cfg,
            &guard,
            1,
            &AtomicBool::new(true),
            Duration::ZERO
        )
        .is_err());
    }
}
