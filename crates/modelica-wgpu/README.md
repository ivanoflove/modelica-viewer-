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

## 验收记录方式

1. Windows 原生 MSVC：`cargo run -p modelica-wgpu --release`
2. Linux 原生：同一命令运行于 X11 或 Wayland
3. 分别切换三个图标，持续缩放、拖拽至少 30 秒
4. 记录窗口标题中的 FPS 和 worst frame；目标是稳定接近显示器刷新率，60 Hz 下不持续低于 60 FPS
5. 对比截图检查渐变是否连续、边缘是否由 MSAA 平滑

通过这组验收后，再把场景缓存、输入模型和完整 UI 迁移到该渲染后端。
