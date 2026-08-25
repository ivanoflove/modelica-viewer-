import type { LoadPackageResult, ReadSourceResult } from "./modelica.js";

export interface DirectoryDialogResult {
  canceled: boolean;
  path?: string;
}

export interface WindowApi {
  ping: () => Promise<string>;
  openDirectory: () => Promise<DirectoryDialogResult>;
  modelica: {
    openAndLoad: () => Promise<LoadPackageResult>;
    loadDirectory: (dirPath: string) => Promise<LoadPackageResult>;
    readSource: (filePath: string) => Promise<ReadSourceResult>;
    reveal: (filePath: string) => Promise<void>;
  };
}
