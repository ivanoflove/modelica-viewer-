import type {
  ApplySourceEditResult,
  GetEditableIconResult,
  GetIconResult,
  LoadPackageResult,
  ReadSourceResult,
  ReloadClassRangeResult,
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
    reloadClassRange: (
      filePath: string,
      qualifiedName: string,
    ) => Promise<ReloadClassRangeResult>;
    reveal: (filePath: string) => Promise<void>;
  };
  onAutoOpen: (callback: (dirPath: string) => void) => void;
}
