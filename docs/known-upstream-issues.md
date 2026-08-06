# 核心依赖已知问题

- 最近更新：2026-08-05
- 记录范围：已经在 Torto 中复现、确认存在上游问题，并需要本地兼容代码的问题

依赖升级时应逐项检查本文。只有在上游修复已经进入当前版本，并且移除本地兼容代码后相关回归测试仍能通过，才删除对应兼容代码和本文条目。

## Parley：两端对齐文本的选区宽度不足

- 影响版本：`parley 0.10.0`
- 上游状态：截至 2026-08-01 仍为 Open
- 上游问题：[linebender/parley#396](https://github.com/linebender/parley/issues/396)
- 本地位置：`crates/renderer/src/lib.rs` 中的 `ShapedTextRegion::selection_rects`
- 回归测试：`selection_covers_the_visual_width_of_justified_middle_lines`

### 表现

跨多行选择两端对齐的正文时，首行和末行通常正常，中间整行的高亮矩形会短于实际文字，导致行尾文字没有被高亮。

### 原因

Parley 会把两端对齐产生的额外空白宽度加入字簇 advance，但 `LineMetrics::advance` 仍保留调整前的宽度。`Selection::geometry_with` 对选区中间行直接使用该值，因此返回了过短的矩形。

### 当前规避方案

对于被完整选择的行，渲染器根据调整后的字簇和行内盒重新计算实际视觉宽度，并扩展 Parley 返回的选区矩形。首尾部分选择和非两端对齐文本仍沿用 Parley 原始几何。

### 升级检查

1. 确认上游问题已关闭，并找到修复进入的 Parley 版本。
2. 升级依赖后临时移除 `selection_rects` 中带上游链接的兼容逻辑。
3. 运行 `cargo test --locked -p rebook-renderer selection_covers_the_visual_width_of_justified_middle_lines`。
4. 使用包含长英文两端对齐段落的真实 EPUB 检查跨行选择。
5. 全部通过后删除兼容逻辑，并删除本条记录。

## egui/epaint：全局羽化导致紧凑圆角控件出现角线

- 影响版本：`egui 0.36.0`
- 上游状态：截至 2026-08-05，相关问题仍为 Open；上游尚无与 Torto 紧凑图标按钮完全相同的最小复现
- 相关上游问题：[emilk/egui#2735](https://github.com/emilk/egui/issues/2735)、[emilk/egui#7424](https://github.com/emilk/egui/issues/7424)
- 本地位置：`apps/desktop/src/ui/mod.rs` 中的 `configure_tessellation`、`painted_icon_button` 和 `paint_compact_rounded_background`
- 回归测试：`rounded_controls_keep_pixel_snapping_and_antialiasing`、`compact_rounding_contains_the_feathering_fringe`

### 表现

全局启用羽化后，小尺寸圆角图标按钮在 hover 或选中状态下可能在角落留下短斜线或残余边角。直接关闭羽化虽然能消除角线，却会让选择框、单选按钮和小圆角控件重新出现明显锯齿，尤其是在 Windows 100% DPI 下。

### 原因

epaint 的羽化由全局 tessellation 选项控制，当前不能针对单个 shape 选择是否使用。圆角路径的羽化带会向路径内外各扩展半个羽化宽度；紧凑控件的路径刚好落在分配矩形边界时，外侧碎片可能与裁剪、相邻背景或像素取整共同形成可见角线。相关上游 issue 还记录了羽化 tessellator 在其他几何形状上产生线状伪影的问题，但 Torto 的具体圆角场景尚未有一一对应的上游 issue。

### 当前规避方案

保留一像素全局羽化和矩形像素对齐，避免整个界面的圆角退化。紧凑图标按钮不再使用 egui 原生按钮的多层 frame/stroke，而是绘制单层背景；先把外边界对齐到物理像素，再将实际圆角路径向内缩半个羽化宽度，并对这个已经对齐的 shape 关闭二次 `round_to_pixels`。

### 升级检查

1. 检查上游是否提供按 shape 控制羽化的 API，或是否修复圆角/多边形羽化伪影。
2. 升级 egui 后，尝试移除 `paint_compact_rounded_background`，恢复普通圆角背景或原生按钮 frame。
3. 运行两个圆角回归测试。
4. 在 Windows 100%、125%、150%、175% 和 200% DPI 下检查 hover、选中和透明状态，确认既无角线也无圆角锯齿。
5. 全部通过后删除局部几何兼容代码，并删除本条记录。

## egui_commonmark/egui：跨组件多行选区覆盖行首内容

- 影响版本：`egui_commonmark 0.24.0`、`egui 0.36.0`
- 上游状态：截至 2026-08-05 仍为 Open；`egui_commonmark 0.24.0` 仍以 `egui 0.35` 发布，Torto 暂时在本地适配到 `egui 0.36`
- 上游问题：[lampsitter/egui_commonmark#80](https://github.com/lampsitter/egui_commonmark/issues/80)；布局限制另见 [emilk/egui#4378](https://github.com/emilk/egui/issues/4378)
- 本地位置：`third_party/egui_commonmark_backend/src/elements.rs` 中的 `newline`，以及 `apps/desktop/src/reader/chat_markdown.rs` 中的 `show_markdown_table`
- 回归测试：`markdown_table_row_height_follows_the_tallest_wrapped_cell`；列表选区需人工检查

### 表现

跨多个 Markdown 组件选择 AI 回复时，纯布局换行也会被当成可选文字，并在下一行开头绘制一个选区矩形。列表编号和项目符号是独立绘制的图形，该矩形可能覆盖它们。上游报告还记录了多行选择吞掉每行首字符的问题，表格内更明显。表格使用 `egui::Grid` 时，较短单元格的背景和边框也不会自动撑到同一行中最高单元格的高度。

### 原因

egui 的多组件文字选择按各个 `Label` 独立生成选区网格，无法识别 egui_commonmark 用来驱动布局的空换行不是文档内容。表格方面，立即模式布局在绘制单元格时尚不知道该行最终最大高度；`Grid` 之后虽然会统一行布局高度，已经绘制的 `Frame` 不会回填。

### 当前规避方案

将 egui_commonmark 的纯布局换行标记为不可选择，真实文本仍保留跨组件选择和复制。AI 表格不再依赖 `Grid` 回填单元格：先按列宽测量每个单元格的换行高度，取整行最大值，再用相同高度的显式矩形绘制该行所有背景和边框。

### 升级检查

1. 检查上游 #80 是否已修复，并确认修复所需的 egui/egui_commonmark 版本。
2. 升级后临时恢复可选择的布局换行，跨多段、多级有序/无序列表拖动选择，确认行首不再被覆盖。
3. 尝试将 AI 表格恢复为上游表格实现，检查长短文本混排、引用链接和窄侧栏换行。
4. 运行 `cargo test -p rebook-desktop markdown_table_row_height_follows_the_tallest_wrapped_cell`。
5. 全部通过后删除本地兼容代码，并删除本条记录。

## egui：滚动时离屏端点导致跨组件文字选区被清空

- 影响版本：`egui 0.36.0`
- 上游状态：截至 2026-08-05，上游当前源码仍包含该清理逻辑，尚未找到专门跟踪此行为的 issue
- 上游代码：[`LabelSelectionState::on_end_pass`](https://github.com/emilk/egui/blob/0.36.0/crates/egui/src/text_selection/label_text_selection.rs)
- 本地位置：`third_party/egui/src/widgets/label.rs` 中的 `Label::ui`

### 表现

在 AI Chat 的长回复中从视口边缘继续拖动选区时，内容区虽然会自动滚动，但只要选区的起点或终点滚出可见区域，整个选区就会被取消，无法继续向上或向下扩展。

### 原因

`Label::ui` 默认只会把可见的标签提交给跨组件选区状态。滚动后，离屏端点所在的标签仍参与 `ScrollArea` 布局，却不会更新选区状态；`LabelSelectionState::on_end_pass` 在一帧内没有同时遇到两个端点时会主动清空选区，以规避虚拟化列表中的位置错乱。

### 当前规避方案

本地接管 `egui 0.36.0`：只要仍存在跨标签选区，`Label::ui` 就继续将裁剪区外的可选择标签提交给选区状态。标签和高亮仍受原有 painter 裁剪，不会绘制到滚动视口之外；已完成的选区在松开鼠标后也能保留并复制。

### 升级检查

1. 检查上游 `Label::ui` 与 `LabelSelectionState::on_end_pass` 是否已经支持离屏端点，或是否新增对应 issue。
2. 升级 egui 后临时移除 `third_party/egui` 与 `[patch.crates-io]` 覆盖。
3. 在长 AI 回复中从开头拖到视口顶部或底部，确认内容持续滚动、选区持续扩展。
4. 松开鼠标后反向滚动，确认离屏选区仍保留，并验证 `Ctrl+C` 能复制完整内容。
5. 全部通过后删除本地 egui 副本与本条记录。

## egui：`ScrollArea::show_rows` 在列表底部抖动

- 影响版本：`egui 0.36.0`
- 上游状态：截至 2026-08-05 仍为 Open
- 上游问题：[emilk/egui#1787](https://github.com/emilk/egui/issues/1787)；程序化定位限制另见 [emilk/egui#3268](https://github.com/emilk/egui/issues/3268)
- 本地位置：`apps/desktop/src/reader/egui_view.rs` 中的 `stable_virtual_row_range` 和 `DesktopReader::toc`
- 回归测试：`virtual_toc_range_does_not_backfill_rows_at_the_bottom_boundary`

### 表现

目录滚动到底部后点击目录项，列表可能在相邻帧间上下抖动一次。长目录更容易观察到，但问题与目录层级和条目数量本身无关。

### 原因

`ScrollArea::show_rows` 会根据视口计算首尾可见行；当末行超过总行数时，它会把尾行截断，并向前移动首行以维持原范围长度。视口底边在行边界附近发生微小变化时，首行会在两个值之间来回切换，进而改变子 UI 的布局范围并产生抖动。这与上游 #1787 的复现一致。

### 当前规避方案

目录保留 `ScrollArea::show_viewport`，但使用本地固定行高虚拟化：尾行只截断到总行数，不再向前补行，因此相同首行在底部边界两侧保持稳定。只有可见行会被创建和绘制。活动目录项不在视口内时，使用合成的目标矩形调用 `scroll_to_rect`，继续沿用 egui 的滚动定位与动画。

### 升级检查

1. 确认 #1787 已关闭，并找到修复进入的 egui 版本。
2. 升级依赖后尝试把目录恢复为原生 `show_rows`，保留正常的活动项定位。
3. 运行 `cargo test -p rebook-desktop virtual_toc_range_does_not_backfill_rows_at_the_bottom_boundary`。
4. 使用两千项以上的真实目录，在底部连续点击当前项、相邻项和远端项，确认底部不抖动且远端定位动画正常。
5. 全部通过后删除本地虚拟化兼容代码，并删除本条记录。

## egui：合法布局触发控件 ID/矩形变化误报

- 影响版本：`egui 0.36.0`
- 上游状态：截至 2026-08-05 两项问题仍为 Open
- 上游问题：[emilk/egui#8343](https://github.com/emilk/egui/issues/8343)、[emilk/egui#8092](https://github.com/emilk/egui/issues/8092)
- 本地位置：`apps/desktop/src/ui/mod.rs` 中的 `configure`

### 表现

在 debug 构建中，右到左子布局、虚拟化列表或动画区域可能被 `warn_if_rect_changes_id` 判断为 ID 不稳定，界面会出现明亮的红色边框并输出警告。实际控件状态和交互并没有发生串用。

### 原因

egui 的调试检查只看到相同屏幕矩形在不同 pass 或帧中对应了不同 ID，无法区分真正的 ID 不稳定和虚拟化、反向布局导致的合法矩形复用。

### 当前规避方案

仅在 debug 构建中关闭 `style.debug.warn_if_rect_changes_id`。这会同时关闭该项 ID 稳定性诊断，因此新增复杂动态布局时需要通过稳定 ID、交互状态和滚动行为测试补足检查。

### 升级检查

1. 确认两个上游问题的修复状态以及修复进入的 egui 版本。
2. 升级依赖后重新启用 `warn_if_rect_changes_id`。
3. 检查右侧工具栏、虚拟化列表、侧栏动画和滚动区域是否仍出现红框或误报警告。
4. 运行桌面端测试，并手动验证控件状态不会在相邻行或相邻帧之间串用。
5. 确认无误报后删除关闭诊断的兼容设置，并删除本条记录。

## egui：显式高度 `TextEdit` 的垂直对齐与感知区域问题

- 影响版本：`egui 0.36.0`
- 上游状态：默认顶部对齐仍属于当前 API 行为；相关感知区域 bug 已由上游修复，0.36 另行修复了 hint 文本未遵循水平/垂直对齐的问题
- 上游问题：[emilk/egui#7433](https://github.com/emilk/egui/issues/7433)、修复 [emilk/egui#7436](https://github.com/emilk/egui/pull/7436)；hint 对齐修复 [emilk/egui#8332](https://github.com/emilk/egui/pull/8332)
- 本地位置：`apps/desktop/src/reader/egui_view.rs` 中的 `pdf_toc_editor_table`、`centered_assistant_text_edit` 和搜索输入框，以及 `apps/desktop/src/shelf/mod.rs` 中的 `shelf_search_field`

### 表现

将单行 `TextEdit` 放进高度高于默认文本行高的固定矩形时，未指定垂直对齐会沿用 `LEFT_TOP`，文字视觉上偏上，与同一行的按钮、数值输入框不居中。较早的 egui 实现即使指定了垂直对齐，点击和拖动的感知区域仍可能停留在控件顶部；后一个问题是上游确认的 #7433。

### 原因

`TextEdit` 的默认 `Align2` 是 `LEFT_TOP`，`ui.add_sized` 只扩大控件矩形，不会自动把单行文字改为垂直居中。上游旧实现还只在水平方向保存文本偏移量，命中测试没有纳入垂直对齐偏移；#7436 将偏移扩展为二维并同步修复了交互区域。

### 当前规避方案

所有放入显式高度容器、且设计上要求居中的单行输入框都显式调用 `.vertical_align(egui::Align::Center)`。不要只依赖外层 `horizontal_centered` 或 `add_sized`，它们只控制控件矩形，不改变 `TextEdit` 内部文字对齐。AI 输入框在升级到 0.36 后恢复使用原生 `hint_text`，其提示文字现在会遵循同一个垂直对齐设置。

### 升级检查

1. 检查 `TextEdit` 的默认垂直对齐是否发生变化，以及 #7436 的二维偏移逻辑是否仍存在。
2. 检查书架搜索、阅读器搜索、AI 输入框和 AI 目录编辑弹窗中文字、光标、点击与拖动选区是否位于同一垂直位置。
3. 在 Windows 100%、125%、150% 和 200% DPI 下重复检查单行输入框。
4. 只有上游默认行为能够满足设计时，才移除显式 `vertical_align`；否则保留该声明并更新本文版本信息。
