use serde_json::{Value, json};
use std::{
    io::{self, Write},
    sync::mpsc,
    thread,
    time::Duration,
};
use vault_core::{
    CleanupReport, ContentReport, Entry, EntryKind, Error, Id, Inspection, Issue, Result,
    StorageUsage,
};

pub enum Response<'a> {
    Empty,
    Created,
    Id(Id),
    Entries(Box<dyn Iterator<Item = &'a Entry> + 'a>),
    Inspection(Inspection),
    Space(StorageUsage),
    Content(ContentReport),
    Cleanup(CleanupReport),
}

pub fn diagnostic(message: &str) {
    let message = message.to_owned();
    let (send, receive) = mpsc::sync_channel(1);
    // Diagnostics are best-effort, including when 2>&1 shares a stalled pipe.
    // The session owner must remain free to drop Vault and exit on cancellation.
    if thread::Builder::new()
        .spawn(move || {
            let _ = writeln!(io::stderr().lock(), "{message}");
            let _ = send.send(());
        })
        .is_ok()
    {
        let _ = receive.recv_timeout(Duration::from_millis(50));
    }
}

pub fn output(value: &impl std::fmt::Display) -> Result<()> {
    writeln!(io::stdout().lock(), "{value}").map_err(Error::from)
}

impl Response<'_> {
    pub fn into_json(mut self) -> (Value, Result<()>) {
        let value = self.json();
        (value, self.finish())
    }

    fn json(&mut self) -> Value {
        match self {
            Self::Empty | Self::Created => json!({"ok": true}),
            Self::Id(id) => json!({"id": id.to_string()}),
            Self::Entries(entries) => {
                let mut value = json!({});
                value["entries"] = Value::Array(entries.by_ref().map(entry_json).collect());
                value
            }
            Self::Inspection(report) => {
                let mut value = json!({
                "head": report.head.map(|id| id.to_string()),
                "temporary_files": report.temporary_files,
                });
                value["commits"] = Value::Array(
                    report
                        .commits
                        .iter()
                        .map(|c| {
                            json!({
                                "id": c.id.to_string(), "sequence": c.sequence,
                                "snapshot_present": c.snapshot_present,
                            })
                        })
                        .collect(),
                );
                value["issues"] = Value::Array(report.issues.iter().map(issue).collect());
                value
            }
            Self::Space(usage) => json!({
                "active_object_bytes": usage.active_object_bytes,
                "trash_object_bytes": usage.trash_object_bytes,
                "reclaimable_object_bytes": usage.reclaimable_object_bytes,
            }),
            Self::Content(report) => {
                let mut value = json!({
                "verified_objects": report.verified_objects,
                "plaintext_bytes": report.plaintext_bytes,
                });
                value["failed_objects"] = Value::Array(
                    report
                        .failed_objects
                        .iter()
                        .map(|id| Value::String(id.to_string()))
                        .collect(),
                );
                value
            }
            Self::Cleanup(report) => json!({
                "trash_committed": report.trash_committed,
                "retired_snapshots": report.retired_snapshots,
                "reclaimed_object_bytes": report.reclaimed_object_bytes,
                "removed_temporary_bytes": report.removed_temporary_bytes,
                "failure": report.failure.as_ref().map(ToString::to_string),
            }),
        }
    }

    pub fn finish(self) -> Result<()> {
        match self {
            Self::Inspection(report) if !report.issues.is_empty() => Err(Error::ReadOnly),
            Self::Content(report) if !report.failed_objects.is_empty() => {
                Err(Error::Authentication)
            }
            Self::Cleanup(report) => {
                if report.failure.is_some() {
                    diagnostic("清理未完成，可重试；已提交的目录变更不会回滚。");
                }
                report.failure.map_or(Ok(()), Err)
            }
            _ => Ok(()),
        }
    }

    pub fn print(mut self, json: bool, may_change: bool) -> Result<()> {
        let (printed, result) = if json {
            let (value, result) = self.into_json();
            (output(&value), result)
        } else {
            (self.print_text(), self.finish())
        };
        if printed.is_err() && may_change {
            diagnostic("保险库操作已执行，但结果输出失败；请检查保险库状态。");
        }
        // Report semantic failure even if the report could not be printed.
        result?;
        printed
    }

    fn print_text(&mut self) -> Result<()> {
        match self {
            Self::Empty => Ok(()),
            Self::Created => output(&"保险库已创建"),
            Self::Id(id) => output(id),
            Self::Entries(entries) => {
                for entry in entries {
                    output(&entry_json(entry))?;
                }
                Ok(())
            }
            Self::Inspection(report) => output(&format_args!("{report:#?}")),
            Self::Space(usage) => output(&format_args!(
                "{usage:#?}\n回收站空间须先清空；回收无引用对象将退役所有本地历史快照。"
            )),
            Self::Content(report) => output(&format_args!("{report:#?}")),
            Self::Cleanup(report) => output(&format_args!(
                "回收站提交={}；退役快照={}；已删除对象字节={}；临时文件字节={}",
                report.trash_committed,
                report.retired_snapshots,
                report.reclaimed_object_bytes,
                report.removed_temporary_bytes
            )),
        }
    }
}

fn entry_json(entry: &Entry) -> Value {
    let kind = match entry.kind {
        EntryKind::Directory => json!({"type": "directory"}),
        EntryKind::File {
            length,
            modified_ns,
            ..
        } => {
            json!({"type": "file", "length": length, "modified_ns": modified_ns})
        }
    };
    json!({
        "id": entry.id.to_string(), "parent": entry.parent.map(|id| id.to_string()),
        "name": entry.name, "trashed": entry.is_trashed(), "kind": kind,
    })
}

fn issue(issue: &Issue) -> Value {
    let (kind, id) = match issue {
        Issue::DamagedCommit(id) => ("damaged_commit", Some(id)),
        Issue::InvalidCommitGraph => ("invalid_commit_graph", None),
        Issue::UnexpectedPhysicalEntry => ("unexpected_physical_entry", None),
        Issue::MissingSnapshot(id) => ("missing_snapshot", Some(id)),
        Issue::DamagedSnapshot(id) => ("damaged_snapshot", Some(id)),
        Issue::MissingObject(id) => ("missing_object", Some(id)),
        Issue::DamagedObject(id) => ("damaged_object", Some(id)),
    };
    json!({"kind": kind, "id": id.map(ToString::to_string)})
}
