use super::*;
use crate::{
    CreateOptions, Credential, OpenMode,
    crypto::{KdfParams, Password},
};

fn fixture() -> (tempfile::TempDir, Vault, Password) {
    let temp = tempfile::tempdir().unwrap();
    let password = Password::new("回收测试".into()).unwrap();
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
fn trash_is_a_reference_root_until_explicit_purge_and_cleanup_is_retryable() {
    let (temp, mut vault, password) = fixture();
    let op = Operation::default();
    let source = temp.path().join("source");
    fs::write(&source, b"old").unwrap();
    let id = vault.import_file(None, "资料", &source, &op).unwrap();
    let historical = vault.current_commit().unwrap();
    fs::write(&source, b"replacement").unwrap();
    vault.replace_file(id, &source, &op).unwrap();
    assert_eq!(vault.storage_usage().unwrap().reclaimable_object_bytes, 149);
    vault.trash_entry(id, &op).unwrap();
    let usage = vault.storage_usage().unwrap();
    assert_eq!(usage.active_object_bytes, 0);
    assert_eq!(usage.trash_object_bytes, 157);
    let report = vault.collect_garbage(&op).unwrap();
    assert!(report.failure.is_none());
    assert_eq!(report.reclaimed_object_bytes, 149);
    assert!(vault.verify_content(&op).unwrap().failed_objects.is_empty());
    vault.restore_entry(id, &op).unwrap();
    vault.trash_entry(id, &op).unwrap();
    let report = vault
        .empty_trash(&op.clone().failing_at(Point::BeforeObjectReclaim))
        .unwrap();
    assert!(report.trash_committed);
    assert!(report.failure.is_some());
    assert_eq!(report.reclaimed_object_bytes, 0);
    assert_eq!(vault.entries().count(), 0);
    assert_eq!(vault.storage_usage().unwrap().reclaimable_object_bytes, 157);
    drop(vault);
    let path = temp.path().join("vault");
    assert!(
        Vault::open(
            &path,
            Credential::Password(&password),
            OpenMode::Recovery(historical)
        )
        .is_err()
    );
    let mut vault =
        Vault::open(&path, Credential::Password(&password), OpenMode::ReadWrite).unwrap();
    fs::write(vault.store.root.join("objects/.tmp-abandoned"), b"partial").unwrap();
    let report = vault.collect_garbage(&op).unwrap();
    assert!(report.failure.is_none());
    assert_eq!(report.reclaimed_object_bytes, 157);
    assert_eq!(report.removed_temporary_bytes, 7);
    assert_eq!(vault.store.inventory(Kind::Object).unwrap().ids.len(), 0);
    assert_eq!(vault.store.inventory(Kind::Snapshot).unwrap().ids.len(), 1);
}

#[test]
fn interrupted_snapshot_retirement_never_reclaims_objects() {
    for point in [Point::RecoverySnapshotsRetired, Point::ObjectReclaimed] {
        let (temp, mut vault, password) = fixture();
        let source = temp.path().join("source");
        fs::write(&source, b"payload").unwrap();
        let op = Operation::default();
        let id = vault.import_file(None, "文件", &source, &op).unwrap();
        vault.trash_entry(id, &op).unwrap();
        let report = vault.empty_trash(&op.clone().failing_at(point)).unwrap();
        assert!(report.trash_committed && report.failure.is_some());
        assert_eq!(
            report.reclaimed_object_bytes,
            if point == Point::ObjectReclaimed {
                153
            } else {
                0
            }
        );
        drop(vault);
        let mut vault = Vault::open(
            &temp.path().join("vault"),
            Credential::Password(&password),
            OpenMode::ReadWrite,
        )
        .unwrap();
        assert_eq!(vault.entries().count(), 0);
        assert!(vault.collect_garbage(&op).unwrap().failure.is_none());
    }
}

#[test]
fn readonly_and_cancelled_sessions_cannot_retire_recovery_points() {
    let (temp, vault, password) = fixture();
    let current = vault.current_commit().unwrap();
    drop(vault);
    let path = temp.path().join("vault");
    let mut vault = Vault::open(
        &path,
        Credential::Password(&password),
        OpenMode::Recovery(current),
    )
    .unwrap();
    assert!(matches!(
        vault.collect_garbage(&Operation::default()),
        Err(Error::ReadOnly)
    ));
    assert!(matches!(
        vault.empty_trash(&Operation::default()),
        Err(Error::ReadOnly)
    ));
    drop(vault);
    let mut vault =
        Vault::open(&path, Credential::Password(&password), OpenMode::ReadWrite).unwrap();
    let op = Operation::default();
    op.cancel();
    assert!(matches!(vault.empty_trash(&op), Err(Error::Cancelled)));
    assert_eq!(vault.current_commit(), Some(current));
}

#[test]
fn damaged_current_snapshot_preserves_historical_recovery_points() {
    let (temp, mut vault, password) = fixture();
    let op = Operation::default();
    let source = temp.path().join("source");
    fs::write(&source, b"old content").unwrap();
    let id = vault.import_file(None, "文件", &source, &op).unwrap();
    let historical = vault.current_commit().unwrap();
    fs::write(&source, b"new content").unwrap();
    vault.replace_file(id, &source, &op).unwrap();
    let graph = ledger::scan(&vault.store, &vault.master, vault.vault_id).unwrap();
    let snapshot = graph.records[&vault.current_commit().unwrap()].snapshot;
    let path = vault.store.path(Kind::Snapshot, snapshot);
    let mut bytes = fs::read(&path).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    fs::write(path, bytes).unwrap();
    let snapshots = vault.store.inventory(Kind::Snapshot).unwrap().ids;
    let objects = vault.store.inventory(Kind::Object).unwrap().ids;

    assert!(matches!(vault.collect_garbage(&op), Err(Error::ReadOnly)));
    assert!(!vault.is_writable());
    assert!(
        vault
            .issues()
            .iter()
            .any(|issue| matches!(issue, crate::Issue::DamagedSnapshot(id) if *id == snapshot))
    );
    assert_eq!(
        vault.store.inventory(Kind::Snapshot).unwrap().ids,
        snapshots
    );
    assert_eq!(vault.store.inventory(Kind::Object).unwrap().ids, objects);
    drop(vault);
    let vault = Vault::open(
        &temp.path().join("vault"),
        Credential::Password(&password),
        OpenMode::Recovery(historical),
    )
    .unwrap();
    let destination = temp.path().join("recovered");
    vault.export_file(id, &destination, &op).unwrap();
    assert_eq!(fs::read(destination).unwrap(), b"old content");
}

#[test]
fn cleanup_removes_stale_key_wrappings_without_touching_the_live_entry() {
    let (temp, mut vault, password) = fixture();
    let root = temp.path().join("vault");
    let keys = fs::read(root.join("keys.json")).unwrap();
    let stale = root.join(".tmp-key-abandoned");
    fs::write(&stale, &keys).unwrap();
    let cancelled = Operation::default();
    cancelled.cancel();
    assert!(matches!(
        vault.collect_garbage(&cancelled),
        Err(Error::Cancelled)
    ));
    assert!(stale.exists());
    let report = vault.collect_garbage(&Operation::default()).unwrap();
    assert!(report.failure.is_none());
    assert_eq!(report.removed_temporary_bytes, keys.len() as u64);
    assert!(!stale.exists());
    assert_eq!(fs::read(root.join("keys.json")).unwrap(), keys);
    drop(vault);
    assert!(Vault::open(&root, Credential::Password(&password), OpenMode::ReadWrite).is_ok());
}

#[cfg(unix)]
#[test]
fn cleanup_rejects_a_temporary_key_symlink_without_touching_its_target() {
    let (temp, mut vault, _) = fixture();
    let target = temp.path().join("keep");
    fs::write(&target, b"keep").unwrap();
    let stale = vault.store.root.join(".tmp-key-link");
    std::os::unix::fs::symlink(&target, &stale).unwrap();
    assert!(vault.collect_garbage(&Operation::default()).is_err());
    assert!(stale.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(fs::read(target).unwrap(), b"keep");
}
