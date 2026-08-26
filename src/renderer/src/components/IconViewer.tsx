import { memo, useEffect, useRef, useState, type ReactNode } from "react";
import type {
  IconDto,
  EditableIconDto,
  EditableGraphic,
  GraphicTransform,
  Extent,
} from "../../../shared/modelicaGraphics";
import { GraphicItem } from "./GraphicItem";
import {
  toSvgTransform,
  boundsOf,
  applyTransform,
} from "../../editor/Transform";
import {
  clientToModelicaWithInverse,
  dragDeltaFromStart,
} from "../../editor/DragController";
import {
  resizeExtent,
  resizeHandles,
  type ResizeHandle,
  type ResizeSession,
} from "../../editor/controllers/ResizeController";
import { HistoryManager } from "../../editor/history/HistoryManager";
import { useSelection } from "../../editor/Selection";
import type { SelectionState } from "../../editor/Selection";
import { GraphicViewport } from "./GraphicViewport";
import type { SourceEditReason } from "../../../shared/modelica";
import { recordViewerPerformance } from "../performance";

type Edit = {
  start: number;
  end: number;
  expectedText?: string;
  replacement: string;
};

interface Props {
  icon: IconDto | null;
  editable?: EditableIconDto | null;
  modelName: string;
  resetKey?: string;
  onEdit?: (edit: Edit, reason: SourceEditReason) => Promise<boolean>;
}

interface DragSession {
  id: string;
  pointerId: number;
  pointerStart: { x: number; y: number };
  originalGraphic: EditableGraphic["graphic"];
  inverseScreenToModel: DOMMatrix;
  transformStart: GraphicTransform;
}

const identity: GraphicTransform = {
  translate: { x: 0, y: 0 },
  scale: { x: 1, y: 1 },
  rotate: 0,
};

export function IconViewer({
  icon,
  editable,
  modelName,
  resetKey = modelName,
  onEdit,
}: Props) {
  const svgRef = useRef<SVGSVGElement>(null);
  const dragRef = useRef<DragSession | null>(null);
  const viewportGroupRef = useRef<SVGGElement>(null);
  const graphicGroupRefs = useRef(new Map<string, SVGGElement>());
  const selectionGroupRefs = useRef(new Map<string, SVGGElement>());
  const screenOverlayRef = useRef<SVGGElement>(null);
  const screenOverlayUpdateRef = useRef<() => void>(() => undefined);
  const resizeRef = useRef<ResizeSession | null>(null);
  const pendingPointRef = useRef<{ x: number; y: number } | null>(null);
  const rafRef = useRef<number | null>(null);
  const pendingResizeRef = useRef<{
    point: { x: number; y: number };
    shift: boolean;
    alt: boolean;
  } | null>(null);
  const resizeRafRef = useRef<number | null>(null);
  const historyRef = useRef(new HistoryManager(100));
  const historyBusyRef = useRef(false);
  const [resizePreview, setResizePreview] = useState<{
    graphicId: string;
    extent: Extent;
  } | null>(null);
  const [optimisticGraphics, setOptimisticGraphics] = useState<
    Map<string, EditableGraphic["graphic"]>
  >(new Map());
  const [interactionNotice, setInteractionNotice] = useState<string | null>(
    null,
  );
  const [historyVersion, setHistoryVersion] = useState(0);
  const sel: SelectionState = useSelection();

  // A successful source reload supplies the canonical graphic. Until then,
  // retain the optimistic graphic so pointerup cannot visibly snap backward.
  useEffect(() => {
    setOptimisticGraphics(new Map());
  }, [editable]);

  useEffect(() => {
    if (rafRef.current !== null) cancelAnimationFrame(rafRef.current);
    if (resizeRafRef.current !== null)
      cancelAnimationFrame(resizeRafRef.current);
    dragRef.current = null;
    resizeRef.current = null;
    pendingPointRef.current = null;
    pendingResizeRef.current = null;
    rafRef.current = null;
    resizeRafRef.current = null;
    historyRef.current = new HistoryManager(100);
    setResizePreview(null);
    setInteractionNotice(null);
    setHistoryVersion((version) => version + 1);
  }, [resetKey]);

  useEffect(() => {
    screenOverlayUpdateRef.current();
  }, [sel.selectedId, icon, editable, optimisticGraphics, resizePreview, resetKey]);

  if (!icon) return <div className="no-icon">No Icon annotation</div>;

  const editables = editable?.editables ?? [];
  const entries = icon.graphics.map((graphic, index) => {
    const ed =
      editables.find(
        (item) =>
          item.id === graphic.graphicId ||
          item.graphic.graphicId === graphic.graphicId,
      ) ?? (graphic.graphicId ? undefined : editables[index]);
    return {
      id: graphic.graphicId ?? ed?.id ?? `view:${index}`,
      graphic,
      editable: ed,
    };
  });

  const transformFor = (id: string): GraphicTransform => {
    const base = editables.find((ed) => ed.id === id)?.transform ?? identity;
    return base;
  };

  const modelDeltaToRootTransform = (dx: number, dy: number) => {
    const svg = svgRef.current;
    const viewport = viewportGroupRef.current;
    const rootInverse = svg?.getScreenCTM()?.inverse();
    const viewportMatrix = viewport?.getScreenCTM();
    if (!rootInverse || !viewportMatrix) return "translate(0,0)";
    const origin = new DOMPoint(0, 0)
      .matrixTransform(viewportMatrix)
      .matrixTransform(rootInverse);
    const moved = new DOMPoint(dx, -dy)
      .matrixTransform(viewportMatrix)
      .matrixTransform(rootInverse);
    return `translate(${moved.x - origin.x},${moved.y - origin.y})`;
  };

  const applyDragPreview = (
    session: DragSession,
    point: { x: number; y: number },
  ) => {
    const delta = dragDeltaFromStart(session.pointerStart, point);
    const graphicGroup = graphicGroupRefs.current.get(session.id);
    const selectionGroup = selectionGroupRefs.current.get(session.id);
    const transform = `translate(${delta.x},${delta.y})`;
    graphicGroup?.setAttribute("transform", transform);
    selectionGroup?.setAttribute("transform", transform);
    screenOverlayRef.current?.setAttribute(
      "transform",
      modelDeltaToRootTransform(delta.x, delta.y),
    );
    recordViewerPerformance("dragPreviewRafUpdates");
  };

  const schedulePreview = (point: { x: number; y: number }) => {
    pendingPointRef.current = point;
    if (rafRef.current !== null) return;
    rafRef.current = requestAnimationFrame(() => {
      rafRef.current = null;
      const session = dragRef.current;
      const pending = pendingPointRef.current;
      if (session && pending) applyDragPreview(session, pending);
    });
  };

  const clearDrag = () => {
    if (rafRef.current !== null) cancelAnimationFrame(rafRef.current);
    rafRef.current = null;
    pendingPointRef.current = null;
    const session = dragRef.current;
    if (session) {
      graphicGroupRefs.current.get(session.id)?.removeAttribute("transform");
      selectionGroupRefs.current
        .get(session.id)
        ?.removeAttribute("transform");
    }
    screenOverlayRef.current?.removeAttribute("transform");
    dragRef.current = null;
  };

  const clearResize = () => {
    if (resizeRafRef.current !== null)
      cancelAnimationFrame(resizeRafRef.current);
    resizeRafRef.current = null;
    pendingResizeRef.current = null;
    resizeRef.current = null;
    setResizePreview(null);
  };

  // Capture on the stable SVG root instead of the transient graphic group.
  // React may replace a graphic group while the async source edit is being
  // committed; root capture keeps pointerup/cancel deterministic in that case.
  const capturePointer = (pointerId: number) => {
    const svg = svgRef.current;
    if (!svg) return;
    try {
      svg.setPointerCapture(pointerId);
    } catch {
      // The pointer can already have been cancelled by the window manager.
    }
  };

  const releasePointer = (pointerId: number) => {
    const svg = svgRef.current;
    if (!svg) return;
    try {
      if (svg.hasPointerCapture(pointerId)) svg.releasePointerCapture(pointerId);
    } catch {
      // Pointer capture is best-effort during teardown/blur.
    }
  };

  useEffect(() => {
    const cancelInteraction = () => {
      const drag = dragRef.current;
      const resize = resizeRef.current;
      if (drag) releasePointer(drag.pointerId);
      if (resize) releasePointer(resize.pointerId);
      clearDrag();
      clearResize();
    };
    window.addEventListener("blur", cancelInteraction);
    return () => window.removeEventListener("blur", cancelInteraction);
  });

  const commitGraphicChange = (
    ed: EditableGraphic,
    before: EditableGraphic["graphic"],
    after: EditableGraphic["graphic"],
    edit: Edit,
    reason: SourceEditReason,
  ) => {
    setOptimisticGraphics((previous) => {
      const next = new Map(previous);
      next.set(ed.id, after);
      return next;
    });
    if (!onEdit) return;
    void onEdit(edit, reason)
      .then((success) => {
        if (success) {
          historyRef.current.push({ graphicId: ed.id, before, after });
          setHistoryVersion((version) => version + 1);
        } else {
          setInteractionNotice("修改未保存：源文件内容已变化，请重新选择图元后重试");
          setOptimisticGraphics((previous) => {
            const next = new Map(previous);
            next.delete(ed.id);
            return next;
          });
        }
      })
      .catch((error: unknown) => {
        setInteractionNotice(
          `修改未保存：${error instanceof Error ? error.message : "未知错误"}`,
        );
        setOptimisticGraphics((previous) => {
          const next = new Map(previous);
          next.delete(ed.id);
          return next;
        });
      });
  };

  const handlePropertyEdit = (
    edit: Edit,
    after: EditableGraphic["graphic"],
  ) => {
    const ed = editables.find((item) => item.id === sel.selectedId);
    if (!ed) return;
    const before = optimisticGraphics.get(ed.id) ?? ed.graphic;
    commitGraphicChange(ed, before, after, edit, "property");
  };

  const handlePointerDown = (e: React.PointerEvent, ed: EditableGraphic) => {
    if (e.button !== 0) return;
    sel.setSelected(ed.id);
    if (ed.inherited) {
      e.preventDefault();
      e.stopPropagation();
      setInteractionNotice(
        `继承图形不可在当前类直接编辑：${ed.ownerQualifiedName ?? "基类"}`,
      );
      return;
    }
    setInteractionNotice(null);
    const svg = svgRef.current;
    if (!svg) return;
    e.preventDefault();
    e.stopPropagation();
    const inverse = svg.getScreenCTM()?.inverse();
    if (!inverse) return;
    const point = clientToModelicaWithInverse(e.clientX, e.clientY, inverse);
    capturePointer(e.pointerId);
    const session: DragSession = {
      id: ed.id,
      pointerId: e.pointerId,
      pointerStart: point,
      originalGraphic: optimisticGraphics.get(ed.id) ?? ed.graphic,
      inverseScreenToModel: inverse,
      transformStart: ed.transform,
    };
    dragRef.current = session;
  };

  const handleReadonlyPointerDown = (
    e: React.PointerEvent,
    id: string,
    owner?: string,
  ) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    sel.setSelected(id);
    setInteractionNotice(`继承图形不可在当前类直接编辑：${owner ?? "基类"}`);
  };

  const handlePointerMove = (e: React.PointerEvent) => {
    const session = dragRef.current;
    if (!session || session.pointerId !== e.pointerId) return;
    e.preventDefault();
    e.stopPropagation();
    const point = clientToModelicaWithInverse(
      e.clientX,
      e.clientY,
      session.inverseScreenToModel,
    );
    if (point) schedulePreview(point);
  };

  const handlePointerUp = (e: React.PointerEvent) => {
    const session = dragRef.current;
    if (!session || session.pointerId !== e.pointerId) {
      releasePointer(e.pointerId);
      clearDrag();
      return;
    }
    e.preventDefault();
    e.stopPropagation();
    const point = clientToModelicaWithInverse(
      e.clientX,
      e.clientY,
      session.inverseScreenToModel,
    );
    const ed = editables.find((item) => item.id === session.id);
    if (point && ed && onEdit) {
      const raw = dragDeltaFromStart(session.pointerStart, point);
      const rawDx = raw.x;
      const rawDy = raw.y;
      const dx = e.shiftKey ? Math.round(rawDx / 10) * 10 : rawDx;
      const dy = e.shiftKey ? Math.round(rawDy / 10) * 10 : rawDy;
      if (dx !== 0 || dy !== 0) {
        const committed = applyTransform(session.originalGraphic, {
          translate: { x: dx, y: dy },
          scale: { x: 1, y: 1 },
          rotate: 0,
        });
        const edit = buildTranslateEdit(ed, session.originalGraphic, dx, dy);
        releasePointer(e.pointerId);
        clearDrag();
        if (edit)
          commitGraphicChange(
            ed,
            session.originalGraphic,
            committed,
            edit,
            "drag",
          );
        return;
      }
    }
    releasePointer(e.pointerId);
    clearDrag();
  };

  const scheduleResize = (
    point: { x: number; y: number },
    shift: boolean,
    alt: boolean,
  ) => {
    pendingResizeRef.current = { point, shift, alt };
    if (resizeRafRef.current !== null) return;
    resizeRafRef.current = requestAnimationFrame(() => {
      resizeRafRef.current = null;
      const session = resizeRef.current;
      const pending = pendingResizeRef.current;
      if (!session || !pending) return;
      const delta = dragDeltaFromStart(
        session.startPointerModel,
        pending.point,
      );
      setResizePreview({
        graphicId: session.graphicId,
        extent: resizeExtent(
          session.originalExtent,
          session.handle,
          delta,
          pending.shift,
          pending.alt,
        ),
      });
    });
  };

  const handleResizeDown = (
    e: React.PointerEvent,
    ed: EditableGraphic,
    handle: ResizeHandle,
  ) => {
    if (e.button !== 0 || ed.inherited || !ed.source.extentRange) return;
    const svg = svgRef.current;
    const baseGraphic = optimisticGraphics.get(ed.id) ?? ed.graphic;
    const extent = (baseGraphic as { extent?: Extent }).extent;
    if (!svg || !extent) return;
    e.preventDefault();
    e.stopPropagation();
    const inverse = svg.getScreenCTM()?.inverse();
    if (!inverse) return;
    const point = clientToModelicaWithInverse(e.clientX, e.clientY, inverse);
    capturePointer(e.pointerId);
    sel.setSelected(ed.id);
    setInteractionNotice(null);
    setResizePreview(null);
    resizeRef.current = {
      graphicId: ed.id,
      handle,
      pointerId: e.pointerId,
      startPointerModel: point,
      originalExtent: {
        p1: { ...extent.p1 },
        p2: { ...extent.p2 },
      },
      inverseScreenToModelMatrix: inverse,
    };
  };

  const handleResizeMove = (e: React.PointerEvent) => {
    const session = resizeRef.current;
    if (!session || session.pointerId !== e.pointerId) return;
    e.preventDefault();
    e.stopPropagation();
    const point = clientToModelicaWithInverse(
      e.clientX,
      e.clientY,
      session.inverseScreenToModelMatrix,
    );
    scheduleResize(point, e.shiftKey, e.altKey);
  };

  const handleResizeUp = (e: React.PointerEvent) => {
    const session = resizeRef.current;
    if (!session || session.pointerId !== e.pointerId) return;
    e.preventDefault();
    e.stopPropagation();
    const ed = editables.find((item) => item.id === session.graphicId);
    const point = clientToModelicaWithInverse(
      e.clientX,
      e.clientY,
      session.inverseScreenToModelMatrix,
    );
    if (ed && onEdit) {
      const delta = dragDeltaFromStart(session.startPointerModel, point);
      const extent = resizeExtent(
        session.originalExtent,
        session.handle,
        delta,
        e.shiftKey,
        e.altKey,
      );
      const before = optimisticGraphics.get(ed.id) ?? ed.graphic;
      const committed = { ...before, extent } as EditableGraphic["graphic"];
      const edit = buildExtentEdit(ed, extent);
      if (edit) {
        releasePointer(e.pointerId);
        clearResize();
        commitGraphicChange(ed, before, committed, edit, "resize");
        return;
      }
    }
    releasePointer(e.pointerId);
    clearResize();
  };

  const handleResizeCancel = (e: React.PointerEvent) => {
    e.preventDefault();
    e.stopPropagation();
    releasePointer(e.pointerId);
    clearResize();
  };

  const applyHistory = async (direction: "undo" | "redo") => {
    if (historyBusyRef.current || !onEdit) return;
    const command =
      direction === "undo"
        ? historyRef.current.peekUndo()
        : historyRef.current.peekRedo();
    if (!command) return;
    const ed = editables.find((item) => item.id === command.graphicId);
    if (!ed) return;
    const target = direction === "undo" ? command.before : command.after;
    const edit = buildGraphicEdit(ed, target);
    if (!edit) return;
    historyBusyRef.current = true;
    setOptimisticGraphics((previous) => {
      const next = new Map(previous);
      next.set(ed.id, target);
      return next;
    });
    try {
      const success = await onEdit(edit, direction);
      if (success) {
        if (direction === "undo") historyRef.current.acceptUndo();
        else historyRef.current.acceptRedo();
        setHistoryVersion((version) => version + 1);
      } else {
        setOptimisticGraphics((previous) => {
          const next = new Map(previous);
          next.delete(ed.id);
          return next;
        });
      }
    } catch {
      setOptimisticGraphics((previous) => {
        const next = new Map(previous);
        next.delete(ed.id);
        return next;
      });
    } finally {
      historyBusyRef.current = false;
    }
  };

  const handlePointerCancel = (e: React.PointerEvent) => {
    e.preventDefault();
    e.stopPropagation();
    releasePointer(e.pointerId);
    clearDrag();
    clearResize();
  };

  const selectedEntry = entries.find(({ id }) => id === sel.selectedId);
  const selectedEditable = selectedEntry?.editable;
  const selectedGraphic = selectedEditable
    ? (optimisticGraphics.get(selectedEditable.id) ?? selectedEditable.graphic)
    : null;
  const selectedBounds = selectedEditable && selectedGraphic
    ? boundsOf(applyTransform(selectedGraphic, transformFor(selectedEditable.id)))
    : null;
  const canResizeSelected =
    !!selectedEditable &&
    !selectedEditable.inherited &&
    !!selectedEditable.source.extentRange &&
    (selectedEditable.graphic.type === "Rectangle" ||
      selectedEditable.graphic.type === "Ellipse" ||
      selectedEditable.graphic.type === "Text");

  const modelPointToRoot = (point: { x: number; y: number }) => {
    const svg = svgRef.current;
    const viewport = viewportGroupRef.current;
    const rootInverse = svg?.getScreenCTM()?.inverse();
    const viewportMatrix = viewport?.getScreenCTM();
    if (!rootInverse || !viewportMatrix) return null;
    return new DOMPoint(point.x, -point.y)
      .matrixTransform(viewportMatrix)
      .matrixTransform(rootInverse);
  };

  const updateScreenOverlay = () => {
    const overlay = screenOverlayRef.current;
    const svg = svgRef.current;
    if (!overlay || !svg || !selectedBounds) {
      overlay?.removeAttribute("transform");
      return;
    }
    const rootMatrix = svg.getScreenCTM();
    if (!rootMatrix) return;
    const rootScale = Math.max(Math.hypot(rootMatrix.a, rootMatrix.b), 1e-6);
    for (const handle of resizeHandles) {
      const point = modelPointToRoot(handlePosition(handle, selectedBounds));
      if (!point) continue;
      const visual = overlay.querySelector<SVGCircleElement>(
        `[data-screen-handle="${handle}"] .resize-handle`,
      );
      const hit = overlay.querySelector<SVGCircleElement>(
        `[data-screen-handle="${handle}"] .resize-hit-target`,
      );
      visual?.setAttribute("cx", String(point.x));
      visual?.setAttribute("cy", String(point.y));
      visual?.setAttribute("r", String(5 / rootScale));
      hit?.setAttribute("cx", String(point.x));
      hit?.setAttribute("cy", String(point.y));
      hit?.setAttribute("r", String(9 / rootScale));
    }
  };

  const screenOverlay = canResizeSelected && selectedBounds && selectedEditable
    ? resizeHandles.map((handle) => (
        <g key={`screen:${selectedEditable.id}:${handle}`} data-screen-handle={handle}>
          <circle
            className="resize-hit-target"
            cx={0}
            cy={0}
            r={9}
            onPointerDown={(e) => handleResizeDown(e, selectedEditable, handle)}
            pointerEvents="all"
          />
          <circle className="resize-handle" cx={0} cy={0} r={5} pointerEvents="none" />
        </g>
      ))
    : null;

  screenOverlayUpdateRef.current = updateScreenOverlay;

  const canUndo = historyVersion >= 0 && historyRef.current.canUndo;
  const canRedo = historyVersion >= 0 && historyRef.current.canRedo;

  return (
    <div className="icon-editor-shell">
      <GraphicViewport
        icon={icon}
        resetKey={resetKey}
        svgRef={svgRef}
        viewportGroupRef={viewportGroupRef}
        screenOverlayRef={screenOverlayRef}
        onViewportTransform={updateScreenOverlay}
        overlay={screenOverlay}
        onPointerMove={(event) => {
          handlePointerMove(event);
          handleResizeMove(event);
        }}
        onPointerUp={(event) => {
          handlePointerUp(event);
          handleResizeUp(event);
        }}
        onPointerCancel={(event) => {
          handlePointerCancel(event);
          handleResizeCancel(event);
        }}
        onUndo={() => void applyHistory("undo")}
        onRedo={() => void applyHistory("redo")}
        canUndo={canUndo}
        canRedo={canRedo}
      >
        <g transform="scale(1,-1)">
          <GraphicLayer>
            {entries.map(({ id, graphic, editable: ed }, index) => {
              const transform = transformFor(id);
              const baseGraphic = ed
                ? (optimisticGraphics.get(id) ?? graphic)
                : graphic;
              const renderGraphic =
                ed && resizePreview?.graphicId === id && "extent" in baseGraphic
                  ? { ...baseGraphic, extent: resizePreview.extent }
                  : baseGraphic;
              return (
                <g
                  key={id}
                  ref={(node) => {
                    if (node) graphicGroupRefs.current.set(id, node);
                    else graphicGroupRefs.current.delete(id);
                  }}
                  onPointerDown={
                    ed
                      ? (e) => handlePointerDown(e, ed)
                      : graphic.inherited
                        ? (e) =>
                            handleReadonlyPointerDown(
                              e,
                              id,
                              graphic.ownerQualifiedName,
                            )
                        : undefined
                  }
                  onPointerEnter={() => sel.setHover(id)}
                  onPointerLeave={() => sel.setHover(null)}
                  onPointerMove={ed ? handlePointerMove : undefined}
                  onPointerUp={ed ? handlePointerUp : undefined}
                  onPointerCancel={ed ? handlePointerCancel : undefined}
                  style={ed ? { cursor: "move" } : undefined}
                >
                  <g transform={ed ? toSvgTransform(transform) : undefined}>
                    <GraphicItem
                      item={renderGraphic}
                      styleId={`graphic-style-${index}`}
                    />
                  </g>
                </g>
              );
            })}
          </GraphicLayer>
          <g className="selection-layer" pointerEvents="none">
            {entries.map(({ id, graphic, editable: ed }) => {
              if (sel.selectedId !== id && sel.hoverId !== id) return null;
              const baseGraphic = ed
                ? (optimisticGraphics.get(id) ?? ed.graphic)
                : graphic;
              const previewGraphic =
                resizePreview?.graphicId === id && "extent" in baseGraphic
                  ? { ...baseGraphic, extent: resizePreview.extent }
                  : baseGraphic;
              const bounds = boundsOf(
                applyTransform(previewGraphic, transformFor(id)),
              );
              if (!bounds) return null;
              return (
                <g
                  key={`selection:${id}`}
                  ref={(node) => {
                    if (sel.selectedId === id) {
                      if (node) selectionGroupRefs.current.set(id, node);
                      else selectionGroupRefs.current.delete(id);
                    }
                  }}
                >
                  <rect
                    className={
                      sel.selectedId === id ? "selection-box" : "hover-outline"
                    }
                    x={bounds.x}
                    y={bounds.y}
                    width={bounds.width}
                    height={bounds.height}
                  />
                </g>
              );
            })}
          </g>
        </g>
      </GraphicViewport>
      {editable && (
        <GraphicProperties
          editable={editables.find((ed) => ed.id === sel.selectedId) ?? null}
          onPropertyEdit={handlePropertyEdit}
        />
      )}
      {interactionNotice && (
        <div className="icon-interaction-notice">{interactionNotice}</div>
      )}
    </div>
  );
}

const GraphicLayer = memo(function GraphicLayer({ children }: { children: ReactNode }) {
  recordViewerPerformance("graphicLayerRenders");
  return <g className="modelica-layer">{children}</g>;
});

function handlePosition(
  handle: ResizeHandle,
  bounds: { x: number; y: number; width: number; height: number },
) {
  const cx = bounds.x + bounds.width / 2;
  const cy = bounds.y + bounds.height / 2;
  const x = handle.includes("w")
    ? bounds.x
    : handle.includes("e")
      ? bounds.x + bounds.width
      : cx;
  const y = handle.includes("s")
    ? bounds.y
    : handle.includes("n")
      ? bounds.y + bounds.height
      : cy;
  return { x, y };
}

function formatModelicaNumber(n: number): string {
  return Number.isInteger(n) ? String(n) : String(Number(n.toFixed(6)));
}

/** Serialize a Modelica extent, including both the point and array braces. */
export function formatModelicaExtent(extent: Extent): string {
  return `{{${formatModelicaNumber(extent.p1.x)},${formatModelicaNumber(extent.p1.y)}},{${formatModelicaNumber(extent.p2.x)},${formatModelicaNumber(extent.p2.y)}}}`;
}

function buildExtentEdit(ed: EditableGraphic, extent: Extent): Edit | null {
  if (!ed.source.extentRange) return null;
  return {
    start: ed.source.extentRange.start,
    end: ed.source.extentRange.end,
    expectedText: ed.source.extentRange.expectedText,
    replacement: formatModelicaExtent(extent),
  };
}

function buildGraphicEdit(
  ed: EditableGraphic,
  graphic: EditableGraphic["graphic"],
): Edit | null {
  const g = graphic as any;
  const format = formatModelicaNumber;
  if (ed.source.originRange && g.origin) {
    return {
      start: ed.source.originRange.start,
      end: ed.source.originRange.end,
      expectedText: ed.source.originRange.expectedText,
      replacement: `{${format(g.origin.x)},${format(g.origin.y)}}`,
    };
  }
  if (ed.source.extentRange && g.extent) {
    return {
      start: ed.source.extentRange.start,
      end: ed.source.extentRange.end,
      expectedText: ed.source.extentRange.expectedText,
      replacement: formatModelicaExtent(g.extent),
    };
  }
  if (ed.source.pointsRange && g.points) {
    return {
      start: ed.source.pointsRange.start,
      end: ed.source.pointsRange.end,
      expectedText: ed.source.pointsRange.expectedText,
      replacement: `{${g.points.map((p: { x: number; y: number }) => `{${format(p.x)},${format(p.y)}}`).join(",")}}`,
    };
  }
  return null;
}

function buildTranslateEdit(
  ed: EditableGraphic,
  graphic: EditableGraphic["graphic"],
  dx: number,
  dy: number,
): Edit | null {
  const source = ed.source;
  const g = graphic as any;
  const format = formatModelicaNumber;
  if (source.originRange && g.origin) {
    return {
      start: source.originRange.start,
      end: source.originRange.end,
      expectedText: source.originRange.expectedText,
      replacement: `{${format(g.origin.x + dx)},${format(g.origin.y + dy)}}`,
    };
  }
  if (source.extentRange && g.extent) {
    const e = g.extent;
    return {
      start: source.extentRange.start,
      end: source.extentRange.end,
      expectedText: source.extentRange.expectedText,
      replacement: formatModelicaExtent({
        p1: { x: e.p1.x + dx, y: e.p1.y + dy },
        p2: { x: e.p2.x + dx, y: e.p2.y + dy },
      }),
    };
  }
  if (source.pointsRange && g.points) {
    return {
      start: source.pointsRange.start,
      end: source.pointsRange.end,
      expectedText: source.pointsRange.expectedText,
      replacement: `{${g.points.map((p: { x: number; y: number }) => `{${format(p.x + dx)},${format(p.y + dy)}}`).join(",")}}`,
    };
  }
  return null;
}

const linePatterns = [
  "LinePattern.Solid",
  "LinePattern.Dash",
  "LinePattern.Dot",
  "LinePattern.DashDot",
  "LinePattern.DashDotDot",
  "LinePattern.None",
];
const fillPatterns = [
  "FillPattern.None",
  "FillPattern.Solid",
  "FillPattern.Horizontal",
  "FillPattern.Vertical",
  "FillPattern.Cross",
  "FillPattern.Forward",
  "FillPattern.Backward",
  "FillPattern.CrossDiag",
  "FillPattern.HorizontalCylinder",
  "FillPattern.VerticalCylinder",
  "FillPattern.Sphere",
];

function colorHex(color?: [number, number, number]): string {
  return color
    ? `#${color.map((part) => part.toString(16).padStart(2, "0")).join("")}`
    : "#000000";
}

function parseHex(hex: string): [number, number, number] {
  return [1, 3, 5].map((offset) =>
    parseInt(hex.slice(offset, offset + 2), 16),
  ) as [number, number, number];
}

function GraphicProperties({
  editable,
  onPropertyEdit,
}: {
  editable: EditableGraphic | null;
  onPropertyEdit?: (edit: Edit, after: EditableGraphic["graphic"]) => void;
}) {
  if (!editable || !onPropertyEdit)
    return (
      <aside className="graphic-properties empty-properties">
        选择图元查看属性
      </aside>
    );
  const graphic = editable.graphic;
  const source = editable.source;
  const patch = (
    range: { start: number; end: number; expectedText?: string } | undefined,
    value: string,
    after: EditableGraphic["graphic"],
  ) => {
    if (!range) return;
    onPropertyEdit(
      {
        start: range.start,
        end: range.end,
        expectedText: range.expectedText,
        replacement: value,
      },
      after,
    );
  };
  const line =
    graphic.type === "Line"
      ? graphic.color
      : graphic.type === "Text"
        ? graphic.textColor
        : graphic.lineColor;
  const fill =
    graphic.type === "Line" || graphic.type === "Text"
      ? undefined
      : graphic.fillColor;
  const lineProperty =
    graphic.type === "Line"
      ? "color"
      : graphic.type === "Text"
        ? "textColor"
        : "lineColor";
  const lineRange =
    graphic.type === "Line"
      ? source.colorRange
      : graphic.type === "Text"
        ? source.textColorRange
        : source.lineColorRange;
  const pattern =
    graphic.type === "Line"
      ? (graphic.pattern ?? "LinePattern.Solid")
      : "LinePattern.Solid";
  const fillPattern =
    graphic.type === "Line" || graphic.type === "Text"
      ? "FillPattern.None"
      : (graphic.fillPattern ?? "FillPattern.None");
  const thickness =
    graphic.type === "Line"
      ? graphic.thickness
      : graphic.type === "Text"
        ? undefined
        : graphic.lineThickness;
  const thicknessName = graphic.type === "Line" ? "thickness" : "lineThickness";
  const thicknessRange =
    graphic.type === "Line" ? source.thicknessRange : source.lineThicknessRange;
  return (
    <aside className="graphic-properties">
      <h3>Selected Graphic</h3>
      <label>
        Type <strong>{graphic.type}</strong>
      </label>
      <label>
        Line Color{" "}
        <input
          type="color"
          value={colorHex(line)}
          onChange={(e) => {
            const value = parseHex(e.target.value);
            patch(lineRange, `{${value.join(",")}}`, {
              ...graphic,
              [lineProperty]: value,
            } as EditableGraphic["graphic"]);
          }}
        />
      </label>
      <label>
        Line Style{" "}
        <select
          value={pattern}
          onChange={(e) =>
            patch(source.patternRange, e.target.value, {
              ...graphic,
              pattern: e.target.value,
            } as EditableGraphic["graphic"])
          }
        >
          {linePatterns.map((value) => (
            <option key={value}>{value}</option>
          ))}
        </select>
      </label>
      <label>
        Line Thickness{" "}
        <input
          type="number"
          min="0"
          step="0.5"
          value={thickness ?? 1}
          onChange={(e) => {
            const value = Number(e.target.value);
            patch(thicknessRange, e.target.value, {
              ...graphic,
              [thicknessName]: value,
            } as EditableGraphic["graphic"]);
          }}
        />
      </label>
      {graphic.type !== "Line" && graphic.type !== "Text" && (
        <>
          <label>
            Fill Color{" "}
            <input
              type="color"
              value={colorHex(fill)}
              onChange={(e) => {
                const value = parseHex(e.target.value);
                patch(source.fillColorRange, `{${value.join(",")}}`, {
                  ...graphic,
                  fillColor: value,
                });
              }}
            />
          </label>
          <label>
            Fill Style{" "}
            <select
              value={fillPattern}
              onChange={(e) =>
                patch(source.fillPatternRange, e.target.value, {
                  ...graphic,
                  fillPattern: e.target.value,
                } as EditableGraphic["graphic"])
              }
            >
              {fillPatterns.map((value) => (
                <option key={value}>{value}</option>
              ))}
            </select>
          </label>
        </>
      )}
    </aside>
  );
}
