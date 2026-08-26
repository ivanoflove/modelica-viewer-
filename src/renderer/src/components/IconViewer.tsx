import {
  memo,
  useEffect,
  useRef,
  useState,
  type MouseEvent,
  type PointerEvent,
  type ReactNode,
} from "react";
import type {
  IconDto,
  EditableIconDto,
  EditableGraphic,
  GraphicTransform,
  Extent,
  LineDto,
  PolygonDto,
  Point,
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
import {
  LINE_HIT_WIDTH_PX,
  VERTEX_HIT_RADIUS_PX,
  VERTEX_VISUAL_RADIUS_PX,
  moveVertex,
  serializeModelicaPoints,
} from "../../editor/VertexEditor";
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

type DeleteEdit = Edit & { deletedText: string };

interface Props {
  icon: IconDto | null;
  editable?: EditableIconDto | null;
  modelName: string;
  resetKey?: string;
  sourceText?: string;
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

interface VertexDragSession {
  graphicId: string;
  vertexIndex: number;
  pointerId: number;
  originalGraphic: LineDto | PolygonDto;
  originalPoints: Point[];
  startPointerLocal: Point;
  inverseScreenToLocal: DOMMatrix;
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
  sourceText = "",
  onEdit,
}: Props) {
  const svgRef = useRef<SVGSVGElement>(null);
  const dragRef = useRef<DragSession | null>(null);
  const viewportGroupRef = useRef<SVGGElement>(null);
  const graphicGroupRefs = useRef(new Map<string, SVGGElement>());
  const hitGraphicGroupRefs = useRef(new Map<string, SVGGElement>());
  const localGroupRefs = useRef(new Map<string, SVGGElement>());
  const selectionGroupRefs = useRef(new Map<string, SVGGElement>());
  const screenOverlayRef = useRef<SVGGElement>(null);
  const screenOverlayUpdateRef = useRef<() => void>(() => undefined);
  const resizeRef = useRef<ResizeSession | null>(null);
  const vertexRef = useRef<VertexDragSession | null>(null);
  const pendingPointRef = useRef<{ x: number; y: number } | null>(null);
  const rafRef = useRef<number | null>(null);
  const pendingResizeRef = useRef<{
    point: { x: number; y: number };
    shift: boolean;
    alt: boolean;
  } | null>(null);
  const resizeRafRef = useRef<number | null>(null);
  const vertexRafRef = useRef<number | null>(null);
  const pendingVertexPointRef = useRef<Point | null>(null);
  const historyRef = useRef(new HistoryManager(100));
  const historyBusyRef = useRef(false);
  const [resizePreview, setResizePreview] = useState<{
    graphicId: string;
    extent: Extent;
  } | null>(null);
  const [vertexSelection, setVertexSelection] = useState<{
    graphicId: string;
    vertexIndex: number;
  } | null>(null);
  const [hiddenGraphicIds, setHiddenGraphicIds] = useState<Set<string>>(
    () => new Set(),
  );
  const [contextMenu, setContextMenu] = useState<{
    graphicId: string;
    x: number;
    y: number;
  } | null>(null);
  const [propertiesGraphicId, setPropertiesGraphicId] = useState<string | null>(
    null,
  );
  const shellRef = useRef<HTMLDivElement>(null);
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
    setHiddenGraphicIds(new Set());
    setContextMenu(null);
    setPropertiesGraphicId(null);
  }, [editable]);

  useEffect(() => {
    if (rafRef.current !== null) cancelAnimationFrame(rafRef.current);
    if (resizeRafRef.current !== null)
      cancelAnimationFrame(resizeRafRef.current);
    if (vertexRafRef.current !== null)
      cancelAnimationFrame(vertexRafRef.current);
    dragRef.current = null;
    resizeRef.current = null;
    vertexRef.current = null;
    pendingPointRef.current = null;
    pendingResizeRef.current = null;
    rafRef.current = null;
    resizeRafRef.current = null;
    vertexRafRef.current = null;
    pendingVertexPointRef.current = null;
    historyRef.current = new HistoryManager(100);
    setResizePreview(null);
    setVertexSelection(null);
    setInteractionNotice(null);
    setHistoryVersion((version) => version + 1);
  }, [resetKey]);

  useEffect(() => {
    screenOverlayUpdateRef.current();
  }, [sel.selectedId, icon, editable, optimisticGraphics, resizePreview, resetKey]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setContextMenu(null);
        setPropertiesGraphicId(null);
      }
    };
    const onPointerDown = () => setContextMenu(null);
    window.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);

  if (!icon) return <div className="no-icon">No Icon annotation</div>;

  const handleCanvasPointerDown = (e: PointerEvent<SVGSVGElement>) => {
    if (e.button !== 0 || e.target !== e.currentTarget) return;
    sel.setSelected(null);
    setPropertiesGraphicId(null);
    setVertexSelection(null);
    setInteractionNotice(null);
    closeContextMenu();
  };

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
  }).filter(({ id }) => !hiddenGraphicIds.has(id));

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
    const hitGraphicGroup = hitGraphicGroupRefs.current.get(session.id);
    const selectionGroup = selectionGroupRefs.current.get(session.id);
    const transform = `translate(${delta.x},${delta.y})`;
    graphicGroup?.setAttribute("transform", transform);
    hitGraphicGroup?.setAttribute("transform", transform);
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
      hitGraphicGroupRefs.current.get(session.id)?.removeAttribute("transform");
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

  const pointShape = (group: SVGGElement | undefined) =>
    group?.querySelector<SVGPolylineElement | SVGPolygonElement>(
      "polyline, polygon",
    );

  const setPointShapePoints = (graphicId: string, points: Point[]) => {
    const value = points.map((point) => `${point.x},${point.y}`).join(" ");
    pointShape(graphicGroupRefs.current.get(graphicId))?.setAttribute(
      "points",
      value,
    );
    pointShape(hitGraphicGroupRefs.current.get(graphicId))?.setAttribute(
      "points",
      value,
    );
  };

  const updateVertexHandle = (session: VertexDragSession, point: Point) => {
    const svg = svgRef.current;
    const localGroup = localGroupRefs.current.get(session.graphicId);
    const rootInverse = svg?.getScreenCTM()?.inverse();
    const localMatrix = localGroup?.getScreenCTM();
    if (!svg || !rootInverse || !localMatrix) return;
    const rootPoint = new DOMPoint(point.x, point.y)
      .matrixTransform(localMatrix)
      .matrixTransform(rootInverse);
    const rootMatrix = svg.getScreenCTM();
    const rootScale = Math.max(
      rootMatrix ? Math.hypot(rootMatrix.a, rootMatrix.b) : 1,
      1e-6,
    );
    const handle = screenOverlayRef.current?.querySelector<SVGCircleElement>(
      `[data-vertex-handle="${session.vertexIndex}"] .vertex-handle`,
    );
    const hit = screenOverlayRef.current?.querySelector<SVGCircleElement>(
      `[data-vertex-handle="${session.vertexIndex}"] .vertex-hit-target`,
    );
    handle?.setAttribute("cx", String(rootPoint.x));
    handle?.setAttribute("cy", String(rootPoint.y));
    handle?.setAttribute("r", String(4 / rootScale));
    hit?.setAttribute("cx", String(rootPoint.x));
    hit?.setAttribute("cy", String(rootPoint.y));
    hit?.setAttribute("r", String(9 / rootScale));
  };

  const applyVertexPreview = (
    session: VertexDragSession,
    point: Point,
  ) => {
    const dx = point.x - session.startPointerLocal.x;
    const dy = point.y - session.startPointerLocal.y;
    const next = moveVertex(
      session.originalPoints,
      session.vertexIndex,
      dx,
      dy,
      session.originalGraphic.type === "Polygon",
    );
    setPointShapePoints(session.graphicId, next);
    updateVertexHandle(session, next[session.vertexIndex] ?? point);
    recordViewerPerformance("vertexPreviewRafUpdates");
  };

  const scheduleVertexPreview = (point: Point) => {
    pendingVertexPointRef.current = point;
    if (vertexRafRef.current !== null) return;
    vertexRafRef.current = requestAnimationFrame(() => {
      vertexRafRef.current = null;
      const session = vertexRef.current;
      const pending = pendingVertexPointRef.current;
      if (session && pending) applyVertexPreview(session, pending);
    });
  };

  const clearVertex = (restore = true) => {
    if (vertexRafRef.current !== null)
      cancelAnimationFrame(vertexRafRef.current);
    const session = vertexRef.current;
    if (restore && session) {
      setPointShapePoints(session.graphicId, session.originalPoints);
    }
    vertexRafRef.current = null;
    pendingVertexPointRef.current = null;
    vertexRef.current = null;
    setVertexSelection(null);
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
      const vertex = vertexRef.current;
      if (drag) releasePointer(drag.pointerId);
      if (resize) releasePointer(resize.pointerId);
      if (vertex) releasePointer(vertex.pointerId);
      clearDrag();
      clearResize();
      clearVertex();
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
    const ed = editables.find((item) => item.id === propertiesGraphicId);
    if (!ed) return;
    const before = optimisticGraphics.get(ed.id) ?? ed.graphic;
    commitGraphicChange(ed, before, after, edit, "property");
  };

  const handleGraphicContextMenu = (
    e: MouseEvent,
    id: string,
  ) => {
    e.preventDefault();
    e.stopPropagation();
    sel.setSelected(id);
    setVertexSelection(null);
    const shell = shellRef.current?.getBoundingClientRect();
    setContextMenu({
      graphicId: id,
      x: e.clientX - (shell?.left ?? 0),
      y: e.clientY - (shell?.top ?? 0),
    });
  };

  const closeContextMenu = () => setContextMenu(null);

  const handleDeleteGraphic = () => {
    const id = contextMenu?.graphicId;
    const ed = editables.find((item) => item.id === id);
    if (!id || !ed) {
      setInteractionNotice("继承图形不能在当前类删除");
      closeContextMenu();
      return;
    }
    if (!onEdit) {
      setInteractionNotice("当前文件不可编辑");
      closeContextMenu();
      return;
    }
    if (ed.inherited) {
      setInteractionNotice(
        `继承图形不能在当前类删除：${ed.ownerQualifiedName ?? "基类"}`,
      );
      closeContextMenu();
      return;
    }
    const edit = buildDeleteEdit(ed, sourceText);
    if (!edit) {
      setInteractionNotice("无法安全定位该图元，已取消删除");
      closeContextMenu();
      return;
    }
    const before = optimisticGraphics.get(ed.id) ?? ed.graphic;
    setHiddenGraphicIds((current) => new Set(current).add(ed.id));
    closeContextMenu();
    void onEdit(edit, "delete")
      .then((success) => {
        if (!success) {
          setHiddenGraphicIds((current) => {
            const next = new Set(current);
            next.delete(ed.id);
            return next;
          });
          return;
        }
        historyRef.current.push({
          graphicId: ed.id,
          before,
          after: null,
          deletion: { start: edit.start, deletedText: edit.deletedText },
        });
        sel.setSelected(null);
        setPropertiesGraphicId(null);
        setVertexSelection(null);
        setHistoryVersion((version) => version + 1);
      })
      .catch((error: unknown) => {
        setHiddenGraphicIds((current) => {
          const next = new Set(current);
          next.delete(ed.id);
          return next;
        });
        setInteractionNotice(
          `删除未保存：${error instanceof Error ? error.message : "未知错误"}`,
        );
      });
  };

  const handlePointerDown = (e: React.PointerEvent, ed: EditableGraphic) => {
    if (e.button !== 0) return;
    sel.setSelected(ed.id);
    setPropertiesGraphicId(null);
    setVertexSelection(null);
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
    setPropertiesGraphicId(null);
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

  const handleVertexDown = (
    e: React.PointerEvent,
    ed: EditableGraphic,
    vertexIndex: number,
  ) => {
    const graphic = optimisticGraphics.get(ed.id) ?? ed.graphic;
    if (!isPointBasedGraphic(graphic) || !ed.source.pointsRange) return;
    const localGroup = localGroupRefs.current.get(ed.id);
    const inverse = localGroup?.getScreenCTM()?.inverse();
    if (!inverse || !graphic.points[vertexIndex]) return;
    e.preventDefault();
    e.stopPropagation();
    const point = new DOMPoint(e.clientX, e.clientY).matrixTransform(inverse);
    setVertexSelection({ graphicId: ed.id, vertexIndex });
    setPropertiesGraphicId(null);
    setInteractionNotice(null);
    capturePointer(e.pointerId);
    vertexRef.current = {
      graphicId: ed.id,
      vertexIndex,
      pointerId: e.pointerId,
      originalGraphic: graphic,
      originalPoints: graphic.points.map((item) => ({ ...item })),
      startPointerLocal: { x: point.x, y: point.y },
      inverseScreenToLocal: inverse,
    };
  };

  const handleVertexMove = (e: React.PointerEvent) => {
    const session = vertexRef.current;
    if (!session || session.pointerId !== e.pointerId) return;
    e.preventDefault();
    e.stopPropagation();
    const point = new DOMPoint(e.clientX, e.clientY).matrixTransform(
      session.inverseScreenToLocal,
    );
    scheduleVertexPreview({ x: point.x, y: point.y });
  };

  const handleVertexUp = (e: React.PointerEvent) => {
    const session = vertexRef.current;
    if (!session || session.pointerId !== e.pointerId) return;
    e.preventDefault();
    e.stopPropagation();
    const point = new DOMPoint(e.clientX, e.clientY).matrixTransform(
      session.inverseScreenToLocal,
    );
    const ed = editables.find((item) => item.id === session.graphicId);
    if (ed) {
      const next = moveVertex(
        session.originalPoints,
        session.vertexIndex,
        point.x - session.startPointerLocal.x,
        point.y - session.startPointerLocal.y,
        session.originalGraphic.type === "Polygon",
      );
      const edit = buildPointsEdit(ed, next);
      if (edit) {
        releasePointer(e.pointerId);
        clearVertex(false);
        commitGraphicChange(
          ed,
          session.originalGraphic,
          { ...session.originalGraphic, points: next },
          edit,
          "vertex",
        );
        return;
      }
    }
    releasePointer(e.pointerId);
    clearVertex();
  };

  const handleVertexCancel = (e: React.PointerEvent) => {
    if (!vertexRef.current) return;
    e.preventDefault();
    e.stopPropagation();
    releasePointer(e.pointerId);
    clearVertex();
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
    setPropertiesGraphicId(null);
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

  const applyHistory = async (direction: "undo" | "redo") => {
    if (historyBusyRef.current || !onEdit) return;
    const command =
      direction === "undo"
        ? historyRef.current.peekUndo()
        : historyRef.current.peekRedo();
    if (!command) return;
    if (command.deletion) {
      const { start, deletedText } = command.deletion;
      const edit: Edit =
        direction === "undo"
          ? {
              start,
              end: start,
              expectedText: "",
              replacement: deletedText,
            }
          : {
              start,
              end: start + deletedText.length,
              expectedText: deletedText,
              replacement: "",
            };
      historyBusyRef.current = true;
      try {
        const success = await onEdit(edit, direction);
        if (success) {
          if (direction === "undo") historyRef.current.acceptUndo();
          else historyRef.current.acceptRedo();
          setHistoryVersion((version) => version + 1);
        }
      } finally {
        historyBusyRef.current = false;
      }
      return;
    }
    const ed = editables.find((item) => item.id === command.graphicId);
    if (!ed) return;
    const target = direction === "undo" ? command.before : command.after;
    if (!target) return;
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
    clearVertex();
  };

  const selectedEntry = entries.find(({ id }) => id === sel.selectedId);
  const selectedEditable = selectedEntry?.editable;
  const selectedGraphic = selectedEntry
    ? selectedEditable
      ? (optimisticGraphics.get(selectedEditable.id) ?? selectedEditable.graphic)
      : selectedEntry.graphic
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
    if (selectedEditable && selectedGraphic && isPointBasedGraphic(selectedGraphic)) {
      const localMatrix = localGroupRefs.current
        .get(selectedEditable.id)
        ?.getScreenCTM();
      const rootInverse = svg.getScreenCTM()?.inverse();
      if (localMatrix && rootInverse) {
        selectedGraphic.points.forEach((point, index) => {
          const rootPoint = new DOMPoint(point.x, point.y)
            .matrixTransform(localMatrix)
            .matrixTransform(rootInverse);
          const visual = overlay.querySelector<SVGCircleElement>(
            `[data-vertex-handle="${index}"] .vertex-handle`,
          );
          const hit = overlay.querySelector<SVGCircleElement>(
            `[data-vertex-handle="${index}"] .vertex-hit-target`,
          );
          visual?.setAttribute("cx", String(rootPoint.x));
          visual?.setAttribute("cy", String(rootPoint.y));
          visual?.setAttribute("r", String(VERTEX_VISUAL_RADIUS_PX / rootScale));
          hit?.setAttribute("cx", String(rootPoint.x));
          hit?.setAttribute("cy", String(rootPoint.y));
          hit?.setAttribute("r", String(VERTEX_HIT_RADIUS_PX / rootScale));
        });
      }
    }
  };

  const resizeOverlay = canResizeSelected && selectedBounds && selectedEditable
    ? resizeHandles.map((handle) => (
        <g key={`screen:${selectedEditable.id}:${handle}`} data-screen-handle={handle}>
          <circle
            className="resize-hit-target"
            cx={0}
            cy={0}
            r={VERTEX_HIT_RADIUS_PX}
            onPointerDown={(e) => handleResizeDown(e, selectedEditable, handle)}
            pointerEvents="all"
          />
          <circle className="resize-handle" cx={0} cy={0} r={5} pointerEvents="none" />
        </g>
      ))
    : null;

  const vertexOverlay = selectedEditable && selectedGraphic && isPointBasedGraphic(selectedGraphic)
    ? selectedGraphic.points.map((_, index) => (
        <g key={`vertex:${selectedEditable.id}:${index}`} data-vertex-handle={index}>
          <circle
            className="vertex-hit-target"
            cx={0}
            cy={0}
            r={9}
            onPointerDown={(e) => handleVertexDown(e, selectedEditable, index)}
            pointerEvents="all"
          />
          <circle
            className={
              vertexSelection?.vertexIndex === index
                ? "vertex-handle active"
                : "vertex-handle"
            }
            cx={0}
            cy={0}
            r={VERTEX_VISUAL_RADIUS_PX}
            pointerEvents="none"
          />
        </g>
      ))
    : null;

  const screenOverlay = <>{resizeOverlay}{vertexOverlay}</>;

  screenOverlayUpdateRef.current = updateScreenOverlay;

  const canUndo = historyVersion >= 0 && historyRef.current.canUndo;
  const canRedo = historyVersion >= 0 && historyRef.current.canRedo;
  const propertiesEntry = entries.find(({ id }) => id === propertiesGraphicId);
  const propertiesEditable = propertiesEntry?.editable;
  const propertiesGraphic = propertiesEntry
    ? propertiesEditable
      ? (optimisticGraphics.get(propertiesEditable.id) ?? propertiesEditable.graphic)
      : propertiesEntry.graphic
    : null;
  const inspectorEditable = propertiesEntry && propertiesGraphic
    ? propertiesEditable
      ? { ...propertiesEditable, graphic: propertiesGraphic }
      : {
          id: propertiesEntry.id,
          graphic: propertiesGraphic,
          ownerQualifiedName: propertiesGraphic.ownerQualifiedName,
          inherited: true,
          selected: true,
          transform: identity,
          source: { itemRange: { start: 0, end: 0 } },
        }
    : null;

  return (
    <div ref={shellRef} className="icon-editor-shell">
      <GraphicViewport
        icon={icon}
        resetKey={resetKey}
        svgRef={svgRef}
        viewportGroupRef={viewportGroupRef}
        screenOverlayRef={screenOverlayRef}
        onViewportTransform={updateScreenOverlay}
        overlay={screenOverlay}
        onCanvasPointerDown={handleCanvasPointerDown}
        onCanvasContextMenu={(event) => {
          event.preventDefault();
          closeContextMenu();
        }}
        onPointerMove={(event) => {
          handleVertexMove(event);
          handlePointerMove(event);
          handleResizeMove(event);
        }}
        onPointerUp={(event) => {
          handleVertexUp(event);
          handlePointerUp(event);
          handleResizeUp(event);
        }}
        onPointerCancel={(event) => {
          handleVertexCancel(event);
          handlePointerCancel(event);
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
                  onContextMenu={(e) => handleGraphicContextMenu(e, id)}
                  onPointerMove={ed ? handlePointerMove : undefined}
                  onPointerUp={ed ? handlePointerUp : undefined}
                  onPointerCancel={ed ? handlePointerCancel : undefined}
                  style={ed ? { cursor: "move" } : undefined}
                >
                  <g transform={ed ? toSvgTransform(transform) : undefined}>
                    <g
                      ref={(node) => {
                        if (node) localGroupRefs.current.set(id, node);
                        else localGroupRefs.current.delete(id);
                      }}
                      transform={
                        graphic.origin
                          ? `translate(${graphic.origin.x},${graphic.origin.y})`
                          : undefined
                      }
                      pointerEvents="none"
                    />
                    <GraphicItem
                      item={renderGraphic}
                      styleId={`graphic-style-${index}`}
                    />
                  </g>
                </g>
              );
            })}
          </GraphicLayer>
          <g className="hit-layer">
            {entries.map(({ id, graphic, editable: ed }) => {
              if (!isPointBasedGraphic(graphic)) return null;
              return (
                <g
                  key={`hit:${id}`}
                  ref={(node) => {
                    if (node) hitGraphicGroupRefs.current.set(id, node);
                    else hitGraphicGroupRefs.current.delete(id);
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
                  onContextMenu={(e) => handleGraphicContextMenu(e, id)}
                  style={ed ? { cursor: "pointer" } : undefined}
                >
                  <g transform={ed ? toSvgTransform(transformFor(id)) : undefined}>
                    <g
                      transform={
                        graphic.origin
                          ? `translate(${graphic.origin.x},${graphic.origin.y})`
                          : undefined
                      }
                    >
                      <GraphicHitTarget item={graphic} />
                    </g>
                  </g>
                </g>
              );
            })}
          </g>
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
      {contextMenu && (
        <div
          className="graphic-context-menu"
          role="menu"
          style={{ left: contextMenu.x, top: contextMenu.y }}
          onPointerDown={(event) => event.stopPropagation()}
        >
          <button
            role="menuitem"
            onClick={() => {
              setPropertiesGraphicId(contextMenu.graphicId);
              closeContextMenu();
            }}
          >
            Properties…
          </button>
          <button
            role="menuitem"
            disabled={
              !editables.some(
                (ed) => ed.id === contextMenu.graphicId && !ed.inherited,
              )
            }
            onClick={handleDeleteGraphic}
          >
            Delete
          </button>
        </div>
      )}
      {inspectorEditable && (
        <GraphicProperties
          editable={inspectorEditable ?? null}
          onPropertyEdit={handlePropertyEdit}
          onClose={() => setPropertiesGraphicId(null)}
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

function isPointBasedGraphic(
  graphic: EditableGraphic["graphic"],
): graphic is LineDto | PolygonDto {
  return graphic.type === "Line" || graphic.type === "Polygon";
}

function pointsAttribute(points: Point[]): string {
  return points.map((point) => `${point.x},${point.y}`).join(" ");
}

function GraphicHitTarget({ item }: { item: LineDto | PolygonDto }) {
  if (item.type === "Line") {
    return (
      <polyline
        className="line-hit-target"
        points={pointsAttribute(item.points)}
        fill="none"
        stroke="transparent"
        strokeWidth={LINE_HIT_WIDTH_PX}
        vectorEffect="non-scaling-stroke"
        pointerEvents="stroke"
      />
    );
  }
  return (
    <polygon
      className="polygon-hit-target"
      points={pointsAttribute(item.points)}
      fill="transparent"
      stroke="transparent"
      strokeWidth={LINE_HIT_WIDTH_PX}
      vectorEffect="non-scaling-stroke"
      pointerEvents="all"
    />
  );
}

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

function buildPointsEdit(ed: EditableGraphic, points: Point[]): Edit | null {
  if (!ed.source.pointsRange) return null;
  return {
    start: ed.source.pointsRange.start,
    end: ed.source.pointsRange.end,
    expectedText: ed.source.pointsRange.expectedText,
    replacement: serializeModelicaPoints(points),
  };
}

export function buildDeleteEdit(
  ed: EditableGraphic,
  sourceText: string,
): DeleteEdit | null {
  if (!sourceText || ed.inherited) return null;
  const { start, end } = ed.source.itemRange;
  if (start < 0 || end <= start || end > sourceText.length) return null;
  const after = sourceText.slice(end);
  const before = sourceText.slice(0, start);
  const followingComma = after.match(/^(\s*,\s*)/)?.[1];
  const precedingComma = before.match(/(,\s*)$/)?.[1];
  const deleteStart = followingComma ? start : precedingComma ? start - precedingComma.length : start;
  const deleteEnd = followingComma ? end + followingComma.length : end;
  const deletedText = sourceText.slice(deleteStart, deleteEnd);
  return {
    start: deleteStart,
    end: deleteEnd,
    expectedText: deletedText,
    replacement: "",
    deletedText,
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

function exactBounds(graphic: EditableGraphic["graphic"]): Extent {
  if ("extent" in graphic) return graphic.extent;
  const xs = graphic.points.map((point) => point.x);
  const ys = graphic.points.map((point) => point.y);
  return {
    p1: { x: Math.min(...xs), y: Math.min(...ys) },
    p2: { x: Math.max(...xs), y: Math.max(...ys) },
  };
}

function graphicPosition(graphic: EditableGraphic["graphic"]): Point {
  if (graphic.origin) return graphic.origin;
  const bounds = exactBounds(graphic);
  return {
    x: (bounds.p1.x + bounds.p2.x) / 2,
    y: (bounds.p1.y + bounds.p2.y) / 2,
  };
}

function translateGraphic(
  graphic: EditableGraphic["graphic"],
  dx: number,
  dy: number,
): EditableGraphic["graphic"] {
  if (graphic.origin) return { ...graphic, origin: { x: graphic.origin.x + dx, y: graphic.origin.y + dy } } as EditableGraphic["graphic"];
  if ("extent" in graphic) {
    return {
      ...graphic,
      extent: {
        p1: { x: graphic.extent.p1.x + dx, y: graphic.extent.p1.y + dy },
        p2: { x: graphic.extent.p2.x + dx, y: graphic.extent.p2.y + dy },
      },
    };
  }
  return {
    ...graphic,
    points: graphic.points.map((point) => ({ x: point.x + dx, y: point.y + dy })),
  };
}

function resizeGraphic(
  graphic: EditableGraphic["graphic"],
  width: number,
  height: number,
): EditableGraphic["graphic"] {
  const bounds = exactBounds(graphic);
  const oldWidth = Math.max(bounds.p2.x - bounds.p1.x, 1e-9);
  const oldHeight = Math.max(bounds.p2.y - bounds.p1.y, 1e-9);
  const cx = (bounds.p1.x + bounds.p2.x) / 2;
  const cy = (bounds.p1.y + bounds.p2.y) / 2;
  const sx = width / oldWidth;
  const sy = height / oldHeight;
  if ("extent" in graphic) {
    return {
      ...graphic,
      extent: {
        p1: { x: cx - width / 2, y: cy - height / 2 },
        p2: { x: cx + width / 2, y: cy + height / 2 },
      },
    };
  }
  return {
    ...graphic,
    points: graphic.points.map((point) => ({
      x: cx + (point.x - cx) * sx,
      y: cy + (point.y - cy) * sy,
    })),
  };
}

function NumericProperty({
  label,
  value,
  disabled,
  onCommit,
}: {
  label: string;
  value: number;
  disabled?: boolean;
  onCommit: (value: number) => void;
}) {
  const [draft, setDraft] = useState(String(value));
  useEffect(() => setDraft(String(value)), [value]);
  const commit = () => {
    const next = Number(draft);
    if (Number.isFinite(next)) onCommit(next);
    else setDraft(String(value));
  };
  return (
    <label>
      {label}
      <input
        type="number"
        min={label === "Width" || label === "Height" ? "0" : undefined}
        step="0.25"
        value={draft}
        disabled={disabled}
        onChange={(event) => setDraft(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            commit();
            event.currentTarget.blur();
          }
        }}
      />
    </label>
  );
}

function GraphicProperties({
  editable,
  onPropertyEdit,
  onClose,
}: {
  editable: EditableGraphic | null;
  onPropertyEdit?: (edit: Edit, after: EditableGraphic["graphic"]) => void;
  onClose: () => void;
}) {
  if (!editable || !onPropertyEdit) return null;
  const graphic = editable.graphic;
  const source = editable.source;
  const readOnly = !!editable.inherited;
  const patch = (
    range: { start: number; end: number; expectedText?: string } | undefined,
    value: string,
    after: EditableGraphic["graphic"],
  ) => {
    if (!range || readOnly) return;
    onPropertyEdit({ start: range.start, end: range.end, expectedText: range.expectedText, replacement: value }, after);
  };
  const position = graphicPosition(graphic);
  const bounds = exactBounds(graphic);
  const width = Math.abs(bounds.p2.x - bounds.p1.x);
  const height = Math.abs(bounds.p2.y - bounds.p1.y);
  const positionRange = source.originRange;
  const extentRange = source.extentRange;
  const pointsRange = source.pointsRange;
  const positionValue = (axis: "x" | "y", value: number) => {
    const delta = value - position[axis];
    const next = translateGraphic(graphic, axis === "x" ? delta : 0, axis === "y" ? delta : 0);
    const range = graphic.origin && positionRange ? positionRange : "extent" in graphic ? extentRange : pointsRange;
    if (graphic.origin && positionRange) {
      patch(positionRange, `{${formatModelicaNumber(next.origin!.x)},${formatModelicaNumber(next.origin!.y)}}`, next);
    } else if ("extent" in next && extentRange) {
      patch(extentRange, formatModelicaExtent(next.extent), next);
    } else if ("points" in next && pointsRange) {
      patch(pointsRange, serializeModelicaPoints(next.points), next);
    } else if (!range) return;
  };
  const sizeValue = (axis: "width" | "height", value: number) => {
    const next = resizeGraphic(graphic, axis === "width" ? value : width, axis === "height" ? value : height);
    if ("extent" in next && extentRange) patch(extentRange, formatModelicaExtent(next.extent), next);
    else if ("points" in next && pointsRange) patch(pointsRange, serializeModelicaPoints(next.points), next);
  };
  const line = graphic.type === "Line" ? graphic.color : graphic.type === "Text" ? graphic.textColor : graphic.lineColor;
  const lineRange = graphic.type === "Line" ? (source.colorRange ?? source.lineColorRange) : graphic.type === "Text" ? source.textColorRange : source.lineColorRange;
  const lineProperty = graphic.type === "Line" ? "color" : graphic.type === "Text" ? "textColor" : "lineColor";
  const fill = graphic.type === "Line" || graphic.type === "Text" ? undefined : graphic.fillColor;
  const thickness = graphic.type === "Line" ? graphic.thickness : graphic.type === "Text" ? undefined : graphic.lineThickness;
  const thicknessRange = graphic.type === "Line" ? source.thicknessRange : source.lineThicknessRange;
  const pattern = graphic.type === "Text" ? "LinePattern.Solid" : (graphic.pattern ?? "LinePattern.Solid");
  const fillPattern = "fillPattern" in graphic ? (graphic.fillPattern ?? "FillPattern.None") : "FillPattern.None";
  return (
    <aside className="graphic-properties floating-inspector" data-graphic-properties>
      <div className="properties-heading"><div><span className="properties-kicker">GRAPHIC PROPERTIES</span><h3>{graphic.type}</h3></div><div className="properties-heading-actions"><span className={readOnly ? "property-badge inherited" : "property-badge"}>{readOnly ? "Inherited" : "Own"}</span><button className="properties-close" type="button" aria-label="Close properties" onClick={onClose}>×</button></div></div>
      {readOnly && <p className="property-note">来自 {editable.ownerQualifiedName ?? "基类"}，当前类不可编辑。</p>}
      <PropertySection title="Geometry">
        <NumericProperty label={graphic.origin ? "Origin X" : "X"} value={position.x} disabled={readOnly || (!positionRange && !extentRange && !pointsRange)} onCommit={(value) => positionValue("x", value)} />
        <NumericProperty label={graphic.origin ? "Origin Y" : "Y"} value={position.y} disabled={readOnly || (!positionRange && !extentRange && !pointsRange)} onCommit={(value) => positionValue("y", value)} />
        <NumericProperty label="Width" value={width} disabled={readOnly || (!extentRange && !pointsRange)} onCommit={(value) => sizeValue("width", Math.max(0, value))} />
        <NumericProperty label="Height" value={height} disabled={readOnly || (!extentRange && !pointsRange)} onCommit={(value) => sizeValue("height", Math.max(0, value))} />
        <label>Rotation <input className="property-disabled" type="number" value="0" disabled title="Add rotation is not yet supported" readOnly /></label>
        {"points" in graphic && <div className="points-list"><span>Points</span><code>{serializeModelicaPoints(graphic.points)}</code></div>}
      </PropertySection>
      <PropertySection title="Line">
        <label>Color <input type="color" value={colorHex(line)} disabled={readOnly || !lineRange} onChange={(event) => { const value = parseHex(event.target.value); patch(lineRange, `{${value.join(",")}}`, { ...graphic, [lineProperty]: value } as EditableGraphic["graphic"]); }} /></label>
        <label>Style <select value={pattern} disabled={readOnly || !source.patternRange} onChange={(event) => patch(source.patternRange, event.target.value, { ...graphic, pattern: event.target.value } as EditableGraphic["graphic"])}>{linePatterns.map((value) => <option key={value} value={value}>{value.replace("LinePattern.", "")}</option>)}</select></label>
        {thickness !== undefined && <NumericProperty label="Thickness" value={thickness} disabled={readOnly || !thicknessRange} onCommit={(value) => patch(thicknessRange, formatModelicaNumber(Math.max(0, value)), { ...graphic, [graphic.type === "Line" ? "thickness" : "lineThickness"]: Math.max(0, value) } as EditableGraphic["graphic"])} />}
      </PropertySection>
      {(graphic.type === "Rectangle" || graphic.type === "Ellipse" || graphic.type === "Polygon") && <PropertySection title="Fill"><label>Color <input type="color" value={colorHex(fill)} disabled={readOnly || !source.fillColorRange} onChange={(event) => { const value = parseHex(event.target.value); patch(source.fillColorRange, `{${value.join(",")}}`, { ...graphic, fillColor: value }); }} /></label><label>Style <select value={fillPattern} disabled={readOnly || !source.fillPatternRange} onChange={(event) => patch(source.fillPatternRange, event.target.value, { ...graphic, fillPattern: event.target.value } as EditableGraphic["graphic"])}>{fillPatterns.map((value) => <option key={value} value={value}>{value.replace("FillPattern.", "")}</option>)}</select></label></PropertySection>}
      {graphic.type === "Text" && <PropertySection title="Text"><TextProperty value={graphic.textString} disabled={readOnly || !source.textStringRange} onCommit={(value) => patch(source.textStringRange, JSON.stringify(value), { ...graphic, textString: value })} /><NumericProperty label="Font Size" value={graphic.fontSize ?? 12} disabled={readOnly || !source.fontSizeRange} onCommit={(value) => patch(source.fontSizeRange, formatModelicaNumber(Math.max(1, value)), { ...graphic, fontSize: Math.max(1, value) })} /><div className="text-style-options">{["TextStyle.Bold", "TextStyle.Italic", "TextStyle.UnderLine"].map((style) => <label key={style}><input type="checkbox" checked={graphic.textStyle?.includes(style) ?? false} disabled={readOnly || !source.textStyleRange} onChange={(event) => { const styles = new Set(graphic.textStyle ?? []); if (event.target.checked) styles.add(style); else styles.delete(style); const list = [...styles]; patch(source.textStyleRange, `{${list.join(",")}}`, { ...graphic, textStyle: list }); }} />{style.replace("TextStyle.", "")}</label>)}</div></PropertySection>}
    </aside>
  );
}

function PropertySection({ title, children }: { title: string; children: ReactNode }) {
  return <section className="property-section"><h4>{title}</h4>{children}</section>;
}

function TextProperty({ value, disabled, onCommit }: { value: string; disabled?: boolean; onCommit: (value: string) => void }) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  const commit = () => onCommit(draft);
  return <label>String <input className="property-text" value={draft} disabled={disabled} onChange={(event) => setDraft(event.target.value)} onBlur={commit} onKeyDown={(event) => { if (event.key === "Enter") { commit(); event.currentTarget.blur(); } }} /></label>;
}
