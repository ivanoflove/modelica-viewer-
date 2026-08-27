import type { CoordinateSystemDto, Point } from "./modelicaGraphics.js";
import type { TransformationDto } from "./modelica.js";

/** Map an Icon coordinate system into one component Placement transform. */
export function computePlacementTransform(
  iconCoordinateSystem: CoordinateSystemDto,
  transformation: TransformationDto,
): string {
  const icon = iconCoordinateSystem.extent;
  const target = transformation.extent;
  const iconWidth = icon.p2.x - icon.p1.x || 1;
  const iconHeight = icon.p2.y - icon.p1.y || 1;
  const scaleX = (target.p2.x - target.p1.x) / iconWidth;
  const scaleY = (target.p2.y - target.p1.y) / iconHeight;
  const centerX = (icon.p1.x + icon.p2.x) / 2;
  const centerY = (icon.p1.y + icon.p2.y) / 2;
  return `translate(${transformation.origin.x},${transformation.origin.y}) rotate(${transformation.rotation}) scale(${scaleX},${scaleY}) translate(${-centerX},${-centerY})`;
}

export function transformPlacementPoint(
  point: Point,
  iconCoordinateSystem: CoordinateSystemDto,
  transformation: TransformationDto,
): Point {
  const icon = iconCoordinateSystem.extent;
  const target = transformation.extent;
  const scaleX = (target.p2.x - target.p1.x) / (icon.p2.x - icon.p1.x || 1);
  const scaleY = (target.p2.y - target.p1.y) / (icon.p2.y - icon.p1.y || 1);
  const angle = (transformation.rotation * Math.PI) / 180;
  const scaled = {
    x: (point.x - (icon.p1.x + icon.p2.x) / 2) * scaleX,
    y: (point.y - (icon.p1.y + icon.p2.y) / 2) * scaleY,
  };
  return {
    x: transformation.origin.x + scaled.x * Math.cos(angle) - scaled.y * Math.sin(angle),
    y: transformation.origin.y + scaled.x * Math.sin(angle) + scaled.y * Math.cos(angle),
  };
}

