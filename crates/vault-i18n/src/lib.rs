//! Built-in presentation text. Locale never changes vault data or machine output.
use vault_core::Error;

mod messages;
pub use messages::{ContentSummary, ErrorContext, Message};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Locale {
    #[default]
    ZhCn,
    EnUs,
}

impl Locale {
    pub fn boolean(self, value: bool) -> &'static str {
        self.text(if value { TextKey::Yes } else { TextKey::No })
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::ZhCn => "zh-CN",
            Self::EnUs => "en-US",
        }
    }

    pub fn error(self, error: &Error) -> &'static str {
        use TextKey::*;
        self.text(match error {
            Error::Io(_) => ErrorIo,
            Error::Crypto => ErrorCrypto,
            Error::Authentication => ErrorAuthentication,
            Error::InvalidFormat => ErrorFormat,
            Error::ResourceLimit => ErrorLimit,
            Error::InvalidInput => ErrorInput,
            Error::Conflict => ErrorConflict,
            Error::Locked => ErrorLocked,
            Error::ReadOnly => ErrorReadOnly,
            Error::Cancelled => ErrorCancelled,
            Error::SourceChanged => ErrorSourceChanged,
            Error::InsufficientSpace => ErrorSpace,
            Error::CommitUncertain => ErrorCommit,
            Error::ExportUncertain => ErrorExport,
        })
    }

    pub fn items(self, count: usize) -> String {
        match self {
            Self::ZhCn => format!("{count} 个项目 · 双击目录进入"),
            Self::EnUs => format!("{count} items · Double-click a folder to open"),
        }
    }

    pub fn file_size(self, size: &str) -> String {
        match self {
            Self::ZhCn => format!("文件 · {size}"),
            Self::EnUs => format!("File · {size}"),
        }
    }

    pub fn invalid_batch_line(self, line: usize) -> String {
        match self {
            Self::ZhCn => format!("批量输入第 {line} 行无效；未执行任何操作。"),
            Self::EnUs => {
                format!("Invalid batch input at line {line}; no operations were executed.")
            }
        }
    }
}

// Every key requires both translations; callers cannot invent string keys.
macro_rules! texts {
    ($($key:ident: $zh:literal, $en:literal;)*) => {
        #[derive(Clone, Copy, Debug)]
        pub enum TextKey { $($key),* }
        impl Locale {
            pub fn text(self, key: TextKey) -> &'static str {
                match (key, self) {
                    $((TextKey::$key, Self::ZhCn) => $zh,
                      (TextKey::$key, Self::EnUs) => $en,)*
                }
            }
        }
    };
}

texts! {
    Yes: "是", "yes";
    No: "否", "no";
    OpenVault: "打开保险库", "Open vault";
    NewVaultTab: "创建新保险库", "New vault";
    CreateVault: "创建保险库", "Create vault";
    UnlockVault: "解锁保险库", "Unlock vault";
    CreateHint: "选择一个新目录，设置你的保险库密码。", "Choose a new directory and set your vault password.";
    UnlockHint: "选择保险库，解锁后即可管理你的文件。", "Choose a vault and unlock it to manage your files.";
    VaultDirectory: "保险库目录", "Vault directory";
    Browse: "浏览…", "Browse…";
    BrowseHint: "新建时会在所选目录下追加“新保险库”", "Creation appends the fixed folder name ‘新保险库’ to the selected directory";
    DirectoryHint: "填写尚不存在的目录；浏览时自动追加“新保险库”。", "Enter a directory that does not exist; browsing appends the fixed folder name ‘新保险库’.";
    RecoveryKey: "恢复密钥", "Recovery key";
    Password: "密码", "Password";
    SetPassword: "设置保险库密码", "Set vault password";
    EnterPassword: "输入以解锁", "Enter to unlock";
    ConfirmPassword: "确认密码", "Confirm password";
    ShowInput: "显示输入", "Show input";
    OpenReadOnly: "只读打开", "Open read-only";
    GenerateRecovery: "同时生成恢复密钥", "Also generate a recovery key";
    UnlockRecovery: "使用恢复密钥解锁", "Unlock with recovery key";
    SaveRecovery: "保存你的恢复密钥", "Save your recovery key";
    RecoveryWarning: "恢复密钥仅在此显示一次，请独立保存；它不替代数据备份。", "The recovery key is shown only once. Save it separately; it does not replace a data backup.";
    RecoverySaved: "已独立保存，继续", "Saved separately — continue";
    FileDetails: "文件详情", "File details";
    Vault: "保险库", "Vault";
    AllFiles: "全部文件", "All files";
    Trash: "回收站", "Trash";
    WritableSession: "可读写会话", "Read-write session";
    ReadOnlySession: "只读会话", "Read-only session";
    LocalEncrypted: "本地加密存储", "Local encrypted storage";
    MyFiles: "我的文件", "My files";
    DamagedReferences: "检测到不完整或损坏引用，请检查后恢复。", "Incomplete or damaged references detected. Inspect the vault before recovery.";
    Root: "根目录", "Root";
    Up: "上一级", "Up";
    SearchHint: "搜索保险库中的全部名称…", "Search all names in the vault…";
    NoMatches: "没有找到匹配的文件", "No matching files";
    EmptyTrash: "回收站是空的", "Trash is empty";
    NoFiles: "这里还没有文件", "No files here yet";
    AddFilesHint: "在右侧展开“导入与新建”以添加文件。", "Expand ‘Import and create’ on the right to add files.";
    SearchAdjustHint: "可以切换目录，或调整搜索内容。", "Choose another folder or change your search.";
    Directory: "目录", "Folder";
    File: "文件", "File";
    OpenDirectory: "打开目录", "Open folder";
    Exit: "退出", "Exit";
    LockCancel: "锁定 / 取消", "Lock / cancel";
    Unlocked: "已解锁", "Unlocked";
    Locked: "已锁定", "Locked";
    Experimental: "实验版本", "Experimental";
    ExperimentalWarning: "格式未稳定，未经独立安全审计，仅使用可丢弃测试数据。", "The format is unstable and has not been independently audited. Use disposable test data only.";
    WaitingPicker: "等待系统选择器；锁定可丢弃选择结果。", "Waiting for the system picker; locking discards the selection.";
    AutoLock: "空闲 5 分钟自动锁定", "Locks after 5 minutes of inactivity";
    Working: "正在处理", "Working";
    CancelWarning: "取消或关闭会等待任务安全结束；已提交操作不会回滚。密码计算不能立即中断。", "Canceling or closing waits for the task to finish safely. Committed operations are not rolled back. Password computation cannot stop immediately.";
    PickerCancelled: "选择已取消或系统选择器不可用；可手填路径。", "Selection canceled or system picker unavailable; enter the path manually.";
    StoppedLocked: "已停止并锁定；操作可能已提交，请重新打开核查。", "Stopped and locked; the operation may have committed. Reopen the vault to inspect it.";
    IdleLocked: "空闲超时，已锁定。", "Locked after inactivity.";
    ConfirmAction: "确认操作", "Confirm operation";
    ConfirmReplace: "确认用本地文件替换所选文件的内容？", "Replace the selected file's contents with the local file?";
    ConfirmTrash: "确认移入加密回收站？源文件不受影响。", "Move to encrypted trash? The source file is unaffected.";
    ConfirmCleanup: "将退役全部本地历史恢复点并回收对象；清空还会永久移除回收站条目。独立备份不受影响。", "This retires all local historical recovery points and reclaims objects. Emptying trash also permanently removes its entries. Independent backups are unaffected.";
    Execute: "确认执行", "Confirm";
    Cancel: "取消", "Cancel";
    Folder: "文件夹", "Folder";
    SelectItemHint: "选择项目以查看详情和管理文件。", "Select an item to view details and manage files.";
    Name: "名称", "Name";
    NameHint: "文件或目录名称", "File or folder name";
    Rename: "重命名", "Rename";
    Transfer: "导入、导出与替换", "Import, export and replace";
    ImportCreate: "导入与新建", "Import and create";
    LocalPath: "本地文件路径", "Local file path";
    SelectSource: "选择源文件", "Select source file";
    ExportLocation: "导出位置", "Export location";
    TransferHint: "导入 / 替换的源文件，或导出目标。导入保留源，导出拒绝覆盖。", "Source for import / replace, or export destination. Import preserves the source; export refuses to overwrite.";
    NewDirectory: "新建目录", "New folder";
    Import: "导入", "Import";
    Export: "导出", "Export";
    Replace: "替换内容", "Replace contents";
    MoveTrash: "移动与回收", "Move and trash";
    MoveTo: "移动到目录", "Move to folder";
    MoveToTrash: "移入回收站", "Move to trash";
    Restore: "恢复所选", "Restore selected";
    BackupVault: "备份保险库", "Back up vault";
    RestoreVault: "整库恢复", "Restore vault";
    RestoreHint: "选择密文备份，恢复到尚不存在的新目录；不会覆盖原库。", "Choose a ciphertext backup and restore to a new directory; existing vaults are never overwritten.";
    CopyOnly: "仅校验复制（不认证文件内容）", "Verify copy only (contents not authenticated)";
    BackupTarget: "新目录路径", "New directory path";
    BackupTargetHint: "父目录必须存在；含回收站，不含历史恢复快照。", "Parent must exist. Includes trash, excludes historical recovery snapshots.";
    Maintenance: "保险库维护", "Vault maintenance";
    CheckContent: "完整内容检查", "Verify all contents";
    Space: "空间统计", "Storage usage";
    ClearTrash: "清空回收站", "Empty trash";
    Collect: "回收空间", "Reclaim space";
    PathHint: "输入路径或浏览选择…", "Enter a path or browse…";
    Done: "操作完成。", "Operation completed.";
    Complete: "完成", "Complete";
    ErrorIo: "文件系统操作失败", "filesystem operation failed";
    ErrorCrypto: "密码运算失败", "cryptographic operation failed";
    ErrorAuthentication: "认证失败（凭据错误或数据损坏）", "authentication failed (wrong credential or damaged data)";
    ErrorFormat: "无效或不支持的实验格式", "invalid or unsupported experimental format";
    ErrorLimit: "超出格式或资源限制", "format or resource limit exceeded";
    ErrorInput: "输入无效", "invalid input";
    ErrorConflict: "名称、目标或提交冲突", "name, destination, or commit conflict";
    ErrorLocked: "保险库正被另一个本地进程使用", "vault is in use by another local process";
    ErrorReadOnly: "保险库需要检查或恢复，禁止写入", "vault requires inspection or recovery; writes are disabled";
    ErrorCancelled: "操作已取消", "operation cancelled";
    ErrorSourceChanged: "导入期间源文件发生变化", "source changed while being imported";
    ErrorSpace: "空间不足或达到文件系统文件大小限制", "insufficient space or filesystem file-size limit";
    ErrorCommit: "提交结果不确定，请关闭并检查保险库", "commit outcome uncertain; close and inspect the vault";
    ErrorExport: "已发布认证导出，但目录持久性不确定", "authenticated export published; directory durability uncertain";
    ParentHelp: "目标目录 ID，默认 root", "Destination directory ID; default: root";
    RecoveryHelp: "从控制终端读取恢复密钥", "Read the recovery key from the controlling terminal";
    CommitHelp: "显式选择历史提交，只读恢复", "Select a historical commit for read-only recovery";
    JsonHelp: "输出单行 JSON 结果；错误仍写入 stderr", "Output a single JSON line; diagnostics still go to stderr";
    CreateHelp: "创建实验保险库，密码输入两遍", "Create an experimental vault; enter the password twice";
    GenerateRecoveryHelp: "生成并在控制终端一次性显示恢复密钥", "Generate a recovery key and show it once on the controlling terminal";
    ListHelp: "列出条目，输出 JSON Lines", "List entries as JSON Lines";
    TrashListHelp: "只显示回收站", "Show only trashed entries";
    SearchHelp: "搜索解锁后的名称", "Search unlocked names";
    TrashSearchHelp: "只搜索回收站", "Search only trashed entries";
    MkdirHelp: "新建逻辑目录", "Create a logical directory";
    ImportHelp: "导入普通文件并保留源文件", "Import a regular file and preserve the source";
    ReplaceHelp: "替换文件内容", "Replace file contents";
    ExportHelp: "完整认证后导出，拒绝覆盖", "Export after full authentication; refuse to overwrite";
    RenameHelp: "重命名条目", "Rename an entry";
    MoveHelp: "移动条目", "Move an entry";
    TrashHelp: "移入加密回收站", "Move to encrypted trash";
    RestoreHelp: "恢复本次删除", "Restore this deletion";
    EmptyTrashHelp: "清空回收站并退役本地历史快照；不影响独立备份", "Empty trash and retire local historical snapshots; independent backups are unaffected";
    ConfirmEmptyHelp: "确认清空及退役所有本地历史恢复点", "Confirm emptying trash and retiring all local historical recovery points";
    CollectHelp: "退役所有本地历史快照并回收无引用对象", "Retire all local historical snapshots and reclaim unreferenced objects";
    ConfirmCollectHelp: "确认退役所有本地历史恢复点", "Confirm retiring all local historical recovery points";
    SpaceHelp: "显示密文对象空间，单位字节", "Show encrypted object storage usage in bytes";
    InspectHelp: "认证提交图并检查当前元数据及引用", "Authenticate the commit graph and inspect current metadata and references";
    CheckHelp: "逐对象完整认证全部内容", "Fully authenticate all contents object by object";
    BackupHelp: "一致性密文备份到新目录，含回收站，不保留历史恢复快照", "Back up ciphertext to a new directory, including trash but excluding historical snapshots";
    RestoreVaultHelp: "从密文备份恢复整库到新目录，不覆盖现有保险库", "Restore a ciphertext backup to a new vault directory; never overwrite an existing vault";
    BackupDestinationHelp: "尚不存在的目标目录，父目录必须存在", "New destination directory; its parent must exist";
    VerificationHelp: "full（默认）认证全部内容；copy 只比较密文并认证元数据", "full (default) authenticates all contents; copy compares ciphertext and authenticates metadata only";
    BackupUncertain: "完整副本已发布，但目录持久性不确定；保留目标并重新打开检查，不要当作完全成功。", "Complete copy published, but directory durability is uncertain. Keep the destination and reopen it for inspection; do not treat this as full success.";
    BatchHelp: "单次解锁顺序执行 JSON Lines；失败即停，已完成操作不回滚", "Execute JSON Lines sequentially after one unlock; stop on failure without rolling back completed operations";
    BatchInputHelp: "命令文件；- 从数据 stdin 读取，最多 1 MiB / 1000 条", "Command file; - reads data stdin, up to 1 MiB / 1000 commands";
    RecoveryDisplayFailed: "保险库已创建，但恢复密钥显示失败；仍可使用密码打开保险库。", "Vault created, but recovery key display failed; the password can still open the vault.";
    CleanupFailed: "清理未完成，可重试；已提交的目录变更不会回滚。", "Cleanup incomplete; retry is possible. Committed directory changes are not rolled back.";
    OutputFailed: "保险库操作已执行，但结果输出失败；请检查保险库状态。", "Vault operation executed, but result output failed; inspect the vault state.";
    Created: "保险库已创建", "Vault created";
    SpaceWarning: "回收站空间须先清空；回收无引用对象将退役所有本地历史快照。", "Empty trash first to reclaim its space; collecting unreferenced objects retires all local historical snapshots.";
    BatchStopped: "批处理已停止；此前完成的操作不会回滚，结果不确定时请检查保险库状态。", "Batch stopped; completed operations are not rolled back. Inspect the vault if the outcome is uncertain.";
    CancelHandlerFailed: "无法安装取消处理器", "Unable to install cancellation handler";
    PasswordPrompt: "密码: ", "Password: ";
    ConfirmPasswordPrompt: "再次输入密码: ", "Repeat password: ";
    RecoveryPrompt: "恢复密钥: ", "Recovery key: ";
    RecoveryDisplay: "恢复密钥（请独立保存）: ", "Recovery key (save separately): ";
    LanguageHelp: "界面语言：zh-CN 或 en-US，默认 zh-CN", "Display language: zh-CN or en-US; default: zh-CN";
    HelpTemplate: "{about-with-newline}\n用法: {usage}\n\n{all-args}{after-help}", "{about-with-newline}\nUsage: {usage}\n\n{all-args}{after-help}";
    CommandsHeading: "命令", "Commands";
    ArgumentsHeading: "参数", "Arguments";
    OptionsHeading: "选项", "Options";
    HelpDescription: "显示帮助", "Print help";
    VersionDescription: "显示版本", "Print version";
    HelpCommandDescription: "显示此消息或指定子命令的帮助", "Print this message or help for a subcommand";
    MissingArguments: "错误：缺少必需参数；请使用 --help 查看用法。", "error: required arguments are missing; use --help for usage.";
    UnknownArgument: "错误：存在未知选项或命令；请使用 --help 查看用法。", "error: unknown option or command; use --help for usage.";
    InvalidArgumentValue: "错误：参数值无效；请使用 --help 查看用法。", "error: invalid argument value; use --help for usage.";
    ArgumentConflict: "错误：参数组合或数量无效；请使用 --help 查看用法。", "error: invalid argument combination or count; use --help for usage.";
    ParseFailed: "错误：无法解析命令；请使用 --help 查看用法。", "error: unable to parse command; use --help for usage.";
    HeadCommit: "当前提交", "Head commit";
    TemporaryFiles: "临时文件数", "Temporary files";
    Commit: "提交", "Commit";
    Sequence: "序号", "Sequence";
    SnapshotPresent: "快照存在", "Snapshot present";
    IssueDamagedCommit: "提交损坏", "Damaged commit";
    IssueInvalidGraph: "提交图无效", "Invalid commit graph";
    IssueUnexpectedEntry: "非预期的物理条目", "Unexpected physical entry";
    IssueMissingSnapshot: "快照缺失", "Missing snapshot";
    IssueDamagedSnapshot: "快照损坏", "Damaged snapshot";
    IssueMissingObject: "对象缺失", "Missing object";
    IssueDamagedObject: "对象损坏", "Damaged object";
}
