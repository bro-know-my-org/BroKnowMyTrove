# 使用手册

本文说明当前实验版的使用约定。先阅读[产品范围](README.md#产品范围)；源码构建与系统依赖见[开发指南](development.md)。示例使用可丢弃数据，`demo-vault` 应为空或尚不存在。

## 密码和语言

中文密码统一 NFC 后按 UTF-8 使用；不裁剪空格，不改变大小写、简繁或标点。创建时输入两遍，规范化前后均最多 4096 UTF-8 字节。常见诗词、歌词和个人信息不应视为高强度密码。

CLI 从控制终端读取并以星号遮蔽，不从命令参数、环境变量或数据 stdin 取密码；无控制终端时失败。每条独立命令重新解锁，批处理只解锁一次。默认 Argon2id 使用 3 次、256 MiB。

`create --recovery` 生成可选恢复入口，密钥仅显示一次，独立于保险库保存。后续命令用 `--recovery-key` 从控制终端读取恢复密钥。列表、计划文件和导出结果可能含明文名称或内容，须自行保管。

GUI 顶栏支持简体中文/English，默认中文；语言仅在当前进程中保存，锁定不重置，不写本地配置。CLI 的全局 `--lang zh-CN` / `--lang en-US` 可在子命令前后使用；命令、参数、JSON 字段及 JSON 错误值不翻译，文件名与路径也不翻译。

```sh
./bkmt --lang en-US --help
./bkmt list ./demo-vault --lang zh-CN --json
```

## GUI

无参数运行 `bkmt`。新建时选择空目录或尚不存在的路径，输入两遍密码；可选恢复密钥只在保存页显示。Unix 已有空目录须属于当前用户，创建时收紧为 `0700`。已有库可用密码或恢复密钥解锁，支持只读打开。

选中条目后执行导出、重命名、移动、替换或回收/恢复；名称字段用于新建、导入及更名，本地路径字段用于源或导出目标。文件选择器不可用时可手填。支持逻辑目录、全库名称搜索、平铺回收站、空间报告、完整内容检查、清空与 GC。替换、回收、清空和 GC 需要确认。

后台任务显示当前内容流的字节进度，不是全库累计进度。点击锁定或空闲 5 分钟会请求取消、清理会话和输入/搜索状态；安全结束后释放锁，迟到结果不会重新解锁。关闭或 Ctrl+Q 同样等待任务安全结束。KDF 不能立即中断，已经提交的操作不会回滚；创建取消后应重新打开核查。

“备份保险库”默认完整认证；仅复制核对必须显式选择。登录页“整库恢复”恢复到新目录，成功后仍保持锁定，并回填目标供重新解锁。发布后持久性错误须单独处理。

凭据面板支持改密码、轮换/移除恢复入口。改密码输入两遍并确认；轮换或移除须验证当前密码。恢复解锁后先设置新密码，再管理恢复入口。新恢复密钥仅在保存页显示，锁定即清除。

当前 X11/XWayland 支持拖放；原生 Wayland 使用导入/导出入口。网站中的虚拟桌面演示不执行真实加密或文件读写。

## CLI 文件操作

先准备只含测试内容的 `sample.txt`。Windows 将 `./bkmt` 换成 `./bkmt.exe`，源码构建换成 `target/debug/bkmt`。

```sh
./bkmt create ./demo-vault --recovery
./bkmt import ./demo-vault ./sample.txt 文档.txt
./bkmt list ./demo-vault
./bkmt check ./demo-vault
# FILE_ID 使用 import 返回或 list 中的 ID；输出文件必须尚不存在
./bkmt export ./demo-vault FILE_ID ./exported.txt
```

`import` / `mkdir` 返回随机 ID；`list` 的 `id` / `parent` 是可复制的 32 位十六进制。具体位置参数以 `bkmt <命令> --help` 为准。

- `list` / `search`：列出或搜索名称，`--trash` 选择回收站。
- `mkdir` / `import`：创建目录或导入文件，`--parent` 指定父目录 ID，默认逻辑根。
- `replace` / `export`：按文件 ID 替换或完整认证导出；替换要求 `--confirm`，导出拒绝覆盖。
- `rename` / `move`：按条目 ID 更名或移动，拒绝循环和名称冲突。
- `trash --confirm` / `restore`：分组回收及恢复；恢复父目录不复活此前独立删除的子项。
- `space`：区分活动、回收站、退役恢复点后可回收的对象字节。
- `empty-trash --confirm` / `gc --confirm`：清空或回收，退役全部非当前本地事务恢复快照，保留小提交记录；不影响独立备份，不保证闪存安全擦除。
- `inspect` / `check`：检查认证提交图、元数据与引用，或逐对象完整认证内容。

`list`、`search`、`export`、`space`、`check` 可用 `--commit ID` 显式选择历史提交，只读恢复。分叉、缺失和损坏不自动回退旧头继续写入，先 inspect，再选择仍有效的提交导出。全快照丢失时无法恢复名称，没有孤立对象抢救入口。

导入保留源文件。导出先创建同目录私有明文暂存，完整认证后才发布正式名称。Windows FAT/exFAT 等无持久 ACL 的目标在写明文前拒绝；macOS 暂存继承任何非空 ACL 也拒绝。可先导出到支持私有权限且不继承额外 ACL 的本地目录，再按明文资料处理结果。Windows/macOS 原生权限验收仍待补。

## 备份、恢复与凭据更新

```sh
./bkmt backup ./demo-vault ./demo-backup --verification full
./bkmt restore-vault ./demo-backup ./demo-restored --recovery-key
./bkmt check ./demo-restored --recovery-key
./bkmt list ./demo-restored --recovery-key
./bkmt change-password ./demo-vault --confirm
./bkmt change-password ./demo-vault --recovery-key --confirm
./bkmt rotate-recovery ./demo-vault --confirm
./bkmt remove-recovery ./demo-vault --confirm
```

备份/整库恢复的目标必须尚不存在，父目录已存在，且不在源库内部。恢复示例使用创建时保存的恢复密钥；去掉 `--recovery-key` 即使用密码。`restore` 仅恢复回收站，整库须用 `restore-vault`。

默认 `--verification full` 认证全部复制内容；显式 `copy` 仅回读比较密文并认证元数据。包含当前快照、当前引用对象（含回收站）和完整提交记录，不保留历史快照、孤立对象和临时文件。备份不接受历史 `--commit`，不接入 batch。

发布前失败/取消尽力清理本次目标；强制终止可留下不完整目录，重试换新目录。`publication_failure` 非空表示副本已发布但目录持久性不确定，保留目标核查，退出 1，不能称为完整成功。

改密码（包括恢复重置）只重新包装同一主密钥；新包装至少 3 次/256 MiB，并保留当前更强参数。两种解锁入口均认证完整 KDF 参数、盐和密钥槽，篡改后拒绝打开。旧凭据配旧包装副本仍可解密，不能撤销旧备份或阻止完整旧入口重放。

轮换/移除恢复入口要求当前密码，不接受恢复凭据；忘记密码时先恢复重置。新密钥只显示到控制终端，不进 JSON/stdout。显示失败仍可能已更新，须用密码重开再生成。发布前失败保留旧入口，发布后不回滚；不确定时用新旧凭据重开核查。凭据管理不接入 batch。

## JSON、批处理和取消

普通 `list` / `search` 输出 JSON Lines；业务命令加 `--json` 输出一行 JSON，列表为 `{"entries":[...]}`，无返回值操作为 `{"ok":true}`，新条目为 `{"id":"..."}`。检查/空间/清理使用具名报告；检查失败也输出报告并退出 1。交互秘密仍走控制终端，错误走 stderr，诊断文案不作为解析协议。

batch 从 UTF-8 JSON Lines 文件或 `-`（数据 stdin）读有限计划。每行是省略保险库路径的参数数组，不执行 shell，路径相对当前工作目录。例如 `commands.jsonl`：

```jsonl
["import", "./sample.txt", "资料.txt"]
["list"]
["check"]
```

执行 `bkmt batch ./demo-vault ./commands.jsonl`。最多 1 MiB / 1000 条，不接受空文件或空行；先全计划语法验证，再一次解锁并持锁顺序执行。每步一行 `{index,command,status,result,error}`，序号从 1 开始。失败、取消或输出失败即停，前序提交不回滚。

支持单会话文件操作、列表/搜索、空间和内容检查；不支持 create、inspect、管理命令、嵌套 batch、逐行凭据、逐行历史版本或逐行 `--lang`。顶层语言和 `--recovery-key` 共用；顶层 `--commit` 只允许全只读计划。危险操作每行仍须 `--confirm`，ID 必须事先已知，不提供变量替换。

SIGINT 请求取消；等待输入 EOF 或输出管道堵塞时也应能取消。提交发布后会完成刷新，不能把取消视为未发生。输出末行可能不完整，不据此判断操作未生效；不确定时关闭并检查。退出码：0 成功，1 操作失败/检查发现问题，2 CLI 参数错误，130 取消；批处理计划无效为 1。
