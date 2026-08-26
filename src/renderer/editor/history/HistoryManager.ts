import type { GraphicItemDto } from "../../../shared/modelicaGraphics.js";

export interface GraphicEditCommand {
  graphicId: string;
  before: GraphicItemDto;
  after: GraphicItemDto;
  sourceBefore?: string;
  sourceAfter?: string;
}

export class HistoryManager {
  private readonly undoStack: GraphicEditCommand[] = [];
  private readonly redoStack: GraphicEditCommand[] = [];

  constructor(private readonly maxSteps = 100) {}

  push(command: GraphicEditCommand): void {
    this.undoStack.push(command);
    if (this.undoStack.length > this.maxSteps) this.undoStack.shift();
    this.redoStack.length = 0;
  }

  peekUndo(): GraphicEditCommand | null {
    return this.undoStack[this.undoStack.length - 1] ?? null;
  }

  peekRedo(): GraphicEditCommand | null {
    return this.redoStack[this.redoStack.length - 1] ?? null;
  }

  acceptUndo(): GraphicEditCommand | null {
    const command = this.undoStack.pop() ?? null;
    if (command) this.redoStack.push(command);
    return command;
  }

  acceptRedo(): GraphicEditCommand | null {
    const command = this.redoStack.pop() ?? null;
    if (command) this.undoStack.push(command);
    return command;
  }

  get canUndo(): boolean { return this.undoStack.length > 0; }
  get canRedo(): boolean { return this.redoStack.length > 0; }
  get size(): number { return this.undoStack.length; }
}
