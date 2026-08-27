import { useTheme, type AccentName, type GlassMode, type ThemeMode } from "../theme/ThemeProvider";

const themes: Array<[ThemeMode, string]> = [
  ["system", "System"],
  ["light", "Light"],
  ["dark", "Dark"],
];

const accents: Array<[AccentName, string]> = [
  ["violet", "Violet"],
  ["blue", "Blue"],
  ["cyan", "Cyan"],
  ["orange", "Orange"],
];

export function AppearancePopover() {
  const { theme, accent, glass, setTheme, setAccent, setGlass } = useTheme();

  return (
    <div className="appearance-popover" role="dialog" aria-label="Appearance">
      <div className="popover-title">
        <span>Appearance</span>
        <span className="popover-kicker">UI</span>
      </div>
      <fieldset>
        <legend>Theme</legend>
        <div className="choice-row">
          {themes.map(([value, label]) => (
            <button
              key={value}
              className={`choice-chip ${theme === value ? "is-selected" : ""}`}
              onClick={() => setTheme(value)}
              aria-pressed={theme === value}
            >
              {label}
            </button>
          ))}
        </div>
      </fieldset>
      <fieldset>
        <legend>Accent</legend>
        <div className="choice-row accent-choices">
          {accents.map(([value, label]) => (
            <button
              key={value}
              className={`choice-chip accent-${value} ${accent === value ? "is-selected" : ""}`}
              onClick={() => setAccent(value)}
              aria-pressed={accent === value}
            >
              <span className="accent-dot" />
              {label}
            </button>
          ))}
        </div>
      </fieldset>
      <fieldset>
        <legend>Glass</legend>
        <div className="choice-row">
          {(["on", "reduced"] as GlassMode[]).map((value) => (
            <button
              key={value}
              className={`choice-chip ${glass === value ? "is-selected" : ""}`}
              onClick={() => setGlass(value)}
              aria-pressed={glass === value}
            >
              {value === "on" ? "On" : "Reduced"}
            </button>
          ))}
        </div>
      </fieldset>
    </div>
  );
}
