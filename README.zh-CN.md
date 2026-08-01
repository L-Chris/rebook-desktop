<p align="center">
  <a href="README.md">English</a> · <strong>简体中文</strong>
</p>

<p align="center">
  <img src="assets/windows/torto-128.png" width="112" height="112" alt="Torto 小龟阅读图标">
</p>

<h1 align="center">Torto · 小龟阅读</h1>

<p align="center">
  一款专注、轻巧的本地电子书阅读器，支持 Windows 和 macOS。<br>
  把书留在自己的电脑里，安静地读，也更清楚地想。
</p>

<p align="center">
  <a href="https://linux.do" aria-label="LINUX DO">
    <img src="https://img.shields.io/badge/LINUX-DO-FFB003.svg" alt="LINUX DO">
  </a>
</p>

## 认识 Torto

Torto（中文名“小龟阅读”）是一款 Windows 和 macOS 原生电子书阅读器。它可以管理本地书库，提供舒适的单页或双页阅读、目录导航、全文搜索、文字高亮、翻译和 AI 阅读助手，也可以通过 WebDAV 在自己的设备之间同步书籍与阅读状态。

书籍默认保存在本机。阅读正文不依赖浏览器或 WebView，翻页和排版由原生渲染引擎完成。

## 产品截图

### 整洁的本地书架

导入电子书后，Torto 会自动读取书名、作者和封面。可以按书名或作者搜索，也可以直接从书架继续阅读。

![Torto 小龟阅读书架](assets/screenshots/library.png)

### 专注的双页阅读

目录、正文和工具栏保持清晰分区。阅读区支持单页与双页切换，方向键和鼠标滚轮都可以翻页。

![Torto 小龟阅读双页阅读界面](assets/screenshots/reader.png)

## 主要功能

- 本地书架：批量导入、封面展示、书名与作者搜索、重复书籍识别。
- 舒适排版：单页/双页布局、字号与字体调整、字重设置、图片自适应页面。
- 界面主题：浅色、深色、玻璃三种主题，阅读页面配色随主题切换。
- 阅读导航：层级目录、章节跟随、键盘和鼠标滚轮翻页、全书进度显示。
- 阅读标记：文字划选、高亮批注和快速定位。
- 全文搜索：在当前电子书中查找内容并直接跳转。
- AI 阅读助手：围绕当前书籍提问，回答附书中引用出处，支持 Markdown、公式、SVG 和 Mermaid 内容展示。
- 翻译阅读：支持替换或双语模式，也可以翻译目录。
- WebDAV 同步：内置坚果云等常用提供商，也支持自定义 WebDAV 地址。
- 多种格式：支持无 DRM 的 EPUB、MOBI、AZW、AZW3/KF8、FB2、FBZ、CBZ 和 PDF。

## 下载与使用

前往 [Releases](https://github.com/L-Chris/torto/releases) 下载最新安装包：Windows 选择 `Torto-*-x86_64.msi`，按安装向导完成安装；macOS 选择 `Torto-*-macos-arm64.dmg`（Apple 芯片）或 `Torto-*-macos-x86_64.dmg`（Intel），打开后将 Torto 拖入「应用程序」。

首次打开后：

1. 点击右上角“导入”，选择一本或多本电子书。
2. 点击封面或书名进入阅读。
3. 使用 `←` / `→` 或鼠标滚轮翻页。
4. 使用 `Ctrl + F` 搜索全书内容。
5. 在右上角菜单中打开设置，调整字体、分页、翻译、AI 和云同步。

Windows 安装包面向 64 位 Windows 10/11；macOS 安装包要求 macOS 12 或更高版本。为获得更流畅的原生渲染体验，建议使用已更新显卡驱动的设备。

## 数据与隐私

- 导入的电子书和阅读数据保存在本机应用数据目录。
- WebDAV 密码和 AI API Key 保存在系统安全凭据存储中（Windows 凭据管理器、macOS 钥匙串），不会写入普通配置文件。
- AI 与翻译功能默认不会自动启用；只有在你配置服务并主动使用时，相关内容才会发送到所选服务商。
- WebDAV 同步由桌面端直接连接你选择的云盘，不经过 Torto 自建中转服务。

## 当前说明

Torto 仍在持续开发中。当前不支持带 DRM 的电子书，也不追求完整浏览器级 HTML/CSS 兼容；复杂固定版式、竖排、Ruby 注音以及部分书内交互内容仍可能无法完整显示。

如果遇到解析、排版或安装问题，欢迎在 [Issues](https://github.com/L-Chris/torto/issues) 中反馈，并附上电子书格式、问题截图和可复现步骤。请勿上传受版权保护的完整书籍。

<details>
<summary>开发者信息</summary>

### 本地开发

项目使用 Rust `1.97.1`，桌面端包名仍为 `rebook-desktop`，运行产物为 `torto.exe`。

```powershell
# Recommended once on Windows; new terminals will discover sccache automatically.
winget install --id Mozilla.sccache --exact

# Uses sccache when installed and falls back to ordinary Cargo otherwise.
.\scripts\cargo-sccache.cmd run --locked -p rebook-desktop
cargo run -p rebook-desktop
cargo run -p rebook-desktop -- "test-data\数学觉醒学会更清晰地思考.epub"

# Release-equivalent runtime profiling build with debug symbols.
cargo run --locked --profile perf -p rebook-desktop
```

修改 Rust 或 TOML 后自动重启：

```powershell
watchexec -r -e rs,toml -- cargo run --locked -p rebook-desktop
```

质量检查：

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
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
