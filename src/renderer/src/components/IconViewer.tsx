import { useRef, useState } from "react";
import type { IconDto } from "../../../shared/modelicaGraphics";
import type { EditableIconDto } from "../../../shared/modelicaGraphics";
import { GraphicItem } from "./GraphicItem";

interface Props {
  icon: IconDto | null;
  editable?: EditableIconDto | null;
  modelName: string;
  onEdit?: (edit: { start: number; end: number; replacement: string }) => void;
}

interface DragState {
  id: string;
  pointerStart: { x: number; y: number };
  originalExtent?: {
    p1: { x: number; y: number };
    p2: { x: number; y: number };
  };
  originalPoints?: { x: number; y: number }[];
  originalOrigin?: { x: number; y: number };
  type: string;
}

function formatNumber(n: number): string {
  if (Number.isInteger(n)) return String(n);
  const fixed = Number(n.toFixed(6));
  return String(fixed);
}

function serializeExtent(extent: {
  p1: { x: number; y: number };
  p2: { x: number; y: number };
}): string {
  return `{{${formatNumber(extent.p1.x)},${formatNumber(extent.p1.y)}},{${formatNumber(extent.p2.x)},${formatNumber(extent.p2.y)}}}`;
}

function serializePoints(points: { x: number; y: number }[]): string {
  return `{${points.map((p) => `{${formatNumber(p.x)},${formatNumber(p.y)}}`).join(",")}}`;
}

function serializeOrigin(origin: { x: number; y: number }): string {
  return `{${formatNumber(origin.x)},${formatNumber(origin.y)}}`;
}

function clientToSvg(svg: SVGSVGElement, clientX: number, clientY: number) {
  const pt = svg.createSVGPoint();
  pt.x = clientX;
  pt.y = clientY;
  const ctm = svg.getScreenCTM()?.inverse();
  if (!ctm) return null;
  const p = pt.matrixTransform(ctm);
  return { x: p.x, y: -p.y };
}

export function IconViewer({ icon, editable, modelName, onEdit }: Props) {
  const svgRef = useRef<SVGSVGElement>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [dragState, setDragState] = useState<DragState | null>(null);
  const [previewMap, setPreviewMap] = useState<Map<string, any>>(new Map());

  const displayIcon = icon;
  const editables = editable?.editables ?? [];

  if (!displayIcon) {
    return <div className="no-icon">No Icon annotation</div>;
  }

  const extent = displayIcon.coordinateSystem.extent;
  const x1 = extent.p1.x;
  const y1 = extent.p1.y;
  const x2 = extent.p2.x;
  const y2 = extent.p2.y;
  const minX = Math.min(x1, x2);
  const minY = Math.min(y1, y2);
  const width = Math.abs(x2 - x1);
  const height = Math.abs(y2 - y1);
  const viewBox = `${minX} ${minY} ${width} ${height}`;

  void modelName;

  const handlePointerDown = (
    e: React.PointerEvent,
    id: string,
    type: string,
  ) => {
    const svg = svgRef.current;
    if (!svg || !editable) return;
    const editableItem = editables.find((ed) => ed.id === id);
    if (!editableItem) return;
    const pt = clientToSvg(svg, e.clientX, e.clientY);
    if (!pt) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    setSelectedId(id);
    // store original
    const graphic: any = editableItem.graphic;
    const state: DragState = {
      id,
      pointerStart: pt,
      type,
    };
    if (graphic.extent)
      state.originalExtent = {
        p1: { ...graphic.extent.p1 },
        p2: { ...graphic.extent.p2 },
      };
    if (graphic.points)
      state.originalPoints = graphic.points.map((p: any) => ({ ...p }));
    if (graphic.origin) state.originalOrigin = { ...graphic.origin };
    // also check if has origin in source
    const src = editableItem.source;
    if (src.originRange && graphic.extent) {
      // For MVP, we still use extent if origin exists? Spec says prefer origin.
      // We'll capture origin if exists
      const originVal = graphic.extent ? undefined : undefined;
      void originVal;
    }
    setDragState(state);
  };

  const handlePointerMove = (e: React.PointerEvent) => {
    if (!dragState || !svgRef.current || !editable) return;
    const pt = clientToSvg(svgRef.current, e.clientX, e.clientY);
    if (!pt) return;
    const dx = pt.x - dragState.pointerStart.x;
    const dy = pt.y - dragState.pointerStart.y;
    const editableItem = editables.find((ed) => ed.id === dragState.id);
    if (!editableItem) return;
    const graphic: any = editableItem.graphic;
    const preview: any = { ...graphic };
    if (dragState.originalExtent) {
      preview.extent = {
        p1: {
          x: dragState.originalExtent.p1.x + dx,
          y: dragState.originalExtent.p1.y + dy,
        },
        p2: {
          x: dragState.originalExtent.p2.x + dx,
          y: dragState.originalExtent.p2.y + dy,
        },
      };
    } else if (dragState.originalPoints) {
      preview.points = dragState.originalPoints.map((p) => ({
        x: p.x + dx,
        y: p.y + dy,
      }));
    }
    // handle origin case: if editable has originRange, we should move origin
    if (editableItem.source.originRange && graphic.origin) {
      preview.origin = {
        x: (graphic.origin.x as number) + dx,
        y: (graphic.origin.y as number) + dy,
      };
      // For origin-based, keep extent unchanged (as per spec)
      if (dragState.originalExtent) preview.extent = dragState.originalExtent;
    }
    setPreviewMap((prev) => {
      const m = new Map(prev);
      m.set(dragState.id, preview);
      return m;
    });
  };

  const handlePointerUp = (e: React.PointerEvent) => {
    if (!dragState || !editable || !onEdit) {
      setDragState(null);
      return;
    }
    const svg = svgRef.current;
    if (!svg) {
      setDragState(null);
      return;
    }
    const pt = clientToSvg(svg, e.clientX, e.clientY);
    if (!pt) {
      setDragState(null);
      setPreviewMap(new Map());
      return;
    }
    const dx = pt.x - dragState.pointerStart.x;
    const dy = pt.y - dragState.pointerStart.y;
    if (Math.abs(dx) < 0.01 && Math.abs(dy) < 0.01) {
      setDragState(null);
      setPreviewMap(new Map());
      return;
    }
    const editableItem = editables.find((ed) => ed.id === dragState.id);
    if (!editableItem) {
      setDragState(null);
      return;
    }
    // Determine which range to patch
    let range: { start: number; end: number } | undefined;
    let replacement = "";
    if (editableItem.source.originRange) {
      range = editableItem.source.originRange;
      const origOrigin = (editableItem.graphic as any).origin ?? { x: 0, y: 0 };
      const newOrigin = { x: origOrigin.x + dx, y: origOrigin.y + dy };
      replacement = serializeOrigin(newOrigin);
    } else if (editableItem.source.extentRange) {
      range = editableItem.source.extentRange;
      const orig = dragState.originalExtent!;
      const newExtent = {
        p1: { x: orig.p1.x + dx, y: orig.p1.y + dy },
        p2: { x: orig.p2.x + dx, y: orig.p2.y + dy },
      };
      replacement = serializeExtent(newExtent);
    } else if (editableItem.source.pointsRange) {
      range = editableItem.source.pointsRange;
      const origPoints = dragState.originalPoints!;
      const newPoints = origPoints.map((p) => ({ x: p.x + dx, y: p.y + dy }));
      replacement = serializePoints(newPoints);
    }
    if (range && replacement) {
      onEdit({ start: range.start, end: range.end, replacement });
    }
    setDragState(null);
    setPreviewMap(new Map());
  };

  // Build display graphics with preview overrides
  const displayGraphics = displayIcon.graphics.map((g: any, idx: number) => {
    const editableItem = editables[idx];
    if (!editableItem) return g;
    const preview = previewMap.get(editableItem.id);
    return preview ?? g;
  });

  return (
    <div
      className="icon-viewer"
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerUp}
    >
      <svg
        ref={svgRef}
        viewBox={viewBox}
        className="modelica-icon"
        preserveAspectRatio="xMidYMid meet"
        style={{ touchAction: "none" }}
      >
        <g transform="scale(1,-1)">
          {displayGraphics.map((item: any, idx: number) => {
            const editableItem = editables[idx];
            const id = editableItem?.id ?? `idx-${idx}`;
            const isSelected = selectedId === id;
            return (
              <g
                key={idx}
                onPointerDown={(e) => handlePointerDown(e, id, item.type)}
                style={{ cursor: editable ? "move" : "default" }}
              >
                <GraphicItemWrapper item={item} />
                {isSelected && <SelectionOutline item={item} />}
              </g>
            );
          })}
        </g>
      </svg>
    </div>
  );
}

function GraphicItemWrapper({ item }: { item: any }) {
  // reuse GraphicItem logic but inline to avoid extra file dependency cycle
  // We'll import GraphicItem component and render
  return <GraphicItem item={item} />;
}

function SelectionOutline({ item }: { item: any }) {
  // Compute bounds from extent or points
  let x = 0,
    y = 0,
    w = 0,
    h = 0;
  if (item.extent) {
    const x1 = item.extent.p1.x,
      y1 = item.extent.p1.y,
      x2 = item.extent.p2.x,
      y2 = item.extent.p2.y;
    x = Math.min(x1, x2);
    y = Math.min(y1, y2);
    w = Math.abs(x2 - x1);
    h = Math.abs(y2 - y1);
  } else if (item.points && item.points.length > 0) {
    const xs = item.points.map((p: any) => p.x);
    const ys = item.points.map((p: any) => p.y);
    x = Math.min(...xs);
    y = Math.min(...ys);
    w = Math.max(...xs) - x;
    h = Math.max(...ys) - y;
    // add small padding for line selection
    x -= 2;
    y -= 2;
    w += 4;
    h += 4;
  } else {
    return null;
  }
  return (
    <rect
      x={x}
      y={y}
      width={w}
      height={h}
      fill="none"
      stroke="#3139fb"
      strokeWidth={1}
      strokeDasharray="4 4"
      vectorEffect="non-scaling-stroke"
      pointerEvents="none"
    />
  );
}
