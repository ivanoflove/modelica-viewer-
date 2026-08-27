import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { basename, join } from "node:path";
import { parseModelicaFile } from "./parser.js";
import type { ClassNode, ModelicaFile, PackageNode } from "./types.js";
import { PackageLoader } from "./loader.js";

export interface ClassLocation {
  target: ClassNode;
  allClasses: ClassNode[];
  source: string;
}

export interface LibraryInfo {
  path: string;
  classCount: number;
  name?: string;
  version?: string;
  builtin?: boolean;
  readOnly?: boolean;
}

function flatten(classes: ClassNode[], result: ClassNode[] = []): ClassNode[] {
  for (const cls of classes) {
    result.push(cls);
    flatten(cls.children, result);
  }
  return result;
}

function walkPackage(node: PackageNode, files: Set<string>): void {
  files.add(node.sourceFile);
  for (const cls of node.classes) files.add(cls.sourceFile);
  for (const child of node.children) walkPackage(child, files);
}

function listModelicaFiles(root: string): string[] {
  const result: string[] = [];
  const visit = (dir: string) => {
    for (const entry of readdirSync(dir)) {
      if (entry.startsWith(".") || entry === "Resources" || entry === "node_modules") continue;
      const path = join(dir, entry);
      let stat;
      try { stat = statSync(path); } catch { continue; }
      if (stat.isDirectory()) visit(path);
      else if (entry.toLowerCase().endsWith(".mo")) result.push(path);
    }
  };
  visit(root);
  return result;
}

/** A session-scoped index of Modelica classes from all registered libraries. */
export class ModelicaLibraryRegistry {
  private readonly roots = new Map<string, LibraryInfo>();
  private readonly locations = new Map<string, ClassLocation>();
  private readonly sources = new Map<string, { sourceFile: string; source: string; parsed: ModelicaFile }>();

  registerSource(sourceFile: string, source: string, parsed = parseModelicaFile(source, sourceFile)): void {
    this.sources.set(sourceFile, { sourceFile, source, parsed });
    const classes = flatten(parsed.classes);
    for (const target of classes) {
      this.locations.set(target.qualifiedName, { target, allClasses: parsed.classes, source });
    }
  }

  registerCurrentPackage(root: PackageNode): void {
    const files = new Set<string>();
    walkPackage(root, files);
    for (const file of Array.from(files)) {
      try {
        const source = readFileSync(file, "utf8");
        this.registerSource(file, source);
      } catch {
        // The active document can still be resolved by the caller's parsed tree.
      }
    }
  }

  addRoot(
    rootPath: string,
    metadata: Pick<LibraryInfo, "name" | "version" | "builtin" | "readOnly"> = {},
  ): LibraryInfo {
    if (!rootPath || !existsSync(join(rootPath, "package.mo"))) {
      throw new Error("Modelica library root must contain package.mo");
    }
    const root = new PackageLoader().load(rootPath);
    const files = new Set<string>();
    walkPackage(root, files);
    let classCount = 0;
    for (const file of listModelicaFiles(rootPath)) files.add(file);
    for (const file of Array.from(files)) {
      try {
        const source = readFileSync(file, "utf8");
        const parsed = parseModelicaFile(source, file);
        this.registerSource(file, source, parsed);
        classCount += flatten(parsed.classes).length;
      } catch {
        // A broken optional file should not prevent the rest of a library loading.
      }
    }
    const info = {
      path: rootPath,
      classCount,
      name: metadata.name ?? basename(rootPath),
      version: metadata.version,
      builtin: metadata.builtin ?? false,
      readOnly: metadata.readOnly ?? false,
    };
    this.roots.set(rootPath, info);
    return info;
  }

  removeRoot(rootPath: string): void {
    if (this.roots.get(rootPath)?.builtin) return;
    this.roots.delete(rootPath);
    this.rebuildLocations();
  }

  isReadOnlyPath(filePath: string): boolean {
    const normalize = (value: string) => value.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
    const candidate = normalize(filePath);
    for (const [rootPath, info] of Array.from(this.roots.entries())) {
      const root = normalize(rootPath);
      if (info.readOnly && (candidate === root || candidate.startsWith(`${root}/`))) return true;
    }
    return false;
  }

  rescan(): LibraryInfo[] {
    this.rebuildLocations();
    return this.listRoots();
  }

  listRoots(): LibraryInfo[] {
    return Array.from(this.roots.values());
  }

  resolve(qualifiedName: string): ClassLocation | null {
    return this.locations.get(qualifiedName) ?? null;
  }

  resolveFor(target: ClassNode, baseName: string): ClassLocation | null {
    const candidates = this.hasTopLevelLibrary(baseName)
      ? [baseName]
      : typeNameCandidates(target, baseName);
    for (const candidate of candidates) {
      const location = this.resolve(candidate);
      if (location) return location;
    }
    return null;
  }

  private hasTopLevelLibrary(typeName: string): boolean {
    const root = typeName.split(".")[0];
    if (!root) return false;
    for (const qualifiedName of Array.from(this.locations.keys())) {
      if (qualifiedName.split(".")[0] === root) return true;
    }
    return false;
  }

  private rebuildLocations(): void {
    const cachedSources = Array.from(this.sources.values());
    this.locations.clear();
    for (const root of Array.from(this.roots.keys())) {
      try {
        const metadata = this.roots.get(root);
        this.addRoot(root, metadata ?? {});
      } catch { /* stale root is ignored */ }
    }
    for (const cached of cachedSources) {
      this.registerSource(cached.sourceFile, cached.source, cached.parsed);
    }
  }
}

/** Candidates for a Modelica type reference, from exact to enclosing scope. */
export function typeNameCandidates(target: ClassNode, typeName: string): string[] {
  // A root-qualified standard library reference must not be prefixed with
  // the lexical package (for example IEH_CPP.Modelica.Blocks...).
  if (typeName === "Modelica" || typeName.startsWith("Modelica.")) return [typeName];
  const namespace = target.qualifiedName.split(".").slice(0, -1);
  const candidates = new Set<string>([typeName]);
  for (let length = namespace.length; length >= 0; length--) {
    const prefix = namespace.slice(0, length).join(".");
    candidates.add(prefix ? `${prefix}.${typeName}` : typeName);
  }
  return Array.from(candidates);
}
