import { app, BrowserWindow, dialog, ipcMain, shell } from "electron";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { PackageLoader } from "./modelica/loader.js";

const __dirname = fileURLToPath(new URL(".", import.meta.url));

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
