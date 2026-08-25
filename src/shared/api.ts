import type {
  GetIconResult,
  LoadPackageResult,
  ReadSourceResult,
  SourceRangeDto,
} from "./modelica.js";

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
    getIcon: (
      filePath: string,
      sourceRange: SourceRangeDto | null,
      modelName: string,
    ) => Promise<GetIconResult>;
    reveal: (filePath: string) => Promise<void>;
  };
}
