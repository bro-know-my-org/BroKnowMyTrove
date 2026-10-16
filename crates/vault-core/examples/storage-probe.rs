//! Disposable ENOSPC exercise. Use ONLY a dedicated 24 MiB test filesystem.
use std::{
    fs,
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use vault_core::{
    CreateOptions, Credential, Error, OpenMode, Vault, crypto::Password, operation::Operation,
};

fn verify_small_tmpfs(root: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    if !cfg!(target_os = "linux") {
        return Err("this probe requires Linux mountpoint and GNU stat".into());
    }
    let root = root.canonicalize()?;
    if !std::process::Command::new("mountpoint")
        .arg("-q")
        .arg("--")
        .arg(&root)
        .status()?
        .success()
    {
        return Err("scratch directory must itself be a dedicated mountpoint".into());
    }
    let output = std::process::Command::new("stat")
        .args(["--file-system", "--format=%S %b %T", "--"])
        .arg(&root)
        .output()?;
    let text = std::str::from_utf8(&output.stdout)?;
    let fields: Vec<_> = text.split_whitespace().collect();
    if !output.status.success()
        || fields.len() != 3
        || fields[2] != "tmpfs"
        || fields[0]
            .parse::<u64>()?
            .checked_mul(fields[1].parse::<u64>()?)
            != Some(24 * 1024 * 1024)
    {
        return Err("scratch must be a dedicated tmpfs of exactly 24 MiB total capacity".into());
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args_os()
        .nth(1)
        .ok_or("expected dedicated 24 MiB mount")?;
    let root = std::path::Path::new(&root);
    verify_small_tmpfs(root)?;
    // Limit both available capacity and the amount written. Never fill a general disk.
    let available = vault_platform::capacity(root)?.available;
    if !(20 * 1024 * 1024..=24 * 1024 * 1024).contains(&available) {
        return Err("expected empty dedicated 24 MiB filesystem".into());
    }
    let temp = tempfile::tempdir_in(root)?;
    let path = temp.path().join("vault");
    let password = Password::new("disposable storage probe".into())?;
    let (mut vault, _) = Vault::create(&path, &password, CreateOptions::default())?;
    let source = temp.path().join("source");
    fs::write(&source, vec![42; 8 * 1024 * 1024])?;
    let filler = temp.path().join("filler");
    let full = Arc::new(AtomicBool::new(false));
    let observed = full.clone();
    let fill_path = filler.clone();
    let operation = Operation::default().with_progress(move |progress| {
        if progress.processed != 1024 * 1024 {
            return;
        }
        let mut file = fs::File::create(&fill_path).expect("private test filler");
        let bytes = [0; 64 * 1024];
        for _ in 0..384 {
            // At most 24 MiB, even on a misconfigured mount.
            match file.write_all(&bytes) {
                Ok(()) => (),
                Err(error) => {
                    observed.store(error.kind() == io::ErrorKind::StorageFull, Ordering::SeqCst);
                    break;
                }
            }
        }
    });
    let head = vault.current_commit();
    assert!(matches!(
        vault.import_file(None, "must not commit", &source, &operation),
        Err(Error::Io(io::ErrorKind::StorageFull))
    ));
    assert!(
        full.load(Ordering::SeqCst),
        "real filesystem ENOSPC was not observed"
    );
    assert_eq!(vault.current_commit(), head);
    fs::remove_file(filler)?;
    drop(vault);
    let mut vault = Vault::open(&path, Credential::Password(&password), OpenMode::ReadWrite)?;
    assert_eq!(vault.entries().count(), 0);
    assert_eq!(vault.current_commit(), head);
    assert_eq!(fs::read_dir(path.join("objects"))?.count(), 0);
    let id = vault.import_file(None, "retry", &source, &Operation::default())?;
    fs::remove_file(&source)?;
    let destination = temp.path().join("exported");
    vault.export_file(id, &destination, &Operation::default())?;
    assert_eq!(fs::read(destination)?, vec![42; 8 * 1024 * 1024]);
    drop(vault);
    temp.close()?;
    println!(
        "ENOSPC midstream: old commit intact, private temporary removed, reopen/retry/export passed"
    );
    Ok(())
}
