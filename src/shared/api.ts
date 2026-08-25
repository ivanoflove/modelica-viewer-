import type { LoadPackageResult } from "./modelica.js";

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
    reveal: (filePath: string) => Promise<void>;
  };
}
