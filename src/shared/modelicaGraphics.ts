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
}

export interface IconDto {
  coordinateSystem: CoordinateSystemDto;
  graphics: GraphicItemDto[];
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
}

export interface EllipseDto {
  type: "Ellipse";
  extent: Extent;
  lineColor?: [number, number, number];
  fillColor?: [number, number, number];
  startAngle?: number;
  endAngle?: number;
}

export interface LineDto {
  type: "Line";
  points: Point[];
  color?: [number, number, number];
  thickness?: number;
}

export interface PolygonDto {
  type: "Polygon";
  points: Point[];
  lineColor?: [number, number, number];
  fillColor?: [number, number, number];
}

export interface TextDto {
  type: "Text";
  extent: Extent;
  textString: string;
  textColor?: [number, number, number];
  fontSize?: number;
}
