import {
  useEffect,
  useRef,
  useState,
  type MouseEvent,
  type PointerEvent,
  type ReactNode,
  type RefObject,
} from "react";
import type { IconDto } from "../../../shared/modelicaGraphics";
import { boundsOf, type Bounds } from "../../editor/Transform";
import { recordViewerPerformance } from "../performance";

export interface ViewBox {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface ViewportStateSnapshot {
  base: ViewBox;
  viewBox: ViewBox;
}

export function modelToViewportRoot(
  point: { x: number; y: number },
  viewport: ViewportStateSnapshot,
): { x: number; y: number } {
  const scaleX = viewport.base.width / Math.max(viewport.viewBox.width, 1e-9);
  const scaleY = viewport.base.height / Math.max(viewport.viewBox.height, 1e-9);
  const translateX = viewport.base.x - viewport.viewBox.x * scaleX;
  const translateY = viewport.base.y - viewport.viewBox.y * scaleY;
  // The model layer is wrapped in scale(1,-1) before the viewport group.
  return {
    x: point.x * scaleX + translateX,
    y: -point.y * scaleY + translateY,
  };
}

/** Convert a client point to root SVG user coordinates without querying CTM. */
export function clientToViewportRoot(
  svg: SVGSVGElement,
  clientX: number,
  clientY: number,
  base: ViewBox,
): { x: number; y: number } | null {
  const rect = svg.getBoundingClientRect();
  const scale = Math.min(
    rect.width / Math.max(base.width, 1e-9),
    rect.height / Math.max(base.height, 1e-9),
  );
  if (!Number.isFinite(scale) || scale <= 0) return null;
  const offsetX = (rect.width - base.width * scale) / 2;
  const offsetY = (rect.height - base.height * scale) / 2;
  return {
    x: base.x + (clientX - rect.left - offsetX) / scale,
    y: base.y + (clientY - rect.top - offsetY) / scale,
  };
}

/** Convert a client point to Modelica coordinates using a captured viewport. */
export function clientToModelicaWithViewport(
  svg: SVGSVGElement,
  clientX: number,
  clientY: number,
  viewport: ViewportStateSnapshot,
): { x: number; y: number } | null {
  const root = clientToViewportRoot(svg, clientX, clientY, viewport.base);
  if (!root) return null;
  const scaleX = viewport.base.width / Math.max(viewport.viewBox.width, 1e-9);
  const scaleY = viewport.base.height / Math.max(viewport.viewBox.height, 1e-9);
  const translateX = viewport.base.x - viewport.viewBox.x * scaleX;
  const translateY = viewport.base.y - viewport.viewBox.y * scaleY;
  return {
    x: (root.x - translateX) / scaleX,
    y: -(root.y - translateY) / scaleY,
  };
}

interface Props {
  icon: IconDto;
  resetKey: string;
  svgRef: RefObject<SVGSVGElement>;
  children: ReactNode;
  overlay?: ReactNode;
  viewportGroupRef?: RefObject<SVGGElement>;
  screenOverlayRef?: RefObject<SVGGElement>;
  onViewportTransform?: (viewport: ViewportStateSnapshot) => void;
  onPointerMove: (event: PointerEvent<SVGSVGElement>) => void;
  onPointerUp: (event: PointerEvent<SVGSVGElement>) => void;
  onPointerCancel: (event: PointerEvent<SVGSVGElement>) => void;
  onCanvasPointerDown?: (event: PointerEvent<SVGSVGElement>) => void;
  onCanvasPointerDownCapture?: (event: PointerEvent<SVGSVGElement>) => void;
  onCanvasContextMenu?: (event: MouseEvent<SVGSVGElement>) => void;
  onCanvasDoubleClick?: (event: MouseEvent<SVGSVGElement>) => void;
  canvasCursor?: string;
  onCanvasDragOver?: (event: React.DragEvent<HTMLDivElement>) => void;
  onCanvasDrop?: (event: React.DragEvent<HTMLDivElement>) => void;
  canvasLabel?: string;
  initialFitMode?: "content" | "coordinateSystem";
  showFitModes?: boolean;
  onUndo?: () => void;
  onRedo?: () => void;
  onDelete?: () => boolean;
  canUndo?: boolean;
  canRedo?: boolean;
}

export function coordinateViewBox(icon: IconDto): ViewBox {
  const extent = icon.coordinateSystem.extent;
  return {
    x: Math.min(extent.p1.x, extent.p2.x),
    y: Math.min(extent.p1.y, extent.p2.y),
    width: Math.max(Math.abs(extent.p2.x - extent.p1.x), 1),
    height: Math.max(Math.abs(extent.p2.y - extent.p1.y), 1),
  };
}

export function contentViewBox(icon: IconDto): ViewBox {
  const boxes = icon.graphics
    .map(boundsOf)
    .filter((box): box is Bounds => box !== null);
  if (boxes.length === 0) return coordinateViewBox(icon);
  const minX = Math.min(...boxes.map((box) => box.x));
  const minY = Math.min(...boxes.map((box) => box.y));
  const maxX = Math.max(...boxes.map((box) => box.x + box.width));
  const maxY = Math.max(...boxes.map((box) => box.y + box.height));
  const padding = Math.max(8, Math.min(maxX - minX, maxY - minY) * 0.08);
  return {
    x: minX - padding,
    y: minY - padding,
    width: Math.max(maxX - minX + padding * 2, 1),
    height: Math.max(maxY - minY + padding * 2, 1),
  };
}

function toViewBoxString(viewBox: ViewBox): string {
  return `${viewBox.x} ${viewBox.y} ${viewBox.width} ${viewBox.height}`;
}

/** Map the stable SVG base viewBox to the current zoomed/panned view. */
export function viewportGroupTransform(base: ViewBox, current: ViewBox): string {
  const scaleX = base.width / Math.max(current.width, 1e-9);
  const scaleY = base.height / Math.max(current.height, 1e-9);
  const translateX = base.x - current.x * scaleX;
  const translateY = base.y - current.y * scaleY;
  return `matrix(${scaleX} 0 0 ${scaleY} ${translateX} ${translateY})`;
}

export function zoomViewBox(
  current: ViewBox,
  fit: ViewBox,
  factor: number,
  anchor?: { x: number; y: number },
): ViewBox {
  const minWidth = fit.width / 12;
  const maxWidth = fit.width / 0.25;
  const nextWidth = Math.min(
    Math.max(current.width / factor, minWidth),
    maxWidth,
  );
  const nextHeight = (current.height * nextWidth) / current.width;
  const center = anchor ?? {
    x: current.x + current.width / 2,
    y: current.y + current.height / 2,
  };
  return {
    x: center.x - (center.x - current.x) * (nextWidth / current.width),
    y: center.y - (center.y - current.y) * (nextHeight / current.height),
    width: nextWidth,
    height: nextHeight,
  };
}

export function panViewBox(
  start: ViewBox,
  deltaX: number,
  deltaY: number,
  viewportWidth: number,
  viewportHeight: number,
): ViewBox {
  return {
    ...start,
    x: start.x - (deltaX * start.width) / Math.max(viewportWidth, 1),
    y: start.y - (deltaY * start.height) / Math.max(viewportHeight, 1),
  };
}

export function wheelZoomFactor(
  deltaY: number,
  ctrlKey: boolean,
): number | null {
  return ctrlKey ? Math.exp(-deltaY * 0.0015) : null;
}

function pointInCurrentView(
  svg: SVGSVGElement,
  clientX: number,
  clientY: number,
  viewport: ViewportStateSnapshot,
): { x: number; y: number } | null {
  const basePoint = clientToViewportRoot(svg, clientX, clientY, viewport.base);
  if (!basePoint) return null;
  const scaleX = viewport.base.width / Math.max(viewport.viewBox.width, 1e-9);
  const scaleY = viewport.base.height / Math.max(viewport.viewBox.height, 1e-9);
  return {
    x: (basePoint.x - viewport.base.x + viewport.viewBox.x * scaleX) / scaleX,
    y: (basePoint.y - viewport.base.y + viewport.viewBox.y * scaleY) / scaleY,
  };
}

export function GraphicViewport({
  icon,
  resetKey,
  svgRef,
  children,
  overlay,
  viewportGroupRef: externalViewportGroupRef,
  screenOverlayRef,
  onViewportTransform,
  onPointerMove,
  onPointerUp,
  onPointerCancel,
  onCanvasPointerDown,
  onCanvasPointerDownCapture,
  onCanvasContextMenu,
  onCanvasDoubleClick,
  canvasCursor,
  onCanvasDragOver,
  onCanvasDrop,
  canvasLabel = "Icon 画布",
  initialFitMode = "content",
  showFitModes = true,
  onUndo,
  onRedo,
  onDelete,
  canUndo = false,
  canRedo = false,
}: Props) {
  const shellRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLDivElement>(null);
  const internalViewportGroupRef = useRef<SVGGElement>(null);
  const viewportGroupRef = externalViewportGroupRef ?? internalViewportGroupRef;
  const spacePressed = useRef(false);
  const renderRafRef = useRef<number | null>(null);
  const stateSyncTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const onViewportTransformRef = useRef(onViewportTransform);
  onViewportTransformRef.current = onViewportTransform;
  const [fitMode, setFitMode] = useState<"content" | "coordinateSystem">(initialFitMode);
  const [viewBox, setViewBox] = useState<ViewBox>(() => contentViewBox(icon));
  const [isPanning, setIsPanning] = useState(false);
  const viewportRef = useRef<ViewportStateSnapshot>({
    base: coordinateViewBox(icon),
    viewBox: contentViewBox(icon),
  });
  const panRef = useRef<{
    startX: number;
    startY: number;
    viewBox: ViewBox;
  } | null>(null);

  const applyViewportTransform = () => {
    const group = viewportGroupRef.current;
    if (!group) return;
    const { base, viewBox: current } = viewportRef.current;
    group.setAttribute("transform", viewportGroupTransform(base, current));
  };

  const scheduleViewportRender = () => {
    if (renderRafRef.current !== null) return;
    renderRafRef.current = requestAnimationFrame(() => {
      renderRafRef.current = null;
      recordViewerPerformance("viewportRafUpdates");
      applyViewportTransform();
      onViewportTransformRef.current?.({
        base: { ...viewportRef.current.base },
        viewBox: { ...viewportRef.current.viewBox },
      });
    });
  };

  const scheduleStateSync = () => {
    if (stateSyncTimerRef.current !== null) {
      clearTimeout(stateSyncTimerRef.current);
    }
    stateSyncTimerRef.current = setTimeout(() => {
      stateSyncTimerRef.current = null;
      setViewBox({ ...viewportRef.current.viewBox });
    }, 120);
  };

  const updateViewport = (next: ViewBox, syncState = true) => {
    viewportRef.current.viewBox = next;
    scheduleViewportRender();
    if (syncState) scheduleStateSync();
  };

  useEffect(() => {
    const base = coordinateViewBox(icon);
    const initial = initialFitMode === "coordinateSystem"
      ? coordinateViewBox(icon)
      : contentViewBox(icon);
    viewportRef.current = { base, viewBox: initial };
    setFitMode(initialFitMode);
    setViewBox(initial);
    onViewportTransformRef.current?.({
      base: { ...base },
      viewBox: { ...initial },
    });
    scheduleViewportRender();
  }, [initialFitMode, resetKey]);

  useEffect(() => {
    applyViewportTransform();
    onViewportTransformRef.current?.({
      base: { ...viewportRef.current.base },
      viewBox: { ...viewportRef.current.viewBox },
    });
    return () => {
      if (renderRafRef.current !== null) {
        cancelAnimationFrame(renderRafRef.current);
        renderRafRef.current = null;
      }
      if (stateSyncTimerRef.current !== null) {
        clearTimeout(stateSyncTimerRef.current);
        stateSyncTimerRef.current = null;
      }
    };
  }, [resetKey]);

  const fit = (mode = fitMode) => {
    updateViewport(mode === "content" ? contentViewBox(icon) : coordinateViewBox(icon));
    setViewBox({
      ...(mode === "content" ? contentViewBox(icon) : coordinateViewBox(icon)),
    });
  };

  const zoomBy = (factor: number, anchor?: { x: number; y: number }) => {
    const current = viewportRef.current.viewBox;
    const fitView =
      fitMode === "content" ? contentViewBox(icon) : coordinateViewBox(icon);
    updateViewport(zoomViewBox(current, fitView, factor, anchor));
  };

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const handleWheel = (event: globalThis.WheelEvent) => {
      const factor = wheelZoomFactor(event.deltaY, event.ctrlKey);
      if (factor === null) return;
      recordViewerPerformance("wheelEvents");
      event.preventDefault();
      event.stopPropagation();
      const svg = svgRef.current;
      if (!svg) return;
      const viewport = viewportRef.current;
      const anchor = pointInCurrentView(
        svg,
        event.clientX,
        event.clientY,
        viewport,
      );
      const fitView =
        fitMode === "content" ? contentViewBox(icon) : coordinateViewBox(icon);
      updateViewport(zoomViewBox(viewport.viewBox, fitView, factor, anchor ?? undefined));
    };
    canvas.addEventListener("wheel", handleWheel, { passive: false });
    return () => canvas.removeEventListener("wheel", handleWheel);
  }, [fitMode, icon, resetKey]);

  const startPan = (event: PointerEvent<SVGSVGElement>) => {
    const spacePan = event.button === 0 && spacePressed.current;
    if (event.button !== 1 && !spacePan) return;
    event.preventDefault();
    event.stopPropagation();
    shellRef.current?.focus({ preventScroll: true });
    event.currentTarget.setPointerCapture(event.pointerId);
    panRef.current = {
      startX: event.clientX,
      startY: event.clientY,
      viewBox: { ...viewportRef.current.viewBox },
    };
    setIsPanning(true);
  };

  const handlePanMove = (event: PointerEvent<SVGSVGElement>) => {
    const pan = panRef.current;
    if (!pan) return;
    event.preventDefault();
    event.stopPropagation();
    const rect = event.currentTarget.getBoundingClientRect();
    updateViewport(
      panViewBox(
        pan.viewBox,
        event.clientX - pan.startX,
        event.clientY - pan.startY,
        rect.width,
        rect.height,
      ),
      false,
    );
  };

  const endPan = (event: PointerEvent<SVGSVGElement>) => {
    if (!panRef.current) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.currentTarget.hasPointerCapture(event.pointerId))
      event.currentTarget.releasePointerCapture(event.pointerId);
    panRef.current = null;
    setIsPanning(false);
    scheduleStateSync();
  };

  const handleKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const activeElement = event.currentTarget.ownerDocument.activeElement;
    const isTextEditing =
      activeElement instanceof HTMLElement &&
      (activeElement.tagName === "INPUT" ||
        activeElement.tagName === "TEXTAREA" ||
        activeElement.tagName === "SELECT" ||
        activeElement.isContentEditable);
    if (event.key === " ") {
      spacePressed.current = true;
      event.preventDefault();
      return;
    }
    if ((event.key === "Delete" || event.key === "Backspace") && !isTextEditing) {
      const handled = event.key === "Delete" ? true : (onDelete?.() ?? false);
      if (handled) {
        event.preventDefault();
        if (event.key === "Delete") onDelete?.();
      }
      return;
    }
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "z") {
      event.preventDefault();
      if (event.shiftKey) onRedo?.();
      else onUndo?.();
      return;
    }
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "y") {
      event.preventDefault();
      onRedo?.();
      return;
    }
    if ((event.ctrlKey || event.metaKey) && event.key === "0") {
      event.preventDefault();
      fit();
    } else if (event.key.toLowerCase() === "f") {
      event.preventDefault();
      fit();
    } else if (event.key === "+" || event.key === "=") {
      event.preventDefault();
      zoomBy(1.25);
    } else if (event.key === "-") {
      event.preventDefault();
      zoomBy(0.8);
    }
  };

  const fitView = fitMode === "content" ? contentViewBox(icon) : coordinateViewBox(icon);
  const zoomPercent = Math.round((fitView.width / viewBox.width) * 100);
  const baseViewBox = viewportRef.current.base;

  return (
    <div
      ref={shellRef}
      className="graphic-viewport"
      tabIndex={0}
      onKeyDown={handleKeyDown}
      onKeyUp={(event) => {
        if (event.key === " ") spacePressed.current = false;
      }}
    >
      <div className="icon-viewer-toolbar">
        <span>{canvasLabel} · {zoomPercent}%</span>
        <div className="fit-toggle">
          {showFitModes && <>
            <button
              className={fitMode === "content" ? "active" : ""}
              onClick={() => {
                setFitMode("content");
                fit("content");
              }}
            >
              Fit Content
            </button>
            <button
              className={fitMode === "coordinateSystem" ? "active" : ""}
              onClick={() => {
                setFitMode("coordinateSystem");
                fit("coordinateSystem");
              }}
            >
              Fit CoordinateSystem
            </button>
          </>}
          <button onClick={() => zoomBy(0.8)} aria-label="Zoom out">−</button>
          <button onClick={() => zoomBy(1.25)} aria-label="Zoom in">+</button>
          <button onClick={() => fit("coordinateSystem")} title="Fit Diagram Coordinate System">Fit</button>
          <button onClick={onUndo} disabled={!canUndo} aria-label="Undo">Undo</button>
          <button onClick={onRedo} disabled={!canRedo} aria-label="Redo">Redo</button>
        </div>
      </div>
      <div
        ref={canvasRef}
        className="icon-canvas-area"
        onDragOver={onCanvasDragOver}
        onDrop={onCanvasDrop}
      >
        <svg
          ref={svgRef}
          viewBox={toViewBoxString(baseViewBox)}
          className="modelica-icon"
          preserveAspectRatio="xMidYMid meet"
          style={{ touchAction: "none", cursor: isPanning ? "grabbing" : canvasCursor ?? "default" }}
          onPointerDownCapture={(event) => {
            if (event.button === 0) shellRef.current?.focus({ preventScroll: true });
            startPan(event);
            onCanvasPointerDownCapture?.(event);
          }}
          onPointerDown={(event) => onCanvasPointerDown?.(event)}
          onPointerMove={(event) => {
            handlePanMove(event);
            onPointerMove(event);
          }}
          onPointerUp={(event) => {
            endPan(event);
            onPointerUp(event);
          }}
          onPointerCancel={(event) => {
            endPan(event);
            onPointerCancel(event);
          }}
          onContextMenu={onCanvasContextMenu}
          onDoubleClick={(event) => {
            onCanvasDoubleClick?.(event);
            if (event.defaultPrevented) return;
            if (event.target === event.currentTarget) fit();
          }}
          onFocus={() => undefined}
        >
          <g ref={viewportGroupRef}>{children}</g>
          <g ref={screenOverlayRef} className="screen-overlay" pointerEvents="none">
            {overlay}
          </g>
        </svg>
      </div>
      <span className="icon-editor-hint">
        Ctrl+滚轮缩放 · 中键或 Space+左键平移 · Ctrl+0 / F 适配 · 拖动图元移动
      </span>
    </div>
  );
}
