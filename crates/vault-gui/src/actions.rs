use crate::session::Session;
use std::path::PathBuf;
use vault_core::{BackupVerification, Id, Result, crypto::Password, operation::Operation};
use vault_i18n::{Message, TextKey};
use zeroize::Zeroizing;

pub enum Action {
    Mkdir(Option<Id>, Zeroizing<String>),
    Import(Option<Id>, Zeroizing<String>, PathBuf),
    ImportDrop(Option<Id>, Vec<PathBuf>),
    Export(Id, PathBuf),
    PrepareDrag(Id),
    Replace(Id, PathBuf),
    Rename(Id, Zeroizing<String>),
    Move(Id, Option<Id>),
    Trash(Id),
    Restore(Id),
    EmptyTrash,
    Collect,
    Check,
    Space,
    Backup(PathBuf, BackupVerification),
    ChangePassword(Password),
    RotateRecovery(Password),
    RemoveRecovery(Password),
}

impl Action {
    pub fn needs_confirmation(&self) -> bool {
        matches!(
            self,
            Self::Replace(..)
                | Self::Trash(_)
                | Self::EmptyTrash
                | Self::Collect
                | Self::ChangePassword(_)
                | Self::RotateRecovery(_)
                | Self::RemoveRecovery(_)
        )
    }

    pub fn run(self, session: &mut Session, operation: &Operation) -> Result<Message> {
        operation.check_cancelled()?;
        let vault = &mut session.vault;
        match self {
            Self::Mkdir(parent, name) => {
                vault.mkdir(parent, &name, operation)?;
            }
            Self::Import(parent, name, source) => {
                vault.import_file(parent, &name, &source, operation)?;
            }
            Self::ImportDrop(parent, sources) => {
                let total = sources.len();
                // Validate the whole drop before committing any imports. Never follow
                // directory/symlink drops or silently discard non-native paths.
                if total == 0 || total > 1000 {
                    return Err(vault_core::Error::InvalidInput);
                }
                for source in &sources {
                    operation.check_cancelled()?;
                    if !source.symlink_metadata()?.is_file()
                        || source.file_name().and_then(|name| name.to_str()).is_none()
                    {
                        return Ok(TextKey::DropFilesOnly.into());
                    }
                }
                for (completed, source) in sources.iter().enumerate() {
                    let name = source.file_name().and_then(|name| name.to_str()).unwrap();
                    if let Err(error) = vault.import_file(parent, name, source, operation) {
                        return Ok(Message::Imported(completed, total, Some(error)));
                    }
                }
                return Ok(Message::Imported(total, total, None));
            }
            Self::Export(id, destination) => vault.export_file(id, &destination, operation)?,
            Self::PrepareDrag(id) => {
                crate::drag_export::prepare(session, id, operation)?;
            }
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
            Self::Backup(destination, verification) => {
                return Ok(Message::Backup(vault.backup_to(
                    &destination,
                    verification,
                    operation,
                )?));
            }
            Self::ChangePassword(password) => vault.change_password(&password, operation)?,
            Self::RotateRecovery(password) => {
                vault.verify_password(&password, operation)?;
                session.recovery = Some(vault.rotate_recovery_key(operation)?.encode()?);
            }
            Self::RemoveRecovery(password) => {
                vault.verify_password(&password, operation)?;
                vault.remove_recovery_key(operation)?;
                session.recovery = None;
            }
        }
        Ok(TextKey::Done.into())
    }
}
