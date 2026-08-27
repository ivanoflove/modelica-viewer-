import type {
  GraphicItemDto,
  GraphicTransform,
} from "../../shared/modelicaGraphics.js";

/** A point in browser client/screen pixels. */
export interface ScreenPoint {
  x: number;
  y: number;
}

/** A point in the SVG root user-coordinate system (Y down). */
export interface SvgPoint {
  x: number;
  y: number;
}

/** A point in Modelica coordinates (Y up). */
export interface ModelPoint {
  x: number;
  y: number;
}

/** A point relative to a Graphic's local origin. */
export interface GraphicLocalPoint {
  x: number;
  y: number;
}

export interface ViewportBox {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface ViewportMapping {
  base: ViewportBox;
  viewBox: ViewportBox;
}

function viewportScale(viewport: ViewportMapping) {
  return {
    x: viewport.base.width / Math.max(viewport.viewBox.width, 1e-9),
    y: viewport.base.height / Math.max(viewport.viewBox.height, 1e-9),
  };
}

/** Modelica (Y up) -> SVG root coordinates (Y down). */
export function modelToSvgRoot(
  point: ModelPoint,
  viewport: ViewportMapping,
): SvgPoint {
  const scale = viewportScale(viewport);
  const translateX = viewport.base.x - viewport.viewBox.x * scale.x;
  const translateY = viewport.base.y - viewport.viewBox.y * scale.y;
  return {
    x: point.x * scale.x + translateX,
    y: -point.y * scale.y + translateY,
  };
}

/** SVG root coordinates (Y down) -> Modelica coordinates (Y up). */
export function svgRootToModel(
  point: SvgPoint,
  viewport: ViewportMapping,
): ModelPoint {
  const scale = viewportScale(viewport);
  const translateX = viewport.base.x - viewport.viewBox.x * scale.x;
  const translateY = viewport.base.y - viewport.viewBox.y * scale.y;
  return {
    x: (point.x - translateX) / scale.x,
    y: -(point.y - translateY) / scale.y,
  };
}

/** Client pixels -> stable SVG root coordinates, respecting SVG letterboxing. */
export function screenToSvgRoot(
  svg: SVGSVGElement,
  screen: ScreenPoint,
  base: ViewportBox,
): SvgPoint | null {
  const rect = svg.getBoundingClientRect();
  const scale = Math.min(
    rect.width / Math.max(base.width, 1e-9),
    rect.height / Math.max(base.height, 1e-9),
  );
  if (!Number.isFinite(scale) || scale <= 0) return null;
  const offsetX = (rect.width - base.width * scale) / 2;
  const offsetY = (rect.height - base.height * scale) / 2;
  return {
    x: base.x + (screen.x - rect.left - offsetX) / scale,
    y: base.y + (screen.y - rect.top - offsetY) / scale,
  };
}

/** Client pixels -> Modelica coordinates. The only viewport-aware input mapper. */
export function screenToModel(
  svg: SVGSVGElement,
  screen: ScreenPoint,
  viewport: ViewportMapping,
): ModelPoint | null {
  const root = screenToSvgRoot(svg, screen, viewport.base);
  return root ? svgRootToModel(root, viewport) : null;
}

/** Modelica coordinates -> client pixels. Useful for explicit round-trip tests. */
export function modelToScreen(
  svg: SVGSVGElement,
  point: ModelPoint,
  viewport: ViewportMapping,
): ScreenPoint | null {
  const root = modelToSvgRoot(point, viewport);
  const rect = svg.getBoundingClientRect();
  const scale = Math.min(
    rect.width / Math.max(viewport.base.width, 1e-9),
    rect.height / Math.max(viewport.base.height, 1e-9),
  );
  if (!Number.isFinite(scale) || scale <= 0) return null;
  return {
    x: rect.left + (root.x - viewport.base.x) * scale + (rect.width - viewport.base.width * scale) / 2,
    y: rect.top + (root.y - viewport.base.y) * scale + (rect.height - viewport.base.height * scale) / 2,
  };
}

/** Graphic local coordinates -> Modelica coordinates. Viewport is not involved. */
export function graphicLocalToModel(
  localPoint: GraphicLocalPoint,
  graphic: Pick<GraphicItemDto, "origin">,
  transform: GraphicTransform,
): ModelPoint {
  const scaled = {
    x: localPoint.x * transform.scale.x,
    y: localPoint.y * transform.scale.y,
  };
  const angle = (transform.rotate * Math.PI) / 180;
  const rotated = {
    x: scaled.x * Math.cos(angle) - scaled.y * Math.sin(angle),
    y: scaled.x * Math.sin(angle) + scaled.y * Math.cos(angle),
  };
  const origin = graphic.origin ?? { x: 0, y: 0 };
  return {
    x: rotated.x + origin.x + transform.translate.x,
    y: rotated.y + origin.y + transform.translate.y,
  };
}

/** Modelica coordinates -> Graphic local coordinates. Viewport is not involved. */
export function modelToGraphicLocal(
  modelPoint: ModelPoint,
  graphic: Pick<GraphicItemDto, "origin">,
  transform: GraphicTransform,
): GraphicLocalPoint {
  const origin = graphic.origin ?? { x: 0, y: 0 };
  const translated = {
    x: modelPoint.x - origin.x - transform.translate.x,
    y: modelPoint.y - origin.y - transform.translate.y,
  };
  const angle = (-transform.rotate * Math.PI) / 180;
  const unrotated = {
    x: translated.x * Math.cos(angle) - translated.y * Math.sin(angle),
    y: translated.x * Math.sin(angle) + translated.y * Math.cos(angle),
  };
  return {
    x: unrotated.x / Math.max(Math.abs(transform.scale.x), 1e-9) * Math.sign(transform.scale.x || 1),
    y: unrotated.y / Math.max(Math.abs(transform.scale.y), 1e-9) * Math.sign(transform.scale.y || 1),
  };
}

export function graphicLocalToSvgRoot(
  graphic: Pick<GraphicItemDto, "origin">,
  localPoint: GraphicLocalPoint,
  transform: GraphicTransform,
  viewport: ViewportMapping,
): SvgPoint {
  return modelToSvgRoot(graphicLocalToModel(localPoint, graphic, transform), viewport);
}
