mod commands;

use std::{
    io::{self, Write},
    process::ExitCode,
};
use vault_core::{Error, operation::Operation};

fn main() -> ExitCode {
    let matches = commands::command().get_matches();
    if matches.subcommand().is_none() {
        let mut stdout = io::stdout().lock();
        return if commands::command()
            .write_help(&mut stdout)
            .and_then(|_| writeln!(stdout))
            .is_ok()
        {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    let operation = Operation::default();
    let cancellation = operation.clone();
    if ctrlc::set_handler(move || cancellation.cancel()).is_err() {
        let _ = writeln!(io::stderr().lock(), "无法安装取消处理器");
        return ExitCode::FAILURE;
    }
    match commands::run(&matches, &operation) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "{error}");
            ExitCode::from(if matches!(error, Error::Cancelled) {
                130
            } else {
                1
            })
        }
    }
}
