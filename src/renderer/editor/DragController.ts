import type {
  EditableGraphic,
  GraphicTransform,
} from "../../shared/modelicaGraphics.js";
import { snap } from "./Transform.js";

export interface DragInfo {
  id: string;
  pointerStart: { x: number; y: number };
  transformStart: GraphicTransform;
}

export interface DragPreview {
  graphicId: string;
  dx: number;
  dy: number;
}

/** Apply a preview only to the graphic that owns the active drag session. */
export function transformWithDragPreview(
  graphicId: string,
  base: GraphicTransform,
  preview: DragPreview | null,
): GraphicTransform {
  if (!preview || preview.graphicId !== graphicId) return base;
  return {
    ...base,
    translate: {
      x: base.translate.x + preview.dx,
      y: base.translate.y + preview.dy,
    },
  };
}

/** Convert client/screen coordinates into Modelica SVG coordinates. */
export function clientToModelica(
  svg: SVGSVGElement,
  clientX: number,
  clientY: number,
): { x: number; y: number } | null {
  const ctm = svg.getScreenCTM()?.inverse();
  if (!ctm) return null;
  return clientToModelicaWithInverse(clientX, clientY, ctm);
}

/** Convert a client point using a matrix captured at drag start. */
export function clientToModelicaWithInverse(
  clientX: number,
  clientY: number,
  inverse: DOMMatrix,
): { x: number; y: number } {
  const p = new DOMPoint(clientX, clientY).matrixTransform(inverse);
  // SVG Y is down; Modelica Y is up
  return { x: p.x, y: -p.y };
}

/** Calculate a drag delta from the fixed pointer-down position. */
export function dragDeltaFromStart(
  start: { x: number; y: number },
  current: { x: number; y: number },
  grid?: number,
): { x: number; y: number } {
  const dx = current.x - start.x;
  const dy = current.y - start.y;
  if (!grid || grid <= 0) return { x: dx, y: dy };
  return { x: snap(dx, grid), y: snap(dy, grid) };
}

/** Compute snapped translate from a drag session's current pointer. */
export function computeDragTranslate(
  drag: DragInfo,
  current: { x: number; y: number },
  grid = 10,
): { x: number; y: number } {
  const raw = dragDeltaFromStart(drag.pointerStart, current);
  return {
    x: snap(drag.transformStart.translate.x + raw.x, grid),
    y: snap(drag.transformStart.translate.y + raw.y, grid),
  };
}

/** Merge a translate into a graphic's transform (identity-preserving). */
export function translateTransform(
  base: GraphicTransform,
  translate: { x: number; y: number },
): GraphicTransform {
  return { ...base, translate };
}

/**
 * DragController helpers: given an editable list, produce a preview map of
 * id -> translated graphic, plus a final commit translate per graphic.
 */
export function buildPreview(
  editables: EditableGraphic[],
  activeDrag: DragInfo | null,
  current: { x: number; y: number } | null,
  grid = 10,
): Map<string, GraphicTransform> {
  const map = new Map<string, GraphicTransform>();
  if (!activeDrag || !current) return map;
  for (const ed of editables) {
    if (ed.id === activeDrag.id) {
      const translate = computeDragTranslate(activeDrag, current, grid);
      map.set(ed.id, translateTransform(ed.transform, translate));
    } else {
      map.set(ed.id, ed.transform);
    }
  }
  return map;
}
