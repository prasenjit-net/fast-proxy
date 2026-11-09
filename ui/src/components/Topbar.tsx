import { useLocation } from "@tanstack/react-router";
import { IconMenu } from "../icons";
import ConnectionBadge from "./ConnectionBadge";
import ThemeToggle from "./ThemeToggle";

const TITLES: Record<string, string> = {
  "/": "Dashboard",
  "/components": "Components",
  "/settings": "Settings",
};

export default function Topbar({ onMenu }: { onMenu: () => void }) {
  const { pathname } = useLocation();
  const title = TITLES[pathname] ?? "Not found";
  return (
    <header className="sticky top-0 z-30 flex h-[60px] items-center gap-3 border-b border-line bg-canvas/80 px-4 backdrop-blur-md md:px-6">
      <button className="icon-btn" onClick={onMenu} aria-label="Toggle sidebar">
        <IconMenu size={20} />
      </button>
      <h1 className="mr-auto text-[1.02rem] font-semibold">{title}</h1>
      <div className="flex items-center gap-2">
        <ConnectionBadge />
        <ThemeToggle />
      </div>
    </header>
  );
}
