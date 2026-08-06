<p align="center">
  <a href="README.md">English</a> · <strong>简体中文</strong>
</p>

<p align="center">
  <img src="assets/windows/torto-128.png" width="112" height="112" alt="Torto 小龟阅读图标">
</p>

<h1 id="torto-小龟阅读" align="center">Torto · 小龟阅读</h1>

<p align="center">
  一款专注、本地优先的 Windows 与 macOS 电子书阅读器。<br>
  不依赖 WebView，以原生 Rust 渲染，让书库始终由你掌控。
</p>

<p align="center">
  <a href="https://github.com/L-Chris/torto/releases/latest"><img src="https://img.shields.io/github/v/release/L-Chris/torto?display_name=tag&sort=semver" alt="最新版本"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/L-Chris/torto" alt="MIT 许可证"></a>
  <img src="https://img.shields.io/badge/platform-Windows%20%7C%20macOS-5b6ee1" alt="支持 Windows 与 macOS">
  <img src="https://img.shields.io/badge/UI-egui-7c3aed" alt="使用 egui 构建">
  <a href="https://linux.do"><img src="https://img.shields.io/badge/LINUX-DO-FFB003.svg" alt="LINUX DO"></a>
</p>

<p align="center">
  <a href="#主要功能">主要功能</a> •
  <a href="#产品截图">产品截图</a> •
  <a href="#下载与使用">下载与使用</a> •
  <a href="#数据与隐私">数据与隐私</a> •
  <a href="#当前说明">当前说明</a> •
  <a href="#开发者信息">开发者信息</a> •
  <a href="#开源许可">开源许可</a>
</p>

## 认识 Torto

Torto（中文名“小龟阅读”）是一款开源的 Windows 与 macOS 桌面电子书阅读器。它将本地书架、灵活的阅读布局、搜索与批注、翻译、可选的 AI 阅读助手，以及直接连接 WebDAV 的跨设备同步整合在一起。

与基于浏览器的阅读器不同，Torto 使用 egui、Parley、Vello 和 wgpu 构建原生 Rust 阅读管线，自行完成书籍解析、排版、分页与渲染。

## 主要功能

<div align="left">✅ 已实现</div>

| **功能** | **说明** | **状态** |
| --- | --- | --- |
| **多格式支持** | 阅读无 DRM 的 EPUB、MOBI、AZW、AZW3/KF8、FB2、FBZ、CBZ 与 PDF 文件。 | ✅ |
| **原生渲染** | 使用 Rust 完成解析、排版、分页与渲染，不嵌入浏览器或 WebView。 | ✅ |
| **分页与滑动模式** | 在单页、双页和章节内纵向滑动三种阅读布局之间切换。 | ✅ |
| **本地书架** | 批量导入书籍，展示元数据与封面，按书名或作者搜索，并识别重复书籍。 | ✅ |
| **导航与搜索** | 支持层级目录、章节跟随、全书搜索、键盘导航、鼠标滚轮翻页和 `F11` 全屏。 | ✅ |
| **字体与主题** | 分别调整正文与界面字体、字号、字重和布局，并提供浅色与深色主题。 | ✅ |
| **文字选择与批注** | 支持自由、按单词、按句子和按段落选择，复制文字、创建高亮与笔记，并准确返回原文位置。 | ✅ |
| **图片预览** | 点击正文图片打开蒙层预览，通过滚轮缩放、拖拽平移，并可复制图片到剪贴板。 | ✅ |
| **翻译阅读** | 以替换或双语模式翻译正文，也可以翻译书籍目录。 | ✅ |
| **AI 阅读助手** | 围绕当前书籍提问，回答附带原文引用，并支持 Markdown、公式、SVG 和 Mermaid 展示。 | ✅ 可选 |
| **WebDAV 同步** | 通过自己的 WebDAV 服务直接同步书籍、阅读进度、高亮与笔记。 | ✅ 可选 |
| **Windows 自动更新** | 自动检查 GitHub Releases，以 SHA-256 校验 MSI，并在用户确认后完成升级。 | ✅ Windows |

## 产品截图

### 整洁的本地书架

导入电子书后自动读取书名、作者和封面，可以搜索书籍，也可以直接继续上次阅读。

![Torto 小龟阅读书架](assets/screenshots/library.png)

### 专注的阅读界面

目录、正文、翻译工具和 AI 助手保持清晰分区，需要时随手可用，不打扰阅读。

![Torto 小龟阅读双页阅读界面](assets/screenshots/reader.png)

## 下载与使用

前往 [GitHub Releases](https://github.com/L-Chris/torto/releases/latest) 下载最新安装包。

| **平台** | **安装包** | **系统要求** |
| --- | --- | --- |
| Windows | `Torto-*-x86_64.msi` | 64 位 Windows 10 或 Windows 11 |
| macOS，Apple 芯片 | `Torto-*-macos-arm64.dmg` | macOS 12 或更高版本 |
| macOS，Intel | `Torto-*-macos-x86_64.dmg` | macOS 12 或更高版本 |

首次打开后：

1. 点击书架右上角“导入”，选择一本或多本电子书。
2. 点击封面或书名进入阅读。
3. 使用 `←` / `→`、鼠标滚轮或滑动模式进行导航。
4. 使用 `Ctrl + F` 搜索当前书籍。
5. 通过阅读器菜单调整布局、主题、翻译、AI 与同步设置。

## 数据与隐私

- 导入的电子书和阅读数据保存在本机应用数据目录。
- WebDAV 密码和 AI API Key 保存在系统安全凭据存储中，而不是普通配置文件里。
- AI 与翻译功能需要主动配置和使用，Torto 不会在后台自动发送书籍内容。
- WebDAV 同步由桌面端直接连接用户选择的服务，不经过 Torto 自建中转服务器。

## 当前说明

Torto 仍在持续开发中。当前不支持带 DRM 的电子书，也不追求完整浏览器级 HTML/CSS 兼容；复杂固定版式、竖排、Ruby 注音以及部分书内交互内容仍可能无法完整显示。

如果遇到解析、排版或安装问题，欢迎在 [Issues](https://github.com/L-Chris/torto/issues) 中反馈，并附上电子书格式、问题截图和可复现步骤。请勿上传受版权保护的完整书籍。

## 开发者信息

<details>
<summary>展开本地开发说明</summary>

### 本地开发

项目使用 Rust `1.97.1`，桌面端包名仍为 `rebook-desktop`，运行产物为 `torto.exe`。

```powershell
# Windows 可选：安装 sccache 以复用编译缓存。
winget install --id Mozilla.sccache --exact

# 使用 sccache；未安装时会回退到普通 Cargo。
.\scripts\cargo-sccache.cmd run --locked -p rebook-desktop
cargo run --locked -p rebook-desktop

# 与 Release 接近并保留调试符号的性能构建。
cargo run --locked --profile perf -p rebook-desktop
```

修改 Rust 或 TOML 后自动重启：

```powershell
watchexec -r -e rs,toml -- cargo run --locked -p rebook-desktop
```

质量检查：

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

重新生成 Windows / macOS 多尺寸图标：

```powershell
cargo run -p rebook-desktop --example generate_windows_icons
cargo run -p rebook-desktop --example generate_macos_icons
```

核心架构采用 `parser → Reading IR → layout → renderer`，正文由 Parley、Vello 和 wgpu 完成原生排版与渲染。

- [原生渲染架构决策](docs/adr-0001-native-epub-renderer.md)
- [WebDAV 同步协议 v1](docs/webdav-sync-v1.md)
- [核心依赖已知问题](docs/known-upstream-issues.md)

</details>

## 开源许可

本项目基于 [MIT License](LICENSE) 开源。
