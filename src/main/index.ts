import { app, BrowserWindow, dialog, ipcMain, shell } from "electron";
import type { OpenDialogOptions } from "electron";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { readFile, rename, unlink, writeFile } from "node:fs/promises";
import { readFileSync, writeFileSync } from "node:fs";
import { loadModelicaFile, PackageLoader } from "./modelica/loader.js";
import {
  extractIconFromSlice,
  extractEditableIconFromSlice,
  toAbsoluteEditableRanges,
  findIconAnnotationOffset,
  findIconSourceRange,
  findClassByQualifiedNameOrUniqueLeaf,
  findClassBySourceRange,
  resolveIconForClass,
  buildGraphicInsertionEdit,
} from "./modelica/iconResolver.js";
import { parseModelicaFile } from "./modelica/parser.js";
import { tokenize } from "./modelica/lexer.js";
import { ModelicaLibraryRegistry } from "./modelica/registry.js";
import {
  applySourceTransaction,
  SourceTransactionError,
} from "./modelica/sourceTransaction.js";
import type {
  CreateGraphicRequest,
  SourceEdit,
} from "../shared/modelica.js";
import type {
  GraphicItemDto,
  GraphicToolType,
  Point,
} from "../shared/modelicaGraphics.js";

const __dirname = fileURLToPath(new URL(".", import.meta.url));

function cliOpenPath(): string | null {
  const idx = process.argv.indexOf("--open");
  if (idx === -1) return null;
  const p = process.argv[idx + 1];
  return p || null;
}

const AUTO_OPEN_PATH = cliOpenPath();

function sourceLocation(source: string, offset: number): string {
  const safeOffset = Math.max(0, Math.min(offset, source.length));
  const before = source.slice(0, safeOffset);
  const line = before.split("\n").length;
  const lastNewline = before.lastIndexOf("\n");
  const column = safeOffset - lastNewline;
  const lineText = source.split("\n")[line - 1]?.trim() ?? "";
  return `line ${line}, column ${column}${lineText ? ` near ${JSON.stringify(lineText.slice(0, 160))}` : ""}`;
}

function formatModelicaNumber(value: number): string {
  return Number.isInteger(value) ? String(value) : String(Number(value.toFixed(6)));
}

function formatPoint(point: Point): string {
  return `{${formatModelicaNumber(point.x)},${formatModelicaNumber(point.y)}}`;
}

function formatExtent(extent: { p1: Point; p2: Point }): string {
  return `{${formatPoint(extent.p1)},${formatPoint(extent.p2)}}`;
}

function formatPoints(points: Point[]): string {
  return `{${points.map(formatPoint).join(",")}}`;
}

function formatColor(color: [number, number, number]): string {
  return `{${color.join(",")}}`;
}

function quoteModelicaString(value: string): string {
  return `"${value.replace(/"/g, '""')}"`;
}

function serializeCreatedGraphic(graphic: GraphicItemDto): string {
  const origin = graphic.origin ? `origin=${formatPoint(graphic.origin)}, ` : "";
  switch (graphic.type) {
    case "Line":
      return `Line(${origin}points=${formatPoints(graphic.points)}, color=${formatColor(graphic.color ?? [0, 0, 0])}, thickness=${formatModelicaNumber(graphic.thickness ?? 0.25)})`;
    case "Polygon":
      return `Polygon(${origin}points=${formatPoints(graphic.points)}, lineColor=${formatColor(graphic.lineColor ?? [0, 0, 0])}, fillColor=${formatColor(graphic.fillColor ?? [255, 255, 255])}, fillPattern=${graphic.fillPattern ?? "FillPattern.None"})`;
    case "Rectangle":
      return `Rectangle(${origin}extent=${formatExtent(graphic.extent)}, lineColor=${formatColor(graphic.lineColor ?? [0, 0, 0])}, fillColor=${formatColor(graphic.fillColor ?? [255, 255, 255])}, fillPattern=${graphic.fillPattern ?? "FillPattern.None"})`;
    case "Ellipse":
      return `Ellipse(${origin}extent=${formatExtent(graphic.extent)}, lineColor=${formatColor(graphic.lineColor ?? [0, 0, 0])}, fillColor=${formatColor(graphic.fillColor ?? [255, 255, 255])}, fillPattern=${graphic.fillPattern ?? "FillPattern.None"})`;
    case "Text":
      return `Text(${origin}extent=${formatExtent(graphic.extent)}, textString=${quoteModelicaString(graphic.textString)}, textColor=${formatColor(graphic.textColor ?? [0, 0, 0])})`;
    case "Bitmap":
      return `Bitmap(${origin}extent=${formatExtent(graphic.extent)})`;
  }
}

function defaultGraphicText(type: GraphicToolType, x: number, y: number): string {
  const origin = `{${formatModelicaNumber(x)},${formatModelicaNumber(y)}}`;
  switch (type) {
    case "Line":
      return `Line(origin=${origin}, points={{-20,0},{20,0}}, color={0,0,0}, thickness=0.25)`;
    case "Polygon":
      return `Polygon(origin=${origin}, points={{-20,-15},{20,-15},{0,20},{-20,-15}}, lineColor={0,0,0}, fillColor={255,255,255}, fillPattern=FillPattern.None)`;
    case "Rectangle":
      return `Rectangle(origin=${origin}, extent={{-20,-15},{20,15}}, lineColor={0,0,0}, fillColor={255,255,255}, fillPattern=FillPattern.None)`;
    case "Ellipse":
      return `Ellipse(origin=${origin}, extent={{-20,-20},{20,20}}, lineColor={0,0,0}, fillColor={255,255,255}, fillPattern=FillPattern.None)`;
    case "Text":
      return `Text(origin=${origin}, extent={{-30,-12},{30,12}}, textString="Text", textColor={0,0,0})`;
    case "Bitmap":
      return `Bitmap(origin=${origin}, extent={{-30,-30},{30,30}})`;
  }
}

async function replaceSourceFile(filePath: string, updated: string): Promise<void> {
  const tempPath = `${filePath}.tmp-${process.pid}-${Date.now()}`;
  let backupPath: string | null = null;
  try {
    await writeFile(tempPath, updated, "utf-8");
    if (process.platform === "win32") {
      backupPath = `${filePath}.bak-${process.pid}-${Date.now()}`;
      await rename(filePath, backupPath);
      try {
        await rename(tempPath, filePath);
      } catch (error) {
        try { await rename(backupPath, filePath); backupPath = null; } catch { /* preserve original error */ }
        throw error;
      }
      try { await unlink(backupPath); } finally { backupPath = null; }
    } else {
      await rename(tempPath, filePath);
    }
  } finally {
    try { await unlink(tempPath); } catch { /* already moved or best effort */ }
    if (backupPath) {
      try { await rename(backupPath, filePath); } catch { /* recoverable backup remains */ }
    }
  }
}

function createWindow(): void {
  const window = new BrowserWindow({
    width: 1280,
    height: 800,
    webPreferences: {
      preload: join(__dirname, "../preload/index.cjs"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false,
    },
  });

  if (process.env.ELECTRON_RENDERER_URL) {
    void window.loadURL(process.env.ELECTRON_RENDERER_URL);
  } else {
    void window.loadFile(join(__dirname, "../renderer/index.html"));
  }

  if (AUTO_OPEN_PATH) {
    window.webContents.on("did-finish-load", () => {
      window.webContents.send("modelica:auto-open", AUTO_OPEN_PATH);
    });
  }
}

function registerIpcHandlers(): void {
  const loader = new PackageLoader();
  const libraryRegistry = new ModelicaLibraryRegistry();
  const sourceVersions = new Map<string, number>();
  const libraryConfigPath = join(
    app.getPath("userData"),
    "modelica-libraries.json",
  );
  try {
    const configured = JSON.parse(
      readFileSync(libraryConfigPath, "utf8"),
    ) as unknown;
    if (Array.isArray(configured)) {
      for (const path of configured) {
        if (typeof path === "string") {
          try {
            libraryRegistry.addRoot(path);
          } catch {
            /* stale library path */
          }
        }
      }
    }
  } catch {
    // First run or an invalid settings file: start with no extra libraries.
  }
  const persistLibraries = () => {
    writeFileSync(
      libraryConfigPath,
      JSON.stringify(
        libraryRegistry.listRoots().map((item) => item.path),
        null,
        2,
      ),
      "utf8",
    );
  };

  ipcMain.handle("ping", () => "pong");

  ipcMain.handle("dialog:openDirectory", async () => {
    const win = BrowserWindow.getFocusedWindow();
    const result = win
      ? await dialog.showOpenDialog(win, { properties: ["openDirectory"] })
      : await dialog.showOpenDialog({ properties: ["openDirectory"] });
    if (result.canceled || result.filePaths.length === 0) {
      return { canceled: true };
    }
    return { canceled: false, path: result.filePaths[0] };
  });

  ipcMain.handle("modelica:loadDirectory", async (_event, dirPath: string) => {
    try {
      if (!dirPath) return { error: "No directory path provided" };
      const root = loader.load(dirPath);
      libraryRegistry.registerCurrentPackage(root);
      return { canceled: false, root };
    } catch (e) {
      return { error: (e as Error).message };
    }
  });

  ipcMain.handle("modelica:loadFile", async (_event, filePath: string) => {
    try {
      if (!filePath) return { error: "No Modelica file path provided" };
      const root = loadModelicaFile(filePath);
      libraryRegistry.registerCurrentPackage(root);
      return { canceled: false, root };
    } catch (e) {
      return { error: (e as Error).message };
    }
  });

  ipcMain.handle("modelica:openFile", async () => {
    const win = BrowserWindow.getFocusedWindow();
    const options: OpenDialogOptions = {
      title: "打开 Modelica 文件",
      properties: ["openFile"],
      filters: [{ name: "Modelica files", extensions: ["mo"] }],
    };
    const picked = win
      ? await dialog.showOpenDialog(win, options)
      : await dialog.showOpenDialog(options);
    if (picked.canceled || picked.filePaths.length === 0) {
      return { canceled: true };
    }
    try {
      const root = loadModelicaFile(picked.filePaths[0]!);
      libraryRegistry.registerCurrentPackage(root);
      return { canceled: false, root };
    } catch (e) {
      return { error: (e as Error).message };
    }
  });

  ipcMain.handle("modelica:openAndLoad", async () => {
    const win = BrowserWindow.getFocusedWindow();
    const picked = win
      ? await dialog.showOpenDialog(win, { properties: ["openDirectory"] })
      : await dialog.showOpenDialog({ properties: ["openDirectory"] });
    if (picked.canceled || picked.filePaths.length === 0) {
      return { canceled: true };
    }
    try {
      const root = loader.load(picked.filePaths[0]!);
      libraryRegistry.registerCurrentPackage(root);
      return { canceled: false, root };
    } catch (e) {
      return { error: (e as Error).message };
    }
  });

  ipcMain.handle("modelica:listLibraries", () => libraryRegistry.listRoots());

  ipcMain.handle("modelica:addLibrary", async () => {
    const win = BrowserWindow.getFocusedWindow();
    const picked = win
      ? await dialog.showOpenDialog(win, {
          title: "添加 Modelica 库",
          properties: ["openDirectory"],
        })
      : await dialog.showOpenDialog({
          title: "添加 Modelica 库",
          properties: ["openDirectory"],
        });
    if (picked.canceled || picked.filePaths.length === 0)
      return { error: "canceled" };
    try {
      const library = libraryRegistry.addRoot(picked.filePaths[0]!);
      persistLibraries();
      return { ok: true, library };
    } catch (e) {
      return { error: (e as Error).message };
    }
  });

  ipcMain.handle("modelica:removeLibrary", (_event, path: string) => {
    if (!path) return { error: "No library path provided" };
    libraryRegistry.removeRoot(path);
    persistLibraries();
    return { ok: true };
  });

  ipcMain.handle("modelica:rescanLibraries", () => {
    const libraries = libraryRegistry.rescan();
    persistLibraries();
    return libraries;
  });

  ipcMain.handle("modelica:reveal", async (_event, filePath: string) => {
    if (filePath) shell.showItemInFolder(filePath);
  });

  ipcMain.handle("modelica:readSource", async (_event, filePath: string) => {
    try {
      if (!filePath) return { error: "No file path provided" };
      const content = await readFile(filePath, "utf-8");
      return { content, filePath };
    } catch (e) {
      return { error: (e as Error).message };
    }
  });

  ipcMain.handle(
    "modelica:getIcon",
    async (
      _event,
      filePath: string,
      sourceRange: { start: number; end: number } | null,
      modelName: string,
    ) => {
      try {
        if (!filePath) return { error: "No file path provided" };
        const content = await readFile(filePath, "utf-8");
        const parsed = parseModelicaFile(content, filePath);
        libraryRegistry.registerSource(filePath, content, parsed);
        const target = findClassBySourceRange(parsed.classes, sourceRange);
        if (target) {
          const resolved = resolveIconForClass(
            target,
            parsed.classes,
            content,
            modelName ?? target.name,
            new Set<string>(),
            (owner, baseName) => libraryRegistry.resolveFor(owner, baseName),
          );
          return {
            icon: resolved.icon,
            warnings: resolved.warnings.length ? resolved.warnings : undefined,
          };
        }
        const slice = sourceRange
          ? content.slice(sourceRange.start, sourceRange.end)
          : content;
        return { icon: extractIconFromSlice(slice, modelName ?? "") };
      } catch (e) {
        return { error: (e as Error).message };
      }
    },
  );

  ipcMain.handle(
    "modelica:getEditableIcon",
    async (
      _event,
      filePath: string,
      sourceRange: { start: number; end: number } | null,
      modelName: string,
    ) => {
      try {
        if (!filePath) return { error: "No file path provided" };
        const content = await readFile(filePath, "utf-8");
        const parsed = parseModelicaFile(content, filePath);
        libraryRegistry.registerSource(filePath, content, parsed);
        const slice = sourceRange
          ? content.slice(sourceRange.start, sourceRange.end)
          : content;
        const sliceBase = sourceRange ? sourceRange.start : 0;
        const annotationIdx = findIconAnnotationOffset(slice);
        const target = findClassBySourceRange(parsed.classes, sourceRange);
        const editable = extractEditableIconFromSlice(
          slice,
          modelName ?? target?.name ?? "",
          target
            ? {
                qualifiedName: target.qualifiedName,
                sourceFile: filePath,
              }
            : undefined,
        );
        if (editable && annotationIdx >= 0) {
          return {
            editable: {
              ...toAbsoluteEditableRanges(editable, sliceBase, annotationIdx),
              sourceVersion: sourceVersions.get(filePath) ?? 0,
            },
          };
        }
        return { editable };
      } catch (e) {
        return { error: (e as Error).message };
      }
    },
  );

  ipcMain.handle(
    "modelica:reloadClassRange",
    async (_event, filePath: string, qualifiedName: string) => {
      try {
        if (!filePath || !qualifiedName) return { error: "Missing args" };
        const content = await readFile(filePath, "utf-8");
        const parsed = parseModelicaFile(content, filePath);
        const found = findClassByQualifiedNameOrUniqueLeaf(
          parsed.classes,
          qualifiedName,
        );
        return { sourceRange: found ? found.sourceRange : null };
      } catch (e) {
        return { error: (e as Error).message };
      }
    },
  );

  ipcMain.handle(
    "modelica:applySourceEdit",
    async (_event, filePath: string, edit: SourceEdit) => {
      let tempPath: string | null = null;
      let backupPath: string | null = null;
      try {
        if (!filePath || !edit) return { error: "Missing filePath or edit" };
        if (edit.expectedText === undefined) {
          return {
            error:
              "STALE_SOURCE_RANGE: expectedText is required for source edits",
          };
        }
        if (edit.sourceVersion === undefined) {
          return {
            error:
              "SOURCE_VERSION_MISMATCH: sourceVersion is required for source edits",
          };
        }
        if (!edit.targetQualifiedName) {
          return {
            error:
              "CLASS_NOT_FOUND: targetQualifiedName is required for source edits",
          };
        }
        const content = await readFile(filePath, "utf-8");
        const currentVersion = sourceVersions.get(filePath) ?? 0;
        const updated = applySourceTransaction(
          content,
          { filePath, sourceVersion: edit.sourceVersion, edits: [edit] },
          currentVersion,
        );
        let candidate;
        try {
          candidate = parseModelicaFile(updated, filePath);
        } catch (e) {
          return {
            error: `SOURCE_PARSE_ERROR: ${(e as Error).message} (${sourceLocation(updated, edit.start)})`,
          };
        }
        const reparsedTarget = findClassByQualifiedNameOrUniqueLeaf(
          candidate.classes,
          edit.targetQualifiedName,
        );
        const target =
          reparsedTarget &&
          reparsedTarget.qualifiedName !== edit.targetQualifiedName
            ? { ...reparsedTarget, qualifiedName: edit.targetQualifiedName }
            : reparsedTarget;
        if (!target)
          return {
            error: `TARGET_CLASS_NOT_FOUND: ${edit.targetQualifiedName}`,
          };
        const resolved = resolveIconForClass(
          target,
          candidate.classes,
          updated,
          target.name,
          new Set<string>(),
          (owner, baseName) => libraryRegistry.resolveFor(owner, baseName),
        );
        if (!resolved.icon) {
          const targetSlice = updated.slice(
            target.sourceRange.start,
            target.sourceRange.end,
          );
          const tokens = tokenize(targetSlice);
          const iconToken = tokens.find(
            (token, index) =>
              token.value === "Icon" && tokens[index + 1]?.type === "LPAREN",
          );
          const hasIconCall = !!iconToken;
          const iconRange = findIconSourceRange(targetSlice);
          const iconStart =
            target.sourceRange.start +
            (iconRange?.start ?? iconToken?.start ?? 0);
          if (!hasIconCall) {
            return {
              error: `OWN_ICON_NOT_FOUND: ${edit.targetQualifiedName} (${sourceLocation(updated, target.sourceRange.start)})`,
            };
          }
          if (!iconRange) {
            return {
              error: `ICON_RANGE_ERROR: Icon range is invalid in ${edit.targetQualifiedName}; iconStart=${iconStart}; iconRange=${JSON.stringify(iconRange)} (${sourceLocation(updated, iconStart)})`,
            };
          }
          const iconText = targetSlice.slice(iconRange.start, iconRange.end);
          if (!iconText.trimStart().startsWith("Icon(")) {
            return {
              error: `ICON_RANGE_ERROR: expected Icon(...) range in ${edit.targetQualifiedName}, got ${JSON.stringify(iconText.slice(0, 120))} (${sourceLocation(updated, iconStart)})`,
            };
          }
          return {
            error: `ICON_SYNTAX_ERROR: invalid Icon annotation in ${edit.targetQualifiedName}; iconRange=${JSON.stringify(iconRange)} (${sourceLocation(updated, iconStart)})`,
          };
        }
        if (resolved.warnings.length > 0) {
          console.debug("[ICON_RESOLVE_WARNING]", {
            targetQualifiedName: edit.targetQualifiedName,
            warnings: resolved.warnings,
          });
        }
        tempPath = `${filePath}.tmp-${process.pid}-${Date.now()}`;
        await writeFile(tempPath, updated, "utf-8");
        if (process.platform === "win32") {
          // Windows rename() cannot replace an existing file. Move the old
          // file aside, then replace it and restore it if the second move
          // fails. The temporary file is still fully written before either
          // rename, so a successful edit never exposes a partial source.
          backupPath = `${filePath}.bak-${process.pid}-${Date.now()}`;
          await rename(filePath, backupPath);
          try {
            await rename(tempPath, filePath);
            tempPath = null;
          } catch (e) {
            try {
              await rename(backupPath, filePath);
              backupPath = null;
            } catch {
              // Keep the original error; the backup remains recoverable.
            }
            throw e;
          }
          try {
            await unlink(backupPath);
          } finally {
            backupPath = null;
          }
        } else {
          await rename(tempPath, filePath);
          tempPath = null;
        }
        sourceVersions.set(filePath, currentVersion + 1);
        return { ok: true };
      } catch (e) {
        if (e instanceof SourceTransactionError) {
          return {
            error: `${e.code}: ${e.message}; target=${edit.targetQualifiedName ?? "unknown"}; range=${edit.start}:${edit.end}; replacement=${JSON.stringify(edit.replacement)}`,
          };
        }
        return { error: (e as Error).message };
      } finally {
        if (tempPath) {
          try {
            await unlink(tempPath);
          } catch {
            // Best-effort cleanup of an interrupted temporary write.
          }
        }
      }
    },
  );

  ipcMain.handle(
    "modelica:createGraphic",
    async (_event, filePath: string, request: CreateGraphicRequest) => {
      try {
        if (!filePath || !request?.targetQualifiedName) {
          return { error: "Missing filePath or create request" };
        }
        if (!Number.isFinite(request.position.x) || !Number.isFinite(request.position.y)) {
          return { error: "INVALID_DROP_POSITION: graphic position is not finite" };
        }
        if (request.sourceVersion === undefined) {
          return { error: "SOURCE_VERSION_MISMATCH: sourceVersion is required" };
        }
        const content = await readFile(filePath, "utf-8");
        const currentVersion = sourceVersions.get(filePath) ?? 0;
        const parsed = parseModelicaFile(content, filePath);
        const target = findClassByQualifiedNameOrUniqueLeaf(
          parsed.classes,
          request.targetQualifiedName,
        );
        if (!target) return { error: `TARGET_CLASS_NOT_FOUND: ${request.targetQualifiedName}` };
        const classSlice = content.slice(target.sourceRange.start, target.sourceRange.end);
        if (request.graphic && request.graphic.type !== request.graphicType) {
          return { error: "INVALID_GRAPHIC: graphic type does not match tool" };
        }
        const graphicText = request.graphic
          ? serializeCreatedGraphic(request.graphic)
          : defaultGraphicText(request.graphicType, request.position.x, request.position.y);
        const insertion = buildGraphicInsertionEdit(classSlice, graphicText);
        if (!insertion) {
          return { error: `ICON_RANGE_ERROR: unable to locate an annotation insertion point in ${request.targetQualifiedName}` };
        }
        const edit: SourceEdit = {
          start: target.sourceRange.start + insertion.start,
          end: target.sourceRange.start + insertion.end,
          expectedText: insertion.expectedText,
          replacement: insertion.replacement,
          sourceVersion: request.sourceVersion,
          targetQualifiedName: request.targetQualifiedName,
        };
        const updated = applySourceTransaction(
          content,
          { filePath, sourceVersion: request.sourceVersion, edits: [edit] },
          currentVersion,
        );
        let candidate;
        try {
          candidate = parseModelicaFile(updated, filePath);
        } catch (error) {
          return { error: `SOURCE_PARSE_ERROR: ${(error as Error).message} (${sourceLocation(updated, edit.start)})` };
        }
        const candidateTarget = findClassByQualifiedNameOrUniqueLeaf(
          candidate.classes,
          request.targetQualifiedName,
        );
        if (!candidateTarget) return { error: `TARGET_CLASS_NOT_FOUND: ${request.targetQualifiedName}` };
        const candidateSlice = updated.slice(candidateTarget.sourceRange.start, candidateTarget.sourceRange.end);
        const candidateEditable = extractEditableIconFromSlice(candidateSlice, candidateTarget.name);
        if (!candidateEditable?.icon) {
          return { error: `ICON_PARSE_ERROR: created graphic could not be resolved in ${request.targetQualifiedName}` };
        }
        const graphicIndex = candidateEditable.editables.length - 1;
        if (graphicIndex < 0 || candidateEditable.editables[graphicIndex]?.graphic.type !== request.graphicType) {
          return { error: `ICON_PARSE_ERROR: created ${request.graphicType} is missing after validation` };
        }
        await replaceSourceFile(filePath, updated);
        sourceVersions.set(filePath, currentVersion + 1);
        return {
          ok: true,
          graphicId: `${request.targetQualifiedName}:Icon.graphics:${graphicIndex}`,
          graphicPath: `Icon.graphics:${graphicIndex}`,
          graphicText,
        };
      } catch (error) {
        if (error instanceof SourceTransactionError) {
          return { error: `${error.code}: ${error.message}` };
        }
        return { error: (error as Error).message };
      }
    },
  );
}

app.whenReady().then(() => {
  registerIpcHandlers();
  createWindow();

  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow();
  });
});

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") app.quit();
});
