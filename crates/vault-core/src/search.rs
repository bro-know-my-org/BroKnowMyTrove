use crate::{
    Id, Result, Vault, model,
    operation::{Operation, Point},
};

/// Immutable, zeroized name index for an unlocked snapshot. Build off the UI thread.
/// Row numbers refer to `Vault::entries()` at `commit()` only.
pub struct SearchIndex {
    commit: Option<Id>,
    names: Vec<(Option<Id>, bool, model::NameKey)>,
}

impl Vault {
    pub fn search_index(&self, operation: &Operation) -> Result<SearchIndex> {
        operation.check_cancelled()?;
        let names = self
            .entries()
            .map(|entry| {
                operation.checkpoint(Point::SearchIndexEntry)?;
                operation.check_cancelled()?;
                Ok((
                    entry.parent,
                    entry.is_trashed(),
                    model::name_key(&entry.name),
                ))
            })
            .collect::<Result<_>>()?;
        operation.check_cancelled()?;
        Ok(SearchIndex {
            commit: self.current_commit(),
            names,
        })
    }
}

impl SearchIndex {
    pub fn commit(&self) -> Option<Id> {
        self.commit
    }

    /// Empty queries list a directory (or the whole trash); nonempty queries search all names.
    pub fn rows(
        &self,
        query: &str,
        parent: Option<Id>,
        trash: bool,
        operation: &Operation,
    ) -> Result<Vec<usize>> {
        operation.check_cancelled()?;
        let key = model::name_key(query);
        let mut rows = Vec::new();
        for (row, (entry_parent, entry_trash, name)) in self.names.iter().enumerate() {
            operation.check_cancelled()?;
            if *entry_trash == trash
                && if query.is_empty() {
                    trash || *entry_parent == parent
                } else {
                    name.as_str().contains(key.as_str())
                }
            {
                rows.push(row);
            }
        }
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CreateOptions, Error,
        crypto::{KdfParams, Password},
    };

    #[test]
    fn index_preserves_unicode_and_trash_rules_and_cancels_during_construction() {
        let temporary = tempfile::tempdir().unwrap();
        let password = Password::new("索引测试".into()).unwrap();
        let options = CreateOptions {
            kdf: KdfParams {
                operations: 1,
                memory_bytes: 8 * 1024 * 1024,
            },
            recovery_key: false,
        };
        let (mut vault, _) =
            Vault::create(&temporary.path().join("vault"), &password, options).unwrap();
        let operation = Operation::default();
        let root = vault.mkdir(None, "Straße", &operation).unwrap();
        let child = vault.mkdir(Some(root), "café", &operation).unwrap();
        vault.trash_entry(child, &operation).unwrap();
        let index = vault.search_index(&operation).unwrap();
        assert_eq!(
            index.rows("STRASSE", None, false, &operation).unwrap(),
            vec![0]
        );
        assert_eq!(
            index.rows("CAFE\u{301}", None, true, &operation).unwrap(),
            vec![1]
        );
        assert_eq!(index.rows("", None, false, &operation).unwrap(), vec![0]);
        let cancelled = Operation::default();
        cancelled.cancel();
        assert!(matches!(
            vault.search_index(&cancelled),
            Err(Error::Cancelled)
        ));
        let interrupted = Operation::default().cancelling_at(Point::SearchIndexEntry);
        assert!(matches!(
            vault.search_index(&interrupted),
            Err(Error::Cancelled)
        ));
    }
}
