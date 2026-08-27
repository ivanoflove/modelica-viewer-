# Modelica Viewer — Rust + GPUI

`rust-gpui` 是当前主开发分支。项目正在从 Electron + React + SVG 迁移到 Rust + GPUI，目标是用原生 GPU custom paint 替代大规模 SVG DOM，优先解决 Diagram/Icon 在拖拽、缩放和大场景下的性能问题。

Electron 版本已经冻结，只作为行为对照和回归参考：

- branch: `legacy/electron`
- tag: `electron-final`

后续新功能默认只进入 Rust + GPUI 实现。

## 当前架构

```text
.mo source
   ↓
modelica-core
   ├─ lexer
   ├─ parser / AST
   ├─ package loader / ClassIndex
   ├─ annotation parser
   ├─ library registry / built-in MSL
   ├─ IconResolver
   └─ Scene DTO
          ↓
modelica-render
   ├─ geometry
   ├─ viewport
   └─ hit testing
          ↓
modelica-gpui
   ├─ native GPUI window
   ├─ class browser
   └─ Icon custom paint canvas
```

边界规则：

- `modelica-core` 不依赖 GPUI。
- GPUI 不直接扫描 Modelica 源码，不直接读取 annotation 文本。
- Resolver 负责 `.mo → semantic scene`。
- Renderer 只消费 `IconScene` / `DiagramScene`。
- viewport/screen 坐标绝不能写回 source geometry。
- package containment、Icon inheritance、Diagram composition 必须严格分离。

## 已完成

### Core

当前已经具备：

- Modelica lexer
- `SourceRange`
- long class parser
- short class definition 基础支持
- package/file loader
- nested class 与 qualified name
- ClassIndex
- generic annotation parser
- annotation number/string/bool/name/array/nested-call 解析
- Graphic scene 类型
- Rectangle / Ellipse / Line / Polygon / Text / Bitmap semantic DTO
- Icon coordinate system
- IconResolver
- `extends` Icon inheritance
- inheritance cycle detection
- package child Icon scope isolation
- LibraryRegistry
- bundled MSL 4.1.0 lazy resolve 基础
- `Modelica.Blocks.Interfaces.RealInput` short connector regression

### Render

当前已经具备：

- model/screen geometry 类型
- `Viewport`
- `model_to_screen()` / `screen_to_model()` round-trip test
- 基础 hit testing

### GPUI

当前已经接入真正的 GPUI window，不再只是 CLI skeleton。

当前 Viewer 支持：

- 从命令行打开 `.mo` 文件或 Modelica package 目录
- 左侧 class 列表
- 精确 class selection
- 选择 class 后调用 `IconResolver`
- 右侧 GPUI native canvas
- Rectangle custom paint
- Ellipse custom paint
- Line custom paint
- Polygon custom paint
- Modelica Y-up → screen Y-down 转换
- graphic `origin + rotation`
- Fit Icon coordinate system
- diagnostics 摘要

GPUI 使用 Zed 仓库中的 GPUI，并固定 revision，避免构建结果随 upstream HEAD 漂移。

## 当前未完成

当前 GPUI Icon renderer 仍有以下工作：

- Text custom paint
- Bitmap custom paint
- FillPattern 完整实现
- LinePattern / dash 完整实现
- Smooth / Bezier
- Arrow
- Text `%name` / `%class` / `%parameter` instance expansion
- public connector Icon placement
- `iconTransformation`
- interactive zoom / pan / fit toolbar
- Source tab
- semantic class tree expand/collapse
- Diagram Viewer
- selection/editing/undo/source write-back

这些缺项不会通过 SVG 或 WebView 临时绕过；Rust 主实现继续使用 GPUI native rendering。

## 运行

Gentoo / Linux 需要先具备正常 Rust toolchain 以及 GPUI/Zed 构建所需的系统依赖。

在仓库根目录：

```bash
cargo run -p modelica-gpui --release -- /path/to/IEH_CPP.mo
```

也可以打开一个带 `package.mo` 的 Modelica library 目录：

```bash
cargo run -p modelica-gpui --release -- /path/to/MyModelicaLibrary
```

第一次构建会拉取固定 revision 的 Zed/GPUI git dependencies，因此会明显比纯 `modelica-core` 构建慢。

## 推荐验证对象

迁移期间优先使用这些真实案例做 regression：

```text
IEH_CPP
IEH_CPP.FluidUnits.Boundary
IEH_CPP.FluidUnits.Compressor
IEH_CPP.FluidUnits.HeatXNTU
Modelica.Blocks.Interfaces.RealInput
```

重点检查：

1. package 选择不会聚合 descendants 的 graphics。
2. `extends` 才参与 Icon inheritance。
3. `Modelica.*` 按 root-qualified library resolve。
4. relative class name 按 lexical scope resolve。
5. viewport 操作不触碰 source coordinates。
6. renderer 不重新扫描整个 `.mo` 文件。

## 下一里程碑

当前下一步不是 Diagram，而是先完成真实 Icon Viewer：

```text
Text custom paint
   ↓
FillPattern / LinePattern
   ↓
Connector + iconTransformation
   ↓
MSL connector render
   ↓
Zoom / Pan / Fit
   ↓
IEH_CPP Boundary / Compressor / HeatXNTU parity
```

Icon 链路稳定后再进入：

```text
DiagramResolver
   ↓
Placement
   ↓
Component instances
   ↓
connect() / connection Line
   ↓
GPUI Diagram canvas
```

最后再恢复 Editor：

```text
selection
→ drag / resize / vertex
→ properties
→ EditorCommand
→ source transaction
→ undo / redo
```

## Quality gates

Core/render 改动应持续通过：

```bash
cargo fmt --check
cargo clippy --workspace --all-targets
cargo test --workspace
cargo build --release
```

GPUI dependency 接入后，建议额外执行：

```bash
cargo check -p modelica-gpui
cargo run -p modelica-gpui --release -- /path/to/IEH_CPP.mo
```

如果 `Cargo.lock` 因新增 GPUI git dependency 发生变化，应把 lockfile 一并提交，保证分支可复现。

## Legacy Electron

如需查看旧实现：

```bash
git switch legacy/electron
```

不要在 Electron 分支继续新增产品功能。旧代码只用于参考正确行为、UI 和已有 regression case；已知的 SVG/React/annotation ownership/source-range 问题不得机械翻译进 Rust。