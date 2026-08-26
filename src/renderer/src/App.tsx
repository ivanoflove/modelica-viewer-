import { useEffect, useRef, useState } from "react";
import type { LoadPackageResult, PackageNodeDto } from "../../shared/modelica";
import type { IconDto, EditableIconDto } from "../../shared/modelicaGraphics";
import { PackageTree, type Selection } from "./components/PackageTree";
import { IconViewer } from "./components/IconViewer";

type ViewMode = "source" | "icon" | "diagram";

function App(): JSX.Element {
  const [ipcStatus, setIpcStatus] = useState("checking…");
  const [root, setRoot] = useState<PackageNodeDto | null>(null);
  const [currentPath, setCurrentPath] = useState<string>("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<Selection | null>(null);
  const [viewMode, setViewMode] = useState<ViewMode>("source");

  const [source, setSource] = useState("");
  const [sourceLoading, setSourceLoading] = useState(false);
  const [sourceError, setSourceError] = useState<string | null>(null);
  const sourceEditorRef = useRef<HTMLDivElement>(null);

  const [icon, setIcon] = useState<IconDto | null>(null);
  const [iconLoading, setIconLoading] = useState(false);
  const [iconError, setIconError] = useState<string | null>(null);
  const [editableIcon, setEditableIcon] = useState<EditableIconDto | null>(
    null,
  );

  const applyLoadResult = (result: LoadPackageResult): void => {
    if ("error" in result) {
      setError(result.error);
      return;
    }
    if (result.canceled) return;
    setRoot(result.root);
    setCurrentPath(result.root.sourceFile);
    setSelected({ kind: "package", node: result.root });
  };

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
      setIcon(null);
      setIconError(null);
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
        if (
          selected.kind !== "package" &&
          (selected.node as { sourceRange?: { start: number; end: number } })
            .sourceRange
        ) {
          const range = (
            selected.node as { sourceRange: { start: number; end: number } }
          ).sourceRange;
          setSource(result.content.slice(range.start, range.end));
        } else {
          setSource(result.content);
        }
      } catch (e) {
        setSourceError((e as Error).message);
        setSource("");
      } finally {
        setSourceLoading(false);
      }
    };
    const loadIcon = async () => {
      setIconLoading(true);
      setIconError(null);
      try {
        if (selected.kind === "package") {
          setIcon(null);
          setEditableIcon(null);
          setIconLoading(false);
          return;
        }
        const range =
          (selected.node as { sourceRange?: { start: number; end: number } })
            .sourceRange ?? null;
        const [iconRes, editableRes] = await Promise.all([
          window.api.modelica.getIcon(
            selected.node.sourceFile,
            range,
            selected.node.name,
          ),
          window.api.modelica.getEditableIcon(
            selected.node.sourceFile,
            range,
            selected.node.name,
          ),
        ]);
        if ("error" in iconRes) {
          setIconError(iconRes.error);
          setIcon(null);
        } else {
          setIcon(iconRes.icon);
        }
        if ("error" in editableRes) {
          setEditableIcon(null);
        } else {
          setEditableIcon(editableRes.editable);
        }
      } catch (e) {
        setIconError((e as Error).message);
        setIcon(null);
        setEditableIcon(null);
      } finally {
        setIconLoading(false);
      }
    };
    void loadSource();
    void loadIcon();
  }, [selected]);

  useEffect(() => {
    sourceEditorRef.current?.scrollTo({ top: 0, left: 0 });
  }, [selected, viewMode]);

  const loadDirectoryIntoView = async (dirPath: string) => {
    if (!window.api) return;
    setLoading(true);
    setError(null);
    try {
      const result = await window.api.modelica.loadDirectory(dirPath);
      applyLoadResult(result);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    if (!window.api?.onAutoOpen) return;
    window.api.onAutoOpen((dirPath) => void loadDirectoryIntoView(dirPath));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handleOpen = async () => {
    if (!window.api) {
      setError("preload 未加载：window.api 为空，请重新构建");
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const result = await window.api.modelica.openAndLoad();
      applyLoadResult(result);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  };

  const handleOpenFile = async () => {
    if (!window.api) {
      setError("preload 未加载：window.api 为空，请重新构建");
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const result = await window.api.modelica.openFile();
      applyLoadResult(result);
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

  const handleIconEdit = async (edit: {
    start: number;
    end: number;
    replacement: string;
  }) => {
    if (!selected || !window.api) return;
    try {
      const res = await window.api.modelica.applySourceEdit(
        selected.node.sourceFile,
        edit,
      );
      if ("error" in res) {
        setIconError(res.error);
        return;
      }
      // refresh source and icon after write: re-parse current class for fresh range
      const freshRangeRes = await window.api.modelica.reloadClassRange(
        selected.node.sourceFile,
        selected.node.qualifiedName,
      );
      const freshRange =
        !("error" in freshRangeRes) && freshRangeRes.sourceRange
          ? freshRangeRes.sourceRange
          : null;
      const [srcRes, iconRes, editableRes] = await Promise.all([
        window.api.modelica.readSource(selected.node.sourceFile),
        window.api.modelica.getIcon(
          selected.node.sourceFile,
          freshRange,
          selected.node.name,
        ),
        window.api.modelica.getEditableIcon(
          selected.node.sourceFile,
          freshRange,
          selected.node.name,
        ),
      ]);
      if ("error" in srcRes) {
        setSourceError(srcRes.error);
      } else if (selected.kind !== "package" && freshRange) {
        setSource(srcRes.content.slice(freshRange.start, freshRange.end));
      } else {
        setSource(srcRes.content);
      }
      if ("error" in iconRes) setIconError(iconRes.error);
      else setIcon(iconRes.icon);
      if ("error" in editableRes) setEditableIcon(null);
      else setEditableIcon(editableRes.editable);
    } catch (e) {
      setIconError((e as Error).message);
    }
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
          <span className="eyebrow">
            MODELICA LIBRARY VIEWER — M1 · M2 Source Viewer · M3 Icon
          </span>
          <h1>Package Browser</h1>
          <p className="ipc-status">IPC: {ipcStatus}</p>
        </div>
        <div className="header-right">
          <button
            className="primary-btn"
            onClick={() => void handleOpenFile()}
            disabled={loading}
          >
            {loading ? "加载中…" : "打开 .mo 文件"}
          </button>
          <button
            className="secondary-btn"
            onClick={() => void handleOpen()}
            disabled={loading}
          >
            打开库目录
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
                <div className="viewer-tabs">
                  <button
                    className={viewMode === "source" ? "active" : ""}
                    onClick={() => setViewMode("source")}
                  >
                    Source
                  </button>
                  <button
                    className={viewMode === "icon" ? "active" : ""}
                    onClick={() => setViewMode("icon")}
                  >
                    Icon
                  </button>
                  <button
                    className={viewMode === "diagram" ? "active" : ""}
                    onClick={() => setViewMode("diagram")}
                  >
                    Diagram
                  </button>
                </div>
                {viewMode === "source" && (
                  <div ref={sourceEditorRef} className="source-editor">
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
                )}
                {viewMode === "icon" && (
                  <div className="source-editor icon-tab">
                    {iconLoading ? (
                      <div className="source-status">加载中…</div>
                    ) : iconError ? (
                      <div className="source-error">{iconError}</div>
                    ) : (
                      <IconViewer
                        icon={icon}
                        editable={editableIcon}
                        modelName={selected.node.name}
                        onEdit={handleIconEdit}
                      />
                    )}
                  </div>
                )}
                {viewMode === "diagram" && (
                  <div className="source-editor">
                    <div className="no-icon">Diagram 暂未实现</div>
                  </div>
                )}
              </>
            ) : (
              <div className="empty-detail">选择左侧节点查看源码</div>
            )}
          </section>
        </div>
      ) : (
        <main className="shell">
          <section className="welcome-card">
            <h2>打开 Modelica 文件或库目录</h2>
            <p className="description">
              单个顶层 package 文件（例如 <code>IEH_CPP.mo</code>）可以直接打开；
              标准目录式库请选择包含 <code>package.mo</code> 的库根目录。
              <br />
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
            <div className="welcome-actions">
              <button
                className="primary-btn large"
                onClick={() => void handleOpenFile()}
              >
                打开 .mo 文件
              </button>
              <button
                className="secondary-btn large"
                onClick={() => void handleOpen()}
              >
                打开库目录
              </button>
            </div>
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
