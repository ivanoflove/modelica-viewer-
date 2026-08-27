export interface ModelicaTextRenderContext {
  classQualifiedName?: string;
  className?: string;
  instanceName?: string;
  parameterBindings?: Record<string, string>;
  parameterDefaults?: Record<string, string>;
}

/** Expand Modelica graphic text macros without coupling rendering to a class AST. */
export function resolveModelicaTextString(
  template: string,
  context: ModelicaTextRenderContext = {},
): string {
  const className =
    context.className ??
    context.classQualifiedName?.split(".").filter(Boolean).pop() ??
    "";
  const values = {
    ...(context.parameterDefaults ?? {}),
    ...(context.parameterBindings ?? {}),
  };
  return template.replace(/%%|%\{([A-Za-z_]\w*)\}|%([A-Za-z_]\w*)/g, (match, braced, plain) => {
    if (match === "%%") return "%";
    const key = braced ?? plain;
    if (key === "name") return context.instanceName ?? className;
    if (key === "class") return className;
    return values[key] ?? match;
  });
}
