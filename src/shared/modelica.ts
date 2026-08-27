import type {
  IconDto,
  EditableIconDto,
  GraphicToolType,
  GraphicItemDto,
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
  | "type";

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
