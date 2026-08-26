import type { SourceEdit } from "../../shared/modelica.js";

export type SourceTransactionErrorCode =
  | "INVALID_EDIT_RANGE"
  | "STALE_SOURCE_RANGE"
  | "SOURCE_VERSION_MISMATCH"
  | "OVERLAPPING_EDITS";

export class SourceTransactionError extends Error {
  constructor(
    public readonly code: SourceTransactionErrorCode,
    message: string,
  ) {
    super(message);
    this.name = "SourceTransactionError";
  }
}

export interface SourceTransaction {
  filePath: string;
  sourceVersion?: number;
  edits: SourceEdit[];
}

/** Validate all ranges before changing the source and apply them back-to-front. */
export function applySourceTransaction(
  source: string,
  transaction: SourceTransaction,
  currentVersion?: number,
): string {
  if (
    transaction.sourceVersion !== undefined &&
    currentVersion !== undefined &&
    transaction.sourceVersion !== currentVersion
  ) {
    throw new SourceTransactionError(
      "SOURCE_VERSION_MISMATCH",
      `Source version mismatch: expected ${transaction.sourceVersion}, current ${currentVersion}`,
    );
  }

  const edits = [...transaction.edits].sort((a, b) => a.start - b.start);
  let previousEnd = -1;
  for (const edit of edits) {
    if (edit.start < 0 || edit.end < edit.start || edit.end > source.length) {
      throw new SourceTransactionError(
        "INVALID_EDIT_RANGE",
        "Invalid source edit range",
      );
    }
    if (edit.start < previousEnd) {
      throw new SourceTransactionError(
        "OVERLAPPING_EDITS",
        "Source edits overlap",
      );
    }
    if (
      edit.expectedText !== undefined &&
      source.slice(edit.start, edit.end) !== edit.expectedText
    ) {
      const actualText = source.slice(edit.start, edit.end);
      throw new SourceTransactionError(
        "STALE_SOURCE_RANGE",
        `Stale source range at ${edit.start}:${edit.end}; expected ${JSON.stringify(edit.expectedText)}, actual ${JSON.stringify(actualText)}`,
      );
    }
    previousEnd = edit.end;
  }

  let result = source;
  for (const edit of [...edits].reverse()) {
    result =
      result.slice(0, edit.start) + edit.replacement + result.slice(edit.end);
  }
  return result;
}
