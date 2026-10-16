use crate::{
    Id, Result,
    codec::{self, COMMIT_LIMIT},
    crypto::Secret,
    operation::{Operation, Point},
    store::{Kind, Store},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Commit {
    pub sequence: u64,
    pub parent: Option<Id>,
    pub snapshot: Id,
}

#[derive(Clone, Debug)]
pub struct CommitInfo {
    pub id: Id,
    pub sequence: u64,
    /// Presence is not a full authentication or content check.
    pub snapshot_present: bool,
}

#[derive(Clone, Debug)]
pub enum Issue {
    DamagedCommit(Id),
    InvalidCommitGraph,
    UnexpectedPhysicalEntry,
    MissingSnapshot(Id),
    DamagedSnapshot(Id),
    MissingObject(Id),
    DamagedObject(Id),
}

#[derive(Default, Debug)]
pub struct Inspection {
    pub commits: Vec<CommitInfo>,
    pub issues: Vec<Issue>,
    pub head: Option<Id>,
    pub temporary_files: usize,
}

pub(crate) struct Ledger {
    pub report: Inspection,
    pub records: BTreeMap<Id, Commit>,
}

pub(crate) fn scan(store: &Store, master: &Secret, vault: Id) -> Result<Ledger> {
    scan_with_operation(store, master, vault, &Operation::default())
}

pub(crate) fn scan_with_operation(
    store: &Store,
    master: &Secret,
    vault: Id,
    operation: &Operation,
) -> Result<Ledger> {
    operation.check()?;
    let mut report = Inspection::default();
    let (unexpected, temporary) = store.root_state(operation)?;
    report.temporary_files = temporary;
    if unexpected {
        report.issues.push(Issue::UnexpectedPhysicalEntry);
    }
    let inventory = store.inventory_with_operation(Kind::Commit, operation)?;
    for kind in [Kind::Object, Kind::Snapshot, Kind::Commit] {
        let listing = store.inventory_with_operation(kind, operation)?;
        if listing.unexpected {
            report.issues.push(Issue::UnexpectedPhysicalEntry);
        }
        report.temporary_files += listing.temporary;
    }
    let key = master.derive(b"BKMCOMM1")?;
    let mut records = BTreeMap::new();
    for id in inventory.ids {
        operation.check()?;
        let opened = store
            .read(Kind::Commit, id, COMMIT_LIMIT)
            .and_then(|bytes| {
                codec::unseal_json::<Commit>(&key, &codec::aad(2, vault, id), &bytes, COMMIT_LIMIT)
            });
        operation.checkpoint(Point::CommitRead)?;
        operation.check()?;
        match opened {
            Ok(record) => {
                records.insert(id, record);
            }
            Err(_) => report.issues.push(Issue::DamagedCommit(id)),
        }
    }
    let mut roots = 0;
    let mut invalid = records.is_empty();
    let mut children = HashMap::<Id, usize>::new();
    for (id, record) in &records {
        operation.check()?;
        match record.parent {
            Some(parent) => {
                *children.entry(parent).or_default() += 1;
                if !records.get(&parent).is_some_and(|previous| {
                    previous.sequence.checked_add(1) == Some(record.sequence)
                }) {
                    invalid = true;
                }
            }
            None => {
                roots += 1;
                if record.sequence != 0 {
                    invalid = true;
                }
            }
        }
        let snapshot_present = store
            .path(Kind::Snapshot, record.snapshot)
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.is_file());
        report.commits.push(CommitInfo {
            id: *id,
            sequence: record.sequence,
            snapshot_present,
        });
    }
    let mut leaves = Vec::new();
    for id in records.keys() {
        operation.check()?;
        if !children.contains_key(id) {
            leaves.push(*id);
        }
    }
    if invalid || roots != 1 || leaves.len() != 1 || children.values().any(|count| *count > 1) {
        report.issues.push(Issue::InvalidCommitGraph);
    }
    if report.issues.is_empty() {
        report.head = leaves.first().copied();
        if let Some(head) = report.head {
            let snapshot = records[&head].snapshot;
            if !store
                .path(Kind::Snapshot, snapshot)
                .symlink_metadata()
                .is_ok_and(|m| m.is_file())
            {
                report.issues.push(Issue::MissingSnapshot(snapshot));
                report.head = None;
            }
        }
    }
    report.commits.sort_by_key(|record| record.sequence);
    operation.check()?;
    Ok(Ledger { report, records })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::Snapshot, operation::Operation};

    fn publish(store: &Store, master: &Secret, sequence: u64, parent: Option<Id>) -> Id {
        let id = Id::random().unwrap();
        let snapshot = Id::random().unwrap();
        let bytes = codec::seal_json(
            &master.derive(b"BKMSNAP1").unwrap(),
            &codec::aad(1, Id([1; 16]), snapshot),
            &Snapshot::default(),
            codec::SNAPSHOT_LIMIT,
        )
        .unwrap();
        store
            .publish_bytes(Kind::Snapshot, snapshot, &bytes, &Operation::default())
            .unwrap();
        let record = Commit {
            sequence,
            parent,
            snapshot,
        };
        let bytes = codec::seal_json(
            &master.derive(b"BKMCOMM1").unwrap(),
            &codec::aad(2, Id([1; 16]), id),
            &record,
            COMMIT_LIMIT,
        )
        .unwrap();
        store
            .publish_bytes(Kind::Commit, id, &bytes, &Operation::default())
            .unwrap();
        id
    }

    #[test]
    fn linear_history_is_writable_but_a_fork_has_no_automatic_winner() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::create(&temp.path().join("vault")).unwrap();
        let master = Secret::generate().unwrap();
        let root = publish(&store, &master, 0, None);
        let child = publish(&store, &master, 1, Some(root));
        assert_eq!(
            scan(&store, &master, Id([1; 16])).unwrap().report.head,
            Some(child)
        );
        publish(&store, &master, 1, Some(root));
        let report = scan(&store, &master, Id([1; 16])).unwrap().report;
        assert!(report.head.is_none());
        assert!(
            report
                .issues
                .iter()
                .any(|issue| matches!(issue, Issue::InvalidCommitGraph))
        );
    }

    #[test]
    fn damaged_new_commit_is_reported_instead_of_silently_ignoring_it() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::create(&temp.path().join("vault")).unwrap();
        let master = Secret::generate().unwrap();
        let root = publish(&store, &master, 0, None);
        let child = publish(&store, &master, 1, Some(root));
        std::fs::write(store.path(Kind::Commit, child), b"damaged").unwrap();
        let report = scan(&store, &master, Id([1; 16])).unwrap().report;
        assert!(report.head.is_none());
        assert!(
            report
                .issues
                .iter()
                .any(|issue| matches!(issue, Issue::DamagedCommit(id) if *id == child))
        );
    }
}
