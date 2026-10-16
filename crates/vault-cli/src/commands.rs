use crate::response::{Response, diagnostic};
use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use std::path::{Path, PathBuf};
use vault_cli::password;
use vault_core::{
    BackupVerification, CreateOptions, Credential, Error, Id, OpenMode, Result, Vault,
    operation::Operation,
};
use vault_i18n::{Locale, TextKey};

/// Select help text before clap handles --help. Clap still validates the option.
pub fn requested_locale(args: &[std::ffi::OsString]) -> Locale {
    let mut args = args.iter().skip(1).take_while(|arg| *arg != "--");
    let mut locale = Locale::default();
    while let Some(arg) = args.next() {
        let value = if arg == "--lang" {
            args.next().and_then(|value| value.to_str())
        } else {
            arg.to_str().and_then(|arg| arg.strip_prefix("--lang="))
        };
        match value {
            Some("en-US") => locale = Locale::EnUs,
            Some("zh-CN") => locale = Locale::ZhCn,
            _ => {}
        }
    }
    locale
}

fn text(name: &'static str) -> Arg {
    Arg::new(name).required(true)
}
fn path(name: &'static str) -> Arg {
    text(name).value_parser(value_parser!(PathBuf))
}
fn id() -> Arg {
    text("id").value_parser(value_parser!(Id))
}
fn flag(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name)
        .long(name)
        .action(ArgAction::SetTrue)
        .help(help)
}
fn parent(locale: Locale) -> Arg {
    Arg::new("parent")
        .long("parent")
        .default_value("root")
        .value_parser(|value: &str| match value {
            "root" => Ok(None),
            value => value.parse::<Id>().map(Some),
        })
        .help(locale.text(TextKey::ParentHelp))
}
fn vault_command(locale: Locale, name: &'static str, help: &'static str) -> Command {
    Command::new(name)
        .about(help)
        .arg(path("vault"))
        .arg(flag("recovery-key", locale.text(TextKey::RecoveryHelp)))
}
fn readable(locale: Locale, name: &'static str, help: &'static str) -> Command {
    vault_command(locale, name, help).arg(
        Arg::new("commit")
            .long("commit")
            .value_parser(value_parser!(Id))
            .help(locale.text(TextKey::CommitHelp)),
    )
}

fn localized_help(command: Command, locale: Locale) -> Command {
    command
        .help_template(locale.text(TextKey::HelpTemplate))
        .subcommand_help_heading(locale.text(TextKey::CommandsHeading))
        .disable_help_flag(true)
        .arg(
            Arg::new("help")
                .short('h')
                .long("help")
                .action(ArgAction::Help)
                .help(locale.text(TextKey::HelpDescription)),
        )
        .mut_args(|arg| {
            let heading = if arg.get_long().is_some() || arg.get_short().is_some() {
                TextKey::OptionsHeading
            } else {
                TextKey::ArgumentsHeading
            };
            let takes_values = arg.get_action().takes_values();
            let arg = arg.help_heading(locale.text(heading));
            if takes_values {
                arg.hide_default_value(true).hide_possible_values(true)
            } else {
                arg
            }
        })
}

pub fn command(locale: Locale) -> Command {
    let vault_command = |name, help| vault_command(locale, name, help);
    let readable = |name, help| readable(locale, name, help);
    let parent = || parent(locale);
    let command = Command::new("bkm")
        .version(env!("CARGO_PKG_VERSION"))
        .disable_version_flag(true)
        .disable_help_subcommand(true)
        .arg(
            Arg::new("version")
                .short('V')
                .long("version")
                .action(ArgAction::Version)
                .help(locale.text(TextKey::VersionDescription)),
        )
        .about(vault_core::PRODUCT_NAME)
        .after_help(locale.text(TextKey::ExperimentalWarning))
        .arg(
            Arg::new("lang")
                .long("lang")
                .global(true)
                .value_parser(["zh-CN", "en-US"])
                .default_value("zh-CN")
                .help(locale.text(TextKey::LanguageHelp)),
        )
        .arg(flag("json", locale.text(TextKey::JsonHelp)).global(true))
        .subcommand(
            Command::new("create")
                .about(locale.text(TextKey::CreateHelp))
                .arg(path("vault"))
                .arg(flag("recovery", locale.text(TextKey::GenerateRecoveryHelp))),
        )
        .subcommand(
            readable("list", locale.text(TextKey::ListHelp))
                .arg(flag("trash", locale.text(TextKey::TrashListHelp))),
        )
        .subcommand(
            readable("search", locale.text(TextKey::SearchHelp))
                .arg(text("query"))
                .arg(flag("trash", locale.text(TextKey::TrashSearchHelp))),
        )
        .subcommand(
            vault_command("mkdir", locale.text(TextKey::MkdirHelp))
                .arg(text("name"))
                .arg(parent()),
        )
        .subcommand(
            vault_command("import", locale.text(TextKey::ImportHelp))
                .arg(path("source"))
                .arg(text("name"))
                .arg(parent()),
        )
        .subcommand(
            vault_command("replace", locale.text(TextKey::ReplaceHelp))
                .arg(id())
                .arg(path("source")),
        )
        .subcommand(
            readable("export", locale.text(TextKey::ExportHelp))
                .arg(id())
                .arg(path("destination")),
        )
        .subcommand(
            vault_command("rename", locale.text(TextKey::RenameHelp))
                .arg(id())
                .arg(text("name")),
        )
        .subcommand(
            vault_command("move", locale.text(TextKey::MoveHelp))
                .arg(id())
                .arg(parent()),
        )
        .subcommand(vault_command("trash", locale.text(TextKey::TrashHelp)).arg(id()))
        .subcommand(vault_command("restore", locale.text(TextKey::RestoreHelp)).arg(id()))
        .subcommand(
            vault_command("empty-trash", locale.text(TextKey::EmptyTrashHelp)).arg(
                Arg::new("confirm")
                    .long("confirm")
                    .required(true)
                    .num_args(0)
                    .action(ArgAction::SetTrue)
                    .help(locale.text(TextKey::ConfirmEmptyHelp)),
            ),
        )
        .subcommand(
            vault_command("gc", locale.text(TextKey::CollectHelp)).arg(
                Arg::new("confirm")
                    .long("confirm")
                    .required(true)
                    .num_args(0)
                    .action(ArgAction::SetTrue)
                    .help(locale.text(TextKey::ConfirmCollectHelp)),
            ),
        )
        .subcommand(readable("space", locale.text(TextKey::SpaceHelp)))
        .subcommand(vault_command("inspect", locale.text(TextKey::InspectHelp)))
        .subcommand(readable("check", locale.text(TextKey::CheckHelp)))
        .subcommands(["backup", "restore-vault"].map(|name| {
            vault_command(
                name,
                locale.text(if name == "backup" {
                    TextKey::BackupHelp
                } else {
                    TextKey::RestoreVaultHelp
                }),
            )
            .arg(path("destination").help(locale.text(TextKey::BackupDestinationHelp)))
            .arg(
                Arg::new("verification")
                    .long("verification")
                    .value_parser(["copy", "full"])
                    .default_value("full")
                    .help(locale.text(TextKey::VerificationHelp)),
            )
        }))
        .subcommand(
            readable("batch", locale.text(TextKey::BatchHelp))
                .arg(path("input").help(locale.text(TextKey::BatchInputHelp))),
        )
        .subcommand(
            Command::new("help")
                .about(locale.text(TextKey::HelpCommandDescription))
                .arg(Arg::new("command").value_parser([
                    "create",
                    "list",
                    "search",
                    "mkdir",
                    "import",
                    "replace",
                    "export",
                    "rename",
                    "move",
                    "trash",
                    "restore",
                    "empty-trash",
                    "gc",
                    "space",
                    "inspect",
                    "check",
                    "backup",
                    "restore-vault",
                    "batch",
                    "help",
                ])),
        )
        .mut_subcommands(|command| localized_help(command, locale));
    localized_help(command, locale)
}

pub(crate) fn get<'a, T: Clone + Send + Sync + 'static>(args: &'a ArgMatches, name: &str) -> &'a T {
    args.get_one(name).expect("required command argument")
}
fn target_parent(args: &ArgMatches) -> Result<Option<Id>> {
    Ok(*get::<Option<Id>>(args, "parent"))
}

pub(crate) fn open(
    args: &ArgMatches,
    path: &Path,
    mode: OpenMode,
    locale: Locale,
    operation: &Operation,
) -> Result<Vault> {
    // Credentials leave scope immediately after unlock, before long file operations.
    if args.get_flag("recovery-key") {
        let key = password::read_recovery_in(locale)?;
        Vault::open_with_operation(path, Credential::Recovery(&key), mode, operation)
    } else {
        let key = password::read_password_in(false, locale)?;
        Vault::open_with_operation(path, Credential::Password(&key), mode, operation)
    }
}

pub(crate) fn readonly(name: &str) -> bool {
    ["list", "search", "export", "check", "space"].contains(&name)
}

pub fn run(matches: &ArgMatches, operation: &Operation, locale: Locale) -> Result<()> {
    let (name, args) = matches.subcommand().ok_or(Error::InvalidInput)?;
    let path = get::<PathBuf>(args, "vault");
    if name == "batch" {
        return crate::batch::run(args, operation, locale);
    }
    if name == "create" {
        let password = password::read_password_in(true, locale)?;
        operation.check_cancelled()?;
        let (_, recovery) = Vault::create_with_operation(
            path,
            &password,
            CreateOptions {
                recovery_key: args.get_flag("recovery"),
                ..CreateOptions::default()
            },
            operation,
        )?;
        drop(password);
        if let Some(recovery) = recovery {
            password::show_recovery_in(&recovery, locale).inspect_err(|_| {
                diagnostic(locale.text(TextKey::RecoveryDisplayFailed));
            })?;
        }
        return Response::Created.print(args.get_flag("json"), true, locale);
    }
    if name == "inspect" {
        let report = if args.get_flag("recovery-key") {
            let key = password::read_recovery_in(locale)?;
            Vault::inspect(path, Credential::Recovery(&key))?
        } else {
            let key = password::read_password_in(false, locale)?;
            Vault::inspect(path, Credential::Password(&key))?
        };
        return Response::Inspection(report).print(args.get_flag("json"), false, locale);
    }
    if matches!(name, "backup" | "restore-vault") {
        let vault = open(args, path, OpenMode::ReadOnly, locale, operation)?;
        let verification = match get::<String>(args, "verification").as_str() {
            "copy" => BackupVerification::Copy,
            _ => BackupVerification::Full,
        };
        let report =
            vault.backup_to(get::<PathBuf>(args, "destination"), verification, operation)?;
        return Response::Backup(report).print(args.get_flag("json"), true, locale);
    }
    let mode = if readonly(name) {
        args.get_one::<Id>("commit")
            .copied()
            .map_or(OpenMode::ReadOnly, OpenMode::Recovery)
    } else {
        OpenMode::ReadWrite
    };
    let mut vault = open(args, path, mode, locale, operation)?;
    execute(name, args, &mut vault, operation)?.print(
        args.get_flag("json"),
        !readonly(name) || name == "export",
        locale,
    )
}

pub(crate) fn execute<'a>(
    name: &str,
    args: &'a ArgMatches,
    vault: &'a mut Vault,
    operation: &Operation,
) -> Result<Response<'a>> {
    operation.check_cancelled()?;
    Ok(match name {
        "list" | "search" => {
            let query = if name == "search" {
                get::<String>(args, "query").as_str()
            } else {
                ""
            };
            Response::Entries(Box::new(vault.search(query, args.get_flag("trash"))))
        }
        "mkdir" => Response::Id(vault.mkdir(
            target_parent(args)?,
            get::<String>(args, "name"),
            operation,
        )?),
        "import" => Response::Id(vault.import_file(
            target_parent(args)?,
            get::<String>(args, "name"),
            get::<PathBuf>(args, "source"),
            operation,
        )?),
        "replace" => {
            vault.replace_file(
                *get::<Id>(args, "id"),
                get::<PathBuf>(args, "source"),
                operation,
            )?;
            Response::Empty
        }
        "export" => {
            vault.export_file(
                *get::<Id>(args, "id"),
                get::<PathBuf>(args, "destination"),
                operation,
            )?;
            Response::Empty
        }
        "rename" => {
            vault.rename(
                *get::<Id>(args, "id"),
                get::<String>(args, "name"),
                operation,
            )?;
            Response::Empty
        }
        "move" => {
            vault.move_entry(*get::<Id>(args, "id"), target_parent(args)?, operation)?;
            Response::Empty
        }
        "trash" => {
            vault.trash_entry(*get::<Id>(args, "id"), operation)?;
            Response::Empty
        }
        "restore" => {
            vault.restore_entry(*get::<Id>(args, "id"), operation)?;
            Response::Empty
        }
        "empty-trash" => Response::Cleanup(vault.empty_trash(operation)?),
        "gc" => Response::Cleanup(vault.collect_garbage(operation)?),
        "space" => Response::Space(vault.storage_usage()?),
        "check" => Response::Content(vault.verify_content(operation)?),
        _ => return Err(Error::InvalidInput),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_commands_default_to_full_and_never_select_history() {
        for name in ["backup", "restore-vault"] {
            for locale in [Locale::ZhCn, Locale::EnUs] {
                let matches = command(locale)
                    .try_get_matches_from(["bkm", name, "source", "new-target", "--json"])
                    .unwrap();
                let args = matches.subcommand().unwrap().1;
                assert_eq!(get::<String>(args, "verification"), "full");
                assert!(args.get_flag("json"));
                assert!(
                    command(locale)
                        .try_get_matches_from([
                            "bkm",
                            name,
                            "source",
                            "new-target",
                            "--commit",
                            "00000000000000000000000000000001"
                        ])
                        .is_err()
                );
            }
        }
    }

    #[test]
    fn language_selection_precedes_help_and_respects_positional_terminator() {
        for args in [
            vec!["bkm", "--lang", "en-US", "--help"],
            vec!["bkm", "list", "vault", "--lang=en-US", "--help"],
            vec!["bkm", "--lang=en-US"],
        ] {
            let args: Vec<_> = args.into_iter().map(Into::into).collect();
            let locale = requested_locale(&args);
            assert_eq!(locale, Locale::EnUs);
            let mut help = Vec::new();
            command(locale).write_long_help(&mut help).unwrap();
            let help = String::from_utf8(help).unwrap();
            assert!(help.contains("Create an experimental vault"));
            assert!(!help.contains("创建实验保险库"));
        }
        let args: Vec<_> = ["bkm", "mkdir", "vault", "--", "--lang=en-US"]
            .into_iter()
            .map(Into::into)
            .collect();
        assert_eq!(requested_locale(&args), Locale::ZhCn);
        let matches = command(Locale::ZhCn).try_get_matches_from(args).unwrap();
        assert_eq!(
            get::<String>(matches.subcommand().unwrap().1, "name"),
            "--lang=en-US"
        );
        assert!(
            command(Locale::ZhCn)
                .try_get_matches_from(["bkm", "--lang", "fr-FR"])
                .is_err()
        );
        for locale in [Locale::ZhCn, Locale::EnUs] {
            let mut command = command(locale);
            command.build();
            for target in [None, Some("create"), Some("list"), Some("help")] {
                let mut command = match target {
                    Some(name) => command.find_subcommand(name).unwrap().clone(),
                    None => command.clone(),
                };
                let mut help = Vec::new();
                command.write_long_help(&mut help).unwrap();
                let help = String::from_utf8(help).unwrap();
                assert!(help.contains(locale.text(TextKey::OptionsHeading)));
                assert!(help.contains(locale.text(TextKey::HelpDescription)));
                if locale == Locale::ZhCn {
                    for english in [
                        "Usage:",
                        "Commands:",
                        "Options:",
                        "Arguments:",
                        "Print help",
                        "Print version",
                        "[default:",
                        "[possible values:",
                    ] {
                        assert!(!help.contains(english), "untranslated {english}: {help}");
                    }
                }
            }
        }
    }
}
