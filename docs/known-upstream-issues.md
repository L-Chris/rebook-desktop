# 核心依赖已知问题

- 最近更新：2026-08-01
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

- 影响版本：`egui 0.35.0`
- 上游状态：截至 2026-08-01，相关问题仍为 Open；上游尚无与 Torto 紧凑图标按钮完全相同的最小复现
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

## egui：合法布局触发控件 ID/矩形变化误报

- 影响版本：`egui 0.35.0`
- 上游状态：截至 2026-08-01 两项问题仍为 Open
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
