use super::*;
use crate::{
    CreateOptions,
    crypto::{KdfParams, Password, RecoveryKey},
};
use BackupVerification::{Copy, Full};
use std::fs;

fn fixture() -> (tempfile::TempDir, Vault, Password, RecoveryKey, Id) {
    let temp = tempfile::tempdir().unwrap();
    let password = Password::new("备份中文café".into()).unwrap();
    let options = CreateOptions {
        kdf: KdfParams {
            operations: 1,
            memory_bytes: 8 * 1024 * 1024,
        },
        recovery_key: true,
    };
    let (mut vault, recovery) =
        Vault::create(&temp.path().join("vault"), &password, options).unwrap();
    let source = temp.path().join("source");
    fs::write(&source, vec![42; 65537]).unwrap();
    let id = vault
        .import_file(None, "资料", &source, &Operation::default())
        .unwrap();
    (temp, vault, password, recovery.unwrap(), id)
}

#[test]
fn independent_backup_restores_password_recovery_trash_and_only_current_objects() {
    let (temp, mut vault, password, recovery, id) = fixture();
    let op = Operation::default();
    let empty = temp.path().join("empty");
    fs::write(&empty, []).unwrap();
    let trash = vault.import_file(None, "空文件", &empty, &op).unwrap();
    vault.trash_entry(trash, &op).unwrap();
    vault
        .replace_file(id, &temp.path().join("source"), &op)
        .unwrap();
    fs::write(vault.store.root.join("objects/.tmp-interrupted"), b"stale").unwrap();
    let backup = temp.path().join("backup");
    let report = vault.backup_to(&backup, Full, &op).unwrap();
    assert!(report.publication_failure.is_none());
    assert_eq!(report.verified_objects, 2);
    assert_eq!(report.verification, Full);
    assert_eq!(fs::read_dir(backup.join("objects")).unwrap().count(), 2);
    assert_eq!(fs::read_dir(backup.join("snapshots")).unwrap().count(), 1);
    assert_eq!(
        fs::read(backup.join("keys.json")).unwrap(),
        *vault.key_bytes
    );
    let original = vault.store.root.clone();
    drop(vault);
    fs::remove_dir_all(original).unwrap();
    let restored = temp.path().join("restored");
    let copied = Vault::restore_to(
        &backup,
        Credential::Recovery(&recovery),
        &restored,
        Copy,
        &op,
    )
    .unwrap();
    assert_eq!(copied.commit, report.commit);
    assert_eq!(copied.verified_objects, 0);
    assert_eq!(copied.ciphertext_bytes, report.ciphertext_bytes);
    let mut reopened = Vault::open(
        &restored,
        Credential::Password(&password),
        OpenMode::ReadWrite,
    )
    .unwrap();
    assert!(reopened.entry(trash).unwrap().is_trashed());
    reopened
        .export_file(id, &temp.path().join("exported"), &op)
        .unwrap();
    assert_eq!(
        fs::read(temp.path().join("exported")).unwrap(),
        vec![42; 65537]
    );
    reopened.restore_entry(trash, &op).unwrap();
    assert!(
        reopened
            .verify_content(&op)
            .unwrap()
            .failed_objects
            .is_empty()
    );
}

#[test]
fn failures_and_cancellation_remove_unpublished_destination_without_touching_source() {
    let (temp, vault, _, _, _) = fixture();
    let destination = temp.path().join("backup");
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
        Point::BackupPrepared,
    ] {
        let op = Operation::default().failing_at(point);
        assert!(vault.backup_to(&destination, Full, &op).is_err());
        assert!(!destination.exists(), "{point:?}");
    }
    let cancelled = Operation::default();
    let cancel = cancelled.clone();
    let cancelled = cancelled.with_progress(move |_| cancel.cancel());
    for op in [
        cancelled,
        Operation::default().cancelling_at(Point::BackupPrepared),
        Operation::default().with_write_limit(1),
    ] {
        assert!(vault.backup_to(&destination, Copy, &op).is_err());
        assert!(!destination.exists());
    }
    assert!(vault.is_writable());
    assert!(
        vault
            .backup_to(&destination, Full, &Operation::default())
            .is_ok()
    );
}

#[test]
fn published_backups_survive_late_cancellation_and_report_durability_failure() {
    let (temp, vault, password, _, _) = fixture();
    for (index, op) in [
        Operation::default().cancelling_at(Point::BackupPublished),
        Operation::default().failing_at(Point::BackupPublished),
        Operation::default().failing_at(Point::BackupDirectorySynced),
    ]
    .iter()
    .enumerate()
    {
        let destination = temp.path().join(format!("backup-{index}"));
        let report = vault.backup_to(&destination, Full, op).unwrap();
        assert_eq!(report.publication_failure.is_some(), index != 0);
        assert!(
            Vault::open(
                &destination,
                Credential::Password(&password),
                OpenMode::ReadWrite
            )
            .is_ok()
        );
    }
}

#[test]
fn publication_attempts_both_syncs_and_retains_first_failure() {
    for injected in [false, true] {
        let mut op = Operation::default().cancelling_at(Point::BackupDirectorySynced);
        if injected {
            op = op.failing_at(Point::BackupPublished);
        }
        let mut attempted = Vec::new();
        let failure = finish_publication(Path::new("target"), Path::new("parent"), &op, |path| {
            attempted.push(path.to_path_buf());
            Err(Error::Io(std::io::ErrorKind::PermissionDenied))
        });
        assert_eq!(attempted, [Path::new("target"), Path::new("parent")]);
        let expected = if injected {
            std::io::ErrorKind::StorageFull
        } else {
            std::io::ErrorKind::PermissionDenied
        };
        assert!(matches!(failure, Some(Error::Io(kind)) if kind == expected));
        assert!(matches!(op.check_cancelled(), Err(Error::Cancelled)));
    }
}

#[test]
fn metadata_cancellation_stops_before_creating_a_destination() {
    let (temp, vault, password, _, _) = fixture();
    let destination = temp.path().join("backup");
    for point in [Point::InventoryEntry, Point::CommitRead] {
        let op = Operation::default().cancelling_at(point);
        assert!(matches!(
            vault.backup_to(&destination, Full, &op),
            Err(Error::Cancelled)
        ));
        assert!(!destination.exists());
    }
    let source = vault.store.root.clone();
    drop(vault);
    let op = Operation::default().cancelling_at(Point::CommitRead);
    assert!(matches!(
        Vault::restore_to(
            &source,
            Credential::Password(&password),
            &destination,
            Full,
            &op
        ),
        Err(Error::Cancelled)
    ));
    assert!(!destination.exists());
}

#[test]
fn refuses_existing_nested_and_changed_key_destinations() {
    let (temp, vault, _, _, _) = fixture();
    let op = Operation::default();
    let destination = temp.path().join("existing");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("keep"), b"untouched").unwrap();
    assert!(vault.backup_to(&destination, Full, &op).is_err());
    assert_eq!(fs::read(destination.join("keep")).unwrap(), b"untouched");
    assert!(
        vault
            .backup_to(&vault.store.root.join("nested"), Full, &op)
            .is_err()
    );
    fs::write(vault.store.root.join("keys.json"), b"changed").unwrap();
    let destination = temp.path().join("changed");
    assert!(matches!(
        vault.backup_to(&destination, Full, &op),
        Err(Error::SourceChanged)
    ));
    assert!(!destination.exists());
}

#[test]
fn copy_validation_does_not_claim_content_authentication_and_damage_blocks_full_backup() {
    let (temp, vault, _, _, id) = fixture();
    let op = Operation::default();
    let EntryKind::File { object, .. } = vault.entry(id).unwrap().kind else {
        panic!()
    };
    let path = vault.store.path(Kind::Object, object);
    let mut bytes = fs::read(&path).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    fs::write(path, bytes).unwrap();
    let copied = vault
        .backup_to(&temp.path().join("copy"), Copy, &op)
        .unwrap();
    assert_eq!(copied.verified_objects, 0);
    let full = temp.path().join("full");
    assert!(vault.backup_to(&full, Full, &op).is_err());
    assert!(!full.exists());
    fs::remove_file(vault.store.path(Kind::Object, object)).unwrap();
    assert!(vault.backup_to(&full, Copy, &op).is_err());
    assert!(!full.exists());
}
