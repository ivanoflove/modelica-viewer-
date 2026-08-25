import { useEffect, useState } from "react";
import type { PackageNodeDto } from "../../shared/modelica";
import { PackageTree, type Selection } from "./components/PackageTree";

function App(): JSX.Element {
  const [ipcStatus, setIpcStatus] = useState("checking…");
  const [root, setRoot] = useState<PackageNodeDto | null>(null);
  const [currentPath, setCurrentPath] = useState<string>("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<Selection | null>(null);

  const [source, setSource] = useState("");
  const [sourceLoading, setSourceLoading] = useState(false);
  const [sourceError, setSourceError] = useState<string | null>(null);

  useEffect(() => {
    if (!window.api) {
      setIpcStatus("preload missing — 请重建后重启");
      return;
    }
    void window.api
      .ping()
      .then((m) => setIpcStatus(m))
      .catch(() => setIpcStatus("unavailable"));
  }, []);

  useEffect(() => {
    if (!selected || !window.api) {
      setSource("");
      setSourceError(null);
      return;
    }
    const loadSource = async () => {
      setSourceLoading(true);
      setSourceError(null);
      try {
        const result = await window.api.modelica.readSource(
          selected.node.sourceFile,
        );
        if ("error" in result) {
          setSourceError(result.error);
          setSource("");
          return;
        }
        setSource(result.content);
      } catch (e) {
        setSourceError((e as Error).message);
        setSource("");
      } finally {
        setSourceLoading(false);
      }
    };
    void loadSource();
  }, [selected]);

  const handleOpen = async () => {
    if (!window.api) {
      setError("preload 未加载：window.api 为空，请重新构建");
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const result = await window.api.modelica.openAndLoad();
      if ("error" in result) {
        setError(result.error);
        return;
      }
      if (result.canceled) return;
      setRoot(result.root);
      setCurrentPath(result.root.sourceFile);
      setSelected({ kind: "package", node: result.root });
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  };

  const handleReveal = async () => {
    if (!selected || !window.api) return;
    const path = selected.node.sourceFile;
    await window.api.modelica.reveal(path);
  };

  const treeCount = (node: PackageNodeDto | null): number => {
    if (!node) return 0;
    let count = 1 + node.classes.length;
    for (const ch of node.children) count += treeCount(ch);
    return count;
  };

  return (
    <div className="app-layout">
      <header className="app-header">
        <div className="header-left">
          <span className="eyebrow">MODELICA LIBRARY VIEWER — M1 · M2 Source Viewer</span>
          <h1>Package Browser</h1>
          <p className="ipc-status">IPC: {ipcStatus}</p>
        </div>
        <div className="header-right">
          <button
            className="primary-btn"
            onClick={() => void handleOpen()}
            disabled={loading}
          >
            {loading ? "加载中…" : "打开目录"}
          </button>
          {currentPath && (
            <span className="current-path" title={currentPath}>
              {currentPath}
            </span>
          )}
        </div>
      </header>

      {error && <div className="error-banner">{error}</div>}

      {root ? (
        <div className="browser">
          <aside className="tree-pane">
            <div className="pane-title">
              <span>📦 {root.qualifiedName}</span>
              <span className="count">{treeCount(root)} 项</span>
            </div>
            <PackageTree
              root={root}
              selected={selected}
              onSelect={setSelected}
            />
            {root.loadErrors && root.loadErrors.length > 0 && (
              <div className="load-errors">
                <h4>加载警告 ({root.loadErrors.length})</h4>
                <ul>
                  {root.loadErrors.slice(0, 10).map((e) => (
                    <li key={e}>{e}</li>
                  ))}
                </ul>
              </div>
            )}
          </aside>

          <section className="detail-pane">
            {selected ? (
              <>
                <div className="source-toolbar">
                  <div>
                    <strong>{selected.node.qualifiedName}</strong>
                    <span className="source-path">
                      {selected.node.sourceFile}
                    </span>
                  </div>
                  <button
                    className="secondary-btn"
                    onClick={() => void handleReveal()}
                  >
                    在文件夹中显示
                  </button>
                </div>
                <div className="source-editor">
                  {sourceLoading ? (
                    <div className="source-status">加载中…</div>
                  ) : sourceError ? (
                    <div className="source-error">{sourceError}</div>
                  ) : (
                    <pre>
                      <code>{source}</code>
                    </pre>
                  )}
                </div>
              </>
            ) : (
              <div className="empty-detail">选择左侧节点查看源码</div>
            )}
          </section>
        </div>
      ) : (
        <main className="shell">
          <section className="welcome-card">
            <h2>选择 Modelica 库目录</h2>
            <p className="description">
              支持单文件内嵌{" "}
              <code>
                package A &#123; package B &#123; model C &#125; &#125;
              </code>{" "}
              与目录式
              <code> Modelica/Electrical/Analog/Basic/package.mo</code>。M1
              仅识别
              <code>
                {" "}
                within / package / model / block / connector / record / function
                / class
              </code>
              。
            </p>
            <button
              className="primary-btn large"
              onClick={() => void handleOpen()}
            >
              打开目录
            </button>
            <p className="hint">
              已内置 demo: <code>demo-modelica/MyLibrary</code> /{" "}
              <code>showcase</code>
            </p>
          </section>
        </main>
      )}
    </div>
  );
}

export default App;
