import { useTheme, type ThemeMode } from "../context/ThemeContext";
import { IconMonitor, IconMoon, IconSun } from "../icons";

const ORDER: ThemeMode[] = ["light", "dark", "auto"];

const LABELS: Record<ThemeMode, string> = {
  light: "Light theme",
  dark: "Dark theme",
  auto: "System theme",
};

/** Cycles light → dark → auto. */
export default function ThemeToggle() {
  const { mode, setMode } = useTheme();
  const next = ORDER[(ORDER.indexOf(mode) + 1) % ORDER.length];
  const icon =
    mode === "light" ? (
      <IconSun size={18} />
    ) : mode === "dark" ? (
      <IconMoon size={18} />
    ) : (
      <IconMonitor size={18} />
    );
  return (
    <button
      className="icon-btn"
      onClick={() => setMode(next)}
      title={`${LABELS[mode]} — click for ${LABELS[next].toLowerCase()}`}
      aria-label="Switch theme"
    >
      {icon}
    </button>
  );
}
