import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

/**
 * Multi-theme engine (Phase 9).
 *
 * A theme is a self-contained token set applied through `[data-theme="…"]`
 * on `<html>` (see index.css). Themes are independent of the legacy
 * light/dark pair: every theme declares its `appearance`, which drives the
 * `dark` class (existing `dark:` utilities) and Doran's `data-doran-theme`
 * attribute. The selection persists in `localStorage` under the historical
 * `tasker-theme` key; index.html re-applies it before first paint (no flash).
 */
export interface ThemeDefinition {
  id: string;
  label: string;
  appearance: "light" | "dark";
  /** Preview colors for the switcher: background, primary, accent. */
  swatch: [string, string, string];
}

export const THEMES: ThemeDefinition[] = [
  { id: "light", label: "Light", appearance: "light", swatch: ["#ffffff", "#524bcf", "#ededf2"] },
  { id: "dark", label: "Dark", appearance: "dark", swatch: ["#0d0e15", "#8c88f7", "#232430"] },
  { id: "midnight", label: "Midnight", appearance: "dark", swatch: ["#0b1120", "#22ae94", "#182338"] },
  { id: "nord", label: "Nord", appearance: "dark", swatch: ["#2e3440", "#88c0d0", "#3b4252"] },
  { id: "dracula", label: "Dracula", appearance: "dark", swatch: ["#282a36", "#bd93f9", "#ff79c6"] },
  { id: "solarized-light", label: "Solarized Light", appearance: "light", swatch: ["#fdf6e3", "#1f77ad", "#eee8d5"] },
  { id: "solarized-dark", label: "Solarized Dark", appearance: "dark", swatch: ["#002b36", "#1d76ae", "#073642"] },
  { id: "rose-pine", label: "Rosé Pine", appearance: "dark", swatch: ["#191724", "#ebbcba", "#f6c177"] },
  { id: "catppuccin-mocha", label: "Catppuccin Mocha", appearance: "dark", swatch: ["#1e1e2e", "#cba6f7", "#f5c2e7"] },
  { id: "github-dark", label: "GitHub Dark", appearance: "dark", swatch: ["#0d1117", "#238636", "#30363d"] },
  { id: "github-light", label: "GitHub Light", appearance: "light", swatch: ["#ffffff", "#1f883d", "#d0d7de"] },
  { id: "one-dark", label: "One Dark", appearance: "dark", swatch: ["#282c34", "#61afef", "#3e4451"] },
  { id: "gruvbox-dark", label: "Gruvbox Dark", appearance: "dark", swatch: ["#282828", "#fabd2f", "#3c3836"] },
  { id: "gruvbox-light", label: "Gruvbox Light", appearance: "light", swatch: ["#fbf1c7", "#b57614", "#ebdbb2"] },
  { id: "tokyo-night", label: "Tokyo Night", appearance: "dark", swatch: ["#1a1b26", "#7aa2f7", "#24283b"] },
  { id: "kanagawa", label: "Kanagawa", appearance: "dark", swatch: ["#1f1f28", "#7e9cd8", "#2a2a3a"] },
];

const THEME_BY_ID = new Map(THEMES.map((t) => [t.id, t]));

export function themeById(id: string): ThemeDefinition | undefined {
  return THEME_BY_ID.get(id);
}

type ThemeId = string;

interface ThemeContextValue {
  /** Active theme id (also mirrored to `<html data-theme>`). */
  theme: ThemeId;
  /** Baseline appearance of the active theme (drives Sonner/Doran/dark:). */
  appearance: "light" | "dark";
  setTheme: (id: string) => void;
}

const ThemeContext = createContext<ThemeContextValue>({
  theme: "light",
  appearance: "light",
  setTheme: () => undefined,
});

function readInitial(): ThemeId {
  try {
    const saved = localStorage.getItem("tasker-theme");
    if (saved && THEME_BY_ID.has(saved)) return saved;
  } catch {
    /* storage unavailable */
  }
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<ThemeId>(readInitial);
  const appearance = themeById(theme)?.appearance ?? "light";

  useEffect(() => {
    const root = document.documentElement;
    root.setAttribute("data-theme", theme);
    // Doran components read their light/dark tokens from this attribute and
    // legacy `dark:` utilities keep working via the class.
    root.classList.toggle("dark", appearance === "dark");
    root.setAttribute("data-doran-theme", appearance);
    root.style.colorScheme = appearance;
    try {
      localStorage.setItem("tasker-theme", theme);
    } catch {
      /* storage unavailable */
    }
  }, [theme, appearance]);

  const setTheme = useCallback((id: string) => {
    if (THEME_BY_ID.has(id)) {
      setThemeState(id);
    }
  }, []);

  const value = useMemo<ThemeContextValue>(
    () => ({ theme, appearance, setTheme }),
    [theme, appearance, setTheme],
  );

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useTheme(): ThemeContextValue {
  return useContext(ThemeContext);
}
