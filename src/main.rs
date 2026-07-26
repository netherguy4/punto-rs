//! punto-rs — исправление раскладки уже набранного текста.
//!
//! Демон пассивно читает события клавиатур через evdev и копит скан-коды
//! набранного. По горячей клавише он стирает последнее слово (или всю фразу
//! с Shift), переключает раскладку и переигрывает те же самые клавиши.
//!
//! Клавиатура намеренно НЕ перехватывается (`EVIOCGRAB`): чтение не мешает
//! обычному набору. Плата за это — горячая клавиша долетает и до активного
//! приложения, поэтому её стоит выбирать безвредной.

mod config;
mod injector;
mod keys;
mod state;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use evdev::{Device, EventType, Key};

use config::Config;
use injector::Injector;
use state::{Buffer, Stroke};

const VIRTUAL_NAME: &str = "punto-rs virtual keyboard";
const DEFAULT_CONFIG: &str = "/etc/punto-rs/config.conf";
const RESCAN_INTERVAL: Duration = Duration::from_secs(3);

struct KeyEvent {
    code: u16,
    value: i32,
}

fn main() {
    let mut config_path = PathBuf::from(DEFAULT_CONFIG);
    let mut verbose = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-c" | "--config" => match args.next() {
                Some(path) => config_path = PathBuf::from(path),
                None => die("--config требует путь к файлу"),
            },
            "-v" | "--verbose" => verbose = true,
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

    let (cfg, warnings) = Config::load(&config_path);
    for warning in &warnings {
        eprintln!("punto-rs: {warning}");
    }

    let injector = match Injector::new(VIRTUAL_NAME) {
        Ok(injector) => injector,
        Err(err) => die(&format!(
            "не удалось создать виртуальную клавиатуру (/dev/uinput): {err}"
        )),
    };

    let (tx, rx) = mpsc::channel::<KeyEvent>();
    let watched: Arc<Mutex<HashSet<PathBuf>>> = Arc::new(Mutex::new(HashSet::new()));

    // Периодическое пересканирование подхватывает клавиатуры, подключённые
    // уже после старта (беспроводной приёмник, док-станция).
    let devices_filter = cfg.devices.clone();
    thread::spawn(move || loop {
        attach_devices(&tx, &watched, &devices_filter, verbose);
        thread::sleep(RESCAN_INTERVAL);
    });

    eprintln!(
        "punto-rs {} запущен: горячая клавиша {}, переключение раскладки {:?}",
        env!("CARGO_PKG_VERSION"),
        cfg.hotkey,
        cfg.layout_switch
    );

    run(rx, injector, cfg, verbose);
}

fn run(rx: Receiver<KeyEvent>, mut injector: Injector, cfg: Config, verbose: bool) {
    let mut buffer = Buffer::new(cfg.max_strokes);
    let mut shift = false;
    let mut held_commands: HashSet<u16> = HashSet::new();

    while let Ok(event) = rx.recv() {
        let KeyEvent { code, value } = event;

        if keys::is_shift(code) {
            shift = value != 0;
            continue;
        }
        if keys::is_command_modifier(code) {
            if value == 0 {
                held_commands.remove(&code);
            } else {
                held_commands.insert(code);
            }
            continue;
        }

        // Интересуют только нажатия и автоповтор, отпускания игнорируем.
        if value == 0 {
            continue;
        }

        if code == cfg.hotkey {
            if value != 1 {
                continue;
            }
            let strokes: Vec<Stroke> = if shift {
                buffer.phrase().to_vec()
            } else {
                buffer.last_word().to_vec()
            };
            if strokes.is_empty() {
                if verbose {
                    eprintln!("punto-rs: буфер пуст, исправлять нечего");
                }
                continue;
            }
            if verbose {
                eprintln!(
                    "punto-rs: исправляю {} нажатий ({})",
                    strokes.len(),
                    if shift { "фраза" } else { "слово" }
                );
            }
            if let Err(err) = injector.fix(&strokes, &cfg) {
                eprintln!("punto-rs: ошибка ввода: {err}");
            }
            // Отбрасываем всё, что накопилось за время инжекта.
            while rx.try_recv().is_ok() {}
            continue;
        }

        // Сочетание с Ctrl/Alt/Super — это команда, а не набор текста.
        if !held_commands.is_empty() {
            buffer.clear();
            continue;
        }

        if code == keys::KEY_BACKSPACE {
            buffer.backspace();
        } else if keys::is_phrase_end(code) {
            buffer.clear();
        } else if keys::is_separator(code) || keys::is_char(code) {
            buffer.push(code, shift);
        } else {
            // Стрелки, Home/End, Delete и прочая навигация — курсор уехал,
            // накопленное больше не соответствует тексту на экране.
            buffer.clear();
        }
    }
}

fn is_keyboard(device: &Device) -> bool {
    device.supported_keys().is_some_and(|keys| {
        keys.contains(Key::KEY_A) && keys.contains(Key::KEY_Z) && keys.contains(Key::KEY_SPACE)
    })
}

/// Открывает подходящие устройства и заводит на каждое отдельный поток чтения.
fn attach_devices(
    tx: &Sender<KeyEvent>,
    watched: &Arc<Mutex<HashSet<PathBuf>>>,
    filter: &[String],
    verbose: bool,
) {
    for (path, device) in evdev::enumerate() {
        let name = device.name().unwrap_or_default().to_string();

        if name == VIRTUAL_NAME {
            continue;
        }
        let wanted = if filter.is_empty() {
            is_keyboard(&device)
        } else {
            filter.iter().any(|allowed| allowed == &name)
        };
        if !wanted {
            continue;
        }

        {
            let mut set = watched.lock().unwrap();
            if !set.insert(path.clone()) {
                continue; // уже слушаем
            }
        }

        eprintln!("punto-rs: слушаю «{name}» ({})", path.display());

        let tx = tx.clone();
        let watched = watched.clone();
        thread::spawn(move || {
            read_device(device, &tx, &name, verbose);
            watched.lock().unwrap().remove(&path);
            if verbose {
                eprintln!("punto-rs: «{name}» отключилась");
            }
        });
    }
}

fn read_device(mut device: Device, tx: &Sender<KeyEvent>, name: &str, verbose: bool) {
    loop {
        let events = match device.fetch_events() {
            Ok(events) => events,
            Err(err) => {
                if verbose {
                    eprintln!("punto-rs: чтение «{name}» прервано: {err}");
                }
                return;
            }
        };
        for event in events {
            if event.event_type() != EventType::KEY {
                continue;
            }
            let sent = tx.send(KeyEvent {
                code: event.code(),
                value: event.value(),
            });
            if sent.is_err() {
                return; // основной цикл завершился
            }
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
        } else {
            ""
        };
        println!("{:<20} {name:<45} {tag}", path.display());
    }
    if !found {
        eprintln!("punto-rs: устройства не видны — нужны права root на /dev/input/*");
    }
}

fn print_help() {
    println!(
        "punto-rs — исправление раскладки набранного текста

Использование:
  punto-rs [опции]

Опции:
  -c, --config <файл>   путь к конфигу (по умолчанию {DEFAULT_CONFIG})
  -l, --list-devices    показать устройства ввода и выйти
  -v, --verbose         подробный вывод
  -V, --version         версия
  -h, --help            эта справка

Нужны права root: чтение /dev/input/* и создание /dev/uinput."
    );
}

fn die(message: &str) -> ! {
    eprintln!("punto-rs: {message}");
    std::process::exit(2);
}
