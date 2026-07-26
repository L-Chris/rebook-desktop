# 原生 Reading IR 重构状态

- 日期：2026-07-25
- 主机：Windows x86_64，Rust 1.97.1，优化的增量 dev profile / 默认 test profile
- 结论：新的 parser -> renderer 主链已贯通，分页和缓存翻页已验证；当前仍是阅读内核，不是完整产品。

## 已交付

| 层 | 当前实现 |
| --- | --- |
| Publication | `BookSource`、`Book/Section/Block/Inline`、文字/块/图片样式子集、PublicationUrl、SourceRange |
| Formats | EPUB archive source，以及 Kindle、FB2、CBZ、PDF 直接 `BookSource`；不再构造内存 EPUB |
| HTML | EPUB、Kindle、FB2 共享的 HTML/CSS → Reading IR parser 与受控 CSS cascade |
| EPUB | 安全 ZIP/OCF/OPF、EPUB 3 Nav、EPUB 2 NCX、层级 TOC、懒章节/资源 |
| Layout | 持久化 Parley context、中文塑形、长段落跨页、多 content fragment 连续分页、图片 CSS 尺寸/单次解码/缩放、单页/双页 spread、renderer-independent PageLayout |
| Renderer | retained PageDisplayList、glyph/font/image/rule 命令、Vello GPU/CPU、DPI 缩放 |
| Reader | `ReaderSnapshot` / `ReaderPosition`、统一导航结果、稳定 content fragment、三个 fragment 一组的有界 layout segment/checkpoint、超长单段与列表 continuation、TOC/href segment 直达、segment LRU、受管 worker 相邻 segment 预取、逐章节解析协调、generation 失效与进度恢复 |
| Desktop | 直接持有 `ReaderSession` 并消费快照；Xilem 0.4.0/Masonry 原生组件、Vello GPU 阅读页与 32 页 Scene LRU、设置弹窗和目录侧边栏 |

Blitz、Stylo、Taffy、DOM adapter 和旧 `Publication` trait 已从当前代码主链移除。

## 自动化验证

以下门禁均通过：

```powershell
cargo fmt --all --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

测试覆盖 Desktop、Publication、HTML、EPUB、Formats、Layout、Renderer 与 Reader。除格式解析外，Reader 还覆盖超长单段的 Unicode/SourceRange 切分、列表 marker continuation、content fragment 边界不提交半页、跨 segment 前后翻页、目录与总进度跨 segment 更新、远距离锚点只编译目标 segment、旧 generation 预取结果隔离、worker 回收，以及人工 300ms 慢章节解析期间的非阻塞快照更新。

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

这些数值是分片重构前的 EPUB 基线，只用于定位数量级，不是 release 性能承诺。当前实现把布局和 display-list compile 限制在单个有界 layout segment；缓存翻页不包含 parser/layout/compile。桌面交互还会缓存最近 32 个 Vello Scene，工具栏显隐、菜单开关和已访问页面不会重新编码整页。CPU paint 是完整页面栅格化成本，GPU 窗口路径另行执行。

## 已知缺口与优先级

1. 大型 authored section 已拆成稳定 content fragment 与有界 layout segment；Paginator 在 segment 内连续分页，只有每三个 fragment 的 checkpoint 会提交当前页。后续可让 Paginator 导出/恢复跨 checkpoint continuation state，进一步减少极长章节中的少量 checkpoint 留白，同时保持随机直达成本有界。
2. CSS cascade 只覆盖 tag/class/id/`tag.class`/selector group 和阅读所需属性；复杂 selector、`@import`、媒体查询与完整盒模型仍按真实书籍需求逐步进入 IR，不回退到完整浏览器 DOM。
3. TOC href fragment 已通过 Reading IR authored element ID 映射到重排后的页面；长段落内部的字符级 fragment 目前仍定位到该段落第一页。
4. 完善稳定 SourceAnchor 到 glyph/rect 的双向映射，为选择、书签、批注和搜索服务。
5. 已完成 `rebook/data/1.azw3` 的真实 Windows GPU 窗口、封面和深层目录直达冒烟；仍需增加固定字体截图、图片像素预算，以及 ruby/bidi/竖排/fixed-layout 能力矩阵。
