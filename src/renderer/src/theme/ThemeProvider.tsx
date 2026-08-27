import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

export type ThemeMode = "system" | "light" | "dark";
export type AccentName = "violet" | "blue" | "cyan" | "orange";
export type GlassMode = "on" | "reduced";

interface ThemeContextValue {
  theme: ThemeMode;
  accent: AccentName;
  glass: GlassMode;
  setTheme: (theme: ThemeMode) => void;
  setAccent: (accent: AccentName) => void;
  setGlass: (glass: GlassMode) => void;
}

const ThemeContext = createContext<ThemeContextValue | null>(null);

function readSetting<T extends string>(key: string, fallback: T): T {
  try {
    const value = localStorage.getItem(key);
    return value ? (value as T) : fallback;
  } catch {
    return fallback;
  }
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setTheme] = useState<ThemeMode>(() =>
    readSetting("modelica-viewer.theme", "system"),
  );
  const [accent, setAccent] = useState<AccentName>(() =>
    readSetting("modelica-viewer.accent", "violet"),
  );
  const [glass, setGlass] = useState<GlassMode>(() =>
    readSetting("modelica-viewer.glass", "on"),
  );

  useEffect(() => {
    try {
      localStorage.setItem("modelica-viewer.theme", theme);
    } catch {
      // Local storage may be unavailable in a locked-down Electron profile.
    }
  }, [theme]);

  useEffect(() => {
    try {
      localStorage.setItem("modelica-viewer.accent", accent);
    } catch {
      // Keep the in-memory setting even when persistence is unavailable.
    }
  }, [accent]);

  useEffect(() => {
    try {
      localStorage.setItem("modelica-viewer.glass", glass);
    } catch {
      // Keep the in-memory setting even when persistence is unavailable.
    }
  }, [glass]);

  useEffect(() => {
    const root = document.documentElement;
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      const resolved = theme === "system" ? (media.matches ? "dark" : "light") : theme;
      root.dataset.theme = resolved;
      root.dataset.themeMode = theme;
      root.dataset.accent = accent;
      root.dataset.glass = glass;
    };
    apply();
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [theme, accent, glass]);

  const value = useMemo(
    () => ({ theme, accent, glass, setTheme, setAccent, setGlass }),
    [theme, accent, glass],
  );

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useTheme(): ThemeContextValue {
  const value = useContext(ThemeContext);
  if (!value) throw new Error("useTheme must be used inside ThemeProvider");
  return value;
}
