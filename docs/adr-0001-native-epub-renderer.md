# ADR-0001：Rust 原生 EPUB 渲染栈

- 状态：接受，Phase 0 有条件验证通过
- 日期：2026-07-23

## 背景

EPUB 3 的正文由 XHTML、CSS、SVG、字体、图片以及可选的 MathML、脚本和媒体资源组成。W3C 的 [EPUB Reading Systems 3.3](https://www.w3.org/TR/epub-rs-33/) 要求视觉阅读系统处理 XHTML、CSS、字体、SVG、固定版式和国际化排版；因此这不是一个“ZIP + 富文本控件”问题，而是受约束的文档浏览器问题。

产品同时要求：桌面原生、不嵌入 Web 容器、核心渲染逻辑由 Rust 实现。

## 决策

正式路线采用 EPUB 专用的 Rust 原生渲染管线：

1. OCF/OPF/导航和资源安全由 `rebook-epub` 自己控制。
2. XHTML/XML 解析复用 `xml5ever`，CSS 级联复用 `Stylo`。
3. 块级盒布局以 `Taffy` 为基础，富文本排版使用 `Parley`。
4. 绘制使用 `Vello`/`wgpu`，桌面事件循环使用 `winit`。
5. 以 [Blitz](https://github.com/DioxusLabs/blitz) 的模块化 crate 作为集成起点，但只通过项目内部适配接口调用，并固定到经过验证的精确版本。
6. EPUB 特有的资源 URL、用户样式、分页/碎片化、Locator、选择、高亮和阅读状态由本项目实现。

应用控件首期可使用 `egui`，但正文表面不使用 `egui` 的文本布局。

## 为什么不是其他路线

### 直接嵌入 Servo

[Servo](https://book.servo.org/embedding/overview.html) 已提供嵌入 API，网页兼容性最高，也不等同于系统 WebView。它适合作为兼容性对照和技术兜底，但包含完整浏览器能力、JavaScript 引擎和更多原生依赖，体积、构建和安全面明显超过无脚本 EPUB 阅读器的需要，也削弱了对分页、Locator 和书籍样式策略的控制。因此不作为正式内核。

### 完全从基础 crate 重写

从 `cssparser`、字体 shaping 和绘图原语开始重写，理论控制力最高，但 CSS 级联、Unicode 双向算法、字体回退、复杂文字 shaping、表格和碎片化会吞掉大量工期。项目的差异化应放在 EPUB Reading System 和阅读体验，不应重新实现已有的浏览器级基础算法。

### 把 Blitz 当稳定黑盒

Blitz 的 README 明确标注为 pre-alpha；其 [CSS 状态表](https://blitz.is/status/css) 目前仍缺 `writing-mode`、`unicode-bidi`、`hyphens` 等电子书关键能力。直接绑定其高层 API 会把上游变更和缺失能力泄漏到整个应用。因此只在内部适配层后使用，并保留维护小型 fork 或替换后端的能力。

## 后果

收益：没有 WebView，主体栈为 Rust；复用浏览器级 CSS 和文本组件；可以原生实现分页、命中测试和稳定阅读位置；长期可以把修复回馈上游。

代价：Blitz/Vello 仍快速演进；真正分页和竖排需要项目投入；“完整 EPUB 3”只能通过长期测试覆盖逐步达到，不能在 MVP 阶段宣称完成。

## Phase 0 的否决条件

出现下列任一情况且两周内无法在适配层或小型补丁中解决，就暂停正式路线并重新比较 Servo：

- 无法从内存资源提供器可靠加载相对 CSS、图片和 `@font-face`。
- 无法获得字符范围到几何矩形的映射，导致选择和 Locator 不可实现。
- 普通中文章节在目标机器上无法稳定字体回退或存在明显错字、漏字。
- 一章 10 万汉字的首次排版、内存或窗口缩放表现不可接受，且无法按章节/视口增量化。
- 上游 API 迫使 EPUB 层依赖其内部 DOM ID、窗口或网络实现，无法通过适配层隔离。

## Phase 0 结果

Publication CSS、资源沙箱、CJK 排版、文字命中和 Vello CPU 首屏已经贯通，Blitz 类型也被限制在 renderer/desktop 边界内，因此不触发立即否决。20k/100k 章节暴露出同步整章布局缺少协作取消，当前无头 ARM 环境无法完成 Vello/wgpu presentation 验收；路线继续，但这两项作为 alpha 前置门槛。具体数据见 [Phase 0 状态](phase-0-status.md)。
