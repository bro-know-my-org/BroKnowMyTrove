use std::{
    io::Read,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

fn run(args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bkm"));
    command.args(args).stdout(Stdio::piped());
    run_command(command)
}

fn capture(mut pipe: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes).unwrap();
        bytes
    })
}

fn run_command(mut command: Command) -> Output {
    // Drain both pipes concurrently so even a verbose regression cannot block exit.
    let mut child = command
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("CLI should start without a display server");
    let stdout = child.stdout.take().map(capture);
    let stderr = capture(child.stderr.take().unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            child.kill().unwrap();
            break child.wait().unwrap();
        }
        thread::sleep(Duration::from_millis(20));
    };
    let output = Output {
        status,
        stdout: stdout
            .map(|reader| reader.join().unwrap())
            .unwrap_or_default(),
        stderr: stderr.join().unwrap(),
    };
    assert!(!timed_out, "CLI timed out: {output:?}");
    output
}

#[test]
fn help_and_version_work_without_a_display() {
    for (args, expected) in [
        (vec![], "用法: bkm"),
        (vec!["--help"], "用法: bkm"),
        (vec!["--lang", "en-US", "--help"], "Usage: bkm"),
        (
            vec!["create", "--lang=en-US", "--help"],
            "Usage: bkm create",
        ),
        (
            vec!["help", "create", "--lang", "zh-CN"],
            "用法: bkm create",
        ),
        (vec!["help", "list", "--lang", "en-US"], "Usage: bkm list"),
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

#[cfg(target_os = "linux")]
#[test]
fn help_output_failure_is_controlled() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bkm"));
    command.stdout(
        std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap(),
    );
    let output = run_command(command);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
}

#[test]
fn missing_arguments_fail_before_requesting_credentials() {
    let output = run(&["create"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("缺少必需参数"));
}

#[test]
fn parsing_errors_are_localized_and_do_not_echo_argument_values() {
    for (locale, missing, unknown, invalid) in [
        ("zh-CN", "缺少必需参数", "未知选项或命令", "参数值无效"),
        (
            "en-US",
            "required arguments are missing",
            "unknown option or command",
            "invalid argument value",
        ),
    ] {
        for (args, expected) in [
            (vec!["--lang", locale, "create"], missing),
            (vec!["--lang", locale, "--private-unknown-option"], unknown),
            (
                vec![
                    "--lang",
                    locale,
                    "mkdir",
                    "private-vault-path",
                    "private-file-name",
                    "--parent",
                    "private-invalid-id",
                ],
                invalid,
            ),
        ] {
            let output = run(&args);
            assert_eq!(output.status.code(), Some(2));
            assert!(output.stdout.is_empty());
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains(expected), "{stderr}");
            assert!(!stderr.contains("private-"), "{stderr}");
        }
    }
}
