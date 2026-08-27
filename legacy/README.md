# Legacy Electron implementation

Electron + React + TypeScript 的最后版本保存在 Git 分支 `legacy/electron`，
并由标签 `electron-final` 标记。

该实现只作为功能参考、UI 参考和 regression oracle，不再接受新功能。
当前开发分支是 `rust-gpui`，新实现位于 `crates/`。

远端保护分支与标签需要在具备网络权限时执行：

```bash
git push origin legacy/electron
git push origin electron-final
git push -u origin rust-gpui
```

