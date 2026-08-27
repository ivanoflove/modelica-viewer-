import type {
  IconDto,
  EditableIconDto,
  GraphicToolType,
  GraphicItemDto,
  LineDto,
  Point,
} from "./modelicaGraphics.js";

export type ClassKind =
  | "package"
  | "model"
  | "block"
  | "connector"
  | "record"
  | "function"
  | "class"
  | "type"
  | "expandable connector"
  | "operator"
  | "operator record"
  | "operator function";

export interface PackageNodeDto {
  name: string;
  within: string | null;
  qualifiedName: string;
  sourceFile: string;
  sourceRange?: SourceRangeDto;
  children: PackageNodeDto[];
  classes: ClassNodeDto[];
  loadErrors?: string[];
}

export interface SourceRangeDto {
  start: number;
  end: number;
}

export interface ClassNodeDto {
  kind: ClassKind;
  name: string;
  qualifiedName: string;
  sourceFile: string;
  sourceRange: SourceRangeDto;
  isPartial: boolean;
  isEncapsulated: boolean;
  extendsClauses: string[];
  children: ClassNodeDto[];
}

export type LoadPackageResult =
  | { canceled: true }
  | { canceled: false; root: PackageNodeDto }
  | { error: string };

export type ReadSourceResult =
  | { content: string; filePath: string }
  | { error: string };

export type GetIconResult =
  | { icon: IconDto | null; warnings?: string[] }
  | { error: string };

export type GetEditableIconResult =
  | { editable: EditableIconDto | null }
  | { error: string };

export interface TransformationDto {
  origin: Point;
  extent: { p1: Point; p2: Point };
  rotation: number;
}

export interface PlacementDto {
  visible: boolean;
  transformation?: TransformationDto;
  iconTransformation?: TransformationDto;
  iconVisible?: boolean;
}

export interface ComponentInstanceDto {
  id: string;
  name: string;
  typeName: string;
  declaredTypeName?: string;
  resolvedTypeQualifiedName?: string;
  classKind?: ClassKind;
  sourceRange: SourceRangeDto;
  placement?: PlacementDto;
  resolvedIcon?: IconDto;
  resolvedDiagram?: IconDto;
  parameterBindings?: Record<string, string>;
}

export interface DiagramBoundsDto {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface DiagramConnectionDto {
  id: string;
  from: string;
  to: string;
  line?: LineDto;
  sourceRange?: SourceRangeDto;
}

export interface DiagramSceneDto {
  classQualifiedName?: string;
  classKind?: ClassKind;
  coordinateSystem: IconDto["coordinateSystem"];
  backgroundGraphics: GraphicItemDto[];
  components: ComponentInstanceDto[];
  connections: DiagramConnectionDto[];
  diagnostics: string[];
  contentBounds?: DiagramBoundsDto;
}

export type GetDiagramResult =
  | { scene: DiagramSceneDto }
  | { error: string };

export interface SourceEdit {
  start: number;
  end: number;
  expectedText?: string;
  replacement: string;
  sourceVersion?: number;
  targetQualifiedName?: string;
}

export type SourceEditReason =
  | "drag"
  | "resize"
  | "vertex"
  | "property"
  | "delete"
  | "create"
  | "undo"
  | "redo";

export type ApplySourceEditResult = { ok: true } | { error: string };

export interface CreateGraphicRequest {
  targetQualifiedName: string;
  graphicType: GraphicToolType;
  position: Point;
  sourceVersion?: number;
  graphic?: GraphicItemDto;
}

export type CreateGraphicResult =
  | { ok: true; graphicId: string; graphicPath: string; graphicText: string }
  | { error: string };

export type ReloadClassRangeResult =
  | { sourceRange: SourceRangeDto | null }
  | { error: string };
