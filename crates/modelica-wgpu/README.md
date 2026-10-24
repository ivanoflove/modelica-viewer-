# modelica-wgpu prototype

这是 Modelica Viewer 的独立 GPU 渲染路线验证器，不包含完整 UI 和 Modelica 解析器。

当前只提供三个代表图标：

- `HeatX`：球面渐变 + 折线
- `Heater`：球面渐变 + 折线
- `Boundary`：连续圆柱渐变 + 边框

## 运行

在仓库根目录执行：

```text
cargo run -p modelica-wgpu --release
```

交互：

- `1` / `2` / `3`：切换图标
- `←` / `→`：切换图标
- 鼠标左键或中键拖拽：平移
- 鼠标滚轮：以光标为中心缩放
- `R`：重置视图

窗口标题每秒更新 FPS 和最近一秒最差帧时间，用于 Windows 原生运行时检查帧稳定性。

默认使用 `Fifo` vsync，观察显示器上的实际帧 pacing；如需区分显示同步和渲染吞吐，可用
`MODELICA_WGPU_VSYNC=off cargo run -p modelica-wgpu --release` 做无同步对照。窗口启动日志会打印实际使用的 GPU adapter。

连接线拖动的聚合 profiler（不在每个 mouse event 输出）可通过以下命令启用：

```text
MODELICA_WGPU_PROFILE_DRAG=1 cargo run -p modelica-wgpu --release <package.mo>
```

拖动时每秒输出一次 `drag-profile`，包含事件/帧数、p50/p95/worst frame，以及 input、snap、cached endpoint reanchor、preview upload、UI、encode 与 present 的累计 CPU 时间。

加载 profiler 可通过以下命令启用：

```text
MODELICA_WGPU_PROFILE_LOAD=1 cargo run -p modelica-wgpu --release <package.mo>
```

它报告 metadata-first package load、class discovery、registry/source cache、tree、首次 UI，以及后续首次 Icon/Diagram resolve 和 GPU scene build 时间；每次首次 lazy resolve 只输出一行，不会为所有 class 预取场景。

## 外观与字体（与 Electron 客户端一致）

- 主题 / 强调色与 Electron 版同一套 token（surface、text、border、accent），默认跟随系统、强调色 Violet。
- 启动时自动读取、修改时自动保存到跨平台设置文件 `settings.json`：Windows `%APPDATA%\modelica-viewer\`、macOS
  `~/Library/Application Support/modelica-viewer/`、Linux `$XDG_CONFIG_HOME`（缺省 `~/.config`）下的 `modelica-viewer/`。
- UI 字体按平台候选自动加载（Windows Inter/Noto、Linux/macOS 常见字体目录）；中文字体缺失时可用
  `MODELICA_VIEWER_CJK_FONT=/path/to/CJK.ttf cargo run -p modelica-wgpu --release` 显式指定。

## 验收记录方式

1. Windows 原生 MSVC：`cargo run -p modelica-wgpu --release`
2. Linux 原生：同一命令运行于 X11 或 Wayland
3. 分别切换三个图标，持续缩放、拖拽至少 30 秒
4. 记录窗口标题中的 FPS 和 worst frame；目标是稳定接近显示器刷新率，60 Hz 下不持续低于 60 FPS
5. 对比截图检查渐变是否连续、边缘是否由 MSAA 平滑

通过这组验收后，再把场景缓存、输入模型和完整 UI 迁移到该渲染后端。
