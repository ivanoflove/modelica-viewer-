export type ViewerPerformanceCounter =
  | "wheelEvents"
  | "viewportRafUpdates"
  | "dragPreviewRafUpdates"
  | "vertexPreviewRafUpdates"
  | "graphicLayerRenders"
  | "graphicItemRenders";

export type ViewerPerformanceCounters = Record<ViewerPerformanceCounter, number>;

declare global {
  interface Window {
    __modelicaViewerPerf?: ViewerPerformanceCounters;
  }
}

export function recordViewerPerformance(counter: ViewerPerformanceCounter) {
  if (typeof window === "undefined") return;
  const counters = (window.__modelicaViewerPerf ??= {
    wheelEvents: 0,
    viewportRafUpdates: 0,
    dragPreviewRafUpdates: 0,
    vertexPreviewRafUpdates: 0,
    graphicLayerRenders: 0,
    graphicItemRenders: 0,
  });
  counters[counter] = (counters[counter] ?? 0) + 1;
}
