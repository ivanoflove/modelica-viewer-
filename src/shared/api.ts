import type {
  ApplySourceEditResult,
  GetEditableIconResult,
  GetIconResult,
  LoadPackageResult,
  ReadSourceResult,
  SourceEdit,
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
    getEditableIcon: (
      filePath: string,
      sourceRange: SourceRangeDto | null,
      modelName: string,
    ) => Promise<GetEditableIconResult>;
    applySourceEdit: (
      filePath: string,
      edit: SourceEdit,
    ) => Promise<ApplySourceEditResult>;
    reveal: (filePath: string) => Promise<void>;
  };
}
