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

export interface PackageNode {
  name: string;
  within: string | null;
  qualifiedName: string;
  sourceFile: string;
  sourceRange?: SourceRange;
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
  extendsClauses: string[];
  children: ClassNode[];
  /** Short class definitions terminate with `;`, not `end <name>;`. */
  isShort?: boolean;
  baseTypeName?: string;
  basePrefixes?: {
    input?: boolean;
    output?: boolean;
    flow?: boolean;
    stream?: boolean;
  };
}

export interface ModelicaFile {
  within: string | null;
  classes: ClassNode[];
}
