//! Сквозная проверка работающего демона. Запускать под root при активном
//! сервисе: `cargo build --release --example e2e && sudo target/release/examples/e2e`.
//!
//! Тест создаёт виртуальную клавиатуру, «печатает» на ней ` ghbdtn`, жмёт
//! Insert и слушает выходное устройство демона. Проверяется и пассивность
//! (до Insert демон не издаёт ни одного события), и сам инжект исправления.
//!
//! ВНИМАНИЕ: события уходят в живую сессию — в сфокусированном окне появится
//! пробел и исправленное слово, а раскладка переключится.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use evdev::uinput::VirtualDeviceBuilder;
use evdev::{AttributeSet, EventType, InputEvent, Key};

const DAEMON_DEVICE: &str = "punto-rs virtual keyboard";
const KEY_BACKSPACE: u16 = 14;
const KEY_SPACE: u16 = 57;
const KEY_INSERT: u16 = 110;
const KEY_LEFTMETA: u16 = 125;
/// `ghbdtn` → «привет».
const WORD: [u16; 6] = [34, 35, 48, 32, 20, 49];

fn main() {
    let mut set = AttributeSet::<Key>::new();
    for code in 1..=255u16 {
        set.insert(Key::new(code));
    }
    let mut test_kbd = VirtualDeviceBuilder::new()
        .expect("нет доступа к /dev/uinput — нужен root")
        .name("punto-rs e2e test keyboard")
        .with_keys(&set)
        .unwrap()
        .build()
        .unwrap();

    // Демон пересканирует устройства раз в 3 секунды.
    eprintln!("жду, пока демон подхватит тестовую клавиатуру…");
    thread::sleep(Duration::from_secs(5));

    let daemon_path = evdev::enumerate()
        .find(|(_, d)| d.name() == Some(DAEMON_DEVICE))
        .map(|(path, _)| path)
        .expect("выходное устройство демона не найдено — сервис punto-rs запущен?");
    let mut daemon_out = evdev::Device::open(&daemon_path).unwrap();

    let (tx, rx) = mpsc::channel::<(u16, i32)>();
    thread::spawn(move || loop {
        let events: Vec<InputEvent> = match daemon_out.fetch_events() {
            Ok(events) => events.collect(),
            Err(_) => return,
        };
        for ev in events {
            if ev.event_type() == EventType::KEY && tx.send((ev.code(), ev.value())).is_err() {
                return;
            }
        }
    });

    let tap = |kbd: &mut evdev::uinput::VirtualDevice, code: u16| {
        kbd.emit(&[InputEvent::new(EventType::KEY, code, 1)])
            .unwrap();
        thread::sleep(Duration::from_millis(15));
        kbd.emit(&[InputEvent::new(EventType::KEY, code, 0)])
            .unwrap();
        thread::sleep(Duration::from_millis(15));
    };

    eprintln!("печатаю ' ghbdtn'…");
    tap(&mut test_kbd, KEY_SPACE);
    for code in WORD {
        tap(&mut test_kbd, code);
    }
    thread::sleep(Duration::from_millis(300));

    let premature: Vec<_> = rx.try_iter().collect();
    assert!(
        premature.is_empty(),
        "FAIL: демон издавал события во время обычного набора: {premature:?}"
    );
    eprintln!("OK: во время набора демон молчал (пассивное чтение)");

    eprintln!("жму Insert…");
    tap(&mut test_kbd, KEY_INSERT);
    thread::sleep(Duration::from_secs(3));

    let events: Vec<(u16, i32)> = rx.try_iter().collect();
    assert!(!events.is_empty(), "FAIL: демон не отреагировал на Insert");

    let backspaces = events
        .iter()
        .filter(|(code, value)| *code == KEY_BACKSPACE && *value == 1)
        .count();
    assert_eq!(
        backspaces,
        WORD.len(),
        "FAIL: стёрто не столько, сколько набрано"
    );
    eprintln!("OK: {backspaces} backspace — ровно по числу букв");

    assert!(
        events
            .iter()
            .any(|(code, value)| *code == KEY_LEFTMETA && *value == 1)
            && events
                .iter()
                .any(|(code, value)| *code == KEY_SPACE && *value == 1),
        "FAIL: не было комбинации смены раскладки (Super+Space)"
    );
    eprintln!("OK: раскладка переключена (Super+Space)");

    let last_backspace = events
        .iter()
        .rposition(|(code, _)| *code == KEY_BACKSPACE)
        .unwrap();
    let replayed: Vec<u16> = events[last_backspace..]
        .iter()
        .filter(|(code, value)| *value == 1 && WORD.contains(code))
        .map(|(code, _)| *code)
        .collect();
    assert_eq!(replayed, WORD, "FAIL: слово переиграно не теми клавишами");
    eprintln!("OK: слово перенабрано теми же скан-кодами");

    println!("PASS: все проверки пройдены");
}
