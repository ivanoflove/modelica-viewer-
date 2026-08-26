# Modelica Library Viewer — M1 Package Browser

Electron + React + TypeScript 的 Modelica 包浏览器。**M1 仅实现「正确识别 Modelica package 并构建包树」**，不做 equation/annotation 图形与仿真。

## M1 已实现

- ✅ 极简 Lexer：先跳过 `//`、`/* */`、`"string"`（含 `""` 转义），再识别 `IDENT/DOT/SEMICOLON/KEYWORD/EOF`，避免 `annotation(" package ... ")` 误识别
- ✅ Parser：`within` → `qualifiedName ;`；`partial/encapsulated` 修饰；`package/model/block/connector/record/function/class/type`；`end <name>;` 严格匹配（`end if;` 不闭合 package）
- ✅ PackageLoader：单文件内嵌 `package A { package B { model C } }` 与目录式 `Modelica/Electrical/Analog/Basic/package.mo + *.mo` 均支持；`qualifiedName` 拼接为 `Modelica.Electrical.Analog.Basic.Resistor`
- ✅ Electron IPC：`modelica:openFile` / `modelica:loadFile` / `modelica:loadDirectory` / `modelica:reveal`，主进程解析、renderer 仅展示 DTO
- ✅ UI：顶部「打开 .mo 文件」/「打开库目录」→ 左侧 TreeView（📦/📘/🧱/🔌）→ 右侧详情（qualifiedName / within / sourceFile / Reveal in Folder）
- ✅ 单文件加载：顶层 `package IEH_CPP ... end IEH_CPP;` 不要求重命名为 `package.mo`；选择包含同名 `.mo` 文件的项目目录也会自动识别，所有嵌套节点保留同一 `sourceFile` 和独立 `sourceRange`
- ✅ Icon 容错与继承：支持 `origin`、qualified enum（如 `Smooth.Bezier`）、`fillPattern`、`textStyle`、短 `type` 与 `extends`；未知 graphics 属性安全忽略，未加载 MSL 时使用安全 fallback 并显示 warning

## Requirements

- Node.js 26+ / npm 10+
- 如需代理，可通过 `HTTP_PROXY` / `HTTPS_PROXY` 环境变量配置，用于 Electron 二进制与后续库下载

## Install

```bash
# 如需代理（示例）
export HTTP_PROXY=http://<proxy-host>:<port>
export HTTPS_PROXY=http://<proxy-host>:<port>

ELECTRON_MIRROR=https://npmmirror.com/mirrors/electron/ npm install
```

失败时：`rm -rf node_modules package-lock.json && ELECTRON_MIRROR=... npm install`

## Development

```bash
npm run dev          # electron-vite dev (主进程 + preload + renderer HMR)
npm run typecheck    # tsc -b
npm run build        # electron-vite build → out/
npm test             # vitest run (lexer/parser/loader)
```

## 使用

1. `npm run dev`
2. 点击「打开 .mo 文件」选择独立文件（例如 `IEH_CPP.mo`），或点击「打开库目录」选择：
   - `demo-modelica/MyLibrary`（目录式库，含 `package.mo`）
   - 任意 Modelica 库目录（如 `Modelica 4.x` 根，需含 `package.mo`）
3. 左侧展开 `📦 Modelica → 📦 Electrical → 📦 Analog → 📦 Basic → 📘 Resistor`，点击节点右侧显示 `qualifiedName` 与文件路径，`在文件夹中显示` 调用 `shell.showItemInFolder`

## 代理与后续下载预留（M2）

- 可通过 `HTTP_PROXY` / `HTTPS_PROXY` 环境变量配置代理。未来下载 Modelica Standard Library zip 时：

  ```ts
  const proxy = process.env.HTTP_PROXY
  // fetch(url, { dispatcher: new ProxyAgent(proxy) }) // undici
  ```

- M1 不实现下载，仅在 `src/main/download/README.md` 预留说明，避免把网络逻辑混入 `PackageLoader`。

## 项目结构

```text
src/main/modelica/
  types.ts    # PackageNode / ClassNode / ModelicaFile
  lexer.ts    # tokenize()
  parser.ts   # parseModelicaFile()
  loader.ts   # PackageLoader.load(rootDir)
  __tests__/  # lexer/parser/loader.test.ts
src/main/index.ts        # IPC 注册
src/preload/index.ts     # contextBridge → window.api.modelica
src/shared/api.ts + modelica.ts
src/renderer/src/
  App.tsx
  components/PackageTree.tsx
  styles.css
demo-modelica/MyLibrary/package.mo
```

## 验证

```bash
npm run typecheck   # 0 errors
npm test            # 15 tests passed (lexer 5, parser 7, loader 3)
npm run build       # out/main/index.js ~20kB, renderer ~239kB
```

手动：`npm run dev` 选择 `IEH_CPP.mo`，校验 `IEH_CPP.ThermoMedium.Functions.create`、`IEH_CPP.ThermoMedium.Units.FlashUnit` 等 qualifiedName；再打开 `demo-modelica/MyLibrary` 校验目录扫描。

选择一个不含 `package.mo`、但根目录只有一个 `.mo` 文件（或有与目录同名的 `.mo` 文件）时，程序会自动按单文件 package 加载；其他目录则提示使用「打开 .mo 文件」。
