import { useRef, useState } from "react";
import type {
  IconDto,
  EditableIconDto,
  EditableGraphic,
  GraphicItemDto,
  GraphicTransform,
} from "../../../shared/modelicaGraphics";
import { GraphicItem } from "./GraphicItem";
import { toSvgTransform, boundsOf, applyTransform, type Bounds } from "../../editor/Transform";
import { clientToModelica } from "../../editor/DragController";
import { useSelection } from "../../editor/Selection";
import type { SelectionState } from "../../editor/Selection";

type FitMode = "content" | "coordinateSystem";
type Edit = { start: number; end: number; replacement: string };

interface Props {
  icon: IconDto | null;
  editable?: EditableIconDto | null;
  modelName: string;
  onEdit?: (edit: Edit) => void;
}

interface DragSession {
  id: string;
  pointerStart: { x: number; y: number };
  transformStart: GraphicTransform;
}

const identity: GraphicTransform = {
  translate: { x: 0, y: 0 },
  scale: { x: 1, y: 1 },
  rotate: 0,
};

function unionBounds(items: GraphicItemDto[]): Bounds | null {
  const bounds = items.map(boundsOf).filter((b): b is Bounds => b !== null);
  if (bounds.length === 0) return null;
  const minX = Math.min(...bounds.map((b) => b.x));
  const minY = Math.min(...bounds.map((b) => b.y));
  const maxX = Math.max(...bounds.map((b) => b.x + b.width));
  const maxY = Math.max(...bounds.map((b) => b.y + b.height));
  return { x: minX, y: minY, width: maxX - minX, height: maxY - minY };
}

function viewBoxFor(icon: IconDto, mode: FitMode): string {
  const coordinate = icon.coordinateSystem.extent;
  const coordinateBounds: Bounds = {
    x: Math.min(coordinate.p1.x, coordinate.p2.x),
    y: Math.min(coordinate.p1.y, coordinate.p2.y),
    width: Math.max(Math.abs(coordinate.p2.x - coordinate.p1.x), 1),
    height: Math.max(Math.abs(coordinate.p2.y - coordinate.p1.y), 1),
  };
  if (mode === "coordinateSystem") {
    return `${coordinateBounds.x} ${coordinateBounds.y} ${coordinateBounds.width} ${coordinateBounds.height}`;
  }
  const content = unionBounds(icon.graphics);
  if (!content) {
    return `${coordinateBounds.x} ${coordinateBounds.y} ${coordinateBounds.width} ${coordinateBounds.height}`;
  }
  const padding = Math.max(8, Math.min(content.width, content.height) * 0.08);
  return `${content.x - padding} ${content.y - padding} ${Math.max(content.width + padding * 2, 1)} ${Math.max(content.height + padding * 2, 1)}`;
}

export function IconViewer({ icon, editable, modelName, onEdit }: Props) {
  const svgRef = useRef<SVGSVGElement>(null);
  const dragRef = useRef<DragSession | null>(null);
  const pendingPointRef = useRef<{ x: number; y: number } | null>(null);
  const rafRef = useRef<number | null>(null);
  const [drag, setDrag] = useState<DragSession | null>(null);
  const [previewTransforms, setPreviewTransforms] = useState<Map<string, GraphicTransform>>(new Map());
  const [fitMode, setFitMode] = useState<FitMode>("content");
  const sel: SelectionState = useSelection();

  if (!icon) return <div className="no-icon">No Icon annotation</div>;

  const editables = editable?.editables ?? [];
  const entries = editables.length > 0
    ? editables.map((ed) => ({ id: ed.id, graphic: ed.graphic, editable: ed }))
    : icon.graphics.map((graphic, index) => ({ id: `view:${index}`, graphic, editable: undefined }));
  const viewBox = viewBoxFor(icon, fitMode);

  const transformFor = (id: string): GraphicTransform =>
    previewTransforms.get(id) ?? editables.find((ed) => ed.id === id)?.transform ?? identity;

  const updatePreview = (session: DragSession, point: { x: number; y: number }) => {
    const translate = {
      x: session.transformStart.translate.x + point.x - session.pointerStart.x,
      y: session.transformStart.translate.y + point.y - session.pointerStart.y,
    };
    setPreviewTransforms((previous) => {
      const next = new Map(previous);
      next.set(session.id, { ...session.transformStart, translate });
      return next;
    });
  };

  const schedulePreview = (point: { x: number; y: number }) => {
    pendingPointRef.current = point;
    if (rafRef.current !== null) return;
    rafRef.current = requestAnimationFrame(() => {
      rafRef.current = null;
      const session = dragRef.current;
      const pending = pendingPointRef.current;
      if (session && pending) updatePreview(session, pending);
    });
  };

  const clearDrag = () => {
    if (rafRef.current !== null) cancelAnimationFrame(rafRef.current);
    rafRef.current = null;
    pendingPointRef.current = null;
    dragRef.current = null;
    setDrag(null);
    setPreviewTransforms(new Map());
  };

  const handlePointerDown = (e: React.PointerEvent, ed: EditableGraphic) => {
    const svg = svgRef.current;
    if (!svg) return;
    const point = clientToModelica(svg, e.clientX, e.clientY);
    if (!point) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    sel.setSelected(ed.id);
    const session: DragSession = {
      id: ed.id,
      pointerStart: point,
      transformStart: ed.transform,
    };
    dragRef.current = session;
    setDrag(session);
  };

  const handlePointerMove = (e: React.PointerEvent) => {
    const session = dragRef.current;
    const svg = svgRef.current;
    if (!session || !svg) return;
    const point = clientToModelica(svg, e.clientX, e.clientY);
    if (point) schedulePreview(point);
  };

  const handlePointerUp = (e: React.PointerEvent) => {
    const session = dragRef.current;
    const svg = svgRef.current;
    if (!session || !svg) {
      clearDrag();
      return;
    }
    const point = clientToModelica(svg, e.clientX, e.clientY);
    const ed = editables.find((item) => item.id === session.id);
    if (point && ed && onEdit) {
      const dx = Math.round((point.x - session.pointerStart.x) / 10) * 10;
      const dy = Math.round((point.y - session.pointerStart.y) / 10) * 10;
      if (dx !== 0 || dy !== 0) commitTranslate(ed, dx, dy, onEdit);
    }
    clearDrag();
  };

  return <div className="icon-editor-shell">
    <div className="icon-viewer-toolbar">
      <span>Icon 画布</span>
      <div className="fit-toggle">
        <button className={fitMode === "content" ? "active" : ""} onClick={() => setFitMode("content")}>Fit Content</button>
        <button className={fitMode === "coordinateSystem" ? "active" : ""} onClick={() => setFitMode("coordinateSystem")}>Fit CoordinateSystem</button>
      </div>
    </div>
    <div className="icon-canvas-area">
      <svg ref={svgRef} viewBox={viewBox} className="modelica-icon" preserveAspectRatio="xMidYMid meet"
        style={{ touchAction: "none" }} onPointerMove={handlePointerMove} onPointerUp={handlePointerUp} onPointerCancel={clearDrag}>
        <g transform="scale(1,-1)">
          {entries.map(({ id, graphic, editable: ed }, index) => {
            const transform = transformFor(id);
            const displayGraphic = ed ? applyTransform(graphic, transform) : graphic;
            const selected = ed ? sel.selectedId === id : false;
            const hovered = ed ? sel.hoverId === id : false;
            const bounds = boundsOf(displayGraphic);
            return <g key={id} onPointerDown={ed ? (e) => handlePointerDown(e, ed) : undefined}
              onPointerEnter={ed ? () => sel.setHover(id) : undefined}
              onPointerLeave={ed ? () => sel.setHover(null) : undefined}
              style={ed ? { cursor: "move" } : undefined}>
              <g transform={ed ? toSvgTransform(transform) : undefined}>
                <GraphicItem item={displayGraphic} styleId={`graphic-style-${index}`} />
              </g>
              {selected && bounds && <rect className="selection-box" x={bounds.x} y={bounds.y} width={bounds.width} height={bounds.height} pointerEvents="none" />}
              {!selected && hovered && bounds && <rect className="hover-outline" x={bounds.x} y={bounds.y} width={bounds.width} height={bounds.height} pointerEvents="none" />}
            </g>;
          })}
        </g>
      </svg>
    </div>
    {editable && <GraphicProperties editable={editables.find((ed) => ed.id === sel.selectedId) ?? null} onEdit={onEdit} />}
    {drag && <span className="drag-status">拖动预览中，松开鼠标后写回源码</span>}
    <span className="icon-editor-hint">拖动图元移动；松开时按 10 单位网格提交</span>
  </div>;
}

function commitTranslate(ed: EditableGraphic, dx: number, dy: number, onEdit: (edit: Edit) => void) {
  const source = ed.source;
  const graphic = ed.graphic as any;
  const format = (n: number) => Number.isInteger(n) ? String(n) : String(Number(n.toFixed(4)));
  if (source.originRange && graphic.origin) {
    onEdit({ start: source.originRange.start, end: source.originRange.end, replacement: `{${format(graphic.origin.x + dx)},${format(graphic.origin.y + dy)}}` });
  } else if (source.extentRange && graphic.extent) {
    const e = graphic.extent;
    onEdit({ start: source.extentRange.start, end: source.extentRange.end, replacement: `{{${format(e.p1.x + dx)},${format(e.p1.y + dy)}},{${format(e.p2.x + dx)},${format(e.p2.y + dy)}}` });
  } else if (source.pointsRange && graphic.points) {
    onEdit({ start: source.pointsRange.start, end: source.pointsRange.end, replacement: `{${graphic.points.map((p: { x: number; y: number }) => `{${format(p.x + dx)},${format(p.y + dy)}}`).join(",")}}` });
  }
}

const linePatterns = ["LinePattern.Solid", "LinePattern.Dash", "LinePattern.Dot", "LinePattern.DashDot", "LinePattern.DashDotDot", "LinePattern.None"];
const fillPatterns = ["FillPattern.None", "FillPattern.Solid", "FillPattern.Horizontal", "FillPattern.Vertical", "FillPattern.Cross", "FillPattern.Forward", "FillPattern.Backward", "FillPattern.CrossDiag", "FillPattern.HorizontalCylinder", "FillPattern.VerticalCylinder", "FillPattern.Sphere"];

function colorHex(color?: [number, number, number]): string {
  return color ? `#${color.map((part) => part.toString(16).padStart(2, "0")).join("")}` : "#000000";
}

function parseHex(hex: string): [number, number, number] {
  return [1, 3, 5].map((offset) => parseInt(hex.slice(offset, offset + 2), 16)) as [number, number, number];
}

function GraphicProperties({ editable, onEdit }: { editable: EditableGraphic | null; onEdit?: (edit: Edit) => void }) {
  if (!editable || !onEdit) return <aside className="graphic-properties empty-properties">选择图元查看属性</aside>;
  const graphic = editable.graphic;
  const source = editable.source;
  const patch = (name: string, range: { start: number; end: number } | undefined, value: string) => {
    const edit = range
      ? { start: range.start, end: range.end, replacement: value }
      : { start: source.itemRange.end - 1, end: source.itemRange.end - 1, replacement: `, ${name}=${value}` };
    onEdit(edit);
  };
  const line = graphic.type === "Line" ? graphic.color : graphic.type === "Text" ? graphic.textColor : graphic.lineColor;
  const fill = graphic.type === "Line" || graphic.type === "Text" ? undefined : graphic.fillColor;
  const pattern = graphic.type === "Line" || graphic.type === "Text" ? "LinePattern.Solid" : graphic.pattern ?? "LinePattern.Solid";
  const fillPattern = graphic.type === "Line" || graphic.type === "Text" ? "FillPattern.None" : graphic.fillPattern ?? "FillPattern.None";
  const thickness = graphic.type === "Line" ? graphic.thickness : graphic.type === "Text" ? undefined : graphic.lineThickness;
  return <aside className="graphic-properties">
    <h3>Selected Graphic</h3>
    <label>Type <strong>{graphic.type}</strong></label>
    <label>Line Color <input type="color" value={colorHex(line)} onChange={(e) => patch("lineColor", source.lineColorRange, `{${parseHex(e.target.value).join(",")}}`)} /></label>
    <label>Line Style <select value={pattern} onChange={(e) => patch("pattern", source.patternRange, e.target.value)}>{linePatterns.map((value) => <option key={value}>{value}</option>)}</select></label>
    <label>Line Thickness <input type="number" min="0" step="0.5" value={thickness ?? 1} onChange={(e) => patch(graphic.type === "Line" ? "thickness" : "lineThickness", graphic.type === "Line" ? source.thicknessRange : source.lineThicknessRange, e.target.value)} /></label>
    {graphic.type !== "Line" && graphic.type !== "Text" && <>
      <label>Fill Color <input type="color" value={colorHex(fill)} onChange={(e) => patch("fillColor", source.fillColorRange, `{${parseHex(e.target.value).join(",")}}`)} /></label>
      <label>Fill Style <select value={fillPattern} onChange={(e) => patch("fillPattern", source.fillPatternRange, e.target.value)}>{fillPatterns.map((value) => <option key={value}>{value}</option>)}</select></label>
    </>}
  </aside>;
}
