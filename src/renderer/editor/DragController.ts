import type { EditableGraphic, GraphicTransform } from "../../shared/modelicaGraphics.js";
import { snap } from "./Transform.js";

export interface DragInfo {
  id: string;
  pointerStart: { x: number; y: number };
  transformStart: GraphicTransform;
}

/** Convert client/screen coordinates into Modelica SVG coordinates. */
export function clientToModelica(
  svg: SVGSVGElement,
  clientX: number,
  clientY: number,
): { x: number; y: number } | null {
  const pt = svg.createSVGPoint();
  pt.x = clientX;
  pt.y = clientY;
  const ctm = svg.getScreenCTM()?.inverse();
  if (!ctm) return null;
  const p = pt.matrixTransform(ctm);
  // SVG Y is down; Modelica Y is up
  return { x: p.x, y: -p.y };
}

/** Compute snapped translate from a drag session's current pointer. */
export function computeDragTranslate(
  drag: DragInfo,
  current: { x: number; y: number },
  grid = 10,
): { x: number; y: number } {
  const rawDx = current.x - drag.pointerStart.x;
  const rawDy = current.y - drag.pointerStart.y;
  return {
    x: snap(drag.transformStart.translate.x + rawDx, grid),
    y: snap(drag.transformStart.translate.y + rawDy, grid),
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
