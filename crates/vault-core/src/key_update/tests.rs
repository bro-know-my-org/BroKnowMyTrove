use super::*;
use crate::{BackupVerification, CreateOptions, Credential, OpenMode, crypto::KdfParams};
use std::fs;

fn password(text: &str) -> Password {
    Password::new(text.into()).unwrap()
}
fn options() -> CreateOptions {
    CreateOptions {
        kdf: KdfParams {
            operations: 1,
            memory_bytes: 8 * 1024 * 1024,
        },
        recovery_key: true,
    }
}

#[test]
fn recovery_password_reset_does_not_republish_tampered_weak_kdf() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("vault");
    let old = password("原始密码");
    let new = password("恢复后的新密码");
    let (mut vault, recovery) = Vault::create(
        &path,
        &old,
        CreateOptions {
            recovery_key: true,
            ..CreateOptions::default()
        },
    )
    .unwrap();
    vault
        .mkdir(None, "保留数据", &Operation::default())
        .unwrap();
    drop(vault);
    let file = path.join("keys.json");
    let mut header: KeyFile = codec::decode(&fs::read(&file).unwrap(), codec::KEY_LIMIT).unwrap();
    header.kdf = options().kdf;
    fs::write(&file, codec::encode(&header, codec::KEY_LIMIT).unwrap()).unwrap();
    assert!(Vault::open(&path, Credential::Password(&old), OpenMode::ReadOnly).is_err());
    let key = recovery.unwrap();
    let mut vault = Vault::open(&path, Credential::Recovery(&key), OpenMode::ReadWrite).unwrap();
    vault.change_password(&new, &Operation::default()).unwrap();
    drop(vault);
    let updated: KeyFile = codec::decode(&fs::read(&file).unwrap(), codec::KEY_LIMIT).unwrap();
    assert!(updated.kdf.operations >= KdfParams::default().operations);
    assert!(updated.kdf.memory_bytes >= KdfParams::default().memory_bytes);
    assert_eq!(header.recovery, updated.recovery);
    let vault = Vault::open(&path, Credential::Password(&new), OpenMode::ReadWrite).unwrap();
    assert_eq!(vault.search("保留数据", false).count(), 1);
    drop(vault);
    assert!(Vault::open(&path, Credential::Recovery(&key), OpenMode::ReadOnly).is_ok());
}

#[test]
fn password_update_keeps_master_recovery_data_and_updates_backup_cache() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("vault");
    let old = password("旧密码");
    let new = password(" 新中文cafe\u{301}！ ");
    let (mut vault, recovery) = Vault::create(&path, &old, options()).unwrap();
    let op = Operation::default();
    vault.mkdir(None, "保留目录", &op).unwrap();
    let current = vault.current_commit();
    let before = vault.key_bytes.clone();
    let previous: KeyFile = codec::decode(&before, codec::KEY_LIMIT).unwrap();
    vault.change_password(&new, &op).unwrap();
    let next: KeyFile = codec::decode(&vault.key_bytes, codec::KEY_LIMIT).unwrap();
    assert_eq!(previous.vault_id, next.vault_id);
    assert_eq!(previous.recovery, next.recovery);
    assert_eq!(next.kdf.memory_bytes, KdfParams::default().memory_bytes);
    assert_eq!(next.kdf.operations, KdfParams::default().operations);
    assert_ne!(previous.salt, next.salt);
    assert_eq!(vault.current_commit(), current);
    assert_eq!(
        *previous.unlock(Credential::Password(&old)).unwrap().0,
        *vault.master.0
    );
    let backup = temp.path().join("backup");
    vault
        .backup_to(&backup, BackupVerification::Full, &op)
        .unwrap();
    drop(vault);
    assert!(Vault::open(&path, Credential::Password(&old), OpenMode::ReadOnly).is_err());
    let vault = Vault::open(
        &path,
        Credential::Recovery(recovery.as_ref().unwrap()),
        OpenMode::ReadWrite,
    )
    .unwrap();
    assert_eq!(vault.search("保留目录", false).count(), 1);
    drop(vault);
    assert!(
        Vault::open(
            &backup,
            Credential::Password(&password(" 新中文café！ ")),
            OpenMode::ReadWrite
        )
        .is_ok()
    );
}

#[test]
fn recovery_rotation_and_removal_keep_password_and_cannot_revoke_old_copies() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("vault");
    let pass = password("保留密码");
    let (mut vault, old) = Vault::create(&path, &pass, options()).unwrap();
    let old = old.unwrap();
    let old_header: KeyFile = codec::decode(&vault.key_bytes, codec::KEY_LIMIT).unwrap();
    let op = Operation::default();
    let key = vault.rotate_recovery_key(&op).unwrap();
    let header: KeyFile = codec::decode(&vault.key_bytes, codec::KEY_LIMIT).unwrap();
    assert_eq!(old_header.password, header.password);
    assert!(header.unlock(Credential::Recovery(&old)).is_err());
    assert_eq!(
        *old_header.unlock(Credential::Recovery(&old)).unwrap().0,
        *vault.master.0
    );
    drop(vault);
    let mut vault = Vault::open(&path, Credential::Recovery(&key), OpenMode::ReadWrite).unwrap();
    // Recovery credentials can also reset a forgotten password.
    let new = password("重置密码");
    vault.change_password(&new, &op).unwrap();
    vault.remove_recovery_key(&op).unwrap();
    drop(vault);
    assert!(Vault::open(&path, Credential::Recovery(&key), OpenMode::ReadOnly).is_err());
    assert!(Vault::open(&path, Credential::Password(&pass), OpenMode::ReadOnly).is_err());
    assert!(Vault::open(&path, Credential::Password(&new), OpenMode::ReadWrite).is_ok());
}

#[test]
fn key_publication_faults_and_cancellation_reopen_as_complete_old_or_new_header() {
    for point in [
        Point::KeyUpdateDerived,
        Point::KeyUpdateWritten,
        Point::KeyUpdateSynced,
        Point::KeyUpdatePublished,
        Point::KeyUpdateDirectorySynced,
    ] {
        for cancel in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("vault");
            let old = password("旧");
            let new = password("新");
            let (mut vault, _) = Vault::create(&path, &old, options()).unwrap();
            let before = vault.key_bytes.clone();
            let operation = if cancel {
                Operation::default().cancelling_at(point)
            } else {
                Operation::default().failing_at(point)
            };
            let result = vault.change_password(&new, &operation);
            let published = matches!(
                point,
                Point::KeyUpdatePublished | Point::KeyUpdateDirectorySynced
            );
            if published && !cancel {
                assert!(matches!(result, Err(Error::CommitUncertain)));
                assert!(!vault.is_writable());
            } else if published {
                assert!(result.is_ok());
            } else {
                assert!(result.is_err());
                assert_eq!(*before, *vault.key_bytes);
            }
            assert_eq!(fs::read(path.join("keys.json")).unwrap(), *vault.key_bytes);
            drop(vault);
            assert!(
                Vault::open(
                    &path,
                    Credential::Password(if published { &new } else { &old }),
                    OpenMode::ReadWrite
                )
                .is_ok()
            );
            assert!(
                Vault::open(
                    &path,
                    Credential::Password(if published { &old } else { &new }),
                    OpenMode::ReadOnly
                )
                .is_err()
            );
        }
    }
}

#[test]
fn partial_writes_stale_temporaries_readonly_and_changed_source_are_safe() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("vault");
    let pass = password("口令");
    let (mut vault, _) = Vault::create(&path, &pass, options()).unwrap();
    let before = vault.key_bytes.clone();
    assert!(
        vault
            .change_password(
                &password("新口令"),
                &Operation::default().with_write_limit(8)
            )
            .is_err()
    );
    assert_eq!(fs::read(path.join("keys.json")).unwrap(), *before);
    assert_eq!(fs::read_dir(&path).unwrap().count(), 5);
    // A killed updater's private encrypted temporary cannot become a key entry.
    fs::write(path.join(".tmp-key-interrupted"), b"partial").unwrap();
    drop(vault);
    let report = Vault::inspect(&path, Credential::Password(&pass)).unwrap();
    assert!(report.issues.is_empty());
    assert_eq!(report.temporary_files, 1);
    let mut vault = Vault::open(&path, Credential::Password(&pass), OpenMode::ReadOnly).unwrap();
    assert!(matches!(
        vault.remove_recovery_key(&Operation::default()),
        Err(Error::ReadOnly)
    ));
    drop(vault);
    let mut vault = Vault::open(&path, Credential::Password(&pass), OpenMode::ReadWrite).unwrap();
    fs::write(path.join("keys.json"), b"external-change").unwrap();
    assert!(matches!(
        vault.remove_recovery_key(&Operation::default()),
        Err(Error::SourceChanged)
    ));
    assert!(!vault.is_writable());
    assert_eq!(
        fs::read(path.join("keys.json")).unwrap(),
        b"external-change"
    );
}
