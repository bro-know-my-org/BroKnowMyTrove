use crate::response::{Response, diagnostic};
use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use std::path::{Path, PathBuf};
use vault_cli::password;
use vault_core::{
    CreateOptions, Credential, Error, Id, OpenMode, Result, Vault, operation::Operation,
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
        .arg(flag("json", "输出单行 JSON 结果；错误仍写入 stderr").global(true))
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

pub(crate) fn get<'a, T: Clone + Send + Sync + 'static>(args: &'a ArgMatches, name: &str) -> &'a T {
    args.get_one(name).expect("required command argument")
}
fn target_parent(args: &ArgMatches) -> Result<Option<Id>> {
    match get::<String>(args, "parent").as_str() {
        "root" => Ok(None),
        id => Ok(Some(id.parse()?)),
    }
}

pub(crate) fn open(args: &ArgMatches, path: &Path, mode: OpenMode) -> Result<Vault> {
    // Credentials leave scope immediately after unlock, before long file operations.
    if args.get_flag("recovery-key") {
        let key = password::read_recovery()?;
        Vault::open(path, Credential::Recovery(&key), mode)
    } else {
        let key = password::read_password(false)?;
        Vault::open(path, Credential::Password(&key), mode)
    }
}

pub(crate) fn readonly(name: &str) -> bool {
    ["list", "search", "export", "check", "space"].contains(&name)
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
        return Response::Created.print(args.get_flag("json"), true);
    }
    if name == "inspect" {
        let report = if args.get_flag("recovery-key") {
            let key = password::read_recovery()?;
            Vault::inspect(path, Credential::Recovery(&key))?
        } else {
            let key = password::read_password(false)?;
            Vault::inspect(path, Credential::Password(&key))?
        };
        return Response::Inspection(report).print(args.get_flag("json"), false);
    }
    let mode = if readonly(name) {
        args.get_one::<Id>("commit")
            .copied()
            .map_or(OpenMode::ReadOnly, OpenMode::Recovery)
    } else {
        OpenMode::ReadWrite
    };
    let mut vault = open(args, path, mode)?;
    execute(name, args, &mut vault, operation)?
        .print(args.get_flag("json"), !readonly(name) || name == "export")
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
