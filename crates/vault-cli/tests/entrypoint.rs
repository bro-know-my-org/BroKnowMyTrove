use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bkm"))
        .args(args)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .expect("CLI should start without a display server")
}

#[test]
fn help_and_version_work_without_a_display() {
    for (args, expected) in [
        (vec!["--help"], "Usage: bkm"),
        (
            vec!["--version"],
            concat!("bkm ", env!("CARGO_PKG_VERSION")),
        ),
    ] {
        let output = run(&args);
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains(expected));
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn unavailable_vault_operation_fails_instead_of_claiming_success() {
    let output = run(&["create"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("error:"));
}
