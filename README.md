# rebook-desktop

Rust 原生桌面电子书阅读器。正文不嵌入 WebView，也不追求完整浏览器兼容；当前主链是：

```text
EPUB container/parser
  -> renderer-independent Reading IR
  -> Parley layout and native pagination
  -> retained page display list
  -> Vello GPU/CPU renderer
```

同章翻页只切换已编译页面的缓存索引，跨章通过前后各两章的五章节 LRU 窗口后台预取。解析、分页和 display-list 编译在持久 worker 上完成，交互线程只投递任务和安装已完成结果；桌面端再保留最近 32 页的 Vello Scene，纯 UI 状态变化和已访问页面翻页不会重复回放 display list。窗口尺寸或阅读样式变化时才重新分页并失效 Scene 缓存。

## Workspace

- `crates/publication`：格式无关的 `BookSource`、Reading IR、资源 URL 与 SourceRange。
- `crates/epub`：受限 ZIP/OCF/OPF/Nav/NCX，以及 XHTML 到 Reading IR 的懒章节解析。
- `crates/layout`：持久化 Parley 上下文、文字塑形、受控图片尺寸和单页/双页 spread 原生分页。
- `crates/renderer`：把页面布局编译成 retained display list，并交给 Vello GPU/CPU 绘制。
- `crates/reader`：阅读位置、翻页、TOC/href 跳转、布局失效、后台两章前瞻/回看预取和五章节 LRU 缓存。
- `apps/inspect`：EPUB 结构诊断 JSON。
- `apps/desktop`：Xilem 0.4.0/Masonry 原生窗口与组件、Vello GPU 阅读页、可交互目录侧边栏，以及无窗口 CPU 诊断模式。

## 环境与质量门禁

项目固定 Rust `1.97.1`，进入目录后 `rust-toolchain.toml` 会让 rustup 自动选择工具链。本机 Cargo 使用 RsProxy sparse 国内镜像。开发 profile 对工作区使用 `opt-level = 1`、对第三方依赖使用 `opt-level = 3` 并启用增量编译，避免 Parley/Vello 在 O0 预览中造成不必要的卡顿。

```powershell
cargo fmt --all --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## 运行

```powershell
# 启动真实测试书籍
cargo run -p rebook-desktop -- "test-data\数学觉醒学会更清晰地思考.epub"

# 输出 parser/layout/cache/paint 性能诊断
cargo run -p rebook-desktop -- --diagnose "test-data\数学觉醒学会更清晰地思考.epub"

# 修改 Rust/TOML/EPUB 后自动重启预览
watchexec -r -e rs,toml,epub -- cargo run --locked -p rebook-desktop -- "test-data\数学觉醒学会更清晰地思考.epub"
```

阅读区顶部 44px 工具栏仅在鼠标进入顶部区域时显示；它复用页面背景，并占用同为 44px 的页面上边距，不覆盖正文也不叠加第二层顶部 padding。方向键 `←` / `→` 翻页；右上角 Lucide 菜单可打开设置弹窗，单栏/双栏在“排版”中配置，默认使用双栏。目录侧栏默认固定并占据布局宽度，左上角按钮收起/展开，右上角图钉可取消固定并切换为覆盖层；目录文字左对齐，使用无滚动条的虚拟列表，支持点击和滚轮导航。侧栏封面优先读取 EPUB 3 `cover-image`，并兼容 EPUB 2 `meta name="cover"`。双栏模式要求每栏至少 320px，窄窗口会自动退回单栏。底部 4px 轨道显示全书阅读进度；窗口 resize 和单双栏切换都会按当前章节的相对页进度恢复位置。

桌面 chrome 复用 `rebook-web` 的浅色阅读设计 token：暖灰页面与工具栏、紧凑顶栏、Lucide 图标、柔和青绿色强调色和低对比度目录选中态。Xilem/Masonry 只负责窗口、组件布局、滚动与无障碍；正文仍由 retained `PageDisplayList` 直接桥接到 Vello GPU scene，不经过 WebView 或 CPU 位图回读。

## 当前能力边界

已实现 EPUB 3 常用容器、EPUB 2 NCX、层级目录、懒资源读取和归档/XML 安全预算；Reading IR 支持标题、段落、列表、引用、pre、图片、分隔线，以及受控的文字/块样式。EPUB parser 会级联 `<style>`、本地 `<link rel="stylesheet">` 和 inline style，支持 tag、class、id、`tag.class`、selector group，并把 `text-align`、`text-indent`、行高、边距、字号、字重、斜体、装饰、颜色，以及图片 `width/height/max-width/max-height` 归一化到 Reading IR；阅读器默认样式会把段落缩进覆盖为 0。图片尺寸支持 px/em/rem/pt 与百分比，最终按栏宽、页高约束并保持纵横比。布局支持中文字体回退、长段落跨页、单页/双页 spread。默认正文采用 rebook demo 的 16px、1.72 行高和 44px 页面边距。

当前不实现完整 DOM/CSS/Web 能力。复杂 selector、完整盒模型、fixed-layout、完整 SVG/MathML、ruby/竖排、选择批注、无障碍树、书内字体混淆和持久化书架仍待后续实现。TOC fragment 当前定位到所属章节开头，待 Reading IR 保留 authored element ID 后再支持章内精确锚点。JavaScript、表单、远程资源和 DRM 明确不属于当前阅读内核。

## 文档

- [当前原生渲染架构 ADR](docs/adr-0001-native-epub-renderer.md)
- [当前实现与性能状态](docs/phase-0-status.md)
- [早期技术调研（历史）](docs/technical-plan.md)
- [`rebook` / `rebook-web` 架构复盘（历史）](docs/rebook-reference-architecture.md)
