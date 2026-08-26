import { describe, expect, it } from "vitest";
import { applySourceTransaction, SourceTransactionError } from "./sourceTransaction.js";

describe("source transactions", () => {
  it("validates expected text and applies variable-length edits back-to-front", () => {
    const source = "A=one; B=two; C=three;";
    const result = applySourceTransaction(source, {
      filePath: "test.mo",
      edits: [
        { start: 2, end: 5, expectedText: "one", replacement: "a-long-value" },
        { start: 9, end: 12, expectedText: "two", replacement: "B" },
      ],
    });
    expect(result).toBe("A=a-long-value; B=B; C=three;");
  });

  it("rejects stale and overlapping ranges without producing output", () => {
    expect(() => applySourceTransaction("abcdef", {
      filePath: "test.mo",
      edits: [{ start: 1, end: 3, expectedText: "xx", replacement: "z" }],
    })).toThrowError(SourceTransactionError);
    expect(() => applySourceTransaction("abcdef", {
      filePath: "test.mo",
      edits: [
        { start: 1, end: 4, replacement: "x" },
        { start: 3, end: 5, replacement: "y" },
      ],
    })).toThrowError("overlap");
  });

  it("rejects an outdated source version", () => {
    expect(() => applySourceTransaction("abc", {
      filePath: "test.mo", sourceVersion: 2, edits: [],
    }, 3)).toThrowError("version mismatch");
  });
});
