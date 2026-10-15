use super::*;
use crate::{
    CreateOptions, Credential, OpenMode,
    crypto::{KdfParams, Password},
};

fn fixture() -> (tempfile::TempDir, Vault, Password) {
    let directory = tempfile::tempdir().unwrap();
    let password = Password::new("测试中文café".into()).unwrap();
    let options = CreateOptions {
        kdf: KdfParams {
            operations: 1,
            memory_bytes: 8 * 1024 * 1024,
        },
        recovery_key: true,
    };
    let (vault, _) = Vault::create(&directory.path().join("vault"), &password, options).unwrap();
    (directory, vault, password)
}

#[test]
fn disk_roundtrip_reopen_unicode_empty_and_chunk_boundary() {
    let (directory, mut vault, _) = fixture();
    let mut expected = Vec::new();
    for length in [0, 1, model::CHUNK + 1] {
        let plaintext = vec![42; length];
        let source = directory.path().join("source");
        fs::write(&source, &plaintext).unwrap();
        let id = vault
            .import_file(
                None,
                &format!("中文café-{length}"),
                &source,
                &Operation::default(),
            )
            .unwrap();
        assert_eq!(fs::read(&source).unwrap(), plaintext);
        expected.push((id, plaintext));
    }
    drop(vault);
    let password = Password::new("测试中文cafe\u{301}".into()).unwrap();
    let vault = Vault::open(
        &directory.path().join("vault"),
        Credential::Password(&password),
        OpenMode::ReadOnly,
    )
    .unwrap();
    for (index, (id, plaintext)) in expected.iter().enumerate() {
        let destination = directory.path().join(format!("导出-{index}"));
        vault
            .export_file(*id, &destination, &Operation::default())
            .unwrap();
        assert_eq!(&fs::read(destination).unwrap(), plaintext);
    }
    let report = vault.verify_content(&Operation::default()).unwrap();
    assert_eq!(report.verified_objects, 3);
    assert!(report.failed_objects.is_empty());
}

#[test]
fn failed_authentication_never_publishes_plaintext_or_leaves_temporary_files() {
    let (directory, mut vault, _) = fixture();
    let source = directory.path().join("source");
    fs::write(&source, vec![17; model::CHUNK + 1]).unwrap();
    let id = vault
        .import_file(None, "文件", &source, &Operation::default())
        .unwrap();
    let EntryKind::File { object, .. } = vault.entry(id).unwrap().kind else {
        panic!()
    };
    let path = vault.store.path(Kind::Object, object);
    let mut bytes = fs::read(&path).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    fs::write(path, bytes).unwrap();
    let destination = directory.path().join("result");
    assert!(
        vault
            .export_file(id, &destination, &Operation::default())
            .is_err()
    );
    assert!(!destination.exists());
    assert!(!fs::read_dir(directory.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".bkm-export-")
    }));
    assert_eq!(
        vault
            .verify_content(&Operation::default())
            .unwrap()
            .failed_objects,
        vec![object]
    );
}

#[test]
fn changed_source_and_cancellation_leave_the_old_tree_intact() {
    let (directory, mut vault, _) = fixture();
    let source = directory.path().join("source");
    fs::write(&source, vec![17; model::CHUNK * 2]).unwrap();
    let path = source.clone();
    let once = std::sync::atomic::AtomicBool::new(false);
    let operation = Operation::default().with_progress(move |_| {
        if !once.swap(true, std::sync::atomic::Ordering::Relaxed) {
            let mut file = fs::OpenOptions::new().write(true).open(&path).unwrap();
            use std::io::Write;
            file.write_all(b"changed").unwrap();
            file.sync_all().unwrap();
        }
    });
    assert!(matches!(
        vault.import_file(None, "文件", &source, &operation),
        Err(Error::SourceChanged)
    ));
    assert_eq!(vault.entries().count(), 0);
    let operation = Operation::default();
    operation.cancel();
    assert!(matches!(
        vault.import_file(None, "文件", &source, &operation),
        Err(Error::Cancelled)
    ));
    assert_eq!(vault.entries().count(), 0);
}

#[test]
fn conflict_checks_preserve_existing_exports_and_replacement_uses_a_new_object() {
    let (directory, mut vault, _) = fixture();
    let source = directory.path().join("source");
    fs::write(&source, b"old").unwrap();
    let id = vault
        .import_file(None, "文件", &source, &Operation::default())
        .unwrap();
    let old = vault.entry(id).unwrap().kind;
    fs::write(&source, b"new").unwrap();
    vault
        .replace_file(id, &source, &Operation::default())
        .unwrap();
    let EntryKind::File { object: old, .. } = old else {
        panic!()
    };
    let EntryKind::File { object: new, .. } = vault.entry(id).unwrap().kind else {
        panic!()
    };
    assert_ne!(old, new);
    fs::write(directory.path().join("CAFÉ"), b"keep").unwrap();
    assert!(matches!(
        vault.export_file(
            id,
            &directory.path().join("cafe\u{301}"),
            &Operation::default()
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(fs::read(directory.path().join("CAFÉ")).unwrap(), b"keep");
    assert!(
        vault
            .export_file(id, &directory.path().join("NUL.txt"), &Operation::default())
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn rejects_symbolic_link_sources() {
    let (directory, mut vault, _) = fixture();
    fs::write(directory.path().join("source"), b"data").unwrap();
    let link = directory.path().join("link");
    std::os::unix::fs::symlink(directory.path().join("source"), &link).unwrap();
    assert!(
        vault
            .import_file(None, "文件", &link, &Operation::default())
            .is_err()
    );
}
