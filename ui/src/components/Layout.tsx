// App shell. The topbar hamburger has two jobs:
//   desktop (≥768px) — toggles the sidebar between expanded (icon + text)
//                      and collapsed (icon-only rail); choice is persisted
//   mobile  (<768px) — opens the sidebar as an overlay drawer
import { Outlet, useLocation } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import Sidebar from "./Sidebar";
import Topbar from "./Topbar";

const COLLAPSE_KEY = "rt-sidebar";

export default function Layout() {
  const [collapsed, setCollapsed] = useState(
    () => localStorage.getItem(COLLAPSE_KEY) === "collapsed",
  );
  const [drawerOpen, setDrawerOpen] = useState(false);
  const { pathname } = useLocation();

  // Close the mobile drawer whenever navigation happens.
  useEffect(() => {
    setDrawerOpen(false);
  }, [pathname]);

  const toggleSidebar = () => {
    if (window.matchMedia("(max-width: 767px)").matches) {
      setDrawerOpen((open) => !open);
    } else {
      setCollapsed((value) => {
        localStorage.setItem(COLLAPSE_KEY, value ? "expanded" : "collapsed");
        return !value;
      });
    }
  };

  return (
    <div className="min-h-screen">
      <Sidebar collapsed={collapsed} open={drawerOpen} onClose={() => setDrawerOpen(false)} />
      {drawerOpen ? (
        <div
          className="fixed inset-0 z-35 bg-scrim md:hidden"
          onClick={() => setDrawerOpen(false)}
          aria-hidden="true"
        />
      ) : null}
      <div
        className={`flex min-h-screen flex-col transition-[margin] duration-200 ${
          collapsed ? "md:ml-[72px]" : "md:ml-[250px]"
        }`}
      >
        <Topbar onMenu={toggleSidebar} />
        <main className="mx-auto w-full max-w-[1360px] flex-1 p-4 md:p-6">
          <Outlet />
        </main>
      </div>
    </div>
  );
}
