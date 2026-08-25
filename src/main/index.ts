import { app, BrowserWindow, dialog, ipcMain, shell } from "electron";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { readFile, writeFile } from "node:fs/promises";
import { PackageLoader } from "./modelica/loader.js";
import {
  extractIconFromSlice,
  extractEditableIconFromSlice,
  toAbsoluteEditableRanges,
  findClassByQualifiedName,
} from "./modelica/iconResolver.js";
import { parseModelicaFile } from "./modelica/parser.js";

const __dirname = fileURLToPath(new URL(".", import.meta.url));

function cliOpenPath(): string | null {
  const idx = process.argv.indexOf("--open");
  if (idx === -1) return null;
  const p = process.argv[idx + 1];
  return p || null;
}

const AUTO_OPEN_PATH = cliOpenPath();

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
      return { canceled: false, root };
    } catch (e) {
      return { error: (e as Error).message };
    }
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
        const slice = sourceRange
          ? content.slice(sourceRange.start, sourceRange.end)
          : content;
        const icon = extractIconFromSlice(slice, modelName ?? "");
        return { icon };
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
        const slice = sourceRange
          ? content.slice(sourceRange.start, sourceRange.end)
          : content;
        const sliceBase = sourceRange ? sourceRange.start : 0;
        const annotationIdx = slice.indexOf("annotation");
        const editable = extractEditableIconFromSlice(slice, modelName ?? "");
        if (editable && annotationIdx >= 0) {
          return {
            editable: toAbsoluteEditableRanges(
              editable,
              sliceBase,
              annotationIdx,
            ),
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
        const found = findClassByQualifiedName(parsed.classes, qualifiedName);
        return { sourceRange: found ? found.sourceRange : null };
      } catch (e) {
        return { error: (e as Error).message };
      }
    },
  );

  ipcMain.handle(
    "modelica:applySourceEdit",
    async (
      _event,
      filePath: string,
      edit: { start: number; end: number; replacement: string },
    ) => {
      try {
        if (!filePath || !edit) return { error: "Missing filePath or edit" };
        const content = await readFile(filePath, "utf-8");
        if (
          edit.start < 0 ||
          edit.end > content.length ||
          edit.start > edit.end
        ) {
          return { error: "Invalid edit range" };
        }
        const updated =
          content.slice(0, edit.start) +
          edit.replacement +
          content.slice(edit.end);
        await writeFile(filePath, updated, "utf-8");
        return { ok: true };
      } catch (e) {
        return { error: (e as Error).message };
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
