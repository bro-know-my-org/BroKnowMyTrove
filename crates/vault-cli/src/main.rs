mod commands;

use std::process::ExitCode;
use vault_core::{Error, operation::Operation};

fn main() -> ExitCode {
    let matches = commands::command().get_matches();
    if matches.subcommand().is_none() {
        let _ = commands::command().print_help();
        println!();
        return ExitCode::SUCCESS;
    }
    let operation = Operation::default();
    let cancellation = operation.clone();
    if ctrlc::set_handler(move || cancellation.cancel()).is_err() {
        eprintln!("无法安装取消处理器");
        return ExitCode::FAILURE;
    }
    match commands::run(&matches, &operation) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(if matches!(error, Error::Cancelled) {
                130
            } else {
                1
            })
        }
    }
}
