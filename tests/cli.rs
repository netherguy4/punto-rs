use std::process::Command;

#[test]
fn config_check_is_read_only_and_needs_no_input_devices() {
    let output = Command::new(env!("CARGO_BIN_EXE_punto-rs"))
        .args(["--check-config", "--config", "config/punto-rs.conf"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("конфиг корректен"));
}

#[test]
fn explicit_missing_config_fails_before_opening_devices() {
    let output = Command::new(env!("CARGO_BIN_EXE_punto-rs"))
        .args(["--config", "/nonexistent/punto-rs.conf"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("не прочитан"));
    assert!(!stderr.contains("uinput"));
    assert!(!stderr.contains("блокировка экземпляра"));
}

#[test]
fn help_and_version_work_without_configuration_or_privileges() {
    for option in ["--help", "--version"] {
        let output = Command::new(env!("CARGO_BIN_EXE_punto-rs"))
            .arg(option)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("punto-rs"));
    }
}
