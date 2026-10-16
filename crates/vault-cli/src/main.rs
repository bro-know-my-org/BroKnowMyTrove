use vault_i18n::TextKey;
mod batch;
mod commands;
mod response;

use clap::error::ErrorKind;
use std::{
    io::{self, Write},
    process::ExitCode,
};
use vault_core::{Error, operation::Operation};

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().collect();
    let locale = commands::requested_locale(&args);
    let matches = match commands::command(locale).try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(error) => {
            let key = match error.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                    return if error.print().is_ok() {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::FAILURE
                    };
                }
                ErrorKind::MissingRequiredArgument => TextKey::MissingArguments,
                ErrorKind::UnknownArgument | ErrorKind::InvalidSubcommand => {
                    TextKey::UnknownArgument
                }
                ErrorKind::InvalidValue | ErrorKind::ValueValidation | ErrorKind::InvalidUtf8 => {
                    TextKey::InvalidArgumentValue
                }
                ErrorKind::ArgumentConflict
                | ErrorKind::TooManyValues
                | ErrorKind::TooFewValues
                | ErrorKind::WrongNumberOfValues => TextKey::ArgumentConflict,
                _ => TextKey::ParseFailed,
            };
            // clap errors can contain plaintext names, paths, and argument values.
            response::diagnostic(locale.text(key));
            return ExitCode::from(2);
        }
    };
    if matches.subcommand().is_none() || matches.subcommand_name() == Some("help") {
        let target = matches
            .subcommand()
            .filter(|(name, _)| *name == "help")
            .and_then(|(_, args)| args.get_one::<String>("command"));
        let mut root = commands::command(locale);
        root.build();
        let command = match target {
            Some(name) => root.find_subcommand_mut(name).expect("validated command"),
            None => &mut root,
        };
        let mut stdout = io::stdout().lock();
        return if command
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
        response::diagnostic(locale.text(TextKey::CancelHandlerFailed));
        return ExitCode::FAILURE;
    }
    match commands::run(&matches, &operation, locale) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            response::diagnostic(locale.error(&error));
            ExitCode::from(if matches!(error, Error::Cancelled) {
                130
            } else {
                1
            })
        }
    }
}
