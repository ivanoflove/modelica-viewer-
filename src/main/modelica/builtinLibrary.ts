import { existsSync } from "node:fs";
import { join } from "node:path";

export const BUILTIN_MODELICA_VERSION = "4.1.0";
export const BUILTIN_MODELICA_NAME = "Modelica";

/** Locate the bundled library in both an unpackaged Electron app and a packaged app. */
export function findBuiltinModelicaRoot(
  resourcesPath: string,
  appPath: string,
  packaged: boolean,
): string | null {
  const candidates = packaged
    ? [join(resourcesPath, "modelica", "msl-4.1.0")]
    : [
        join(appPath, "resources", "modelica", "msl-4.1.0"),
        join(resourcesPath, "modelica", "msl-4.1.0"),
      ];
  for (const root of candidates) {
    const modelicaRoot = join(root, "Modelica");
    if (existsSync(join(modelicaRoot, "package.mo"))) return modelicaRoot;
  }
  return null;
}
