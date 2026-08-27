import type {
  ApplySourceEditResult,
  CreateGraphicRequest,
  CreateGraphicResult,
  GetEditableIconResult,
  GetDiagramResult,
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

export interface LibraryInfo {
  path: string;
  classCount: number;
}

export type LibraryMutationResult =
  | { ok: true; library: LibraryInfo }
  | { error: string };

export interface WindowApi {
  ping: () => Promise<string>;
  openDirectory: () => Promise<DirectoryDialogResult>;
  modelica: {
    openFile: () => Promise<LoadPackageResult>;
    loadFile: (filePath: string) => Promise<LoadPackageResult>;
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
    getDiagram: (
      filePath: string,
      sourceRange: SourceRangeDto | null,
      qualifiedName: string,
    ) => Promise<GetDiagramResult>;
    applySourceEdit: (
      filePath: string,
      edit: SourceEdit,
    ) => Promise<ApplySourceEditResult>;
    createGraphic: (
      filePath: string,
      request: CreateGraphicRequest,
    ) => Promise<CreateGraphicResult>;
    reloadClassRange: (
      filePath: string,
      qualifiedName: string,
    ) => Promise<ReloadClassRangeResult>;
    reveal: (filePath: string) => Promise<void>;
    listLibraries: () => Promise<LibraryInfo[]>;
    addLibrary: () => Promise<LibraryMutationResult>;
    removeLibrary: (path: string) => Promise<{ ok: true } | { error: string }>;
    rescanLibraries: () => Promise<LibraryInfo[]>;
  };
  onAutoOpen: (callback: (dirPath: string) => void) => void;
}
