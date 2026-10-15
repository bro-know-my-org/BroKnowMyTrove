use crate::{Entry, EntryKind, Error, Id, Result, Vault, model, operation::Operation};
use zeroize::Zeroize;

impl Vault {
    pub fn mkdir(&mut self, parent: Option<Id>, name: &str, operation: &Operation) -> Result<Id> {
        self.active_parent(parent)?;
        model::validate_name(name)?;
        let id = Id::random()?;
        let mut next = self.snapshot.clone();
        next.entries.push(Entry {
            id,
            parent,
            name: name.into(),
            trash_group: None,
            kind: EntryKind::Directory,
        });
        self.commit(next, operation)?;
        Ok(id)
    }

    pub fn rename(&mut self, id: Id, name: &str, operation: &Operation) -> Result<()> {
        model::validate_name(name)?;
        let mut next = self.snapshot.clone();
        let entry = next
            .entries
            .iter_mut()
            .find(|entry| entry.id == id && !entry.is_trashed())
            .ok_or(Error::InvalidInput)?;
        entry.name.zeroize();
        entry.name = name.into();
        self.commit(next, operation)
    }

    pub fn move_entry(&mut self, id: Id, parent: Option<Id>, operation: &Operation) -> Result<()> {
        self.active_parent(parent)?;
        let mut next = self.snapshot.clone();
        let entry = next
            .entries
            .iter_mut()
            .find(|entry| entry.id == id && !entry.is_trashed())
            .ok_or(Error::InvalidInput)?;
        entry.parent = parent;
        self.commit(next, operation)
    }

    pub fn trash_entry(&mut self, id: Id, operation: &Operation) -> Result<()> {
        self.entry(id)
            .filter(|entry| !entry.is_trashed())
            .ok_or(Error::InvalidInput)?;
        let subtree = self.snapshot.subtree(id);
        let group = Id::random()?;
        let mut next = self.snapshot.clone();
        for entry in &mut next.entries {
            if subtree.contains(&entry.id) && !entry.is_trashed() {
                entry.trash_group = Some(group);
            }
        }
        self.commit(next, operation)
    }

    /// Restore this deletion only; children deleted earlier stay in the trash.
    pub fn restore_entry(&mut self, id: Id, operation: &Operation) -> Result<()> {
        let entry = self.entry(id).ok_or(Error::InvalidInput)?;
        let group = entry.trash_group.ok_or(Error::InvalidInput)?;
        self.active_parent(entry.parent)?;
        let subtree = self.snapshot.subtree(id);
        let mut next = self.snapshot.clone();
        for entry in &mut next.entries {
            if subtree.contains(&entry.id) && entry.trash_group == Some(group) {
                entry.trash_group = None;
            }
        }
        self.commit(next, operation)
    }

    /// Search only the unlocked snapshot; callers control the lifetime of the query.
    pub fn search<'a>(&'a self, query: &str, trash: bool) -> impl Iterator<Item = &'a Entry> {
        let query = model::name_key(query);
        self.entries().filter(move |entry| {
            entry.is_trashed() == trash && model::name_key(&entry.name).contains(&query)
        })
    }

    fn active_parent(&self, parent: Option<Id>) -> Result<()> {
        if parent.is_some_and(|id| {
            !self.entry(id).is_some_and(|entry| {
                !entry.is_trashed() && matches!(entry.kind, EntryKind::Directory)
            })
        }) {
            return Err(Error::InvalidInput);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CreateOptions, Credential, OpenMode,
        crypto::{KdfParams, Password},
    };

    #[test]
    fn tree_changes_reject_conflicts_cycles_and_preserve_independent_trash() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("vault");
        let password = Password::new("树操作测试".into()).unwrap();
        let options = CreateOptions {
            kdf: KdfParams {
                operations: 1,
                memory_bytes: 8 * 1024 * 1024,
            },
            recovery_key: false,
        };
        let (mut vault, _) = Vault::create(&path, &password, options).unwrap();
        let op = Operation::default();
        let root = vault.mkdir(None, "目录", &op).unwrap();
        let child = vault.mkdir(Some(root), "子目录", &op).unwrap();
        let old = vault.mkdir(Some(root), "先删除", &op).unwrap();
        let other = vault.mkdir(None, "Straße", &op).unwrap();
        assert!(matches!(
            vault.mkdir(None, "STRASSE", &op),
            Err(Error::Conflict)
        ));
        let head = vault.current_commit();
        assert!(vault.move_entry(root, Some(child), &op).is_err());
        assert!(vault.rename(root, "strasse", &op).is_err());
        assert_eq!(vault.current_commit(), head);
        vault.rename(other, "新名称", &op).unwrap();
        vault.move_entry(other, Some(child), &op).unwrap();
        vault.trash_entry(old, &op).unwrap();
        vault.trash_entry(root, &op).unwrap();
        assert!(vault.restore_entry(child, &op).is_err());
        vault.restore_entry(root, &op).unwrap();
        assert!(!vault.entry(child).unwrap().is_trashed());
        assert!(vault.entry(old).unwrap().is_trashed());
        let collision = vault.mkdir(Some(root), "先删除", &op).unwrap();
        assert!(matches!(
            vault.restore_entry(old, &op),
            Err(Error::Conflict)
        ));
        assert!(vault.entry(old).unwrap().is_trashed());
        vault.trash_entry(collision, &op).unwrap();
        vault.restore_entry(old, &op).unwrap();
        assert_eq!(vault.search("新名", false).count(), 1);
        drop(vault);
        let vault =
            Vault::open(&path, Credential::Password(&password), OpenMode::ReadOnly).unwrap();
        assert_eq!(vault.entry(other).unwrap().parent, Some(child));
        assert!(vault.entry(collision).unwrap().is_trashed());
        assert!(!vault.entry(old).unwrap().is_trashed());
    }
}
