import { useEffect, useRef, useState } from "react";
import type {
  LoadPackageResult,
  PackageNodeDto,
  SourceEditReason,
} from "../../shared/modelica";
import type { LibraryInfo } from "../../shared/api";
import type { IconDto, EditableIconDto } from "../../shared/modelicaGraphics";
import { PackageTree, type Selection } from "./components/PackageTree";
import { IconViewer } from "./components/IconViewer";
import { AppearancePopover } from "./components/AppearancePopover";

type ViewMode = "source" | "icon" | "diagram";

function App(): JSX.Element {
  const [ipcStatus, setIpcStatus] = useState("checking…");
  const [root, setRoot] = useState<PackageNodeDto | null>(null);
  const [currentPath, setCurrentPath] = useState<string>("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<Selection | null>(null);
  const [viewMode, setViewMode] = useState<ViewMode>("source");
  const [libraries, setLibraries] = useState<LibraryInfo[]>([]);
  const [showLibraries, setShowLibraries] = useState(false);
  const [showAppearance, setShowAppearance] = useState(false);
  const [libraryError, setLibraryError] = useState<string | null>(null);

  const [source, setSource] = useState("");
  const [documentSource, setDocumentSource] = useState("");
  const [sourceLoading, setSourceLoading] = useState(false);
  const [sourceError, setSourceError] = useState<string | null>(null);
  const sourceEditorRef = useRef<HTMLDivElement>(null);

  const [icon, setIcon] = useState<IconDto | null>(null);
  const [iconLoading, setIconLoading] = useState(false);
  const [iconError, setIconError] = useState<string | null>(null);
  const [iconWarning, setIconWarning] = useState<string | null>(null);
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
    if (!window.api) return;
    void window.api.modelica
      .listLibraries()
      .then(setLibraries)
      .catch(() => undefined);
  }, []);

  const addLibrary = async () => {
    if (!window.api) return;
    setLibraryError(null);
    const result = await window.api.modelica.addLibrary();
    if ("error" in result) {
      if (result.error !== "canceled") setLibraryError(result.error);
      return;
    }
    setLibraries((current) => [
      ...current.filter((item) => item.path !== result.library.path),
      result.library,
    ]);
  };

  const removeLibrary = async (path: string) => {
    if (!window.api) return;
    const result = await window.api.modelica.removeLibrary(path);
    if ("error" in result) setLibraryError(result.error);
    else
      setLibraries((current) => current.filter((item) => item.path !== path));
  };

  const rescanLibraries = async () => {
    if (!window.api) return;
    setLibraries(await window.api.modelica.rescanLibraries());
  };

  useEffect(() => {
    if (!selected || !window.api) {
      setSource("");
      setDocumentSource("");
      setSourceError(null);
      setIcon(null);
      setIconError(null);
      setIconWarning(null);
      return;
    }
    let active = true;
    const loadSource = async () => {
      setSourceLoading(true);
      setSourceError(null);
      try {
        const result = await window.api.modelica.readSource(
          selected.node.sourceFile,
        );
        if (!active) return;
        if ("error" in result) {
          setSourceError(result.error);
          setSource("");
          setDocumentSource("");
          return;
        }
        setDocumentSource(result.content);
        if (selected.node.sourceRange) {
          const range = selected.node.sourceRange;
          setSource(result.content.slice(range.start, range.end));
        } else {
          setSource(result.content);
        }
      } catch (e) {
        if (!active) return;
        setSourceError((e as Error).message);
        setSource("");
      } finally {
        if (active) setSourceLoading(false);
      }
    };
    const loadIcon = async () => {
      setIconLoading(true);
      setIconError(null);
      setIconWarning(null);
      try {
        const range = selected.node.sourceRange ?? null;
        const iconRes = await window.api.modelica.getIcon(
          selected.node.sourceFile,
          range,
          selected.node.name,
        );
        const editableRes =
          selected.kind === "package"
            ? { editable: null }
            : await window.api.modelica.getEditableIcon(
                selected.node.sourceFile,
                range,
                selected.node.name,
              );
        if (!active) return;
        if ("error" in iconRes) {
          setIconError(iconRes.error);
          setIcon(null);
        } else {
          setIcon(iconRes.icon);
          setIconWarning(iconRes.warnings?.join("；") ?? null);
        }
        if ("error" in editableRes) {
          setEditableIcon(null);
        } else {
          setEditableIcon(editableRes.editable);
        }
      } catch (e) {
        if (!active) return;
        setIconError((e as Error).message);
        setIcon(null);
        setEditableIcon(null);
        setIconWarning(null);
      } finally {
        if (active) setIconLoading(false);
      }
    };
    void loadSource();
    void loadIcon();
    return () => {
      active = false;
    };
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

  const handleIconEdit = async (
    edit: {
      start: number;
      end: number;
      expectedText?: string;
      replacement: string;
    },
    reason: SourceEditReason,
  ): Promise<boolean> => {
    if (!selected || !window.api) return false;
    console.debug(
      "[SOURCE_COMMIT]",
      JSON.stringify({
        reason,
        targetQualifiedName: selected.node.qualifiedName,
        ...edit,
      }),
    );
    try {
      const transactionEdit = {
        ...edit,
        sourceVersion: editableIcon?.sourceVersion,
        targetQualifiedName: selected.node.qualifiedName,
      };
      const res = await window.api.modelica.applySourceEdit(
        selected.node.sourceFile,
        transactionEdit,
      );
      if ("error" in res) {
        setIconError(res.error);
        return false;
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
      if (!freshRange) {
        setIconError("保存后无法重新定位当前 Modelica 类，已保留原 Icon");
        return false;
      }
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
      } else if (freshRange) {
        setDocumentSource(srcRes.content);
        setSource(srcRes.content.slice(freshRange.start, freshRange.end));
      } else {
        setDocumentSource(srcRes.content);
        setSource(srcRes.content);
      }
      if ("error" in iconRes || !iconRes.icon) {
        setIconError(
          "error" in iconRes
            ? iconRes.error
            : "保存后重新解析未找到 Icon，已保留原 Icon",
        );
        return false;
      }
      setIconError(null);
      setIcon(iconRes.icon);
      if ("error" in editableRes) setEditableIcon(null);
      else setEditableIcon(editableRes.editable);
      return true;
    } catch (e) {
      setIconError((e as Error).message);
      return false;
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
        <div className="brand-lockup">
          <div className="brand-mark" aria-hidden="true">∿</div>
          <div>
            <p className="eyebrow">MODELICA WORKSPACE</p>
            <strong className="brand-title">Library Viewer</strong>
          </div>
        </div>
        <div className="header-context">
          <span className="connection-pill">
            <span className={`status-dot ${ipcStatus === "pong" ? "is-live" : ""}`} />
            {ipcStatus === "pong" ? "Ready" : ipcStatus}
          </span>
          {currentPath && (
            <span className="current-path" title={currentPath}>
              {currentPath}
            </span>
          )}
        </div>
        <div className="header-actions">
          <button
            className="primary-btn"
            onClick={() => void handleOpenFile()}
            disabled={loading}
          >
            {loading ? "加载中…" : "打开 .mo 文件"}
          </button>
          <button
            className="ghost-btn"
            onClick={() => void handleOpen()}
            disabled={loading}
          >
            打开库目录
          </button>
          <button
            className="ghost-btn"
            onClick={() => setShowLibraries((value) => !value)}
          >
            库路径
          </button>
          <div className="appearance-anchor">
            <button
              className={`icon-button ${showAppearance ? "is-active" : ""}`}
              onClick={() => setShowAppearance((value) => !value)}
              aria-label="Appearance settings"
              aria-expanded={showAppearance}
            >
              ◐
            </button>
            {showAppearance && <AppearancePopover />}
          </div>
        </div>
      </header>

      {showLibraries && (
        <div className="library-popover">
          <div className="library-popover-header">
            <strong>Modelica Library Paths</strong>
            <div>
              <button
                className="ghost-btn"
                onClick={() => void addLibrary()}
              >
                添加库
              </button>
              <button
                className="ghost-btn"
                onClick={() => void rescanLibraries()}
              >
                重新扫描
              </button>
            </div>
          </div>
          {libraryError && <div className="library-error">{libraryError}</div>}
          {libraries.length === 0 ? (
            <div className="library-empty">
              尚未添加标准库。请选择包含 package.mo 的库根目录。
            </div>
          ) : (
            <ul className="library-list">
              {libraries.map((library) => (
                <li key={library.path}>
                  <span title={library.path}>
                    {library.path} <small>({library.classCount} classes)</small>
                  </span>
                  <button
                    className="library-remove"
                    onClick={() => void removeLibrary(library.path)}
                  >
                    移除
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}

      {error && <div className="error-banner">{error}</div>}

      {root ? (
        <div className="browser">
          <aside className="tree-pane">
            <div className="sidebar-heading">
              <div>
                <p className="section-kicker">LIBRARY</p>
                <strong>{root.qualifiedName}</strong>
              </div>
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
                  <div className="workspace-title">
                    <span className="section-kicker">OPEN CLASS</span>
                    <strong>{selected.node.qualifiedName}</strong>
                    <span className="source-path" title={selected.node.sourceFile}>
                      {selected.node.sourceFile}
                    </span>
                  </div>
                  <button className="ghost-btn" onClick={() => void handleReveal()}>
                    ↗ 在文件夹中显示
                  </button>
                </div>
                <div className="viewer-tabs" role="tablist" aria-label="Viewer mode">
                  <button
                    className={viewMode === "source" ? "active" : ""}
                    onClick={() => setViewMode("source")}
                    role="tab"
                    aria-selected={viewMode === "source"}
                  >
                    <span className="tab-glyph">{`{ }`}</span>Source
                  </button>
                  <button
                    className={viewMode === "icon" ? "active" : ""}
                    onClick={() => setViewMode("icon")}
                    role="tab"
                    aria-selected={viewMode === "icon"}
                  >
                    <span className="tab-glyph">◇</span>Icon
                  </button>
                  <button
                    className={viewMode === "diagram" ? "active" : ""}
                    onClick={() => setViewMode("diagram")}
                    role="tab"
                    aria-selected={viewMode === "diagram"}
                  >
                    <span className="tab-glyph">⌘</span>Diagram
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
                      <>
                        {iconWarning && (
                          <div className="source-status icon-warning">
                            {iconWarning}
                          </div>
                        )}
                        <IconViewer
                          icon={icon}
                          editable={editableIcon}
                          modelName={selected.node.name}
                          resetKey={selected.node.qualifiedName}
                          sourceText={documentSource}
                          onEdit={handleIconEdit}
                        />
                      </>
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
            <p className="eyebrow">A CALM SPACE FOR COMPLEX MODELS</p>
            <h1>打开 Modelica 文件或库目录</h1>
            <p className="description">
              单个顶层 package 文件（例如 <code>IEH_CPP.mo</code>
              ）可以直接打开； 标准目录式库请选择包含 <code>package.mo</code>{" "}
              的库根目录。
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
      {root && (
        <footer className="status-bar">
          <span><span className="status-dot is-live" /> {selected?.node.qualifiedName ?? "No selection"}</span>
          <span>Ready <span className="status-separator">·</span> Grid 10</span>
        </footer>
      )}
    </div>
  );
}

export default App;
