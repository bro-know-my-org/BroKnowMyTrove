use std::path::PathBuf;
use vault_core::{Id, Result, Vault, operation::Operation};
use vault_i18n::{Message, TextKey};
use zeroize::Zeroizing;

pub enum Action {
    Mkdir(Option<Id>, Zeroizing<String>),
    Import(Option<Id>, Zeroizing<String>, PathBuf),
    Export(Id, PathBuf),
    Replace(Id, PathBuf),
    Rename(Id, Zeroizing<String>),
    Move(Id, Option<Id>),
    Trash(Id),
    Restore(Id),
    EmptyTrash,
    Collect,
    Check,
    Space,
}

impl Action {
    pub fn needs_confirmation(&self) -> bool {
        matches!(
            self,
            Self::Replace(..) | Self::Trash(_) | Self::EmptyTrash | Self::Collect
        )
    }

    pub fn run(self, vault: &mut Vault, operation: &Operation) -> Result<Message> {
        operation.check_cancelled()?;
        match self {
            Self::Mkdir(parent, name) => {
                vault.mkdir(parent, &name, operation)?;
            }
            Self::Import(parent, name, source) => {
                vault.import_file(parent, &name, &source, operation)?;
            }
            Self::Export(id, destination) => vault.export_file(id, &destination, operation)?,
            Self::Replace(id, source) => vault.replace_file(id, &source, operation)?,
            Self::Rename(id, name) => vault.rename(id, &name, operation)?,
            Self::Move(id, parent) => vault.move_entry(id, parent, operation)?,
            Self::Trash(id) => vault.trash_entry(id, operation)?,
            Self::Restore(id) => vault.restore_entry(id, operation)?,
            Self::EmptyTrash | Self::Collect => {
                let report = if matches!(self, Self::EmptyTrash) {
                    vault.empty_trash(operation)?
                } else {
                    vault.collect_garbage(operation)?
                };
                return Ok(Message::Cleanup(report));
            }
            Self::Check => {
                let report = vault.verify_content(operation)?;
                return Ok(Message::Content((&report).into()));
            }
            Self::Space => {
                let report = vault.storage_usage()?;
                return Ok(Message::Space(report));
            }
        }
        Ok(TextKey::Done.into())
    }
}
