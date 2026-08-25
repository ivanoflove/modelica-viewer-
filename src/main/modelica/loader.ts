import { readdirSync, readFileSync, statSync, existsSync } from "node:fs";
import { join, basename } from "node:path";
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
    children: pkgChildren,
    classes: pkgClasses,
  };
}

export class PackageLoader {
  /**
   * Load a Modelica library rooted at `rootDir`.
   * - If root/package.mo exists, it is the root package.
   * - Else, root directory name is used as root package name (fallback).
   * - Recursively scans subdirectories containing package.mo and *.mo files.
   */
  load(rootDir: string): PackageNode {
    const errors: string[] = [];
    const rootPackageMo = join(rootDir, "package.mo");
    const hasRootPackageMo = existsSync(rootPackageMo);

    if (hasRootPackageMo) {
      return this.loadPackageDirectory(rootDir, null, errors);
    }

    // Fallback: treat root as package named after directory
    const fallbackName = basename(rootDir);
    const moFiles = listDirEntries(rootDir).filter((f) => f.endsWith(".mo"));
    const classes: ClassNode[] = [];
    for (const f of moFiles) {
      const fp = join(rootDir, f);
      const content = safeReadFile(fp);
      if (!content) continue;
      try {
        const parsed = parseModelicaFile(content, fp);
        for (const cls of parsed.classes) {
          const rq = requalifyClassTree(cls, parsed.within ?? "", fp);
          // if within is null, we need to prefix with fallback
          const finalQ = parsed.within
            ? rq.qualifiedName
            : `${fallbackName}.${rq.name}`;
          const finalNode: ClassNode = {
            ...rq,
            qualifiedName: parsed.within ? rq.qualifiedName : finalQ,
            sourceFile: fp,
            children: rq.children.map((c) => requalifyClassTree(c, finalQ, fp)),
          };
          classes.push(finalNode);
        }
      } catch (e) {
        errors.push(`${fp}: ${(e as Error).message}`);
      }
    }

    // scan subdirectories
    const children = this.scanSubdirectories(rootDir, fallbackName, errors);

    return {
      name: fallbackName,
      within: null,
      qualifiedName: fallbackName,
      sourceFile: rootDir,
      children,
      classes: classes.filter((c) => c.kind !== "package") as ClassNode[],
      loadErrors: errors.length ? errors : undefined,
    };
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
