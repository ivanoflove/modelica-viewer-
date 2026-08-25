import { contextBridge, ipcRenderer } from "electron";
import type { DirectoryDialogResult, WindowApi } from "../shared/api";
import type {
  ApplySourceEditResult,
  GetEditableIconResult,
  GetIconResult,
  LoadPackageResult,
  ReadSourceResult,
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
    reveal: (filePath: string) =>
      ipcRenderer.invoke("modelica:reveal", filePath) as Promise<void>,
  },
};

contextBridge.exposeInMainWorld("api", api);
