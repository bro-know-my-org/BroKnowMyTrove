# 架构与首版格式

现行格式为 `BKMT-1`，首版磁盘格式与下述兼容规则已冻结。格式冻结不代表独立安全审计、正式产品发布或六目标验收。实现入口为 [vault-core](../crates/vault-core/src/lib.rs)。

## 设计边界

采用随机 ID 的不可变密文对象、加密目录快照与认证提交链。名称和逻辑目录不映射为物理路径；文件管理无需驱动、管理员权限或虚拟磁盘。CLI/GUI 共用核心，只有前端交互不同。

- `vault-core`：密码、模型、格式、内容流、事务、锁、恢复、回收和备份。
- `vault-platform`：卷信息、控制台、私有文件创建、macOS ACL 等平台接口；必要 FFI 限于小边界。
- `vault-cli`：单二进制入口、命令、控制终端、输出和退出码；按参数分流到 GUI。
- `vault-gui`：内部桌面库，后台任务与确认、浏览、进度、取消和会话清理。
- `vault-i18n`：中英语义键与报告呈现；核心格式不依赖语言。

核心不依赖 clap、egui、终端或云厂商 SDK。Rust 自有业务代码禁止 unsafe，必要平台封装局部承担 FFI。用可注入的 I/O 与发布边界测试短写、写满和中断，不为未来功能建立通用存储框架。

保护锁定态丢失或被复制的库，检测内容篡改、交换、截断和引用损坏。名称、逻辑目录、原始时间加密，但密文数量、大小、变化关联和活动时间仍泄露，未填充长度可推算明文长度。感染的主机、输入法、截屏、读内存和完整旧库重放不在可靠防护范围内。

导入源和导出结果是明文；内存零化不能覆盖宿主、终端、输入法、寄存器、交换或休眠中的全部副本。GC 不等于闪存安全擦除，改凭据不能撤销旧备份。仅从同一渠道取得的 SHA-256 不能独立认证发布者。

## 版本与兼容

应用沿用 `v<SemVer>` 发布，源码默认 `0.0.0`；应用版本不写入库，不等于磁盘格式编号。磁盘没有独立 `format_version`、`algorithm`、`min_reader` 或 `min_writer` 字段，算法由 magic、context、AAD 和 schema 固定。

普通应用更新不改变编码时无需迁移。仍声明支持 `BKMT-1` 的未来程序必须保留其合法语义。此前未发布、没有需要保留的已有库，旧 `BKMT-EXPERIMENT-*` 格式不受支持，直接拒绝打开，不提供迁移器。未知字段/枚举不以忽略方式写回；影响旧读写器解释的 schema、算法、AAD 或布局变更必须使用新的格式标识。

当前不开发通用升级器。未来确有格式 2 时实现专用迁移：先独立可恢复备份，在新目录转换，验证全部元数据/内容、名称/长度和凭据重开后切换；失败保留旧库，成功也不自动删除旧库，不原地降级。

## 目录与资源上限

库根仅有 `keys.json`、`write.lock`、`objects/`、`snapshots/`、`commits/`。三个子目录正式文件名为随机 16 字节 ID 的 32 位小写十六进制；同目录临时名以 `.tmp-` 开头。根与内部结构拒绝符号链接。

JSON 为 UTF-8、明确 serde schema、十进制整数，字节数组编码为整数数组；拒绝未知/重复字段、尾部数据和超过默认递归上限的输入。Option 写值/null，现有读取也接受缺省为 None；其他必需字段不能缺失。字段顺序和空白不作规范化签名承诺。除 keys 外，JSON 在认证解密后解析。

物理读取上限：keys 16 KiB、commit 4 KiB、snapshot 64 MiB；后两者包含 24 字节 nonce 和 16 字节标签，明文上限分别为 4056 字节与 64 MiB−40。快照最多 100,000 项、逻辑深度 128；每个内部目录和根枚举最多 100,000 项，临时/异常项也计数，新增前检查正式与临时文件总数。

二进制整数均为无符号小端；长度使用 u64，加法/乘法 checked。资源上限不是总库容量或性能承诺。

## 密钥与认证绑定

`keys.json` 包含 `magic="BKMT-1"`、`vault_id:[u8;16]`、`kdf:{operations:u32,memory_bytes:u64}`、`salt:[u8;16]`、`password:Vec<u8>`、`recovery:Option<Vec<u8>>` 和必需的 `authentication:Vec<u8>`。两种密钥包装均为 nonce[24] + XChaCha20-Poly1305 密文/标签，共 72 字节，解包恰为 32 字节主密钥。`authentication` 是同种 AEAD 对空明文的封装，共 40 字节。主密钥、盐、nonce 和 ID 使用 libsodium 系统随机源，失败即停。

Argon2id v1.3 派生 32 字节包装密钥，默认 operations=3、memory_bytes=268435456。认证前仅接受 operations 1..10、memory 8 MiB..1 GiB；低成本范围也可由核心 API 读取，不称为仅测试格式。

密码包装 AAD 为 `BKMTKEY1 + vault_id + operations:u32 + memory_bytes:u64 + salt`。恢复包装 AAD 为 `BKMTREC1 + vault_id`；恢复密钥为独立随机 32 字节，显示为 64 位小写十六进制、`-`、BLAKE2b-256 前 4 字节的 8 位校验码，校验码仅检测抄写错误。

两种入口解包主密钥后，都必须先验证 `authentication`，成功后才建立会话。认证密钥由主密钥按下述 KDF、context=`BKMKEYS1` 派生。认证 AAD 按顺序连接 `BKMTKEYS1`、固定 ASCII magic `BKMT-1`、完整密码包装 AAD、72 字节 password、一个字节的恢复槽存在标志（无=0、有=1），有恢复槽时再附其完整 72 字节。长度与顺序固定，不依赖 JSON 字段顺序或空白。认证失败拒绝打开，恢复入口也不能沿用被篡改的 KDF、盐或密码包装。

创建、改密码、恢复重置、恢复密钥轮换及移除均重新生成整体认证封装。改密码/恢复重置对两个已认证 KDF 参数分别取当前值与生产默认值的较大值，至少 3 次/256 MiB，保留更强值及原恢复槽；不需要持有恢复密钥。整体旧入口重放仍在既有威胁边界之外，更新凭据不能撤销旧副本。

主密钥通过 libsodium BLAKE2b KDF、subkey_id=1、8 字节 context 派生用途密钥：`BKMKEYS1` 入口整体认证、`BKMSNAP1` 快照、`BKMCOMM1` 提交、`BKMFILE1` 文件密钥包装。主密钥/派生值零化，包装与内容临时密钥使用 alkali 受保护内存。

固定入口兼容向量见 [keyfile.json](../crates/vault-core/tests/vectors/keyfile.json)，通过独立 libsodium C API 编码并由 Rust 验证两种入口。向量中的密码、恢复密钥和主密钥均为公开测试数据，低 KDF 成本不用于生产。

## 元数据与内容编码

快照/提交外层为 nonce[24] + AEAD 密文/标签；AAD 为 `BKMTFMT1 + purpose:u8 + vault_id[16] + physical_id[16]`，purpose 1/2/3 对应快照/提交/内容头。不能交换用途、库或对象 ID。

快照为 `{entries:[Entry...]}`，条目含 `id`、`parent:Option<Id>`、`name`、`trash_group:Option<Id>`、`kind`。Id 在 JSON 内为 16 个 u8 数组，物理文件名才用十六进制。逻辑根 parent=None，活动条目 trash_group=None。kind 是 `{"type":"directory"}`，或含 `type:"file"`、`object:Id`、`length:u64`、`modified_ns:null|u64` 的对象。

验证 ID 唯一、父目录存在、无环、深度、删除状态及活动兄弟名称唯一。名称保留原输入，比较用 NFC → Unicode 默认全量 case fold → NFC；UTF-8 字节和 UTF-16 单元均不超过 240。拒绝空名、控制字符、`\\/:*?"<>|`、`.`/`..`、末尾点/空格和 Windows 设备保留名（含扩展名及 COM/LPT 的 ¹/²/³）。不静默改名或覆盖。

内容布局：`BKMTSTR1[8] + wrapped_file_key[72] + secretstream_header[24]`，共 104 字节头；后续每帧为 `ciphertext_length:u32 + ciphertext`。文件密钥随机 32 字节，每次替换/重试生成新对象及密钥；包装绑定 purpose=3、库和物理 ID。

XChaCha20-Poly1305 secretstream 数据记录必须为 Message，明文长度恰为 min(剩余声明长度, 1,048,576)，密文增加 17 字节；最后必须有独立空 Final，之后立即 EOF。每帧 AAD 为内容头 AAD + record_index:u64，从 0 起。空文件只有 Final，总物理长度恰为 `L + ceil(L / 1048576) * 21 + 125`。

验证长度、序号、tag、Final 和尾部；允许短读/短写，使用 read_exact/write_all。流缓冲有界，不支持随机访问或原地修改。完整认证前仅写私有导出暂存或丢弃 sink，不发布正式导出名。

未知 keys magic 在解锁时拒绝，未知元数据 schema 在解析时拒绝；坏提交禁写。未知内容头在实际解密时拒绝，快速打开只核对对象存在与精确大小，不认证全内容。拒绝未知格式前可能取得锁并创建 write.lock，不代表自动转换。

## 提交、锁与持久性

提交明文为 `{sequence:u64,parent:Option<Id>,snapshot:Id}`，根为 0/None，后续比父序号恰大 1。打开扫描并认证有界提交图；恰有一个根、一条连通线性链、一个叶才可写，没有可覆盖的未认证 HEAD。缺父、多根/叶、损坏正式提交、重复序号及同步冲突均禁写，不自动选择最高序号或旧头。

打开至关闭持有根 write.lock 的 OS 独占锁，只读/恢复会话也用同一把锁；不删除“过期锁”抢占，进程退出释放。每次修改前重查当前提交一致，否则只读。这只能协调同机，无法防远端并写或完整旧库回放。

提交按对象 → 快照 → 提交顺序进行，每层先同目录暂存、sync_all、无覆盖发布，再刷新目录。提交发布后刷新失败返回不确定并禁写；取消仅在发布前生效，发布后完成刷新。Unix 刷新目录；Windows 使用带 BACKUP_SEMANTICS 与 OPEN_REPARSE_POINT 的可写目录句柄调用 FlushFileBuffers，不支持或权限不足时返回错误，不静默跳过。Windows 原生与真实断电验收仍未完成，不据此承诺同等断电保证。

首次创建先发布 keys，再完成空快照和初始提交；开始触盘后不再因取消中断初始化。强制终止可留不完整库，无有效提交图时拒绝正常打开，不伪造空库。

密钥更新在同一锁下写私有 `.tmp-key-*`、同步、同目录原子替换 keys，再刷根目录，不先删旧入口。发布前保留旧入口，发布后错误禁写并报告不确定；成功更新缓存，拒绝覆盖外部入口变化。残留根临时包装不阻止完整入口重开；显式 GC 在持锁时清理普通 `.tmp-key-*` 文件并刷新根目录，拒绝符号链接及未知条目。清理前仍有旧凭据副本风险，清理不等于安全擦除。

## 回收、导入导出与备份

回收递归标记当前活动后代为同组，保留此前独立删除组；恢复要求父目录活动，只恢复同组后代。清空先提交无垃圾快照，GC 先退役全部非当前恢复快照并刷目录，再删当前快照（含回收站）不引用的对象，保留提交父链。中断宁可多留对象，不破坏引用；失败报告“已提交/清理未完成”。空间报告为逻辑密文字节，不等于实际块空间增量。

导入只接受普通文件，检查打开前/句柄/读完后的源状态，尽力检测变化，不保证恶意并发写者下的文件系统快照。保留明文源；空间预估含旧新对象、快照和提交，仍须处理 ENOSPC。FAT32 按密文对象上限提前拒绝超大文件，不做物理分卷。

导出父目录须存在，检查便携名称、Unicode/casefold 冲突及路径长度；同目录明文暂存 Unix 使用并核对 0600，macOS 句柄检查并拒绝非空 ACL，Windows 创建时设置受保护 owner/SYSTEM DACL、拒绝额外数据句柄，卷须支持持久 ACL。检查失败清理空暂存，写入/认证/flush/sync 全成功后无覆盖发布。失败清理不能保证介质无残留；Windows/macOS 原生权限验收仍待补。

备份全程持锁，复制前后核对提交和 keys；复制原 keys、全部认证提交、当前快照及其引用对象，不带历史快照、孤立对象或临时文件。复制有界流、同步、回读比较；Full 另认证目标全部内容，Copy 不认证内容。新目标最后发布 keys，发布前失败尽力清理；发布后即使有错仍尝试目标及父目录刷新，用 publication_failure 保留首错，禁止伪报完全成功。

云盘须完整下载、写前同步并暂停客户端、关闭库后再同步。缺失/冲突保留证据并禁写，不自动合并；整段最新后缀未到而已有链自洽时无法识别远端新状态。全快照丢失保留密文但不能恢复目录/名称，没有孤立对象抢救命令。

## 选型与原始资料

对象库适合应用内整理、改名不重写内容与用户态携带，代价是自有格式/事务必须审查。Cryptomator 互通、VeraCrypt 挂载与 age 归档均已排除，作为技术参照，不另建兼容层。VeraCrypt 的 XTS 不提供文件级认证；age 支持 seek，单个归档仍不等于可变文件库。

密码资料：[secretstream](https://doc.libsodium.org/secret-key_cryptography/secretstream)、[Argon2id](https://doc.libsodium.org/password_hashing/default_phf)、[AEAD](https://doc.libsodium.org/secret-key_cryptography/aead/chacha20-poly1305/xchacha20-poly1305_construction)、[内存管理](https://doc.libsodium.org/memory_management)。参考格式：[Cryptomator 安全目标](https://github.com/cryptomator/docs/blob/develop/docs/security/security-target.mdx)、[VeraCrypt 便携限制](https://www.veracrypt.fr/en/Portable%20Mode.html)、[age 格式](https://age-encryption.org/v1)。
