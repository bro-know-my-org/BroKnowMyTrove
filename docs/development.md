# 开发指南

以下命令均从仓库根目录执行，网站步骤除外。构建版本以仓库配置为准；检查命令见下文[工程检查](#工程检查)，执行结果以当前环境实际输出为准。

## Rust 构建

使用 rustup、[rust-toolchain.toml](../rust-toolchain.toml) 固定的 Rust 1.98.1，以及本机 C/C++ 工具链；workspace Edition 2024、最低 Rust 1.98，升级时同步维护。使用 Cargo.lock，不启用 `target-cpu=native`。

```sh
cargo build --locked
cargo run --locked -p vault-cli
cargo run --locked -p vault-cli -- --help
cargo run --locked -p vault-cli -- --version
# 可选服务器构建，无 GUI 开发依赖；产物名称仍为 bkmt
cargo build --locked -p vault-cli --no-default-features
```

默认生成 `target/debug/bkmt`（Windows 为 `bkmt.exe`），无参数打开 GUI，有参数执行 CLI。纯 CLI 构建会替换同目录一体版，重新执行默认构建可恢复；错误参数不回退 GUI。

三平台发布构建统一使用根清单的 `[profile.release]`：剥离符号表、启用 ThinLTO，设置 `codegen-units = 1`。本机执行 `cargo build --locked --release` 使用相同优化配置；发布压缩格式仍为 Unix `tar.gz`、Windows `zip`。

Windows 使用对应 MSVC/Windows SDK，保留控制台子系统，GUI 仅脱离独占控制台，共享终端继续连接；双击闪窗及错误对话框原生体验待验证。macOS 使用 Xcode Command Line Tools。

Linux 默认 GUI 使用 eframe 0.36.2、glow/OpenGL，编译 X11/Wayland 后端。Ubuntu 24.04 的构建依赖示例：

```sh
sudo apt-get install build-essential pkg-config libx11-dev libxcursor-dev \
  libxi-dev libxrandr-dev libxkbcommon-dev libxkbcommon-x11-0 libwayland-dev \
  libegl1-mesa-dev libgl1-mesa-dev
```

GUI 运行需要图形会话和相应库/驱动；X11 后端会动态加载 `libxkbcommon-x11.so.0`，Ubuntu 需安装 `libxkbcommon-x11-0`（仅安装 `libxkbcommon-dev` 不足以提供该库）。CLI 分支不初始化图形后端。rfd 0.17.2 原生选择器在 Linux 依赖会话 D-Bus、xdg-desktop-portal 和桌面文件选择后端，隔离会话可能失败，保留手填路径入口。

## 固定密码依赖与许可

固定 alkali 0.3.0、libsodium-sys-stable 1.24.3、libsodium 1.0.22。构建输入在 [vendor/libsodium](../vendor/libsodium)，由 [.cargo/config.toml](../.cargo/config.toml) 强制绑定读取，不随远端 latest 更新。

`LATEST.tar.gz` 及 minisign 签名原样取自锁定的 crates.io 绑定包；`libsodium-1.0.22-stable-msvc.zip` 及签名来自 [libsodium 官方发布目录](https://download.libsodium.org/libsodium/releases/)，含 x64/ARM64 静态库。四个文件的 SHA-256 固定在 SHA256SUMS，绑定内置公钥验证官方签名。

Linux/macOS 从固定源码构建，需要 C 编译器与 make；源码已有 configure，不需 autoconf/libtool。Windows 使用固定 MSVC 库。禁用 fetch-latest、optimized、use-pkg-config；不以系统 libsodium 或宿主专用 CPU 选项替代发布输入，构建环境不设置 SODIUM_LIB_DIR/SODIUM_USE_PKG_CONFIG。

```sh
cd vendor/libsodium
sha256sum --check SHA256SUMS
```

libsodium 使用 ISC，完整许可在源码包 libsodium-stable/LICENSE；alkali/绑定使用 MIT OR Apache-2.0。GUI 嵌入精简的 BKMT Sans UI 静态文案子集和 NotoSerifUI 标题字体，附 Debian 来源及 SIL OFL，许可保留在 [assets/fonts](../assets/fonts)。网站 Octicons 使用 MIT，见 [octicons-LICENSE.txt](../website/public/octicons-LICENSE.txt)。公开分发须保留相关许可并复核供应链。

## GUI 字体

正文和输入框优先使用本机系统字体，保留 egui 默认拉丁/符号字体，并查找系统 CJK 字体作为回退；标题优先使用内嵌的 NotoSerifUI。系统字体通过 fontdb 从各平台字体目录发现，保留 TTC face index，仅将选中的字体读取到内存，不复制进发布包。字体文件不可读或不存在时跳过，并使用内嵌文案子集保证中英文界面可读。用户文件名、路径和输入的字形覆盖取决于已安装的系统字体；精简 Linux 环境可安装 `fonts-noto-cjk` 补充中文覆盖。

egui 0.36 按行高居中回退字形，不会自动对齐不同字体的基线。加载时使用 skrifa 读取实际 face 的 ascent、descent、leading 和 units-per-em，以正文主字体为共同参照，通过 `FontTweak::y_offset_factor` 校正文字字体的相对基线。校正随字号缩放，不改变字体选择和字形覆盖，不增加字体资源；emoji/符号字体保留原有光学校正。字体回归测试检查实际栅格字形的位置，覆盖正文、等宽与标题字族、多个字号和缩放，以及没有系统字体的回退；允许一个物理像素加内部度量取整误差。升级 egui 时需复核其回退排版规则。

`assets/fonts/NotoSansCJK-Regular.ttc` 仅作为字体子集生成和测试输入，程序不再嵌入完整集合。修改 GUI 或翻译文案后，在仓库根目录重建静态文案子集并运行字体覆盖测试：

```sh
uv run --no-project --with fonttools==4.60.1 python scripts/subset-ui-font.py
cargo test --locked -p vault-gui fonts::tests
# 需本机已安装 CJK 字体，验证实际系统字体发现和文件名排版
cargo test --locked -p vault-gui installed_system_fonts_render_filename -- --ignored
```

子集收集 GUI 与翻译源码中的字符；覆盖测试会检查无系统字体时的界面文字，新增文案缺字时会失败，提示重新生成。生成的字体文件随源码提交，正常构建不需要 Python 或 fontTools。

## 工程检查

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --workspace
cargo test --locked -p vault-cli --no-default-features
cargo build --locked -p vault-cli --example password-probe
python3 scripts/test-terminal.py target/debug/examples/password-probe
python3 scripts/test-cli.py target/debug/bkmt
python3 scripts/test-management.py target/debug/bkmt
# 需要 Xvfb、xdotool 及 GL 运行库
bash scripts/smoke-gui.sh
# 收尾恢复默认一体版
cargo build --locked
```

测试应覆盖 schema 上限/未知版本、空文件/块边界、损坏/交换/截断/尾部、错误凭据、缺失/分叉、进程重开/同机锁、短写/写满、取消和各持久化边界。不能用现有通过数量代替当前重新执行；按需基准不在常规测试中运行。

## 翻译维护

在 [vault-i18n/src/lib.rs](../crates/vault-i18n/src/lib.rs) 的 `texts!` 中添加 TextKey 时同时提供中英文。参数化报告保存在 Message，显示时 `render(locale)`，完整句子由 Rust 格式化检查参数，不在业务代码拼接翻译片段。语言不影响核心错误类型、格式或文件名。

## 发布

推送 `vMAJOR.MINOR.PATCH` 标签会触发 [Release 工作流](../.github/workflows/release.yml)，从标签对应提交构建 Windows、macOS、Linux 的 x64 / ARM64 二进制，附单平台包、六平台合集和 SHA-256 校验和。构建时从标签注入版本，源码中的 `0.0.0` 是开发占位值。

如标签推送没有触发工作流，或需要重试，在 Actions → GitHub Release → Run workflow 中选择 main，并填写已存在的标签。工作流验证标签后，测试与构建都检出该标签；无需移动标签。附件准备齐全后才公开 Release，已公开的 Release 不自动覆盖。

各平台压缩包与合集包含字体和 libsodium 的 `LICENSES/`，解压分发时一并保留。

发布构建与功能测试使用标签源码；GUI 冒烟测试脚本使用工作流提交的版本，以便修复 CI 显示服务的就绪等待等测试环境问题，无需移动已有标签。

## 网站

[website](../website) 使用 Vite + TypeScript + 原生 CSS；Node 24（.nvmrc）、pnpm 11.25.0。进入 website 执行：

```sh
pnpm install --prod=false
pnpm dev
pnpm check
pnpm build
pnpm preview
```

开发地址以终端为准，通常为 localhost:5173；build 先做 TypeScript 检查，再生成 dist，资源为相对路径，可用于自定义域名根目录或 GitHub Pages 项目子目录。[Pages 工作流](../.github/workflows/pages.yml) 在 main 的网站文件变更时自动构建并部署，也支持手动运行；PR 只构建验证。仓库 Settings → Pages 的 Source 使用 GitHub Actions，自定义域名设为 `trove.bro-know-my.org` 并启用 Enforce HTTPS，站点为 https://trove.bro-know-my.org/ 。[public/CNAME](../website/public/CNAME) 随构建复制到 dist 根目录，保存相同域名。

域名与缓存使用 GitHub Pages + Cloudflare：

- Cloudflare DNS 的 `trove` CNAME 指向 `bro-know-my-org.github.io`。首次绑定时先使用 DNS only，待 GitHub Pages 的域名证书签发后开启代理；SSL/TLS 使用 Full (strict)。
- Cache Rules 只匹配 `trove.bro-know-my.org`：`/assets/` 下带内容哈希的 JS/CSS 在边缘和浏览器缓存一年；其他路径在边缘缓存 5 分钟、浏览器不缓存；两条规则均不缓存 400–599 响应。普通 SVG、HTML 和 CNAME 不适用一年缓存。
- DNS、证书和 Cache Rules 由托管平台配置，源码构建不会自动创建这些设置。页面更新通常在边缘缓存过期后可见；紧急更新可在 Cloudflare 按 URL 清除相关页面缓存。长期缓存仅适用于文件名含内容哈希的资源，后续不要向 `/assets/` 放入固定文件名的文件。

index.html 为结构，src/main.ts/style.css 为入口，demo.ts/demo.css 为 GUI 演示，desktop.ts/desktop.css 为虚拟桌面和拖放。演示使用虚拟文件，不执行真实加解密或文件 I/O；支持拖入/拖出、触摸指针、键盘 Enter/空格、Esc 取消、五步跳转、进度、暂停/重播及 0.5×/1×/2×，1× 约 30 秒。

下载按钮指向 `https://github.com/bro-know-my-org/BroKnowMyTrove/releases`，GitHub 图标指向同仓库。更名时同步维护 `index.html` 两个链接；下载入口保持 Releases 列表，无须随版本更改。发布前应确认远端仓库和附件可访问。
