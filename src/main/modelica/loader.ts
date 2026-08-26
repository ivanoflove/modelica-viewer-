import { readdirSync, readFileSync, statSync, existsSync } from "node:fs";
import { join, basename, extname } from "node:path";
import { parseModelicaFile, requalifyClassTree } from "./parser.js";
import type { PackageNode, ClassNode } from "./types.js";

function safeReadFile(filePath: string): string | null {
  try {
    return readFileSync(filePath, "utf-8");
  } catch {
    return null;
  }
}

function isDirectory(p: string): boolean {
  try {
    return statSync(p).isDirectory();
  } catch {
    return false;
  }
}

function listDirEntries(dir: string): string[] {
  try {
    return readdirSync(dir);
  } catch {
    return [];
  }
}

function classNodeToPackageNode(
  cls: ClassNode,
  parentQualified: string | null,
  sourceFile: string,
  within: string | null,
): PackageNode {
  // cls is kind===package
  const parentQ = parentQualified ?? within ?? "";
  const qualified = parentQ ? `${parentQ}.${cls.name}` : cls.name;
  // Requalify children
  const requalifiedChildren = cls.children.map((c) =>
    requalifyClassTree(c, qualified, sourceFile),
  );

  const pkgChildren: PackageNode[] = [];
  const pkgClasses: ClassNode[] = [];

  for (const ch of requalifiedChildren) {
    if (ch.kind === "package") {
      pkgChildren.push(classNodeToPackageNode(ch, qualified, sourceFile, null));
    } else {
      pkgClasses.push(ch);
    }
  }

  return {
    name: cls.name,
    within,
    qualifiedName: qualified,
    sourceFile,
    sourceRange: cls.sourceRange,
    children: pkgChildren,
    classes: pkgClasses,
  };
}

export const MISSING_PACKAGE_ERROR =
  "未找到 package.mo。该目录可能不是目录式 Modelica 库；如果你要打开单个模型/库文件，请使用‘打开 .mo 文件’。";

/**
 * Load one standalone .mo file. A top-level package is the semantic root;
 * unlike PackageLoader this path does not require the filename to be
 * package.mo or the containing directory to have a package.mo.
 */
export function loadModelicaFile(filePath: string): PackageNode {
  const content = readFileSync(filePath, "utf-8");
  const parsed = parseModelicaFile(content, filePath);
  const topLevelPackage = parsed.classes.find((cls) => cls.kind === "package");

  if (topLevelPackage) {
    const qualifiedPackage = requalifyClassTree(
      topLevelPackage,
      parsed.within ?? "",
      filePath,
    );
    return classNodeToPackageNode(
      qualifiedPackage,
      parsed.within,
      filePath,
      parsed.within,
    );
  }

  // A standalone file may contain a model/function without a package. Keep
  // it visible under a virtual root instead of rejecting a valid .mo file.
  const rootName = basename(filePath, extname(filePath));
  const rootQualified = parsed.within
    ? `${parsed.within}.${rootName}`
    : rootName;
  const classes = parsed.classes.map((cls) =>
    requalifyClassTree(cls, rootQualified, filePath),
  );
  const packageChildren = classes
    .filter((cls) => cls.kind === "package")
    .map((cls) => classNodeToPackageNode(cls, rootQualified, filePath, null));

  return {
    name: rootName,
    within: parsed.within,
    qualifiedName: rootQualified,
    sourceFile: filePath,
    children: packageChildren,
    classes: classes.filter((cls) => cls.kind !== "package"),
  };
}

export class PackageLoader {
  /**
   * Load a Modelica library rooted at `rootDir`.
   * - If root/package.mo exists, it is the root package.
   * - Recursively scans subdirectories containing package.mo and *.mo files.
   */
  load(rootDir: string): PackageNode {
    const errors: string[] = [];
    const rootPackageMo = join(rootDir, "package.mo");
    const hasRootPackageMo = existsSync(rootPackageMo);

    if (hasRootPackageMo) {
      return this.loadPackageDirectory(rootDir, null, errors);
    }

    // A common Modelica project layout is a directory named after a single
    // top-level package, containing `IEH_CPP/IEH_CPP.mo` plus Resources/.
    // Treat that unambiguous file as the directory's package entry point.
    const rootMoFiles = listDirEntries(rootDir).filter((entry) =>
      entry.toLowerCase().endsWith(".mo"),
    );
    const directoryName = basename(rootDir).toLowerCase();
    const matchingMoFile = rootMoFiles.find(
      (entry) => basename(entry, extname(entry)).toLowerCase() === directoryName,
    );
    const singleFile =
      matchingMoFile ?? (rootMoFiles.length === 1 ? rootMoFiles[0] : null);
    if (singleFile) {
      return loadModelicaFile(join(rootDir, singleFile));
    }

    throw new Error(MISSING_PACKAGE_ERROR);
  }

  private loadPackageDirectory(
    dir: string,
    parentQualified: string | null,
    errors: string[],
  ): PackageNode {
    const packageMoPath = join(dir, "package.mo");
    const content = safeReadFile(packageMoPath);

    if (!content) {
      // No package.mo but directory exists — treat as package by directory name
      const name = basename(dir);
      const q = parentQualified ? `${parentQualified}.${name}` : name;
      const children = this.scanSubdirectories(dir, q, errors);
      const classes = this.scanMoFiles(dir, q, errors);
      return {
        name,
        within: parentQualified,
        qualifiedName: q,
        sourceFile: packageMoPath,
        children,
        classes,
        loadErrors: errors.length ? undefined : undefined,
      };
    }

    let parsed;
    try {
      parsed = parseModelicaFile(content, packageMoPath);
    } catch (e) {
      errors.push(`${packageMoPath}: ${(e as Error).message}`);
      const name = basename(dir);
      const q = parentQualified ? `${parentQualified}.${name}` : name;
      return {
        name,
        within: parentQualified,
        qualifiedName: q,
        sourceFile: packageMoPath,
        children: this.scanSubdirectories(dir, q, errors),
        classes: this.scanMoFiles(dir, q, errors),
        loadErrors: errors.length ? errors : undefined,
      };
    }

    // Determine package class inside file: first class with matching name, or fallback to dir name
    // parsed.within is the within clause; parsed.classes are top-level definitions
    // For package.mo, there should be exactly one top-level package class
    let pkgClass: ClassNode | undefined;
    // Prefer package class whose name matches directory basename
    const dirName = basename(dir);
    pkgClass = parsed.classes.find(
      (c) => c.kind === "package" && c.name === dirName,
    );
    if (!pkgClass) {
      pkgClass = parsed.classes.find((c) => c.kind === "package");
    }

    let pkgName: string;
    let pkgWithin: string | null;
    let pkgQualified: string;
    const inlinePackages: PackageNode[] = [];
    const inlineClasses: ClassNode[] = [];

    if (pkgClass) {
      // within determines qualified name, but parentQualified from loader recursion takes precedence if mismatched
      pkgName = pkgClass.name;
      pkgWithin = parsed.within;

      // Build qualified: prefer within-based, fallback to parentQualified
      if (pkgWithin) {
        pkgQualified = `${pkgWithin}.${pkgName}`;
      } else if (parentQualified) {
        pkgQualified = `${parentQualified}.${pkgName}`;
      } else {
        pkgQualified = pkgName;
      }

      // Inline children of the package class
      for (const child of pkgClass.children) {
        const rq = requalifyClassTree(child, pkgQualified, packageMoPath);
        if (rq.kind === "package") {
          inlinePackages.push(
            classNodeToPackageNode(rq, pkgQualified, packageMoPath, null),
          );
        } else {
          inlineClasses.push(rq);
        }
      }
    } else {
      // No package class inside — maybe package.mo only contains within and then the package is implied by directory
      pkgName = dirName;
      pkgWithin = parsed.within;
      if (pkgWithin) pkgQualified = `${pkgWithin}.${pkgName}`;
      else if (parentQualified) pkgQualified = `${parentQualified}.${pkgName}`;
      else pkgQualified = pkgName;

      // Any top-level non-package classes become direct members
      for (const cls of parsed.classes) {
        if (cls.kind === "package") {
          inlinePackages.push(
            classNodeToPackageNode(
              requalifyClassTree(cls, pkgWithin ?? "", packageMoPath),
              pkgWithin,
              packageMoPath,
              pkgWithin,
            ),
          );
        } else {
          const rq = requalifyClassTree(cls, pkgQualified, packageMoPath);
          // parsed qualified already includes within if present; ensure it starts with pkgQualified if nested
          inlineClasses.push(rq);
        }
      }
    }

    // Scan filesystem for additional members:
    // - subdirectories with package.mo → recursively load
    // - *.mo files (excluding package.mo) → parse and add
    const subdirChildren = this.scanSubdirectories(dir, pkgQualified, errors);
    const moFileClasses = this.scanMoFiles(dir, pkgQualified, errors);

    // Merge inline packages with subdir packages: dedup by name (subdir wins or merge?)
    // For now, concat and allow duplicates — UI will show both; ideally merge by name
    const mergedChildren = this.mergePackageChildren(
      inlinePackages,
      subdirChildren,
    );
    const mergedClasses = [...inlineClasses, ...moFileClasses];

    return {
      name: pkgName!,
      within: pkgWithin!,
      qualifiedName: pkgQualified!,
      sourceFile: packageMoPath,
      sourceRange: pkgClass?.sourceRange,
      children: mergedChildren,
      classes: mergedClasses,
      loadErrors: errors.length ? [...errors] : undefined,
    };
  }

  private scanSubdirectories(
    parentDir: string,
    parentQualified: string,
    errors: string[],
  ): PackageNode[] {
    const entries = listDirEntries(parentDir);
    const result: PackageNode[] = [];
    for (const entry of entries) {
      const full = join(parentDir, entry);
      if (!isDirectory(full)) continue;
      // hidden / build dirs skip
      if (
        entry.startsWith(".") ||
        entry === "node_modules" ||
        entry === "__pycache__"
      )
        continue;
      const pkgMo = join(full, "package.mo");
      if (existsSync(pkgMo)) {
        // This subdir is a package
        const childNode = this.loadPackageDirectory(full, parentQualified, []);
        // propagate its errors upward? collect into local array and merge later
        if (childNode.loadErrors) errors.push(...childNode.loadErrors);
        // Re-qualify if its qualifiedName doesn't start with parentQualified
        // loadPackageDirectory already used parentQualified, so it should be correct
        result.push(childNode);
      } else {
        // Check if there are any .mo files inside that might indicate a package without package.mo
        const subEntries = listDirEntries(full);
        const hasMo = subEntries.some((f) => f.endsWith(".mo"));
        const hasSubPkgMo = subEntries.some((f) => {
          try {
            return (
              statSync(join(full, f)).isDirectory() &&
              existsSync(join(full, f, "package.mo"))
            );
          } catch {
            return false;
          }
        });
        if (hasMo || hasSubPkgMo) {
          // Treat as package directory without package.mo
          const q = `${parentQualified}.${entry}`;
          const classes = this.scanMoFiles(full, q, errors);
          const grandchildren = this.scanSubdirectories(full, q, errors);
          result.push({
            name: entry,
            within: parentQualified,
            qualifiedName: q,
            sourceFile: full,
            children: grandchildren,
            classes,
          });
        }
      }
    }
    return result;
  }

  private scanMoFiles(
    dir: string,
    parentQualified: string,
    errors: string[],
  ): ClassNode[] {
    const entries = listDirEntries(dir);
    const result: ClassNode[] = [];
    for (const entry of entries) {
      if (entry === "package.mo") continue;
      if (!entry.endsWith(".mo")) continue;
      const full = join(dir, entry);
      if (isDirectory(full)) continue;
      const content = safeReadFile(full);
      if (!content) continue;
      try {
        const parsed = parseModelicaFile(content, full);
        for (const cls of parsed.classes) {
          if (parsed.within) {
            const rq = requalifyClassTree(cls, parsed.within, full);
            result.push(rq);
          } else {
            const rq = requalifyClassTree(cls, parentQualified, full);
            result.push(rq);
          }
        }
      } catch (e) {
        errors.push(`${full}: ${(e as Error).message}`);
      }
    }
    return result;
  }

  private mergePackageChildren(
    inline: PackageNode[],
    subdirs: PackageNode[],
  ): PackageNode[] {
    const map = new Map<string, PackageNode>();
    for (const p of inline) map.set(p.name, p);
    for (const p of subdirs) {
      const existing = map.get(p.name);
      if (existing) {
        // merge children/classes of duplicate name (e.g., inline package + directory package)
        existing.children.push(...p.children);
        existing.classes.push(...p.classes);
        // keep existing qualified/source
      } else {
        map.set(p.name, p);
      }
    }
    return Array.from(map.values());
  }
}
