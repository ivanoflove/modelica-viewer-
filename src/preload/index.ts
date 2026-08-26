import { contextBridge, ipcRenderer } from "electron";
import type { DirectoryDialogResult, WindowApi } from "../shared/api";
import type {
  ApplySourceEditResult,
  GetEditableIconResult,
  GetIconResult,
  LoadPackageResult,
  ReadSourceResult,
  ReloadClassRangeResult,
  SourceEdit,
  SourceRangeDto,
} from "../shared/modelica";

const api: WindowApi = {
  ping: () => ipcRenderer.invoke("ping") as Promise<string>,
  openDirectory: () =>
    ipcRenderer.invoke(
      "dialog:openDirectory",
    ) as Promise<DirectoryDialogResult>,
  modelica: {
    openFile: () =>
      ipcRenderer.invoke("modelica:openFile") as Promise<LoadPackageResult>,
    loadFile: (filePath: string) =>
      ipcRenderer.invoke(
        "modelica:loadFile",
        filePath,
      ) as Promise<LoadPackageResult>,
    openAndLoad: () =>
      ipcRenderer.invoke("modelica:openAndLoad") as Promise<LoadPackageResult>,
    loadDirectory: (dirPath: string) =>
      ipcRenderer.invoke(
        "modelica:loadDirectory",
        dirPath,
      ) as Promise<LoadPackageResult>,
    readSource: (filePath: string) =>
      ipcRenderer.invoke(
        "modelica:readSource",
        filePath,
      ) as Promise<ReadSourceResult>,
    getIcon: (
      filePath: string,
      sourceRange: SourceRangeDto | null,
      modelName: string,
    ) =>
      ipcRenderer.invoke(
        "modelica:getIcon",
        filePath,
        sourceRange,
        modelName,
      ) as Promise<GetIconResult>,
    getEditableIcon: (
      filePath: string,
      sourceRange: SourceRangeDto | null,
      modelName: string,
    ) =>
      ipcRenderer.invoke(
        "modelica:getEditableIcon",
        filePath,
        sourceRange,
        modelName,
      ) as Promise<GetEditableIconResult>,
    applySourceEdit: (filePath: string, edit: SourceEdit) =>
      ipcRenderer.invoke(
        "modelica:applySourceEdit",
        filePath,
        edit,
      ) as Promise<ApplySourceEditResult>,
    reloadClassRange: (filePath: string, qualifiedName: string) =>
      ipcRenderer.invoke(
        "modelica:reloadClassRange",
        filePath,
        qualifiedName,
      ) as Promise<ReloadClassRangeResult>,
    reveal: (filePath: string) =>
      ipcRenderer.invoke("modelica:reveal", filePath) as Promise<void>,
  },
  onAutoOpen: (callback: (dirPath: string) => void) => {
    ipcRenderer.on("modelica:auto-open", (_e, dirPath: string) =>
      callback(dirPath),
    );
  },
};

contextBridge.exposeInMainWorld("api", api);
