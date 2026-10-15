use std::{
    collections::{HashMap, HashSet},
    fmt,
    str::FromStr,
};

use serde::{Deserialize, Serialize};
use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroize;

use crate::{Error, Result, crypto::random_array};

pub const MAX_ENTRIES: usize = 100_000;
pub const CHUNK: usize = 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Id(pub(crate) [u8; 16]);

impl Id {
    pub(crate) fn random() -> Result<Self> {
        Ok(Self(random_array()?))
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl FromStr for Id {
    type Err = Error;
    fn from_str(input: &str) -> Result<Self> {
        if input.len() != 32
            || !input
                .bytes()
                .all(|ch| ch.is_ascii_digit() || (b'a'..=b'f').contains(&ch))
        {
            return Err(Error::InvalidInput);
        }
        let bytes = hex::decode(input).map_err(|_| Error::InvalidInput)?;
        Ok(Self(bytes.try_into().map_err(|_| Error::InvalidInput)?))
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum EntryKind {
    Directory,
    File {
        object: Id,
        length: u64,
        modified_ns: Option<u64>,
    },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub id: Id,
    pub parent: Option<Id>,
    pub name: String,
    pub trash_group: Option<Id>,
    pub kind: EntryKind,
}

impl Entry {
    pub fn is_trashed(&self) -> bool {
        self.trash_group.is_some()
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.name.zeroize();
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Snapshot {
    pub entries: Vec<Entry>,
}

impl Snapshot {
    pub fn validate(&self) -> Result<()> {
        if self.entries.len() > MAX_ENTRIES {
            return Err(Error::ResourceLimit);
        }
        let entries: HashMap<_, _> = self.entries.iter().map(|entry| (entry.id, entry)).collect();
        if entries.len() != self.entries.len() {
            return Err(Error::InvalidFormat);
        }
        let mut siblings = HashSet::new();
        for entry in &self.entries {
            validate_name(&entry.name)?;
            if !entry.is_trashed() && !siblings.insert((entry.parent, name_key(&entry.name))) {
                return Err(Error::Conflict);
            }
            if let EntryKind::File { length, .. } = entry.kind {
                object_size(length)?;
            }
            let mut parent = entry.parent;
            for depth in 0..=128 {
                let Some(id) = parent else {
                    break;
                };
                if id == entry.id {
                    return Err(Error::InvalidFormat);
                }
                if depth == 128 {
                    return Err(Error::ResourceLimit);
                }
                let node = entries.get(&id).ok_or(Error::InvalidFormat)?;
                if !matches!(node.kind, EntryKind::Directory)
                    || (node.is_trashed() && !entry.is_trashed())
                {
                    return Err(Error::InvalidFormat);
                }
                parent = node.parent;
            }
        }
        Ok(())
    }

    pub fn referenced_objects(&self) -> HashSet<Id> {
        self.entries
            .iter()
            .filter_map(|entry| match entry.kind {
                EntryKind::File { object, .. } => Some(object),
                EntryKind::Directory => None,
            })
            .collect()
    }

    pub fn subtree(&self, root: Id) -> HashSet<Id> {
        let mut children = HashMap::<Id, Vec<Id>>::new();
        for entry in &self.entries {
            if let Some(parent) = entry.parent {
                children.entry(parent).or_default().push(entry.id);
            }
        }
        let mut result = HashSet::new();
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            if result.insert(id) {
                pending.extend(children.get(&id).into_iter().flatten().copied());
            }
        }
        result
    }
}

pub(crate) fn name_key(name: &str) -> String {
    name.nfc().case_fold().nfc().collect()
}

pub(crate) fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 240
        || name.encode_utf16().count() > 240
        || name.ends_with(['.', ' '])
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|ch| ch.is_control() || "\\/:*?\"<>|".contains(ch))
    {
        return Err(Error::InvalidInput);
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "CLOCK$"].contains(&stem.as_str())
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|n| {
                ["1", "2", "3", "4", "5", "6", "7", "8", "9", "¹", "²", "³"].contains(&n)
            })
        })
    {
        return Err(Error::InvalidInput);
    }
    Ok(())
}

pub(crate) fn object_size(length: u64) -> Result<u64> {
    let records = length.div_ceil(CHUNK as u64);
    length
        .checked_add(records.checked_mul(21).ok_or(Error::ResourceLimit)?)
        .and_then(|size| size.checked_add(125))
        .ok_or(Error::ResourceLimit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_cycles_invalid_parents_and_portable_name_conflicts() {
        let id = Id([1; 16]);
        let a = Entry {
            id,
            parent: Some(id),
            name: "目录".into(),
            trash_group: None,
            kind: EntryKind::Directory,
        };
        assert!(Snapshot { entries: vec![a] }.validate().is_err());
        let a = Entry {
            id,
            parent: None,
            name: "café".into(),
            trash_group: None,
            kind: EntryKind::Directory,
        };
        let mut b = a.clone();
        b.id = Id([2; 16]);
        b.name = "CAFE\u{301}".into();
        assert!(
            Snapshot {
                entries: vec![a, b]
            }
            .validate()
            .is_err()
        );
        for name in [
            "../escape",
            "NUL.txt",
            "COM¹.txt",
            "a\\b",
            "trailing.",
            "bad:",
        ] {
            assert!(validate_name(name).is_err());
        }
        assert_eq!(name_key("Straße"), name_key("STRASSE"));
        assert_eq!(object_size(0).unwrap(), 125);
        assert!(object_size(u64::MAX).is_err());
    }

    #[test]
    fn schema_vectors_reject_unknown_duplicate_and_trailing_fields() {
        let cases: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../tests/vectors/metadata.json")).unwrap();
        for case in cases {
            let result = serde_json::from_str::<Snapshot>(case["json"].as_str().unwrap());
            assert_eq!(result.is_ok(), case["valid"].as_bool().unwrap());
        }
    }
}
