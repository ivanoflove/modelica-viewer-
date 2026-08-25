import type { IconDto } from "./modelicaGraphics.js";

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
  children: ClassNodeDto[];
}

export type LoadPackageResult =
  | { canceled: true }
  | { canceled: false; root: PackageNodeDto }
  | { error: string };

export type ReadSourceResult =
  | { content: string; filePath: string }
  | { error: string };

export type GetIconResult = { icon: IconDto | null } | { error: string };
