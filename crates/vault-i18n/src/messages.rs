use crate::{Locale, TextKey};
use vault_core::{
    BackupReport, BackupVerification, CleanupReport, ContentReport, Error, StorageUsage,
};

pub enum ErrorContext {
    Operation,
    Input,
    Task,
    Picker,
}

/// Only counters are needed for GUI notices; do not retain per-object report data.
#[derive(Clone, Copy)]
pub struct ContentSummary {
    verified_objects: u64,
    plaintext_bytes: u64,
    failed_objects: usize,
}

impl From<&ContentReport> for ContentSummary {
    fn from(report: &ContentReport) -> Self {
        Self {
            verified_objects: report.verified_objects,
            plaintext_bytes: report.plaintext_bytes,
            failed_objects: report.failed_objects.len(),
        }
    }
}

#[derive(Default)]
pub enum Message {
    #[default]
    Empty,
    Text(TextKey),
    Error(ErrorContext, Error),
    Cleanup(CleanupReport),
    Content(ContentSummary),
    Space(StorageUsage),
}

// Both format strings are checked by Rust, including argument types and count.
macro_rules! bilingual {
    ($locale:expr, $zh:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        match $locale {
            Locale::ZhCn => format!($zh $(, $arg)*),
            Locale::EnUs => format!($en $(, $arg)*),
        }
    };
}

impl Message {
    pub fn render(&self, locale: Locale) -> String {
        match self {
            Self::Empty => String::new(),
            Self::Text(key) => locale.text(*key).into(),
            Self::Error(context, error) => {
                let error = locale.error(error);
                match context {
                    ErrorContext::Operation => bilingual!(
                        locale,
                        "{}；已完成的提交不会回滚，结果不确定时请锁定并重新检查。",
                        "{}; completed commits are not rolled back. Lock and inspect again if the outcome is uncertain.",
                        error
                    ),
                    ErrorContext::Input => bilingual!(
                        locale,
                        "输入或启动失败：{}（创建时两次密码须一致）。",
                        "Input or startup failed: {} (passwords must match when creating a vault).",
                        error
                    ),
                    ErrorContext::Task => bilingual!(
                        locale,
                        "任务未启动，会话已锁定：{}",
                        "Task did not start; session locked: {}",
                        error
                    ),
                    ErrorContext::Picker => bilingual!(
                        locale,
                        "无法启动选择器：{}；可手填路径。",
                        "Unable to start picker: {}; enter the path manually.",
                        error
                    ),
                }
            }
            Self::Cleanup(report) => locale.cleanup(report),
            Self::Content(report) => locale.content_summary(report),
            Self::Space(report) => locale.storage(report),
        }
    }
}

impl Locale {
    pub fn backup(self, report: &BackupReport) -> String {
        let mode = match report.verification {
            BackupVerification::Copy => bilingual!(
                self,
                "密文复制校验和元数据认证（未认证文件内容）",
                "ciphertext copy and metadata verification (contents not authenticated)"
            ),
            BackupVerification::Full => {
                bilingual!(self, "完整内容认证", "full content authentication")
            }
        };
        let summary = bilingual!(
            self,
            "密文副本：{} 个文件、{} 字节；{}；认证 {} 个内容对象。",
            "Ciphertext copy: {} files, {} bytes; {}; {} content objects authenticated.",
            report.copied_files,
            report.ciphertext_bytes,
            mode,
            report.verified_objects
        );
        if report.publication_failure.is_some() {
            format!("{summary}\n{}", self.text(TextKey::BackupUncertain))
        } else {
            summary
        }
    }

    pub fn cleanup(self, report: &CleanupReport) -> String {
        let locale = self;
        bilingual!(
            locale,
            "回收站清空提交：{}；退役 {} 个恢复点；释放对象 {} 字节、临时文件 {} 字节；清理结果：{}",
            "Trash clearing committed: {}; retired {} recovery points; reclaimed {} object bytes and {} temporary file bytes; cleanup: {}",
            locale.boolean(report.trash_committed),
            report.retired_snapshots,
            report.reclaimed_object_bytes,
            report.removed_temporary_bytes,
            report
                .failure
                .as_ref()
                .map_or(locale.text(TextKey::Complete), |error| locale.error(error))
        )
    }

    pub fn content(self, report: &ContentReport) -> String {
        self.content_summary(&report.into())
    }

    fn content_summary(self, report: &ContentSummary) -> String {
        bilingual!(
            self,
            "完整内容认证：通过 {} 个对象、{} 字节；失败 {} 个对象。",
            "Content verification: {} objects and {} bytes verified; {} objects failed.",
            report.verified_objects,
            report.plaintext_bytes,
            report.failed_objects
        )
    }

    pub fn storage(self, report: &StorageUsage) -> String {
        bilingual!(
            self,
            "活动 {} 字节；回收站 {} 字节；退役本地恢复点后可回收 {} 字节。",
            "Active: {} bytes; trash: {} bytes; reclaimable after retiring local recovery points: {} bytes.",
            report.active_object_bytes,
            report.trash_object_bytes,
            report.reclaimable_object_bytes
        )
    }
}

impl From<TextKey> for Message {
    fn from(key: TextKey) -> Self {
        Self::Text(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_notice_keeps_only_copyable_counters_and_retranslates_them() {
        let report = ContentReport {
            verified_objects: 17,
            plaintext_bytes: 4096,
            failed_objects: vec!["00000000000000000000000000000001".parse().unwrap(); 1024],
        };
        let summary = ContentSummary::from(&report);
        let copied = summary;
        drop(report);
        let message = Message::Content(summary);
        assert_eq!(
            message.render(Locale::ZhCn),
            "完整内容认证：通过 17 个对象、4096 字节；失败 1024 个对象。"
        );
        assert_eq!(
            Message::Content(copied).render(Locale::EnUs),
            "Content verification: 17 objects and 4096 bytes verified; 1024 objects failed."
        );
    }
}
