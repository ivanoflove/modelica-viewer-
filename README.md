# Modelica Viewer — Rust + GPUI

当前产品实现迁移到 Rust + GPUI。Electron 版本已经冻结为 legacy：它保留在
`legacy/electron` 分支和 `electron-final` 标签，只用于行为对照和回归参考。

## 当前状态

本分支 `rust-gpui` 已建立三层 workspace 骨架：

```text
modelica-core    source / lexer / AST / library / resolver / scene
modelica-render  geometry / viewport / hit testing
modelica-gpui    window、workspace 和 GPUI UI（待接入 GPUI）
```

当前本地没有 GPUI 原型代码，因此本轮没有假装完成 Viewer 迁移，也没有把
Electron 代码逐行翻译成 Rust。当前已完成 lexer、source document、基础严格
class parser、包目录扫描和 qualified-name ClassIndex；下一阶段补齐 annotation、
MSL registry、Icon/Diagram resolver，再接入 GPUI custom paint。

## 目标架构

```text
.mo → Lexer → Parser → AST → ClassResolver
                                  ↓
                         Icon/Diagram Resolver
                                  ↓
                             Scene Graph
                                  ↓
                    modelica-render geometry
                                  ↓
                         GPUI custom paint
```

`modelica-gpui` 不直接访问 AST，只消费 `IconScene` 和 `DiagramScene`。Icon 与
Diagram 共享 `Graphic`；Viewport 统一提供 `model_to_screen()` 和
`screen_to_model()`，交互状态不得污染 source geometry。

## 迁移顺序

1. Viewer：打开 `.mo`、Library Tree、Source、Icon、Diagram、MSL。
2. Selection：Graphic、Component、Connection 的独立 hit testing。
3. Editor：move、resize、vertex、create、delete、properties。
4. Source transaction：Command、parse validation、atomic write、Undo/Redo。

第一阶段目标覆盖 IEH_CPP、Boundary、Compressor、HeatXNTU、Mixer、Flash、FluidPort、
RealInput、Icon inheritance、Placement、Text macro 和 Diagram connection，并保证
package 不聚合 descendants；当前只读 core 基础已经可以加载 IEH_CPP fixture。

## 检查

```bash
cargo fmt --check
cargo clippy --all-targets --all-features
cargo test
cargo build --release
```

Electron 旧实现仍可查看：

```bash
git switch legacy/electron
```
