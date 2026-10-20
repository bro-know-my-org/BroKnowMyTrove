use serde_json::{Value, json};
use std::{
    io::{self, Write},
    sync::mpsc,
    thread,
    time::Duration,
};
use vault_core::{
    BackupReport, BackupVerification, CleanupReport, ContentReport, Entry, EntryKind, Error, Id,
    Inspection, Issue, Result, StorageUsage, operation::Operation,
};
use vault_i18n::{Locale, TextKey};

pub enum Response<'a> {
    Empty,
    Created,
    Id(Id),
    Entries(Box<dyn Iterator<Item = &'a Entry> + 'a>),
    Inspection(Inspection),
    Space(StorageUsage),
    Content(ContentReport),
    Cleanup(CleanupReport),
    Backup(BackupReport),
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

struct Output {
    lines: mpsc::SyncSender<String>,
    results: mpsc::Receiver<Result<()>>,
}

impl Output {
    fn new() -> io::Result<Self> {
        let (lines, input) = mpsc::sync_channel::<String>(1);
        let (finished, results) = mpsc::sync_channel(1);
        // One bounded worker per response; it owns no session or credentials.
        // Cancellation can drop the session without joining a blocked writer.
        thread::Builder::new().spawn(move || {
            for line in input {
                if finished.send(output(&line)).is_err() {
                    break;
                }
            }
        })?;
        Ok(Self { lines, results })
    }

    fn write(&self, value: &dyn std::fmt::Display, operation: &Operation) -> Result<()> {
        operation.check_cancelled()?;
        // Each line is acknowledged before the next is sent, bounding memory
        // and keeping this send independent of the stdout consumer's progress.
        self.lines
            .send(value.to_string())
            .map_err(|_| Error::Io(io::ErrorKind::Other))?;
        loop {
            operation.check_cancelled()?;
            match self.results.recv_timeout(Duration::from_millis(50)) {
                Ok(result) => {
                    operation.check_cancelled()?;
                    return result;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(Error::Io(io::ErrorKind::Other));
                }
            }
        }
    }
}

impl Response<'_> {
    pub fn into_json(mut self, locale: Locale) -> (Value, Result<()>) {
        let value = self.json();
        (value, self.finish(locale))
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
            Self::Backup(report) => json!({
                "commit": report.commit.to_string(),
                "copied_files": report.copied_files,
                "ciphertext_bytes": report.ciphertext_bytes,
                "verification": match report.verification {
                    BackupVerification::Copy => "copy",
                    BackupVerification::Full => "full",
                },
                "verified_objects": report.verified_objects,
                "publication_failure": report.publication_failure.as_ref().map(ToString::to_string),
            }),
        }
    }

    pub fn finish(self, locale: Locale) -> Result<()> {
        match self {
            Self::Inspection(report) if !report.issues.is_empty() => Err(Error::ReadOnly),
            Self::Content(report) if !report.failed_objects.is_empty() => {
                Err(Error::Authentication)
            }
            Self::Cleanup(report) => {
                if report.failure.is_some() {
                    diagnostic(locale.text(TextKey::CleanupFailed));
                }
                report.failure.map_or(Ok(()), Err)
            }
            Self::Backup(report) => {
                if report.publication_failure.is_some() {
                    diagnostic(locale.text(TextKey::BackupUncertain));
                }
                report.publication_failure.map_or(Ok(()), Err)
            }
            _ => Ok(()),
        }
    }

    pub fn print(
        mut self,
        json: bool,
        may_change: bool,
        locale: Locale,
        operation: &Operation,
    ) -> Result<()> {
        let printer = Output::new();
        let output = |value: &dyn std::fmt::Display| {
            printer
                .as_ref()
                .map_err(|error| Error::Io(error.kind()))?
                .write(value, operation)
        };
        let (printed, result) = if json {
            let (value, result) = self.into_json(locale);
            (output(&value), result)
        } else {
            (self.print_text(locale, output), self.finish(locale))
        };
        if printed.is_err() && may_change {
            diagnostic(locale.text(TextKey::OutputFailed));
        }
        // Report semantic failure even if the report could not be printed.
        result?;
        printed
    }

    fn print_text(
        &mut self,
        locale: Locale,
        mut output: impl FnMut(&dyn std::fmt::Display) -> Result<()>,
    ) -> Result<()> {
        match self {
            Self::Empty => Ok(()),
            Self::Created => output(&locale.text(TextKey::Created)),
            Self::Id(id) => output(id),
            Self::Entries(entries) => {
                for entry in entries {
                    output(&entry_json(entry))?;
                }
                Ok(())
            }
            Self::Inspection(report) => {
                output(&format_args!(
                    "{}: {}",
                    locale.text(TextKey::HeadCommit),
                    report.head.map_or_else(|| "—".into(), |id| id.to_string())
                ))?;
                output(&format_args!(
                    "{}: {}",
                    locale.text(TextKey::TemporaryFiles),
                    report.temporary_files
                ))?;
                for commit in &report.commits {
                    output(&format_args!(
                        "{}: {} · {}: {} · {}: {}",
                        locale.text(TextKey::Commit),
                        commit.id,
                        locale.text(TextKey::Sequence),
                        commit.sequence,
                        locale.text(TextKey::SnapshotPresent),
                        locale.boolean(commit.snapshot_present)
                    ))?;
                }
                for issue in &report.issues {
                    let (key, id) = match issue {
                        Issue::DamagedCommit(id) => (TextKey::IssueDamagedCommit, Some(id)),
                        Issue::InvalidCommitGraph => (TextKey::IssueInvalidGraph, None),
                        Issue::UnexpectedPhysicalEntry => (TextKey::IssueUnexpectedEntry, None),
                        Issue::MissingSnapshot(id) => (TextKey::IssueMissingSnapshot, Some(id)),
                        Issue::DamagedSnapshot(id) => (TextKey::IssueDamagedSnapshot, Some(id)),
                        Issue::MissingObject(id) => (TextKey::IssueMissingObject, Some(id)),
                        Issue::DamagedObject(id) => (TextKey::IssueDamagedObject, Some(id)),
                    };
                    output(&format_args!(
                        "{}: {}",
                        locale.text(key),
                        id.map_or_else(String::new, ToString::to_string)
                    ))?;
                }
                Ok(())
            }
            Self::Space(usage) => output(&format_args!(
                "{}\n{}",
                locale.storage(usage),
                locale.text(TextKey::SpaceWarning)
            )),
            Self::Content(report) => {
                output(&locale.content(report))?;
                for id in &report.failed_objects {
                    output(id)?;
                }
                Ok(())
            }
            Self::Cleanup(report) => output(&locale.cleanup(report)),
            Self::Backup(report) => output(&locale.backup(report)),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn report(failure: Option<Error>) -> BackupReport {
        BackupReport {
            commit: "00000000000000000000000000000001".parse().unwrap(),
            copied_files: 7,
            ciphertext_bytes: 4096,
            verification: BackupVerification::Copy,
            verified_objects: 0,
            publication_failure: failure,
        }
    }

    #[test]
    fn published_copy_failure_preserves_report_and_failing_exit() {
        let (zh, result) =
            Response::Backup(report(Some(Error::Io(io::ErrorKind::Other)))).into_json(Locale::ZhCn);
        assert!(result.is_err());
        assert_eq!(zh["verification"], "copy");
        assert_eq!(zh["verified_objects"], 0);
        assert!(zh["publication_failure"].is_string());
        let (en, result) =
            Response::Backup(report(Some(Error::Io(io::ErrorKind::Other)))).into_json(Locale::EnUs);
        assert!(result.is_err());
        assert_eq!(zh, en);
        let (value, result) = Response::Backup(report(None)).into_json(Locale::EnUs);
        assert!(result.is_ok());
        assert!(value["publication_failure"].is_null());
        assert!(
            Locale::EnUs
                .backup(&report(None))
                .contains("contents not authenticated")
        );
    }
}
