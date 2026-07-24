# ADR-0001：Reading IR 驱动的 Rust 原生 EPUB 渲染器

- 状态：接受，替代早期 Blitz/DOM 方案
- 日期：2026-07-23

## 背景

产品需要原生分页、稳定的阅读位置和低延迟翻页，但不需要复刻浏览器或完全兼容 Web。早期样板复用了 Blitz、Stylo 和 Taffy，能够显示 XHTML，却把 DOM/CSS 浏览器模型、同步整章布局和上游内部类型带入了核心路径；分页难以成为一等能力，缓存边界也不清晰。

`rebook` 已验证更适合本产品的领域边界：parser 先把书籍归一化成中间表示，renderer 只消费稳定的阅读语义。这里接受破坏性重构，不兼容旧 renderer API。

## 决策

采用以下单向管线：

```text
EPUB container -> Reading IR -> PageLayout -> PageDisplayList -> Vello
```

1. `publication` 定义格式和 renderer 无关的 `BookSource` 与 Reading IR：`Book/Section/Block/Inline`。
2. `epub` 负责安全容器、metadata/spine/resource，并按章节懒解析 XHTML；容器解析与 Reading IR 解析分模块。
3. `layout` 持有 Parley 字体与布局上下文，直接从 Reading IR 产生分页后的 `PageLayout`。长段落必须可跨页，图片只解码一次；图片作者尺寸和最大尺寸在列宽/页高内解析。单页和双页 spread 使用同一分页器，双页只改变每个 surface 的列几何。
4. `renderer` 把页面编译为 retained `PageDisplayList`，缓存 glyph/font/image/rule 命令；GPU 和 CPU 后端共享同一绘制协议。
5. `reader` 是唯一的会话编排者。当前章节页面常驻，当前章前后各两章使用五项 LRU；持久 prefetch worker 拥有独立的 `LayoutEngine`/字体缓存，在 UI 线程外解析、分页并编译章节窗口。Reader 解析 TOC href 到 spine section，普通翻页不得触发解析、排版或 display-list 编译。
6. `desktop` 使用 crates.io 发布版 Xilem 0.4.0/Masonry 管理窗口生命周期、输入、DPI/resize、目录侧边栏、滚动和无障碍，不持有 EPUB/DOM 逻辑。阅读页继续消费 retained `PageDisplayList`；窄桥接层把 AnyRender/Vello 0.9 命令转换为 Xilem 所用的 Vello 0.6 scene，并共享字体/图片 blob，不进行 CPU 位图回读。桌面层可缓存最近使用的 Vello Scene，但该后端缓存不能反向进入 renderer-independent Reading IR、PageLayout 或 PageDisplayList。

核心 crate 的依赖方向为：

```text
desktop -> reader -> layout -> publication <- epub
                 \-> renderer -> layout
```

## 明确不采用

- 不把 Blitz、DOM、Stylo 或 Taffy 作为正文主链。
- 不为旧的 Blitz renderer API 保留适配层。
- 不以完整 HTML/CSS 兼容作为 MVP 验收标准。
- 不在 display list 或桌面层反向读取 EPUB ZIP。

## 后果

收益是分页和缓存成为显式模型，同章翻页为 O(1) 索引切换；解析、排版、绘制可以独立测试或替换；EPUB 层不会依赖 GPU/window 类型。

代价是项目需要自行定义并逐步扩展 Reading IR/CSS 子集。复杂 table/float/ruby/竖排和 fixed-layout 不能从浏览器引擎自动获得，必须按真实书籍需求进入 IR 与 layout。当前打开的首章仍同步整章排版；相邻章节已后台整章预取，超长章节后续仍可演进为增量分页。

## 架构不变量

- `BookSource::parse_section` 保持章节懒加载。
- 缓存页面翻页不再次调用 parser。
- 章节窗口预取只能向 worker 投递任务，不能在 winit/UI 线程执行 parser、layout 或 display-list 编译。
- resize/样式失效通过 generation 隔离旧预取结果，过期结果不得回填当前缓存。
- resize/样式变化会失效布局，但保留近似阅读进度。
- 单页/双页切换属于阅读样式失效；双页模式在最小列宽不足时自动使用单页几何。
- 目录组件只消费 `Book.table_of_contents` 和 Reader 跳转 API，不直接读取 EPUB Nav/NCX。
- `LayoutViewport` 和 `PageLayout` 只使用逻辑像素；设备 DPI 只在最终 `paint_scaled` 中应用一次，不能改变分页结果。
- `PageLayout` 不包含 Vello 类型，`PageDisplayList` 不包含 EPUB 类型。
- 新能力优先扩展 Reading IR 和对应测试，不绕过中间层直接把 XHTML 交给 renderer。
- 桌面 UI 视觉 token 可以参考 `rebook-web`，但组件实现和正文渲染保持原生；产品 chrome 的变化不得侵入 parser、Reading IR、layout 或 renderer。
