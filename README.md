# rebook-desktop

Rust 原生桌面电子书阅读器实验工程。正文不嵌入 WebView，也不使用 Electron、Wry、CEF；EPUB 解析、CSS 级联、文字排版、命中测试和绘制都在 Rust 进程内完成。

Phase 0 纵向样板已经可构建、可测试：`EPUB -> Publication -> XHTML/CSS -> Stylo/Taffy/Parley -> Vello -> 原生窗口或离屏 RGBA`。当前是内核样板，不是可日常使用的阅读器；多章节滚动、稳定 Locator 映射、分页、书架和持久化仍未实现。实测与验收缺口见 [Phase 0 状态](docs/phase-0-status.md)。

## Workspace

- `crates/publication`：格式无关的 Publication、SpineItem、资源 URL、Locator 和 SourceRange。
- `crates/reader`：串行 ReaderController、generation、任务取消和过期结果隔离。
- `crates/epub`：受限 ZIP、OCF、OPF、EPUB 3 Nav、EPUB 2 NCX、懒资源读取。
- `crates/renderer`：隔离 Blitz 内部类型的 XHTML/CSS 布局、资源沙箱、文字命中和 Vello CPU 回归绘制。
- `apps/inspect`：EPUB 结构与诊断 JSON。
- `apps/desktop`：Blitz shell + Vello/wgpu 的原生窗口，以及无显示服务器的诊断模式。

## Rust 环境

项目固定 Rust `1.97.1`，MSRV 为 `1.97`。当前机器使用 `rustup` 管理工具链，Cargo crates.io 已替换为 RsProxy Sparse 国内镜像；配置位于 `~/.cargo/config.toml`。在本机验证：

```bash
rustup show active-toolchain
rustc -V
cargo -V
```

进入此目录后，`rust-toolchain.toml` 会自动选择项目工具链并安装 `rustfmt`、`clippy`。

## 构建与运行

```bash
# 全工作区测试
cargo test --workspace --all-targets

# 生成自研的规范 EPUB 3 样本并检查结构
cargo run -p rebook-inspect --example build_fixture
cargo run -p rebook-inspect -- target/fixtures/minimal.epub

# 启动 Vello/wgpu 原生窗口（需要桌面会话和兼容 GPU）
cargo run -p rebook-desktop -- target/fixtures/minimal.epub

# 无窗口完成排版和 Vello CPU 首屏绘制，输出 JSON 性能/资源诊断
cargo run -p rebook-desktop -- --diagnose target/fixtures/minimal.epub
```

可生成约 2 万或任意规模的中文长章节：

```bash
cargo run -p rebook-inspect --example build_benchmark
cargo run -p rebook-inspect --example build_benchmark -- \
  target/fixtures/benchmark-100k.epub 100000
cargo run -p rebook-desktop -- --diagnose target/fixtures/benchmark-20k.epub
```

## 当前支持边界

已实现 EPUB 3 常用容器、OPF/manifest/spine、Nav，兼容 EPUB 2 NCX；所有归档资源按需读取，并限制路径、entry 数量、单项/总解压大小、压缩比和 XML 深度，禁止加密 entry、符号链接、DOCTYPE 与网络子资源。渲染器支持外链 publication CSS、中文复杂脚本分词/塑形、首屏绘制和 point-to-text 命中。

尚未实现书内字体混淆、图片像素预算、规范 XML DOM 到稳定 SourceAnchor 的双向映射、多 spine 连续滚动、增量布局、分页、fixed-layout、完整 SVG/MathML、竖排、批注和无障碍树。JavaScript、表单、远程资源和 DRM 是明确禁用或非 MVP 能力。

## 设计文档

- [完整技术方案](docs/technical-plan.md)
- [`rebook` / `rebook-web` 架构复盘与迁移边界](docs/rebook-reference-architecture.md)
- [原生 EPUB 渲染栈 ADR](docs/adr-0001-native-epub-renderer.md)
- [Phase 0 实现、实测和下一阶段门槛](docs/phase-0-status.md)
