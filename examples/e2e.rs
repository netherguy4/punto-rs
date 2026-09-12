//! Реальные evdev/uinput проверки. Только в отдельной VM: события попадают
//! в активную сессию. Требуются стандартные хоткеи и задержки демона.

use evdev::{
    uinput::{VirtualDevice, VirtualDeviceBuilder},
    AttributeSet, EventType, InputEvent, Key,
};
use std::{
    collections::HashSet,
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

const DAEMON_DEVICE: &str = "punto-rs virtual keyboard";
const WORD: [u16; 6] = [34, 35, 48, 32, 20, 49];

fn keyboard() -> VirtualDevice {
    let mut keys = AttributeSet::<Key>::new();
    for code in 1..=255 {
        keys.insert(Key::new(code));
    }
    VirtualDeviceBuilder::new()
        .expect("нет доступа к /dev/uinput")
        .name("punto-rs e2e test keyboard")
        .with_keys(&keys)
        .unwrap()
        .build()
        .unwrap()
}
fn key(kbd: &mut VirtualDevice, code: u16, value: i32) {
    kbd.emit(&[InputEvent::new(EventType::KEY, code, value)])
        .unwrap();
    thread::sleep(Duration::from_millis(15));
}
fn tap(kbd: &mut VirtualDevice, code: u16) {
    key(kbd, code, 1);
    key(kbd, code, 0);
}
fn type_word(kbd: &mut VirtualDevice) {
    for code in WORD {
        tap(kbd, code);
    }
}
fn pause(kbd: &mut VirtualDevice) {
    key(kbd, 125, 1);
    tap(kbd, 119);
    key(kbd, 125, 0);
}
fn quiet(rx: &Receiver<(u16, i32)>, scenario: &str) {
    assert!(
        rx.recv_timeout(Duration::from_millis(500)).is_err(),
        "unexpected output: {scenario}"
    );
}
fn output(rx: &Receiver<(u16, i32)>) -> Vec<(u16, i32)> {
    let mut events = vec![rx
        .recv_timeout(Duration::from_secs(3))
        .expect("no correction")];
    while let Ok(event) = rx.recv_timeout(Duration::from_millis(250)) {
        events.push(event);
    }
    released(&events);
    events
}
fn released(events: &[(u16, i32)]) {
    let mut held = HashSet::new();
    for &(code, value) in events {
        if value == 1 {
            held.insert(code);
        } else if value == 0 {
            held.remove(&code);
        }
    }
    assert!(held.is_empty(), "stuck synthetic keys: {held:?}");
}
fn expected(strokes: &[(u16, bool)]) -> Vec<(u16, i32)> {
    let mut events = Vec::new();
    for _ in strokes {
        events.extend([(14, 1), (14, 0)]);
    }
    events.extend([(125, 1), (57, 1), (57, 0), (125, 0)]);
    for &(code, shift) in strokes {
        if shift {
            events.push((42, 1));
        }
        events.extend([(code, 1), (code, 0)]);
        if shift {
            events.push((42, 0));
        }
    }
    events
}
fn main() {
    if std::env::args().skip(1).collect::<Vec<_>>() != ["--live-session"] {
        eprintln!("Только в тестовой VM: e2e --live-session. События попадут в активное окно.");
        std::process::exit(2);
    }
    let paths: Vec<_> = evdev::enumerate()
        .filter(|(_, d)| d.name() == Some(DAEMON_DEVICE))
        .map(|(p, _)| p)
        .collect();
    assert_eq!(paths.len(), 1, "нужен ровно один демон");
    let mut daemon = evdev::Device::open(&paths[0]).unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        while let Ok(events) = daemon.fetch_events() {
            for event in events {
                if event.event_type() == EventType::KEY
                    && tx.send((event.code(), event.value())).is_err()
                {
                    return;
                }
            }
        }
    });
    let mut kbd = keyboard();
    thread::sleep(Duration::from_secs(5));
    tap(&mut kbd, 28);
    type_word(&mut kbd);
    quiet(&rx, "passive input");
    tap(&mut kbd, 110);
    assert_eq!(output(&rx), expected(&WORD.map(|code| (code, false))));
    println!("PASS: passive input and exact word correction");

    tap(&mut kbd, 110);
    assert_eq!(output(&rx), expected(&WORD.map(|code| (code, false))));
    println!("PASS: repeated correction");

    for shift in [false, true] {
        tap(&mut kbd, 28);
        type_word(&mut kbd);
        if shift {
            key(&mut kbd, 42, 1);
        }
        tap(&mut kbd, 15);
        if shift {
            key(&mut kbd, 42, 0);
        }
        tap(&mut kbd, 110);
        quiet(&rx, "Tab/Shift+Tab");
    }
    println!("PASS: Tab and Shift+Tab invalidate previous field");

    tap(&mut kbd, 28);
    tap(&mut kbd, 30);
    tap(&mut kbd, 57);
    key(&mut kbd, 42, 1);
    tap(&mut kbd, 48);
    key(&mut kbd, 42, 0);
    key(&mut kbd, 125, 1);
    tap(&mut kbd, 110);
    key(&mut kbd, 125, 0);
    assert_eq!(
        output(&rx),
        expected(&[(30, false), (57, false), (48, true)])
    );
    println!("PASS: phrase and Shift replay");

    pause(&mut kbd);
    type_word(&mut kbd);
    tap(&mut kbd, 110);
    quiet(&rx, "paused");
    pause(&mut kbd);
    tap(&mut kbd, 110);
    quiet(&rx, "resume with empty history");
    println!("PASS: pause/resume discard history");

    type_word(&mut kbd);
    key(&mut kbd, 110, 1);
    quiet(&rx, "held hotkey");
    tap(&mut kbd, 30);
    key(&mut kbd, 110, 0);
    tap(&mut kbd, 110);
    quiet(&rx, "cancel pending correction");
    println!("PASS: held hotkey waits; new input cancels pending correction");

    tap(&mut kbd, 28);
    for _ in 0..80 {
        tap(&mut kbd, 30);
    }
    tap(&mut kbd, 110);
    let first = rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_eq!(first, (14, 1));
    key(&mut kbd, 29, 1);
    let mut aborted = vec![first];
    while let Ok(event) = rx.recv_timeout(Duration::from_millis(250)) {
        aborted.push(event);
    }
    key(&mut kbd, 29, 0);
    assert!(
        aborted.iter().all(|(code, _)| *code == 14),
        "replay continued after Ctrl"
    );
    assert!(aborted.iter().filter(|(_, value)| *value == 1).count() < 80);
    released(&aborted);
    tap(&mut kbd, 110);
    quiet(&rx, "aborted buffer reused");
    println!("PASS: input interrupts live injection and releases synthetic keys");

    type_word(&mut kbd);
    drop(kbd);
    thread::sleep(Duration::from_millis(250));
    let mut kbd = keyboard();
    thread::sleep(Duration::from_secs(5));
    tap(&mut kbd, 110);
    quiet(&rx, "hotplug reused history");
    type_word(&mut kbd);
    tap(&mut kbd, 110);
    assert_eq!(output(&rx), expected(&WORD.map(|code| (code, false))));
    println!("PASS: hotplug clears history and correction resumes");
    println!("E2E PASS: all evdev/uinput scenarios completed");
}
