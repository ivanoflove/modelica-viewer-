import {
  useEffect,
  useRef,
  useState,
  type PointerEvent,
  type ReactNode,
  type RefObject,
} from "react";
import type { IconDto } from "../../../shared/modelicaGraphics";
import { boundsOf, type Bounds } from "../../editor/Transform";

interface ViewBox {
  x: number;
  y: number;
  width: number;
  height: number;
}

interface Props {
  icon: IconDto;
  resetKey: string;
  svgRef: RefObject<SVGSVGElement>;
  children: ReactNode;
  onPointerMove: (event: PointerEvent<SVGSVGElement>) => void;
  onPointerUp: (event: PointerEvent<SVGSVGElement>) => void;
  onPointerCancel: (event: PointerEvent<SVGSVGElement>) => void;
  onUndo?: () => void;
  onRedo?: () => void;
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

function pointInSvg(
  svg: SVGSVGElement,
  clientX: number,
  clientY: number,
): { x: number; y: number } | null {
  const point = svg.createSVGPoint();
  point.x = clientX;
  point.y = clientY;
  const inverse = svg.getScreenCTM()?.inverse();
  if (!inverse) return null;
  return point.matrixTransform(inverse);
}

export function GraphicViewport({
  icon,
  resetKey,
  svgRef,
  children,
  onPointerMove,
  onPointerUp,
  onPointerCancel,
  onUndo,
  onRedo,
  canUndo = false,
  canRedo = false,
}: Props) {
  const shellRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLDivElement>(null);
  const spacePressed = useRef(false);
  const [fitMode, setFitMode] = useState<"content" | "coordinateSystem">(
    "content",
  );
  const [viewBox, setViewBox] = useState<ViewBox>(() => contentViewBox(icon));
  const viewBoxRef = useRef(viewBox);
  viewBoxRef.current = viewBox;
  const [pan, setPan] = useState<{
    startX: number;
    startY: number;
    viewBox: ViewBox;
  } | null>(null);

  useEffect(() => {
    setFitMode("content");
    setViewBox(contentViewBox(icon));
  }, [resetKey]);

  const fit = (mode = fitMode) => {
    setViewBox(
      mode === "content" ? contentViewBox(icon) : coordinateViewBox(icon),
    );
  };

  const zoomBy = (factor: number, anchor?: { x: number; y: number }) => {
    setViewBox((current) => {
      const fitView =
        fitMode === "content" ? contentViewBox(icon) : coordinateViewBox(icon);
      return zoomViewBox(current, fitView, factor, anchor);
    });
  };

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    // React's delegated wheel event can be passive in an Electron/Chromium
    // host. Use a native non-passive listener so Ctrl+wheel is handled by the
    // canvas without allowing the surrounding source pane to scroll.
    const handleWheel = (event: globalThis.WheelEvent) => {
      // Deliberately require Ctrl: ordinary wheel scrolling should not change
      // the viewport, matching the requested Windows interaction.
      const factor = wheelZoomFactor(event.deltaY, event.ctrlKey);
      if (factor === null) return;
      event.preventDefault();
      event.stopPropagation();
      const svg = svgRef.current;
      if (!svg) return;
      const anchor = pointInSvg(svg, event.clientX, event.clientY);
      const current = viewBoxRef.current;
      const fitView =
        fitMode === "content" ? contentViewBox(icon) : coordinateViewBox(icon);
      const next = zoomViewBox(current, fitView, factor, anchor ?? undefined);
      console.debug("[VIEWPORT_ZOOM]", {
        from: current.width,
        to: next.width,
      });
      setViewBox(next);
    };
    canvas.addEventListener("wheel", handleWheel, { passive: false });
    return () => canvas.removeEventListener("wheel", handleWheel);
  }, [fitMode, icon]);

  const startPan = (event: PointerEvent<SVGSVGElement>) => {
    const spacePan = event.button === 0 && spacePressed.current;
    if (event.button !== 1 && !spacePan) return;
    event.preventDefault();
    event.stopPropagation();
    shellRef.current?.focus({ preventScroll: true });
    event.currentTarget.setPointerCapture(event.pointerId);
    setPan({ startX: event.clientX, startY: event.clientY, viewBox });
  };

  const handlePanMove = (event: PointerEvent<SVGSVGElement>) => {
    if (!pan) return;
    event.preventDefault();
    event.stopPropagation();
    const rect = event.currentTarget.getBoundingClientRect();
    setViewBox(
      panViewBox(
        pan.viewBox,
        event.clientX - pan.startX,
        event.clientY - pan.startY,
        rect.width,
        rect.height,
      ),
    );
  };

  const endPan = (event: PointerEvent<SVGSVGElement>) => {
    if (!pan) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.currentTarget.hasPointerCapture(event.pointerId))
      event.currentTarget.releasePointerCapture(event.pointerId);
    setPan(null);
  };

  const handleKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === " ") {
      spacePressed.current = true;
      event.preventDefault();
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
        <span>
          Icon 画布 ·{" "}
          {Math.round(
            ((fitMode === "content"
              ? contentViewBox(icon).width
              : coordinateViewBox(icon).width) /
              viewBox.width) *
              100,
          )}
          %
        </span>
        <div className="fit-toggle">
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
          <button onClick={() => zoomBy(0.8)} aria-label="Zoom out">
            −
          </button>
          <button onClick={() => zoomBy(1.25)} aria-label="Zoom in">
            +
          </button>
          <button onClick={() => fit()}>Fit</button>
          <button onClick={onUndo} disabled={!canUndo} aria-label="Undo">
            Undo
          </button>
          <button onClick={onRedo} disabled={!canRedo} aria-label="Redo">
            Redo
          </button>
        </div>
      </div>
      <div ref={canvasRef} className="icon-canvas-area">
        <svg
          ref={svgRef}
          viewBox={toViewBoxString(viewBox)}
          className="modelica-icon"
          preserveAspectRatio="xMidYMid meet"
          style={{ touchAction: "none", cursor: pan ? "grabbing" : "default" }}
          onPointerDownCapture={startPan}
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
          onDoubleClick={(event) => {
            if (event.target === event.currentTarget) fit();
          }}
          onFocus={() => undefined}
        >
          {children}
        </svg>
      </div>
      <span className="icon-editor-hint">
        Ctrl+滚轮缩放 · 中键或 Space+左键平移 · Ctrl+0 / F 适配 · 拖动图元移动
      </span>
    </div>
  );
}
