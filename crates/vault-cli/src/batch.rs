use crate::{
    commands,
    response::{diagnostic, output},
};
use clap::ArgMatches;
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{self, Read},
    path::PathBuf,
    sync::mpsc::{self, RecvTimeoutError},
    thread,
    time::Duration,
};
use vault_core::{Error, Id, OpenMode, Result, operation::Operation};

const INPUT_LIMIT: u64 = 1024 * 1024;
const COMMAND_LIMIT: usize = 1000;

fn parse(input: impl Read) -> Result<Vec<ArgMatches>> {
    let mut bytes = Vec::new();
    input.take(INPUT_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > INPUT_LIMIT {
        return Err(Error::ResourceLimit);
    }
    let input = std::str::from_utf8(&bytes).map_err(|_| Error::InvalidInput)?;
    let mut plan = Vec::new();
    for (index, line) in input.lines().enumerate() {
        if plan.len() == COMMAND_LIMIT {
            return Err(Error::ResourceLimit);
        }
        plan.push(parse_line(line).inspect_err(|_| {
            diagnostic(&format!(
                "批量输入第 {} 行无效；未执行任何操作。",
                index + 1
            ));
        })?);
    }
    if plan.is_empty() {
        return Err(Error::InvalidInput);
    }
    Ok(plan)
}

fn parse_line(line: &str) -> Result<ArgMatches> {
    let tokens: Vec<String> = serde_json::from_str(line).map_err(|_| Error::InvalidInput)?;
    let name = tokens.first().ok_or(Error::InvalidInput)?;
    if !commands::readonly(name)
        && ![
            "mkdir",
            "import",
            "replace",
            "rename",
            "move",
            "trash",
            "restore",
            "empty-trash",
            "gc",
        ]
        .contains(&name.as_str())
    {
        return Err(Error::InvalidInput);
    }
    // Reuse the CLI grammar, injecting a placeholder for the one shared vault.
    // Never render clap errors: they may contain plaintext names or source paths.
    let matches = commands::command()
        .try_get_matches_from(
            ["bkm", name, "batch-vault"]
                .into_iter()
                .map(String::from)
                .chain(tokens.iter().skip(1).cloned()),
        )
        .map_err(|_| Error::InvalidInput)?;
    let (_, args) = matches.subcommand().ok_or(Error::InvalidInput)?;
    if args.get_flag("recovery-key") || args.try_get_one::<Id>("commit").ok().flatten().is_some() {
        return Err(Error::InvalidInput);
    }
    Ok(matches)
}

pub fn run(args: &ArgMatches, operation: &Operation) -> Result<()> {
    let input = commands::get::<PathBuf>(args, "input").clone();
    // Validate the entire bounded plan before prompting or opening a vault.
    let plan = interruptible(
        move || {
            if input.as_os_str() == "-" {
                parse(io::stdin().lock())
            } else {
                parse(File::open(input)?)
            }
        },
        operation,
    )?;
    let readonly = plan
        .iter()
        .all(|m| commands::readonly(m.subcommand_name().unwrap_or_default()));
    let commit = args.get_one::<Id>("commit").copied();
    if commit.is_some() && !readonly {
        return Err(Error::InvalidInput);
    }
    operation.check_cancelled()?;
    let mode = if readonly {
        commit.map_or(OpenMode::ReadOnly, OpenMode::Recovery)
    } else {
        OpenMode::ReadWrite
    };
    let mut vault = commands::open(args, commands::get::<PathBuf>(args, "vault"), mode)?;
    for (index, matches) in plan.iter().enumerate() {
        let (name, args) = matches.subcommand().ok_or(Error::InvalidInput)?;
        let (data, result) = match commands::execute(name, args, &mut vault, operation) {
            Ok(response) => response.into_json(),
            Err(error) => (Value::Null, Err(error)),
        };
        let mut record = json!({
            "index": index + 1, "command": name,
            "status": if result.is_ok() { "ok" } else { "error" },
            "error": result.as_ref().err().map(ToString::to_string),
        });
        record["result"] = data;
        let printed = interruptible(move || output(&record), operation);
        if result.is_err() || printed.is_err() {
            diagnostic("批处理已停止；此前完成的操作不会回滚，结果不确定时请检查保险库状态。");
        }
        // Preserve cancellation/uncertain-commit semantics even on output failure.
        result?;
        printed?;
    }
    Ok(())
}

fn interruptible<T: Send + 'static>(
    io: impl FnOnce() -> Result<T> + Send + 'static,
    operation: &Operation,
) -> Result<T> {
    operation.check_cancelled()?;
    let (send, receive) = mpsc::sync_channel(1);
    // Stalled input/output must not swallow SIGINT. On cancellation main exits;
    // the I/O worker owns no vault, lock, or credentials and is not joined.
    thread::Builder::new().spawn(move || {
        let _ = send.send(io());
    })?;
    loop {
        operation.check_cancelled()?;
        match receive.recv_timeout(Duration::from_millis(50)) {
            Ok(result) => return result,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(Error::Io(io::ErrorKind::Other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_input_does_not_wait_for_eof() {
        let operation = Operation::default();
        let cancellation = operation.clone();
        let (started, ready) = mpsc::sync_channel(1);
        let (release, blocked) = mpsc::sync_channel(1);
        let cancel = thread::spawn(move || {
            ready.recv().unwrap();
            cancellation.cancel();
        });
        let result = interruptible(
            move || {
                started.send(()).unwrap();
                blocked.recv().unwrap();
                Ok(())
            },
            &operation,
        );
        release.send(()).unwrap();
        cancel.join().unwrap();
        assert!(matches!(result, Err(Error::Cancelled)));
    }

    #[test]
    fn preflight_rejects_invalid_or_unsafe_plans() {
        for input in [
            "",
            "\n",
            "{}",
            "[\"batch\"]",
            "[\"create\"]",
            "[\"inspect\"]",
            "[\"list\",\"--help\"]",
            "[\"gc\"]",
            "[\"empty-trash\"]",
            "[\"list\",\"--recovery-key\"]",
            "[\"list\",\"--commit\",\"00000000000000000000000000000000\"]",
            "[\"mkdir\",\"ok\"]\n[\"mkdir\",\"bad\",\"--parent\",\"invalid\"]",
        ] {
            assert!(parse(input.as_bytes()).is_err(), "accepted {input}");
        }
        assert!(parse("[\"list\"]\n[\"gc\",\"--confirm\"]\n".as_bytes()).is_ok());
        assert!(matches!(
            parse(io::repeat(b' ').take(INPUT_LIMIT + 1)),
            Err(Error::ResourceLimit)
        ));
        assert!(matches!(
            parse("[\"list\"]\n".repeat(COMMAND_LIMIT + 1).as_bytes()),
            Err(Error::ResourceLimit)
        ));
    }
}
