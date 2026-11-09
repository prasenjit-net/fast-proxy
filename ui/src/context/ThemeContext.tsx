// Light/dark/auto theming. The effective theme is stamped onto
// <html data-theme="…">, which the CSS custom properties key off.
// Priority: the visitor's saved choice > the server's default_theme
// ([ui] in config.toml) > "auto" (follow the OS setting live).
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { useConfig } from "./ConfigContext";

export type ThemeMode = "light" | "dark" | "auto";

const STORAGE_KEY = "rt-theme";

interface ThemeContextValue {
  mode: ThemeMode;
  resolved: "light" | "dark";
  setMode: (mode: ThemeMode) => void;
}

const ThemeContext = createContext<ThemeContextValue | null>(null);

const prefersDark = () => window.matchMedia("(prefers-color-scheme: dark)");

const resolve = (mode: ThemeMode): "light" | "dark" =>
  mode === "auto" ? (prefersDark().matches ? "dark" : "light") : mode;

export function ThemeProvider({ children }: { children: ReactNode }) {
  const config = useConfig();
  const [mode, setModeState] = useState<ThemeMode>(() => {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved === "light" || saved === "dark" || saved === "auto") return saved;
    const fallback = config.ui.defaultTheme;
    return fallback === "light" || fallback === "dark" ? fallback : "auto";
  });
  const [resolved, setResolved] = useState<"light" | "dark">(() => resolve(mode));

  useEffect(() => {
    const apply = () => setResolved(resolve(mode));
    apply();
    if (mode !== "auto") return;
    const media = prefersDark();
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [mode]);

  useEffect(() => {
    document.documentElement.dataset.theme = resolved;
  }, [resolved]);

  const setMode = useCallback((next: ThemeMode) => {
    setModeState(next);
    localStorage.setItem(STORAGE_KEY, next);
  }, []);

  const value = useMemo(() => ({ mode, resolved, setMode }), [mode, resolved, setMode]);

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useTheme(): ThemeContextValue {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error("useTheme must be used inside <ThemeProvider>");
  return ctx;
}
