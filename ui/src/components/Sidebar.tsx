import { Link, useLocation } from "@tanstack/react-router";
import type { ReactElement } from "react";
import { useConfig } from "../context/ConfigContext";
import {
  IconDashboard,
  IconExternal,
  IconLayers,
  IconSliders,
  IconX,
} from "../icons";
import Logo from "./Logo";

interface NavItem {
  to: "/" | "/components" | "/settings";
  label: string;
  icon: ReactElement;
}

const NAV: NavItem[] = [
  { to: "/", label: "Dashboard", icon: <IconDashboard size={20} /> },
  { to: "/components", label: "Components", icon: <IconLayers size={20} /> },
  { to: "/settings", label: "Settings", icon: <IconSliders size={20} /> },
];

interface SidebarProps {
  /** Desktop: icon-only rail when true, icon + text when false. */
  collapsed: boolean;
  /** Mobile: overlay drawer visibility. */
  open: boolean;
  onClose: () => void;
}

export default function Sidebar({ collapsed, open, onClose }: SidebarProps) {
  const config = useConfig();
  const { pathname } = useLocation();

  // On mobile the drawer always shows labels; `md:hidden` only kicks in
  // for the collapsed desktop rail.
  const labelCls = collapsed ? "md:hidden" : "";
  const itemCls = (isActive: boolean) =>
    [
      "flex items-center gap-2.5 rounded-lg px-3 py-2 text-sm font-medium transition-colors",
      collapsed ? "md:justify-center md:px-0" : "",
      isActive
        ? "bg-accent-soft text-accent"
        : "text-ink-muted hover:bg-surface-2 hover:text-ink",
    ].join(" ");

  return (
    <aside
      className={[
        "fixed inset-y-0 left-0 z-40 flex w-[260px] flex-col border-r border-line bg-surface",
        "transition-[width,transform] duration-200",
        collapsed ? "md:w-[72px]" : "md:w-[250px]",
        open ? "max-md:translate-x-0 max-md:shadow-2xl" : "max-md:-translate-x-full",
      ].join(" ")}
    >
      <div
        className={`flex min-h-[60px] items-center gap-3 border-b border-line px-3 py-3 ${
          collapsed ? "md:justify-center md:px-1" : ""
        }`}
      >
        <Logo size={34} />
        <div className={`flex min-w-0 flex-col leading-tight ${labelCls}`}>
          <strong className="truncate text-[0.95rem]">{config.ui.appName}</strong>
          <span className="truncate font-mono text-[0.66rem] text-ink-faint">
            {config.ui.tagline}
          </span>
        </div>
        <button className="icon-btn ml-auto md:hidden" onClick={onClose} aria-label="Close menu">
          <IconX size={18} />
        </button>
      </div>

      <nav className="flex flex-1 flex-col gap-0.5 overflow-y-auto p-2.5">
        {NAV.map((item) => (
          <Link
            key={item.to}
            to={item.to}
            title={collapsed ? item.label : undefined}
            className={itemCls(pathname === item.to)}
          >
            <span className="inline-flex shrink-0">{item.icon}</span>
            <span className={labelCls}>{item.label}</span>
          </Link>
        ))}
      </nav>

      <div className="flex flex-col gap-0.5 border-t border-line p-2.5">
        {config.ui.repoUrl ? (
          <a
            className={[
              "flex items-center gap-2.5 rounded-lg px-3 py-2 text-sm font-medium text-ink-muted transition-colors hover:bg-surface-2 hover:text-ink",
              collapsed ? "md:justify-center md:px-0" : "",
            ].join(" ")}
            href={config.ui.repoUrl}
            target="_blank"
            rel="noreferrer"
            title={collapsed ? "Repository" : undefined}
          >
            <span className="inline-flex shrink-0">
              <IconExternal size={18} />
            </span>
            <span className={labelCls}>Repository</span>
          </a>
        ) : null}
        <span className={`px-3 py-1 font-mono text-[0.66rem] text-ink-faint ${labelCls}`}>
          v{config.version}
        </span>
      </div>
    </aside>
  );
}
