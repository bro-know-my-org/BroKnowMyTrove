use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use std::{
    fmt,
    io::{self, Write},
    path::{Path, PathBuf},
};
use vault_cli::password;
use vault_core::{
    CleanupReport, CreateOptions, Credential, Error, Id, OpenMode, Result, Vault,
    operation::Operation,
};

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
fn parent() -> Arg {
    Arg::new("parent")
        .long("parent")
        .default_value("root")
        .help("目标目录 ID 或 root")
}
fn vault_command(name: &'static str, help: &'static str) -> Command {
    Command::new(name)
        .about(help)
        .arg(path("vault"))
        .arg(flag("recovery-key", "从控制终端读取恢复密钥"))
}
fn readable(name: &'static str, help: &'static str) -> Command {
    vault_command(name, help).arg(
        Arg::new("commit")
            .long("commit")
            .value_parser(value_parser!(Id))
            .help("显式选择历史提交，只读恢复"),
    )
}

pub fn command() -> Command {
    Command::new("bkm")
        .version(env!("CARGO_PKG_VERSION"))
        .about(vault_core::PRODUCT_NAME)
        .after_help(vault_core::DEVELOPMENT_STATUS)
        .subcommand(
            Command::new("create")
                .about("创建实验保险库，密码输入两遍")
                .arg(path("vault"))
                .arg(flag("recovery", "生成并在控制终端一次性显示恢复密钥")),
        )
        .subcommand(
            readable("list", "列出条目，输出 JSON Lines").arg(flag("trash", "只显示回收站")),
        )
        .subcommand(
            readable("search", "搜索解锁后的名称")
                .arg(text("query"))
                .arg(flag("trash", "只搜索回收站")),
        )
        .subcommand(
            vault_command("mkdir", "新建逻辑目录")
                .arg(text("name"))
                .arg(parent()),
        )
        .subcommand(
            vault_command("import", "导入普通文件并保留源文件")
                .arg(path("source"))
                .arg(text("name"))
                .arg(parent()),
        )
        .subcommand(
            vault_command("replace", "替换文件内容")
                .arg(id())
                .arg(path("source")),
        )
        .subcommand(
            readable("export", "完整认证后导出，拒绝覆盖")
                .arg(id())
                .arg(path("destination")),
        )
        .subcommand(
            vault_command("rename", "重命名条目")
                .arg(id())
                .arg(text("name")),
        )
        .subcommand(vault_command("move", "移动条目").arg(id()).arg(parent()))
        .subcommand(vault_command("trash", "移入加密回收站").arg(id()))
        .subcommand(vault_command("restore", "恢复本次删除").arg(id()))
        .subcommand(
            vault_command(
                "empty-trash",
                "清空回收站并退役本地历史快照；不影响独立备份",
            )
            .arg(
                Arg::new("confirm")
                    .long("confirm")
                    .required(true)
                    .num_args(0)
                    .action(ArgAction::SetTrue)
                    .help("确认清空及退役所有本地历史恢复点"),
            ),
        )
        .subcommand(
            vault_command("gc", "退役所有本地历史快照并回收无引用对象").arg(
                Arg::new("confirm")
                    .long("confirm")
                    .required(true)
                    .num_args(0)
                    .action(ArgAction::SetTrue)
                    .help("确认退役所有本地历史恢复点"),
            ),
        )
        .subcommand(readable("space", "显示密文对象空间，单位字节"))
        .subcommand(vault_command("inspect", "认证提交图并检查当前元数据及引用"))
        .subcommand(readable("check", "逐对象完整认证全部内容"))
}

fn get<'a, T: Clone + Send + Sync + 'static>(args: &'a ArgMatches, name: &str) -> &'a T {
    args.get_one(name).expect("required command argument")
}
fn target_parent(args: &ArgMatches) -> Result<Option<Id>> {
    match get::<String>(args, "parent").as_str() {
        "root" => Ok(None),
        id => Ok(Some(id.parse()?)),
    }
}

fn open(args: &ArgMatches, path: &Path, mode: OpenMode) -> Result<Vault> {
    // Credentials leave scope immediately after unlock, before long file operations.
    if args.get_flag("recovery-key") {
        let key = password::read_recovery()?;
        Vault::open(path, Credential::Recovery(&key), mode)
    } else {
        let key = password::read_password(false)?;
        Vault::open(path, Credential::Password(&key), mode)
    }
}

fn output(message: fmt::Arguments<'_>) -> Result<()> {
    writeln!(io::stdout().lock(), "{message}").map_err(Error::from)
}

fn diagnostic(message: &str) {
    let _ = writeln!(io::stderr().lock(), "{message}");
}

fn committed_output(message: fmt::Arguments<'_>) -> Result<()> {
    output(message)
        .inspect_err(|_| diagnostic("保险库操作已执行，但结果输出失败；请检查保险库状态。"))
}

pub fn run(matches: &ArgMatches, operation: &Operation) -> Result<()> {
    let (name, args) = matches.subcommand().ok_or(Error::InvalidInput)?;
    let path = get::<PathBuf>(args, "vault");
    if name == "create" {
        let password = password::read_password(true)?;
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
            password::show_recovery(&recovery).inspect_err(|_| {
                diagnostic("保险库已创建，但恢复密钥显示失败；仍可使用密码打开保险库。");
            })?;
        }
        return committed_output(format_args!("保险库已创建"));
    }
    if name == "inspect" {
        let report = if args.get_flag("recovery-key") {
            let key = password::read_recovery()?;
            Vault::inspect(path, Credential::Recovery(&key))?
        } else {
            let key = password::read_password(false)?;
            Vault::inspect(path, Credential::Password(&key))?
        };
        let printed = output(format_args!("{report:#?}"));
        return if report.issues.is_empty() {
            printed
        } else {
            Err(Error::ReadOnly)
        };
    }
    let readonly = ["list", "search", "export", "check", "space"].contains(&name);
    let mode = if readonly {
        args.get_one::<Id>("commit")
            .copied()
            .map_or(OpenMode::ReadOnly, OpenMode::Recovery)
    } else {
        OpenMode::ReadWrite
    };
    let mut vault = open(args, path, mode)?;
    operation.check_cancelled()?;
    match name {
        "list" | "search" => {
            let query = if name == "search" {
                get::<String>(args, "query").as_str()
            } else {
                ""
            };
            for entry in vault.search(query, args.get_flag("trash")) {
                let kind = match entry.kind {
                    vault_core::EntryKind::Directory => serde_json::json!({"type": "directory"}),
                    vault_core::EntryKind::File {
                        length,
                        modified_ns,
                        ..
                    } => {
                        serde_json::json!({"type": "file", "length": length, "modified_ns": modified_ns})
                    }
                };
                output(format_args!(
                    "{}",
                    serde_json::json!({
                        "id": entry.id.to_string(), "parent": entry.parent.map(|id| id.to_string()),
                        "name": entry.name, "trashed": entry.is_trashed(), "kind": kind,
                    })
                ))?;
            }
        }
        "mkdir" => committed_output(format_args!(
            "{}",
            vault.mkdir(target_parent(args)?, get::<String>(args, "name"), operation)?
        ))?,
        "import" => committed_output(format_args!(
            "{}",
            vault.import_file(
                target_parent(args)?,
                get::<String>(args, "name"),
                get::<PathBuf>(args, "source"),
                operation
            )?
        ))?,
        "replace" => vault.replace_file(
            *get::<Id>(args, "id"),
            get::<PathBuf>(args, "source"),
            operation,
        )?,
        "export" => vault.export_file(
            *get::<Id>(args, "id"),
            get::<PathBuf>(args, "destination"),
            operation,
        )?,
        "rename" => vault.rename(
            *get::<Id>(args, "id"),
            get::<String>(args, "name"),
            operation,
        )?,
        "move" => vault.move_entry(*get::<Id>(args, "id"), target_parent(args)?, operation)?,
        "trash" => vault.trash_entry(*get::<Id>(args, "id"), operation)?,
        "restore" => vault.restore_entry(*get::<Id>(args, "id"), operation)?,
        "empty-trash" => cleanup(vault.empty_trash(operation)?)?,
        "gc" => cleanup(vault.collect_garbage(operation)?)?,
        "space" => output(format_args!(
            "{:#?}\n回收站空间须先清空；回收无引用对象将退役所有本地历史快照。",
            vault.storage_usage()?
        ))?,
        "check" => {
            let report = vault.verify_content(operation)?;
            let printed = output(format_args!("{report:#?}"));
            if !report.failed_objects.is_empty() {
                return Err(Error::Authentication);
            }
            printed?;
        }
        _ => return Err(Error::InvalidInput),
    }
    Ok(())
}

fn cleanup(report: CleanupReport) -> Result<()> {
    let printed = committed_output(format_args!(
        "回收站提交={}；退役快照={}；已删除对象字节={}；临时文件字节={}",
        report.trash_committed,
        report.retired_snapshots,
        report.reclaimed_object_bytes,
        report.removed_temporary_bytes
    ));
    if let Some(error) = report.failure {
        diagnostic("清理未完成，可重试；已提交的目录变更不会回滚。");
        return Err(error);
    }
    printed
}
