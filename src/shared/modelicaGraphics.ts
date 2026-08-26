export interface Point {
  x: number;
  y: number;
}

export interface Extent {
  p1: Point;
  p2: Point;
}

export interface CoordinateSystemDto {
  extent: Extent;
  preserveAspectRatio?: boolean;
  grid?: Point;
  initialScale?: number;
}

export interface IconDto {
  coordinateSystem: CoordinateSystemDto;
  graphics: GraphicItemDto[];
}

export interface GraphicTransform {
  translate: { x: number; y: number };
  scale: { x: number; y: number };
  rotate: number;
}

export const identityTransform: GraphicTransform = {
  translate: { x: 0, y: 0 },
  scale: { x: 1, y: 1 },
  rotate: 0,
};

export interface EditableGraphic {
  id: string;
  graphic: GraphicItemDto;
  selected: boolean;
  transform: GraphicTransform;
  source: {
    itemRange: { start: number; end: number };
    extentRange?: { start: number; end: number };
    pointsRange?: { start: number; end: number };
    originRange?: { start: number; end: number };
    lineColorRange?: { start: number; end: number };
    fillColorRange?: { start: number; end: number };
    lineThicknessRange?: { start: number; end: number };
    thicknessRange?: { start: number; end: number };
    patternRange?: { start: number; end: number };
    fillPatternRange?: { start: number; end: number };
  };
}

export interface EditableIconDto {
  icon: IconDto;
  editables: EditableGraphic[];
}

export type GraphicItemDto =
  | RectangleDto
  | EllipseDto
  | LineDto
  | PolygonDto
  | TextDto;

export interface RectangleDto {
  type: "Rectangle";
  extent: Extent;
  lineColor?: [number, number, number];
  fillColor?: [number, number, number];
  lineThickness?: number;
  radius?: number;
  origin?: Point;
  fillPattern?: string;
  pattern?: string;
}

export interface EllipseDto {
  type: "Ellipse";
  extent: Extent;
  lineColor?: [number, number, number];
  fillColor?: [number, number, number];
  lineThickness?: number;
  startAngle?: number;
  endAngle?: number;
  origin?: Point;
  fillPattern?: string;
  pattern?: string;
}

export interface LineDto {
  type: "Line";
  points: Point[];
  color?: [number, number, number];
  thickness?: number;
  origin?: Point;
  smooth?: string;
  pattern?: string;
}

export interface PolygonDto {
  type: "Polygon";
  points: Point[];
  lineColor?: [number, number, number];
  fillColor?: [number, number, number];
  lineThickness?: number;
  origin?: Point;
  fillPattern?: string;
  pattern?: string;
}

export interface TextDto {
  type: "Text";
  extent: Extent;
  textString: string;
  textColor?: [number, number, number];
  fontSize?: number;
  origin?: Point;
  textStyle?: string[];
}
