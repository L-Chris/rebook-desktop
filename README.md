# rebook-desktop

Rust 原生桌面电子书阅读器。正文不嵌入 WebView，也不追求完整浏览器兼容；当前主链是：

```text
EPUB archive source / Kindle / FB2 / CBZ / PDF direct sources
  -> shared HTML/CSS parser or fixed-page Section IR
  -> renderer-independent Reading IR
  -> Parley layout and native pagination
  -> retained page display list
  -> Vello GPU/CPU renderer
```

authored section 会先解析一次，再按与视口和阅读样式无关的文字/块预算切成稳定 content fragment；超长单段也会在 Unicode 标量边界切分并连续调整 `SourceRange`。每三个 content fragment 组成一个有界 layout segment，Paginator 在 segment 内连续运行，不会因内容分片提交半页；分页、display-list 缓存、目录定位和预取则以 `(section, segment)` 为单位。这样大型 KF8 section 不要求一次排完整章，随机目录跳转也最多排一个有界 segment。持久 worker 负责后台 segment 编译，交互线程只投递任务和安装结果；桌面端另保留最近 32 页的 Vello Scene。窗口尺寸或阅读样式变化只重排当前 segment，不重新解析 authored section。

## Workspace

- `crates/publication`：格式无关的 `BookSource`、Reading IR、资源 URL 与 SourceRange。
- `crates/formats`：EPUB、MOBI/AZW/AZW3、FB2/FBZ、CBZ 与 PDF 的统一注册入口；各格式直接提供 `BookSource`，其中 EPUB 模块负责受限 ZIP/OCF/OPF/Nav/NCX 与懒资源读取，PDF 通过纯 Rust 后端按需栅格化固定页面。
- `crates/html`：EPUB、Kindle 和 FB2 共享的 HTML/CSS → Reading IR 解析器。
- `crates/layout`：持久化 Parley 上下文、跨 content fragment 连续分页、文字塑形、受控图片尺寸和单页/双页 spread 原生分页。
- `crates/renderer`：把页面布局编译成 retained display list，并交给 Vello GPU/CPU 绘制。
- `crates/reader`：稳定 content fragment、有界 layout segment/checkpoint、命名位置类型、跨 segment 翻页、TOC/href 直达、布局失效、逐章节并发解析协调、后台相邻 segment 预取和 segment LRU 缓存。
- `apps/inspect`：受支持电子书的统一结构诊断 JSON。
- `apps/desktop`：Xilem 0.4.0/Masonry 原生书架与阅读窗口、Vello GPU 阅读页和可交互目录侧边栏。

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
# 启动本地书架，可多选导入受支持的电子书
cargo run -p rebook-desktop

# 直接打开真实测试书籍（不导入书架）
cargo run -p rebook-desktop -- "test-data\数学觉醒学会更清晰地思考.epub"

# Kindle、FB2/FBZ、CBZ 和 PDF 使用同一个阅读入口
cargo run -p rebook-desktop -- "..\rebook\data\1.azw3"

# 输出统一 publication 和逐章节解析诊断
cargo run -p rebook-inspect -- "..\rebook\data\1.azw3"

# 修改 Rust/TOML 后自动重启书架预览
watchexec -r -e rs,toml -- cargo run --locked -p rebook-desktop
```

默认首页参考 `rebook-web` 的书架布局，提供标题/作者搜索、封面卡片、本地状态、多选导入和移除。导入的原始电子书会保留其格式并复制到系统应用数据目录；内容哈希用于跳过重复导入，也是跨设备同步时唯一的书籍身份。点击封面或书名进入阅读器，阅读器菜单可回到书架。命令行直接传入书籍时仍保持原有的快速预览流程，不会自动写入书架。

阅读区顶部 44px 工具栏仅在鼠标进入顶部区域时显示；它复用页面背景，并占用同为 44px 的页面上边距，不覆盖正文也不叠加第二层顶部 padding。方向键 `←` / `→` 翻页，`Ctrl+F` 打开侧栏全文搜索；右上角 Lucide 菜单可打开设置弹窗，单栏/双栏在“排版”中配置，默认使用双栏。目录侧栏默认固定并占据布局宽度，左上角按钮收起/展开，右上角图钉可取消固定并切换为覆盖层；目录文字左对齐，使用无滚动条的虚拟列表，支持点击和滚轮导航。侧栏封面优先读取 EPUB 3 `cover-image`，并兼容 EPUB 2 `meta name="cover"`。双栏模式要求每栏至少 320px，窄窗口会自动退回单栏。底部 4px 轨道显示全书阅读进度；窗口 resize 和单双栏切换都会按当前章节的相对页进度恢复位置。

设置中的“AI”页维护 OpenAI 兼容 Provider、会话级 API Key 及其模型清单；“AI Chat”和“翻译”页分别选择所用的 Provider/模型，翻译页另行配置目标语言。旧版单 Provider 的 `plugins.json` 会在读取时迁移到该结构。

## WebDAV 同步

书架工具栏的云设置按钮可配置 WebDAV 地址、用户名、应用专用密码和设备名称。桌面端直接连接 WebDAV，不经过 `rebook-service`；密码由 Windows 凭据管理器保存，配置文件不含明文密码。启用后会立即同步，并在书架打开期间按配置周期自动同步，也可以通过云同步按钮手动触发。

远端协议位于 WebDAV 根目录的 `Rebook/v1/`。书籍资源按 SHA-256 内容寻址且写入后不可变；每台设备只写自己的书架清单与阅读状态文件，以避免多设备覆盖同一 JSON。可变清单使用 ETag 条件写入，下载资源会校验长度与 SHA-256。阅读位置使用 `LocatorV1` 的源锚点恢复，字体、窗口和分页变化后仍可定位；进度按 HLC 的最后一次阅读事件合并，不按最大百分比。高亮存储在本地 SQLite，旧 `highlights.json` 会幂等迁移；同步包含向量时钟、删除墓碑和并发冲突副本。当前协议面向桌面端之间同步，暂不承诺与 `rebook-web` 互操作。

桌面 chrome 复用 `rebook-web` 的浅色阅读设计 token：暖灰页面与工具栏、紧凑顶栏、Lucide 图标、柔和青绿色强调色和低对比度目录选中态。Xilem/Masonry 只负责窗口、组件布局、滚动与无障碍；正文仍由 retained `PageDisplayList` 直接桥接到 Vello GPU scene，不经过 WebView 或 CPU 位图回读。

## 构建 Windows 安装包

推送与根目录 `Cargo.toml` 中 workspace 版本一致的 tag（可带 `v` 前缀）后，GitHub Actions 会在 Windows runner 上运行测试、构建 MSI，并把安装包上传到对应的 GitHub Release。

```powershell
git tag v0.1.0
git push origin v0.1.0
```

## 当前能力边界

当前支持无 DRM 的 EPUB、MOBI、AZW、AZW3/KF8、FB2、FBZ（含 `.fb2.zip`）、CBZ 和 PDF。Kindle 路径支持 PalmDOC 与 HUFF/CDIC 文本、KF8 SKEL/FRAG 正文重建、NCX 目录和内嵌图片；FB2 支持元数据、层级正文、Base64 图片与封面；CBZ 支持自然排序图片和 `ComicInfo.xml`；PDF 支持文档元数据、书签目录、单/双页翻页和按需页面渲染。非 EPUB 格式直接构造格式无关的 publication、章节和资源模型；EPUB、Kindle 与 FB2 共享 HTML Reading IR 解析器，CBZ 与 PDF 使用固定页面模型。

EPUB 路径已实现 EPUB 3 常用容器、EPUB 2 NCX、层级目录、懒资源读取和归档/XML 安全预算；Reading IR 支持标题、段落、列表、引用、pre、图片、分隔线，以及受控的文字/块样式。EPUB parser 会级联 `<style>`、本地 `<link rel="stylesheet">` 和 inline style，支持 tag、class、id、`tag.class`、selector group，并把 `text-align`、`text-indent`、行高、边距、字号、字重、斜体、装饰、颜色，以及图片 `width/height/max-width/max-height` 归一化到 Reading IR；阅读器默认样式会把段落缩进覆盖为 0。图片尺寸支持 px/em/rem/pt 与百分比，最终按栏宽、页高约束并保持纵横比。布局支持中文字体回退、长段落跨页、单页/双页 spread。默认正文采用 rebook demo 的 16px、1.72 行高和 44px 页面边距。

当前不实现完整 DOM/CSS/Web 能力。复杂 selector、完整盒模型、fixed-layout、完整 SVG/MathML、ruby/竖排、无障碍树和书内字体混淆仍待后续实现。WebDAV 同步当前只覆盖桌面端，不包含 Web 端协议兼容、全局删除或服务端账号体系。Reading IR 会保留 XHTML 的 `id`/`name`，并把目录 fragment 映射到重排后的目标页；目标位于长段落内部时目前定位到该段落的第一页。JavaScript、表单、远程资源和 DRM 明确不属于当前阅读内核。

## 文档

- [当前原生渲染架构 ADR](docs/adr-0001-native-epub-renderer.md)
- [当前实现与性能状态](docs/phase-0-status.md)
- [早期技术调研（历史）](docs/technical-plan.md)
- [`rebook` / `rebook-web` 架构复盘（历史）](docs/rebook-reference-architecture.md)
