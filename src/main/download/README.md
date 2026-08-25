# Download (M2 预留)

M1 不实现下载。本目录为 M2 预留：

- 可通过代理（`HTTP_PROXY` / `HTTPS_PROXY`）访问网络
- 用 `undici` + `ProxyAgent` 拉取 Modelica Standard Library zip，解压到用户选定目录后调用 `PackageLoader`
- 保持 `PackageLoader` 纯本地 FS 逻辑，不混入网络

示例（M2 实现时）：

```ts
import { ProxyAgent, fetch } from 'undici'
const proxy = process.env.HTTP_PROXY
const res = await fetch(url, { dispatcher: new ProxyAgent(proxy) })
```
