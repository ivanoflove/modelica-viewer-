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
  const translateX = target.p1.x - icon.p1.x * scaleX;
  const translateY = target.p1.y - icon.p1.y * scaleY;
  return `translate(${transformation.origin.x},${transformation.origin.y}) rotate(${transformation.rotation}) translate(${translateX},${translateY}) scale(${scaleX},${scaleY})`;
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
  const translated = {
    x: point.x * scaleX + target.p1.x - icon.p1.x * scaleX,
    y: point.y * scaleY + target.p1.y - icon.p1.y * scaleY,
  };
  return {
    x: transformation.origin.x + translated.x * Math.cos(angle) - translated.y * Math.sin(angle),
    y: transformation.origin.y + translated.x * Math.sin(angle) + translated.y * Math.cos(angle),
  };
}
