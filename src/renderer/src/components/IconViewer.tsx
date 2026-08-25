import { useRef, useState } from "react";
import type {
  IconDto,
  EditableIconDto,
  GraphicTransform,
} from "../../../shared/modelicaGraphics";
import { GraphicItem } from "./GraphicItem";
import {
  toSvgTransform,
  boundsOf,
  applyTransform,
} from "../../editor/Transform";
import { clientToModelica } from "../../editor/DragController";
import { useSelection } from "../../editor/Selection";
import type { SelectionState } from "../../editor/Selection";

interface Props {
  icon: IconDto | null;
  editable?: EditableIconDto | null;
  modelName: string;
  onEdit?: (edit: { start: number; end: number; replacement: string }) => void;
}

interface DragSession {
  id: string;
  pointerStart: { x: number; y: number };
  transformStart: GraphicTransform;
}

export function IconViewer({ icon, editable, modelName, onEdit }: Props) {
  const svgRef = useRef<SVGSVGElement>(null);
  const [drag, setDrag] = useState<DragSession | null>(null);
  const [previewTransforms, setPreviewTransforms] = useState<
    Map<string, GraphicTransform>
  >(new Map());
  const sel: SelectionState = useSelection();

  void modelName;

  if (!icon) {
    return <div className="no-icon">No Icon annotation</div>;
  }

  const editables = editable?.editables ?? [];
  const extent = icon.coordinateSystem.extent;
  const minX = Math.min(extent.p1.x, extent.p2.x);
  const minY = Math.min(extent.p1.y, extent.p2.y);
  const width = Math.abs(extent.p2.x - extent.p1.x);
  const height = Math.abs(extent.p2.y - extent.p1.y);
  const viewBox = `${minX} ${minY} ${width} ${height}`;

  const transformFor = (id: string): GraphicTransform => {
    const preview = previewTransforms.get(id);
    if (preview) return preview;
    const ed = editables.find((e) => e.id === id);
    return (
      ed?.transform ?? {
        translate: { x: 0, y: 0 },
        scale: { x: 1, y: 1 },
        rotate: 0,
      }
    );
  };

  const handlePointerDown = (e: React.PointerEvent, id: string) => {
    const svg = svgRef.current;
    if (!svg) return;
    const pt = clientToModelica(svg, e.clientX, e.clientY);
    if (!pt) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    sel.setSelected(id);
    const ed = editables.find((x) => x.id === id);
    setDrag({
      id,
      pointerStart: pt,
      transformStart: ed?.transform ?? {
        translate: { x: 0, y: 0 },
        scale: { x: 1, y: 1 },
        rotate: 0,
      },
    });
  };

  const handlePointerMove = (e: React.PointerEvent) => {
    if (!drag || !svgRef.current) return;
    const pt = clientToModelica(svgRef.current, e.clientX, e.clientY);
    if (!pt) return;
    const rawDx = pt.x - drag.pointerStart.x;
    const rawDy = pt.y - drag.pointerStart.y;
    // grid snap on Modelica coords
    const snapped = (v: number) => Math.round(v / 10) * 10;
    const translate = {
      x: snapped(drag.transformStart.translate.x + rawDx),
      y: snapped(drag.transformStart.translate.y + rawDy),
    };
    const newTransform: GraphicTransform = {
      ...drag.transformStart,
      translate,
    };
    setPreviewTransforms((prev) => {
      const m = new Map(prev);
      m.set(drag.id, newTransform);
      return m;
    });
  };

  const handlePointerUp = (e: React.PointerEvent) => {
    if (!drag || !svgRef.current || !onEdit) {
      setDrag(null);
      setPreviewTransforms(new Map());
      return;
    }
    const pt = clientToModelica(svgRef.current, e.clientX, e.clientY);
    if (pt) {
      const rawDx = pt.x - drag.pointerStart.x;
      const rawDy = pt.y - drag.pointerStart.y;
      const snappedDx = Math.round(rawDx / 10) * 10;
      const snappedDy = Math.round(rawDy / 10) * 10;
      const ed = editables.find((x) => x.id === drag.id);
      if (ed && (snappedDx !== 0 || snappedDy !== 0)) {
        // Which range to patch: origin first, then extent, then points
        const src = ed.source;
        if (src.originRange && (ed.graphic as any).origin) {
          const o = (ed.graphic as any).origin as { x: number; y: number };
          const replacement = `{${formatNum(o.x + snappedDx)},${formatNum(o.y + snappedDy)}}`;
          onEdit({
            start: src.originRange.start,
            end: src.originRange.end,
            replacement,
          });
        } else if (src.extentRange) {
          const ext = (ed.graphic as any).extent as {
            p1: { x: number; y: number };
            p2: { x: number; y: number };
          };
          const replacement = `{{${formatNum(ext.p1.x + snappedDx)},${formatNum(ext.p1.y + snappedDy)}},{${formatNum(ext.p2.x + snappedDx)},${formatNum(ext.p2.y + snappedDy)}}}`;
          onEdit({
            start: src.extentRange.start,
            end: src.extentRange.end,
            replacement,
          });
        } else if (src.pointsRange) {
          const pts = (ed.graphic as any).points as { x: number; y: number }[];
          const replacement = `{${pts.map((p) => `{${formatNum(p.x + snappedDx)},${formatNum(p.y + snappedDy)}}`).join(",")}}`;
          onEdit({
            start: src.pointsRange.start,
            end: src.pointsRange.end,
            replacement,
          });
        }
      }
    }
    setDrag(null);
    setPreviewTransforms(new Map());
  };

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
          {editables.map((ed, idx) => {
            const t = transformFor(ed.id);
            const displayGraphic = applyTransform(ed.graphic, t);
            const isSelected = sel.selectedId === ed.id;
            const isHover = sel.hoverId === ed.id;
            const bounds = boundsOf(displayGraphic);
            return (
              <g
                key={idx}
                onPointerDown={(e) => handlePointerDown(e, ed.id)}
                onPointerEnter={() => sel.setHover(ed.id)}
                onPointerLeave={() => sel.setHover(null)}
                style={{ cursor: "move" }}
              >
                <g transform={toSvgTransform(t)}>
                  <GraphicItem item={displayGraphic} />
                </g>
                {isSelected && bounds && (
                  <rect
                    className="selection-box"
                    x={bounds.x}
                    y={bounds.y}
                    width={bounds.width}
                    height={bounds.height}
                    pointerEvents="none"
                  />
                )}
                {!isSelected && isHover && bounds && (
                  <rect
                    className="hover-outline"
                    x={bounds.x}
                    y={bounds.y}
                    width={bounds.width}
                    height={bounds.height}
                    pointerEvents="none"
                  />
                )}
              </g>
            );
          })}
        </g>
      </svg>
    </div>
  );
}

function formatNum(n: number): string {
  if (Number.isInteger(n)) return String(n);
  return String(Number(n.toFixed(4)));
}
