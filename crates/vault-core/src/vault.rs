use crate::{
    Entry, EntryKind, Error, Id, Result,
    codec::{self, COMMIT_LIMIT, KEY_LIMIT, SNAPSHOT_LIMIT},
    crypto::{Password, RecoveryKey, Secret},
    keyfile::{CreateOptions, Credential, KeyFile},
    ledger::{self, Commit, Inspection, Issue},
    model::{Snapshot, object_size},
    operation::Operation,
    store::{Kind, Store, sync_directory},
};
use std::{io::Write, path::Path};

#[derive(Clone, Copy)]
pub enum OpenMode {
    ReadWrite,
    ReadOnly,
    Recovery(Id),
}

/// Owns the local lock and decrypted state. Drop releases and clears both.
pub struct Vault {
    pub(crate) store: Store,
    pub(crate) master: Secret,
    pub(crate) vault_id: Id,
    pub(crate) snapshot: Snapshot,
    pub(crate) current: Option<Id>,
    pub(crate) sequence: u64,
    pub(crate) writable: bool,
    pub(crate) issues: Vec<Issue>,
}

impl Vault {
    pub fn create(
        path: &Path,
        password: &Password,
        options: CreateOptions,
    ) -> Result<(Self, Option<RecoveryKey>)> {
        let (header, master, recovery) = KeyFile::create(password, options)?;
        let store = Store::create(path)?;
        let mut temporary = tempfile::Builder::new()
            .prefix(".tmp-key-")
            .tempfile_in(&store.root)?;
        temporary.write_all(&codec::encode(&header, KEY_LIMIT)?)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(store.root.join("keys.json"))
            .map_err(|error| Error::from(error.error))?;
        sync_directory(&store.root)?;
        let mut vault = Self {
            store,
            master,
            vault_id: header.vault_id,
            snapshot: Snapshot::default(),
            current: None,
            sequence: 0,
            writable: true,
            issues: Vec::new(),
        };
        vault.commit(Snapshot::default(), &Operation::default())?;
        Ok((vault, recovery))
    }

    pub fn open(path: &Path, credential: Credential<'_>, mode: OpenMode) -> Result<Self> {
        let store = Store::open(path)?;
        let header: KeyFile = codec::decode(
            &codec::read_limited(&store.root.join("keys.json"), KEY_LIMIT)?,
            KEY_LIMIT,
        )?;
        let master = header.unlock(credential)?;
        let graph = ledger::scan(&store, &master, header.vault_id)?;
        let current = match mode {
            OpenMode::Recovery(id) if graph.records.contains_key(&id) => id,
            OpenMode::Recovery(_) => return Err(Error::InvalidInput),
            _ => graph.report.head.ok_or(Error::ReadOnly)?,
        };
        let record = &graph.records[&current];
        let snapshot = read_snapshot(&store, &master, header.vault_id, record.snapshot)?;
        let mut issues = graph.report.issues;
        check_references(&store, &snapshot, &mut issues)?;
        if matches!(mode, OpenMode::ReadWrite) && !issues.is_empty() {
            return Err(Error::ReadOnly);
        }
        Ok(Self {
            store,
            master,
            vault_id: header.vault_id,
            snapshot,
            current: Some(current),
            sequence: record.sequence,
            writable: matches!(mode, OpenMode::ReadWrite),
            issues,
        })
    }

    pub fn inspect(path: &Path, credential: Credential<'_>) -> Result<Inspection> {
        let store = Store::open(path)?;
        let header: KeyFile = codec::decode(
            &codec::read_limited(&store.root.join("keys.json"), KEY_LIMIT)?,
            KEY_LIMIT,
        )?;
        let master = header.unlock(credential)?;
        let mut graph = ledger::scan(&store, &master, header.vault_id)?;
        if let Some(head) = graph.report.head {
            let id = graph.records[&head].snapshot;
            match read_snapshot(&store, &master, header.vault_id, id) {
                Ok(snapshot) => check_references(&store, &snapshot, &mut graph.report.issues)?,
                Err(_) => graph.report.issues.push(Issue::DamagedSnapshot(id)),
            }
        }
        Ok(graph.report)
    }

    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.snapshot.entries.iter()
    }
    pub fn entry(&self, id: Id) -> Option<&Entry> {
        self.entries().find(|entry| entry.id == id)
    }
    pub fn current_commit(&self) -> Option<Id> {
        self.current
    }
    pub fn is_writable(&self) -> bool {
        self.writable
    }
    pub fn issues(&self) -> &[Issue] {
        &self.issues
    }

    pub(crate) fn check_writable(&mut self) -> Result<()> {
        if !self.writable {
            return Err(Error::ReadOnly);
        }
        if let Some(current) = self.current {
            let graph = ledger::scan(&self.store, &self.master, self.vault_id)?;
            if graph.report.head != Some(current) || !graph.report.issues.is_empty() {
                self.writable = false;
                return Err(Error::ReadOnly);
            }
            let mut issues = Vec::new();
            check_references(&self.store, &self.snapshot, &mut issues)?;
            if !issues.is_empty() {
                self.writable = false;
                self.issues.extend(issues);
                return Err(Error::ReadOnly);
            }
        }
        Ok(())
    }

    pub(crate) fn commit(&mut self, snapshot: Snapshot, operation: &Operation) -> Result<()> {
        self.check_writable()?;
        snapshot.validate()?;
        operation.check()?;
        let snapshot_id = Id::random()?;
        let bytes = codec::seal_json(
            &self.master.derive(b"BKMSNAP1")?,
            &codec::aad(1, self.vault_id, snapshot_id),
            &snapshot,
            SNAPSHOT_LIMIT,
        )?;
        self.store
            .publish_bytes(Kind::Snapshot, snapshot_id, &bytes, operation)?;
        operation.check()?;
        let id = Id::random()?;
        let sequence = if self.current.is_some() {
            self.sequence.checked_add(1).ok_or(Error::ResourceLimit)?
        } else {
            0
        };
        let record = Commit {
            sequence,
            parent: self.current,
            snapshot: snapshot_id,
        };
        let bytes = codec::seal_json(
            &self.master.derive(b"BKMCOMM1")?,
            &codec::aad(2, self.vault_id, id),
            &record,
            COMMIT_LIMIT,
        )?;
        if let Err(error) = self
            .store
            .publish_bytes(Kind::Commit, id, &bytes, operation)
        {
            if matches!(error, Error::CommitUncertain) {
                self.writable = false;
            }
            return Err(error);
        }
        self.snapshot = snapshot;
        self.current = Some(id);
        self.sequence = sequence;
        Ok(())
    }
}

pub(crate) fn read_snapshot(store: &Store, master: &Secret, vault: Id, id: Id) -> Result<Snapshot> {
    let snapshot: Snapshot = codec::unseal_json(
        &master.derive(b"BKMSNAP1")?,
        &codec::aad(1, vault, id),
        &store.read(Kind::Snapshot, id, SNAPSHOT_LIMIT)?,
        SNAPSHOT_LIMIT,
    )?;
    snapshot.validate()?;
    Ok(snapshot)
}

fn check_references(store: &Store, snapshot: &Snapshot, issues: &mut Vec<Issue>) -> Result<()> {
    for entry in &snapshot.entries {
        if let EntryKind::File { object, length, .. } = entry.kind {
            match store.path(Kind::Object, object).symlink_metadata() {
                Ok(metadata) if metadata.is_file() && metadata.len() == object_size(length)? => {}
                Ok(_) => issues.push(Issue::DamagedObject(object)),
                Err(_) => issues.push(Issue::MissingObject(object)),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn options() -> CreateOptions {
        CreateOptions {
            kdf: crate::crypto::KdfParams {
                operations: 1,
                memory_bytes: 8 * 1024 * 1024,
            },
            recovery_key: true,
        }
    }

    #[test]
    fn create_reopen_with_password_or_recovery_and_reject_wrong_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("vault");
        let password = Password::new("中文测试口令".into()).unwrap();
        let (vault, recovery) = Vault::create(&path, &password, options()).unwrap();
        let current = vault.current_commit();
        assert!(Vault::open(&path, Credential::Password(&password), OpenMode::ReadWrite).is_err());
        drop(vault);
        let vault = Vault::open(
            &path,
            Credential::Recovery(&recovery.unwrap()),
            OpenMode::ReadOnly,
        )
        .unwrap();
        assert_eq!(vault.current_commit(), current);
        assert!(!vault.is_writable());
        drop(vault);
        assert!(
            Vault::open(
                &path,
                Credential::Password(&Password::new("错误".into()).unwrap()),
                OpenMode::ReadWrite
            )
            .is_err()
        );
        let report = Vault::inspect(&path, Credential::Password(&password)).unwrap();
        assert_eq!(report.head, current);
        assert!(report.issues.is_empty());
    }
}
