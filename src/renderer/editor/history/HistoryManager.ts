export type GraphicHistoryType =
  | "move"
  | "resize"
  | "vertex"
  | "property"
  | "delete"
  | "create";

/** A source property, not a cached source range. Ranges are resolved on use. */
export type GraphicHistoryProperty =
  | "item"
  | "origin"
  | "extent"
  | "points"
  | "lineColor"
  | "color"
  | "textColor"
  | "fillColor"
  | "textString"
  | "fontSize"
  | "textStyle"
  | "lineThickness"
  | "thickness"
  | "pattern"
  | "fillPattern";

export interface GraphicHistoryTarget {
  ownerQualifiedName: string;
  /** Stable within the owning Icon: `Icon.graphics:<logical index>`. */
  graphicPath: string;
  property: GraphicHistoryProperty;
}

/**
 * A history entry deliberately contains source values only. It must not hold
 * a GraphicDto, AST node, or source range because all three become stale after
 * a successful reparse.
 */
export interface GraphicHistoryCommand {
  type: GraphicHistoryType;
  target: GraphicHistoryTarget;
  before: string;
  after: string;
}

export class HistoryManager {
  private readonly undoStack: GraphicHistoryCommand[] = [];
  private readonly redoStack: GraphicHistoryCommand[] = [];

  constructor(private readonly maxSteps = 100) {}

  push(command: GraphicHistoryCommand): void {
    this.undoStack.push(command);
    if (this.undoStack.length > this.maxSteps) this.undoStack.shift();
    this.redoStack.length = 0;
  }

  peekUndo(): GraphicHistoryCommand | null {
    return this.undoStack[this.undoStack.length - 1] ?? null;
  }

  peekRedo(): GraphicHistoryCommand | null {
    return this.redoStack[this.redoStack.length - 1] ?? null;
  }

  acceptUndo(): GraphicHistoryCommand | null {
    const command = this.undoStack.pop() ?? null;
    if (command) this.redoStack.push(command);
    return command;
  }

  acceptRedo(): GraphicHistoryCommand | null {
    const command = this.redoStack.pop() ?? null;
    if (command) this.undoStack.push(command);
    return command;
  }

  get canUndo(): boolean { return this.undoStack.length > 0; }
  get canRedo(): boolean { return this.redoStack.length > 0; }
  get size(): number { return this.undoStack.length; }
}
