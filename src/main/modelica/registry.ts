import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
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

  addRoot(rootPath: string): LibraryInfo {
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
    const info = { path: rootPath, classCount };
    this.roots.set(rootPath, info);
    return info;
  }

  removeRoot(rootPath: string): void {
    this.roots.delete(rootPath);
    this.rebuildLocations();
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
    const namespace = target.qualifiedName.split(".").slice(0, -1);
    const candidates = baseName.includes(".")
      ? [baseName]
      : [...Array(namespace.length + 1)].map((_, index) => {
          const prefix = namespace.slice(0, namespace.length - index).join(".");
          return prefix ? `${prefix}.${baseName}` : baseName;
        });
    for (const candidate of candidates) {
      const location = this.resolve(candidate);
      if (location) return location;
    }
    return null;
  }

  private rebuildLocations(): void {
    const cachedSources = Array.from(this.sources.values());
    this.locations.clear();
    for (const root of Array.from(this.roots.keys())) {
      try { this.addRoot(root); } catch { /* stale root is ignored */ }
    }
    for (const cached of cachedSources) {
      this.registerSource(cached.sourceFile, cached.source, cached.parsed);
    }
  }
}
