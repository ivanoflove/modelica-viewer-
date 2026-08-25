import { contextBridge, ipcRenderer } from "electron";
import type { DirectoryDialogResult, WindowApi } from "../shared/api";
import type { LoadPackageResult } from "../shared/modelica";

const api: WindowApi = {
  ping: () => ipcRenderer.invoke("ping") as Promise<string>,
  openDirectory: () =>
    ipcRenderer.invoke(
      "dialog:openDirectory",
    ) as Promise<DirectoryDialogResult>,
  modelica: {
    openAndLoad: () =>
      ipcRenderer.invoke("modelica:openAndLoad") as Promise<LoadPackageResult>,
    loadDirectory: (dirPath: string) =>
      ipcRenderer.invoke(
        "modelica:loadDirectory",
        dirPath,
      ) as Promise<LoadPackageResult>,
    reveal: (filePath: string) =>
      ipcRenderer.invoke("modelica:reveal", filePath) as Promise<void>,
  },
};

contextBridge.exposeInMainWorld("api", api);
