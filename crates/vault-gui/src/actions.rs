use std::path::PathBuf;
use vault_core::{Id, Result, Vault, operation::Operation};
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

    pub fn run(self, vault: &mut Vault, operation: &Operation) -> Result<String> {
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
                return Ok(format!(
                    "回收站清空提交：{}；退役 {} 个恢复点；释放对象 {} 字节、临时文件 {} 字节；清理结果：{}",
                    report.trash_committed,
                    report.retired_snapshots,
                    report.reclaimed_object_bytes,
                    report.removed_temporary_bytes,
                    report
                        .failure
                        .map_or_else(|| "完成".into(), |error| error.to_string())
                ));
            }
            Self::Check => {
                let report = vault.verify_content(operation)?;
                return Ok(format!(
                    "完整内容认证：通过 {} 个对象、{} 字节；失败 {} 个对象。",
                    report.verified_objects,
                    report.plaintext_bytes,
                    report.failed_objects.len()
                ));
            }
            Self::Space => {
                let report = vault.storage_usage()?;
                return Ok(format!(
                    "活动 {} 字节；回收站 {} 字节；退役本地恢复点后可回收 {} 字节。",
                    report.active_object_bytes,
                    report.trash_object_bytes,
                    report.reclaimable_object_bytes
                ));
            }
        }
        Ok("操作完成。".into())
    }
}
