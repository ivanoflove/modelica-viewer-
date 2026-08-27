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

export interface GraphicProvenance {
  graphicId?: string;
  ownerQualifiedName?: string;
  ownerSourceFile?: string;
  inherited?: boolean;
  inheritancePath?: string[];
  sourceRange?: { start: number; end: number };
}

export interface SourceRangeRef {
  start: number;
  end: number;
  expectedText?: string;
}

export const identityTransform: GraphicTransform = {
  translate: { x: 0, y: 0 },
  scale: { x: 1, y: 1 },
  rotate: 0,
};

export interface EditableGraphic {
  id: string;
  graphic: GraphicItemDto;
  /** Provenance of the source class that owns this graphic. */
  ownerQualifiedName?: string;
  ownerSourceFile?: string;
  inherited?: boolean;
  inheritancePath?: string[];
  selected: boolean;
  transform: GraphicTransform;
  source: {
    itemRange: SourceRangeRef;
    extentRange?: SourceRangeRef;
    pointsRange?: SourceRangeRef;
    originRange?: SourceRangeRef;
    lineColorRange?: SourceRangeRef;
    colorRange?: SourceRangeRef;
    textColorRange?: SourceRangeRef;
    fillColorRange?: SourceRangeRef;
    textStringRange?: SourceRangeRef;
    fontSizeRange?: SourceRangeRef;
    textStyleRange?: SourceRangeRef;
    lineThicknessRange?: SourceRangeRef;
    thicknessRange?: SourceRangeRef;
    patternRange?: SourceRangeRef;
    fillPatternRange?: SourceRangeRef;
  };
}

export interface EditableIconDto {
  icon: IconDto;
  editables: EditableGraphic[];
  /** Source range of the Icon.graphics array, used for logical delete undo. */
  graphicsRange?: SourceRangeRef;
  sourceVersion?: number;
}

export type GraphicToolType =
  | "Line"
  | "Polygon"
  | "Rectangle"
  | "Text"
  | "Ellipse"
  | "Bitmap";

export type GraphicItemDto =
  | RectangleDto
  | EllipseDto
  | LineDto
  | PolygonDto
  | TextDto
  | BitmapDto;

export interface RectangleDto extends GraphicProvenance {
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

export interface EllipseDto extends GraphicProvenance {
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

export interface LineDto extends GraphicProvenance {
  type: "Line";
  points: Point[];
  color?: [number, number, number];
  thickness?: number;
  origin?: Point;
  smooth?: string;
  pattern?: string;
}

export interface PolygonDto extends GraphicProvenance {
  type: "Polygon";
  points: Point[];
  lineColor?: [number, number, number];
  fillColor?: [number, number, number];
  lineThickness?: number;
  origin?: Point;
  fillPattern?: string;
  pattern?: string;
}

export interface TextDto extends GraphicProvenance {
  type: "Text";
  extent: Extent;
  textString: string;
  textColor?: [number, number, number];
  fontSize?: number;
  origin?: Point;
  textStyle?: string[];
}

export interface BitmapDto extends GraphicProvenance {
  type: "Bitmap";
  extent: Extent;
  origin?: Point;
  fileName?: string;
  imageSource?: string;
  lineColor?: [number, number, number];
  fillColor?: [number, number, number];
  lineThickness?: number;
  fillPattern?: string;
  pattern?: string;
}

export interface BitmapDto extends GraphicProvenance {
  type: "Bitmap";
  extent: Extent;
  origin?: Point;
  fileName?: string;
  imageSource?: string;
}
