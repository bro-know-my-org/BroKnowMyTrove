use crate::{
    CreateOptions, Credential, EntryKind, Error, Id, Issue, OpenMode, Vault, codec,
    crypto::{KdfParams, Password},
    ledger::{self, Commit},
    operation::{Operation, Point},
    store::Kind,
};
use std::fs;

fn fixture() -> (tempfile::TempDir, Vault, Password) {
    let temp = tempfile::tempdir().unwrap();
    let password = Password::new("故障恢复测试".into()).unwrap();
    let options = CreateOptions {
        kdf: KdfParams {
            operations: 1,
            memory_bytes: 8 * 1024 * 1024,
        },
        recovery_key: false,
    };
    let (vault, _) = Vault::create(&temp.path().join("vault"), &password, options).unwrap();
    (temp, vault, password)
}

#[test]
fn every_publication_boundary_reopens_as_complete_old_or_complete_new_state() {
    for point in [
        Point::ObjectWritten,
        Point::ObjectSynced,
        Point::ObjectPublished,
        Point::ObjectDirectorySynced,
        Point::SnapshotWritten,
        Point::SnapshotSynced,
        Point::SnapshotPublished,
        Point::SnapshotDirectorySynced,
        Point::CommitWritten,
        Point::CommitSynced,
        Point::CommitPublished,
        Point::CommitDirectorySynced,
    ] {
        let (temp, mut vault, password) = fixture();
        let op = Operation::default();
        let parent = vault.mkdir(None, "嵌套目录", &op).unwrap();
        let source = temp.path().join("source");
        fs::write(&source, b"old").unwrap();
        let id = vault
            .import_file(Some(parent), "资料", &source, &op)
            .unwrap();
        let old_head = vault.current_commit();
        fs::write(&source, b"complete-new").unwrap();
        let error = vault
            .replace_file(id, &source, &op.clone().failing_at(point))
            .unwrap_err();
        let published = matches!(point, Point::CommitPublished | Point::CommitDirectorySynced);
        assert_eq!(
            matches!(error, Error::CommitUncertain),
            published,
            "{point:?}"
        );
        assert_eq!(vault.current_commit(), old_head);
        assert_eq!(vault.is_writable(), !published);
        drop(vault);
        let mut vault = Vault::open(
            &temp.path().join("vault"),
            Credential::Password(&password),
            OpenMode::ReadWrite,
        )
        .unwrap();
        assert_eq!(vault.entries().count(), 2);
        assert_eq!(vault.entry(id).unwrap().parent, Some(parent));
        assert_eq!(vault.current_commit() != old_head, published);
        let destination = temp.path().join("result");
        vault.export_file(id, &destination, &op).unwrap();
        assert_eq!(
            fs::read(destination).unwrap(),
            if published {
                b"complete-new".as_slice()
            } else {
                b"old".as_slice()
            }
        );
        assert!(vault.verify_content(&op).unwrap().failed_objects.is_empty());
        // An interrupted operation may be retried, with a fresh object and file key.
        vault.replace_file(id, &source, &op).unwrap();
        assert_eq!(vault.entries().count(), 2);
    }
}

#[test]
fn partial_enospc_and_export_publication_failures_have_explicit_outcomes() {
    let (temp, mut vault, _) = fixture();
    let source = temp.path().join("source");
    fs::write(&source, vec![42; 2 * 1024 * 1024]).unwrap();
    let old_head = vault.current_commit();
    for limit in [0, 7, 104, 1024 * 1024 + 17] {
        assert!(matches!(
            vault.import_file(
                None,
                "文件",
                &source,
                &Operation::default().with_write_limit(limit)
            ),
            Err(Error::Io(std::io::ErrorKind::StorageFull))
        ));
        assert_eq!(vault.current_commit(), old_head);
        assert_eq!(vault.entries().count(), 0);
        assert_eq!(vault.store.inventory(Kind::Object).unwrap().temporary, 0);
    }
    let id = vault
        .import_file(None, "文件", &source, &Operation::default())
        .unwrap();
    let target = temp.path().join("result");
    for operation in [
        Operation::default().with_write_limit(1024 * 1024 + 3),
        Operation::default().failing_at(Point::ExportSynced),
    ] {
        assert!(vault.export_file(id, &target, &operation).is_err());
        assert!(!target.exists());
        assert!(!fs::read_dir(temp.path()).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".bkm-export-")
        }));
    }
    assert!(matches!(
        vault.export_file(
            id,
            &target,
            &Operation::default().failing_at(Point::ExportPublished)
        ),
        Err(Error::ExportUncertain)
    ));
    assert_eq!(fs::read(target).unwrap(), fs::read(source).unwrap());
}

#[test]
fn missing_objects_snapshots_and_conflicted_keys_disable_writes_without_silent_rollback() {
    for damage in 0..4 {
        let (temp, mut vault, password) = fixture();
        let old = vault.current_commit().unwrap();
        let source = temp.path().join("source");
        fs::write(&source, b"payload").unwrap();
        let id = vault
            .import_file(None, "文件", &source, &Operation::default())
            .unwrap();
        let current = vault.current_commit().unwrap();
        let graph = ledger::scan(&vault.store, &vault.master, vault.vault_id).unwrap();
        let EntryKind::File { object, .. } = vault.entry(id).unwrap().kind else {
            panic!()
        };
        match damage {
            0 => fs::remove_file(vault.store.path(Kind::Object, object)).unwrap(),
            1 => fs::remove_file(
                vault
                    .store
                    .path(Kind::Snapshot, graph.records[&current].snapshot),
            )
            .unwrap(),
            2 => fs::write(vault.store.path(Kind::Commit, current), b"damaged").unwrap(),
            _ => fs::write(vault.store.root.join("keys (conflicted).json"), b"conflict").unwrap(),
        }
        drop(vault);
        let path = temp.path().join("vault");
        assert!(Vault::open(&path, Credential::Password(&password), OpenMode::ReadWrite).is_err());
        let report = Vault::inspect(&path, Credential::Password(&password)).unwrap();
        assert!(!report.issues.is_empty());
        let mut recovered = Vault::open(
            &path,
            Credential::Password(&password),
            OpenMode::Recovery(old),
        )
        .unwrap();
        assert_eq!(recovered.entries().count(), 0);
        assert!(!recovered.is_writable());
        assert!(matches!(
            recovered.mkdir(None, "不可写", &Operation::default()),
            Err(Error::ReadOnly)
        ));
    }
}

#[test]
fn authenticated_fork_allows_explicit_readonly_export_but_no_automatic_winner() {
    let (temp, mut vault, password) = fixture();
    let root = vault.current_commit().unwrap();
    let source = temp.path().join("source");
    fs::write(&source, b"keep me").unwrap();
    let id = vault
        .import_file(None, "资料", &source, &Operation::default())
        .unwrap();
    let child = vault.current_commit().unwrap();
    let graph = ledger::scan(&vault.store, &vault.master, vault.vault_id).unwrap();
    let fork = Id::random().unwrap();
    let bytes = codec::seal_json(
        &vault.master.derive(b"BKMCOMM1").unwrap(),
        &codec::aad(2, vault.vault_id, fork),
        &Commit {
            sequence: 1,
            parent: Some(root),
            snapshot: graph.records[&child].snapshot,
        },
        codec::COMMIT_LIMIT,
    )
    .unwrap();
    vault
        .store
        .publish_bytes(Kind::Commit, fork, &bytes, &Operation::default())
        .unwrap();
    assert!(matches!(
        vault.mkdir(None, "阻止分叉写入", &Operation::default()),
        Err(Error::ReadOnly)
    ));
    drop(vault);
    let path = temp.path().join("vault");
    let report = Vault::inspect(&path, Credential::Password(&password)).unwrap();
    assert!(report.head.is_none());
    assert!(
        report
            .issues
            .iter()
            .any(|issue| matches!(issue, Issue::InvalidCommitGraph))
    );
    let mut vault = Vault::open(
        &path,
        Credential::Password(&password),
        OpenMode::Recovery(child),
    )
    .unwrap();
    vault
        .export_file(id, &temp.path().join("recovered"), &Operation::default())
        .unwrap();
    assert_eq!(fs::read(temp.path().join("recovered")).unwrap(), b"keep me");
    assert!(
        vault
            .verify_content(&Operation::default())
            .unwrap()
            .failed_objects
            .is_empty()
    );
}

#[test]
fn cancellation_before_publication_keeps_old_state_but_after_publication_finishes_commit() {
    for point in [
        Point::CommitSynced,
        Point::CommitPublished,
        Point::CommitDirectorySynced,
    ] {
        let (temp, mut vault, password) = fixture();
        let old = vault.current_commit();
        let result = vault.mkdir(None, "目录", &Operation::default().cancelling_at(point));
        let published = point != Point::CommitSynced;
        if published {
            assert!(result.is_ok());
        } else {
            assert!(matches!(result, Err(Error::Cancelled)));
        }
        drop(vault);
        let vault = Vault::open(
            &temp.path().join("vault"),
            Credential::Password(&password),
            OpenMode::ReadWrite,
        )
        .unwrap();
        assert_eq!(vault.entries().count(), usize::from(published));
        assert_eq!(vault.current_commit() != old, published);
    }
}

#[test]
fn interrupted_uncommitted_import_restarts_with_fresh_object_and_midstream_cancel_cleans_up() {
    let (temp, mut vault, password) = fixture();
    let source = temp.path().join("source");
    fs::write(&source, vec![23; 3 * 1024 * 1024]).unwrap();
    let op = Operation::default();
    let cancel = op.clone();
    let op = op.with_progress(move |_| cancel.cancel());
    assert!(matches!(
        vault.import_file(None, "文件", &source, &op),
        Err(Error::Cancelled)
    ));
    assert_eq!(vault.store.inventory(Kind::Object).unwrap().temporary, 0);
    assert!(
        vault
            .import_file(
                None,
                "文件",
                &source,
                &Operation::default().failing_at(Point::ObjectPublished)
            )
            .is_err()
    );
    let orphan = vault.store.inventory(Kind::Object).unwrap().ids[0];
    drop(vault);
    let mut vault = Vault::open(
        &temp.path().join("vault"),
        Credential::Password(&password),
        OpenMode::ReadWrite,
    )
    .unwrap();
    assert_eq!(vault.entries().count(), 0);
    let id = vault
        .import_file(None, "文件", &source, &Operation::default())
        .unwrap();
    let EntryKind::File { object, .. } = vault.entry(id).unwrap().kind else {
        panic!()
    };
    assert_ne!(orphan, object);
    assert!(
        vault
            .verify_content(&Operation::default())
            .unwrap()
            .failed_objects
            .is_empty()
    );
}

#[test]
fn sync_arrivals_never_turn_missing_data_into_deletions_and_reopen_after_completion() {
    for missing_parent in [false, true] {
        let (temp, mut vault, password) = fixture();
        let op = Operation::default();
        let parent = vault.current_commit().unwrap();
        let source = temp.path().join("source");
        fs::write(&source, b"arrives later").unwrap();
        let id = vault.import_file(None, "保留名称", &source, &op).unwrap();
        let current = vault.current_commit().unwrap();
        let EntryKind::File { object, .. } = vault.entry(id).unwrap().kind else {
            panic!()
        };
        let delayed = if missing_parent {
            vault.store.path(Kind::Commit, parent)
        } else {
            vault.store.path(Kind::Object, object)
        };
        let bytes = fs::read(&delayed).unwrap();
        drop(vault);
        fs::remove_file(&delayed).unwrap();
        let path = temp.path().join("vault");
        assert!(Vault::open(&path, Credential::Password(&password), OpenMode::ReadWrite).is_err());
        let mut recovery = Vault::open(
            &path,
            Credential::Password(&password),
            OpenMode::Recovery(current),
        )
        .unwrap();
        assert_eq!(recovery.entry(id).unwrap().name, "保留名称");
        assert!(recovery.issues().iter().any(|issue| if missing_parent {
            matches!(issue, Issue::InvalidCommitGraph)
        } else {
            matches!(issue, Issue::MissingObject(found) if *found == object)
        }));
        assert!(matches!(
            recovery.trash_entry(id, &op),
            Err(Error::ReadOnly)
        ));
        fs::write(&delayed, bytes).unwrap();
        assert!(
            !recovery.is_writable(),
            "arrival cannot upgrade an existing recovery session"
        );
        assert!(matches!(
            recovery.mkdir(None, "still read only", &op),
            Err(Error::ReadOnly)
        ));
        drop(recovery);
        let mut vault =
            Vault::open(&path, Credential::Password(&password), OpenMode::ReadWrite).unwrap();
        assert_eq!(vault.current_commit(), Some(current));
        assert_eq!(vault.entry(id).unwrap().name, "保留名称");
        assert!(vault.verify_content(&op).unwrap().failed_objects.is_empty());
        vault.mkdir(None, "完整到达后可写", &op).unwrap();
    }
}

#[test]
fn sync_conflict_copies_in_each_directory_require_explicit_resolution() {
    for directory in ["objects", "snapshots", "commits"] {
        let (temp, vault, password) = fixture();
        let head = vault.current_commit().unwrap();
        let conflict = vault
            .store
            .root
            .join(directory)
            .join("00000000000000000000000000000001 (conflicted copy)");
        fs::write(&conflict, b"never select automatically").unwrap();
        drop(vault);
        let path = temp.path().join("vault");
        let report = Vault::inspect(&path, Credential::Password(&password)).unwrap();
        assert!(
            report
                .issues
                .iter()
                .any(|issue| matches!(issue, Issue::UnexpectedPhysicalEntry))
        );
        assert!(report.head.is_none());
        assert!(Vault::open(&path, Credential::Password(&password), OpenMode::ReadWrite).is_err());
        let mut vault = Vault::open(
            &path,
            Credential::Password(&password),
            OpenMode::Recovery(head),
        )
        .unwrap();
        assert!(matches!(
            vault.collect_garbage(&Operation::default()),
            Err(Error::ReadOnly)
        ));
        assert!(
            conflict.exists(),
            "inspection must preserve conflict evidence"
        );
        drop(vault);
        fs::remove_file(conflict).unwrap();
        assert!(Vault::open(&path, Credential::Password(&password), OpenMode::ReadWrite).is_ok());
    }
}

#[test]
fn loss_of_all_snapshots_never_fabricates_an_empty_vault_or_deletes_objects() {
    let (temp, mut vault, password) = fixture();
    let source = temp.path().join("source");
    fs::write(&source, b"object survives without its name").unwrap();
    let id = vault
        .import_file(None, "lost metadata", &source, &Operation::default())
        .unwrap();
    let EntryKind::File { object, .. } = vault.entry(id).unwrap().kind else {
        panic!()
    };
    let object_path = vault.store.path(Kind::Object, object);
    let head = vault.current_commit().unwrap();
    let ciphertext = fs::read(&object_path).unwrap();
    for snapshot in vault.store.inventory(Kind::Snapshot).unwrap().ids {
        fs::remove_file(vault.store.path(Kind::Snapshot, snapshot)).unwrap();
    }
    drop(vault);
    let path = temp.path().join("vault");
    let report = Vault::inspect(&path, Credential::Password(&password)).unwrap();
    assert!(report.head.is_none());
    assert!(
        report
            .commits
            .iter()
            .any(|commit| commit.id == head && !commit.snapshot_present)
    );
    assert!(
        Vault::open(
            &path,
            Credential::Password(&password),
            OpenMode::Recovery(head)
        )
        .is_err()
    );
    assert!(
        report
            .issues
            .iter()
            .any(|issue| matches!(issue, Issue::MissingSnapshot(_)))
    );
    for commit in &report.commits {
        assert!(!commit.snapshot_present);
        assert!(
            Vault::open(
                &path,
                Credential::Password(&password),
                OpenMode::Recovery(commit.id)
            )
            .is_err()
        );
    }
    assert!(Vault::open(&path, Credential::Password(&password), OpenMode::ReadWrite).is_err());
    assert_eq!(fs::read(object_path).unwrap(), ciphertext);
}
