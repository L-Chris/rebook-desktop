# Phase 0 纵向样板状态

- 日期：2026-07-23
- 结论：技术路线可继续，但属于“有条件 Go”；增量首屏和目标平台 GPU 验收是进入可用 alpha 前的硬门槛。
- 主机：`aarch64-unknown-linux-gnu`，Rust 1.97.1，dev profile（未优化、关闭 debug info）。

## 已交付

| 层 | 当前实现 |
| --- | --- |
| 工程 | Cargo workspace、精确工具链、Cargo.lock、rustfmt/clippy、RsProxy 国内源 |
| Publication | 稳定 ID、metadata、manifest、spine、pull-based Resource、PublicationUrl、LocatorV1、SourceRange |
| Reader | 串行命令状态机、generation、CancellationToken、任务替换、过期 completion 丢弃 |
| EPUB | 内存/文件打开、懒解压、OCF/container、OPF、EPUB 3 Nav、EPUB 2 NCX、结构诊断 |
| 安全 | zip-slip/绝对路径/重复规范路径、symlink、encrypted entry、大小/数量/压缩比/XML 深度预算、DOCTYPE 禁止、默认无网络 |
| 排版 | XHTML、publication CSS、Stylo 级联、Taffy 布局、Parley CJK 字典分词/塑形、中文 point-to-text 命中 |
| 绘制 | Vello CPU 离屏首屏 RGBA 回归；Vello/wgpu + winit 原生窗口已接线并通过构建 |
| 诊断 | EPUB JSON inspector、规范最小样本、可参数化长章节生成器、布局/绘制/资源失败/RSS JSON |

Blitz 只出现在 `crates/renderer` 内部和桌面壳边界；Publication、Reader 与 EPUB 公共接口没有泄漏 Blitz/Stylo/Taffy/Parley 类型。

## 自动化验证

`cargo test --workspace --all-targets` 覆盖：

- Publication URL 规范化、根目录逃逸、外部 URL 与 Locator 验证；
- Reader 任务取消、generation 替换、初次布局调度、跨出版物 Locator 拒绝；
- EPUB 3 Nav、EPUB 2 NCX、资源懒读取、zip-slip、解压预算、mimetype strict mode、DOCTYPE；
- publication CSS 加载、HTTP 资源拦截、中文文字命中、非空 Vello CPU 首屏绘制。

样本由 `build_fixture` 写入，确保 `mimetype` 是第一个 Stored entry，且内容严格为 `application/epub+zip`。检查器解析样本时 diagnostics 为空。

## 本机基准

以下是 2026-07-23 在当前 ARM 主机、dev profile 的单次热构建运行结果，只用于识别数量级，不是发布承诺：

| 样本 | 文本量 | 打开 | 整章布局 | 900×700 Vello CPU 首屏绘制 | 总计 | 峰值 RSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| benchmark-20k | 20,002 字符 | 1.5 ms | 459 ms | 496 ms | 957 ms | 42,496 KiB |
| benchmark-100k | 100,010 字符 | 1.9 ms | 2.14 s | 519 ms | 2.66 s | 52,224 KiB |

20k 整章布局低于 1 秒目标，100k 内存有界；但当前 `resolve` 仍同步布局整章，没有独立的“首个可见 fragment”时间，因此不能宣称达到首屏 300 ms。下一步要做视口优先布局、后台续排和布局任务中的协作式取消。

复现：

```bash
cargo run -p rebook-inspect --example build_benchmark
cargo run -p rebook-inspect --example build_benchmark -- \
  target/fixtures/benchmark-100k.epub 100000
cargo run -p rebook-desktop -- --diagnose target/fixtures/benchmark-20k.epub
cargo run -p rebook-desktop -- --diagnose target/fixtures/benchmark-100k.epub
```

## 未通过或未完成的门槛

| 门槛 | 状态 | 说明 |
| --- | --- | --- |
| 相对 CSS 只经受限 resolver | 通过 | 自动化测试覆盖，HTTP 被拦截并留诊断 |
| 中文分词、字体 fallback、命中 | 部分通过 | CJK 字典分词和命中通过；还需固定字体的跨平台字形截图矩阵 |
| 首屏 <300 ms、后台整章 <1 s | 部分通过 | 20k 整章 459 ms；未实现增量首屏 |
| 100k 可取消且 UI 不冻结 | 未通过 | ReaderController 有取消语义，Blitz 同步 resolve 内部尚无协作取消点 |
| point -> range -> rect 稳定往返 | 部分通过 | point -> 后端 UTF-8 offset 已有；Canonical DOM/SourceAnchor/rect 往返未完成 |
| Vello/wgpu 目标平台首帧 | 环境阻塞 | 当前 Xvfb 缺 DRI3，ARM GPU 也不满足 Vello 离屏 wgpu 设备要求；CPU 首屏通过，需在真实 Windows/macOS/Linux GPU 主机验证 |

## 下一阶段优先级

1. 将章节解析/布局移到可取消 worker，先提交视口附近 fragment，再续排整章。
2. 建立 CanonicalDocument 与 backend node/UTF-8 offset 的 SourceAnchor 映射，补齐 point/range/rect 往返。
3. 在真实三平台 GPU runner 上增加窗口首帧冒烟和软件 fallback；不得以本机 Xvfb 黑屏视为 GPU 通过。
4. 加图片解码预算、书内字体与 IDPF/Adobe 字体混淆测试，再实现多 spine 连续滚动。
5. 引入固定许可字体和像素阈值截图，开始 W3C EPUB Tests 能力矩阵。
