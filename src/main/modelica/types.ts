export type ClassKind =
  | "package"
  | "model"
  | "block"
  | "connector"
  | "record"
  | "function"
  | "class"
  | "type";

export interface PackageNode {
  name: string;
  within: string | null;
  qualifiedName: string;
  sourceFile: string;
  children: PackageNode[];
  classes: ClassNode[];
  loadErrors?: string[];
}

export interface SourceRange {
  start: number;
  end: number;
}

export interface ClassNode {
  kind: ClassKind;
  name: string;
  qualifiedName: string;
  sourceFile: string;
  sourceRange: SourceRange;
  isPartial: boolean;
  isEncapsulated: boolean;
  children: ClassNode[];
}

export interface ModelicaFile {
  within: string | null;
  classes: ClassNode[];
}
