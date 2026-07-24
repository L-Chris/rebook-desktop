# 原生 Reading IR 重构状态

- 日期：2026-07-24
- 主机：Windows x86_64，Rust 1.97.1，优化的增量 dev profile / 默认 test profile
- 结论：新的 parser -> renderer 主链已贯通，分页和缓存翻页已验证；当前仍是阅读内核，不是完整产品。

## 已交付

| 层 | 当前实现 |
| --- | --- |
| Publication | `BookSource`、`Book/Section/Block/Inline`、文字/块/图片样式子集、PublicationUrl、SourceRange |
| EPUB | 安全 ZIP/OCF/OPF、EPUB 3 Nav、EPUB 2 NCX、层级 TOC、懒章节/资源、独立 XHTML Reading IR parser、受控内外联 CSS cascade |
| Layout | 持久化 Parley context、中文塑形、长段落跨页、图片 CSS 尺寸/单次解码/缩放、单页/双页 spread、renderer-independent PageLayout |
| Renderer | retained PageDisplayList、glyph/font/image/rule 命令、Vello GPU/CPU、DPI 缩放 |
| Reader | 当前章节/页状态、TOC/href 跳转、五章节 LRU、持久 worker 前后各两章预取、缓存翻页、resize/样式 generation 失效与进度恢复 |
| Desktop | Xilem 0.4.0/Masonry 原生组件、Lucide 图标、Vello GPU 阅读页与 32 页 Scene LRU、44px 悬浮工具栏、默认双栏、设置弹窗、固定/浮动目录侧边栏、虚拟目录滚动、EPUB2/3 封面、方向键翻页、resize 重排、4px 阅读进度条、CPU 诊断模式 |

Blitz、Stylo、Taffy、DOM adapter 和旧 `Publication` trait 已从当前代码主链移除。

## 自动化验证

以下门禁均通过：

```powershell
cargo fmt --all --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

当前共有 26 项核心单元测试：Desktop 4、Publication 4、EPUB 9、Layout 3、Renderer 1、Reader 5。Desktop 测试覆盖层级目录展开顺序/深度、Canvas 逻辑尺寸转换、顶部悬停区和方向键翻页映射；EPUB 测试同时覆盖 EPUB 3 `cover-image` 与 EPUB 2 `meta name="cover"` 封面定位。其余测试覆盖 fragment 章节导航、双栏两列几何，以及图片百分比 width、max-width 和纵横比约束；Reader 仍以人工 300ms 慢章节确认预取投递不会阻塞调用线程。

## 真实 EPUB 诊断

测试书籍：`test-data/数学觉醒学会更清晰地思考.epub`，1200×800，Vello CPU 离屏，优化的 dev profile。最近一次单次运行：

| 指标 | 结果 |
| --- | ---: |
| parser | 8.80 ms |
| 当前章节 layout + display-list compile | 21.88 ms |
| prefetch dispatch（UI 线程） | 0.014 ms |
| prefetch complete（worker，两章前瞻） | 7.94 ms |
| cached same-section page turn | 0.0002 ms |
| cached section switch | 0.0095 ms |
| Vello Scene 编码 | 0.054 ms |
| CPU raster paint | 3.30 ms |
| total | 41.98 ms |

这些数值用于定位数量级，不是 release 性能承诺。章节工作发生在持久 worker，UI 线程只负责投递和安装结果；缓存翻页不包含 parser/layout/compile。桌面交互还会缓存最近 32 个 Vello Scene，工具栏显隐、菜单开关和已访问页面不会重新编码整页。CPU paint 是完整页面栅格化成本，GPU 窗口路径另行执行。

## 已知缺口与优先级

1. 当前打开的首章和未能提前命中的章节仍需等待整章分页；前后各两章已有后台预取，后续可增加增量分页，但保持相同 Reading IR 和页面缓存协议。
2. CSS cascade 只覆盖 tag/class/id/`tag.class`/selector group 和阅读所需属性；复杂 selector、`@import`、媒体查询与完整盒模型仍按真实书籍需求逐步进入 IR，不回退到完整浏览器 DOM。
3. TOC href fragment 当前解析到所属 spine section；章内精确定位需要 Reading IR 保留 authored element ID 并建立 anchor 到页面的索引。
4. 完善稳定 SourceAnchor 到 glyph/rect 的双向映射，为选择、书签、批注和搜索服务。
5. 增加固定字体截图、真实 Windows GPU 窗口冒烟、图片像素预算，以及 ruby/bidi/竖排/fixed-layout 能力矩阵。
