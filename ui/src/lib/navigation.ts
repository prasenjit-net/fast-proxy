// Which page is showing and which sidebar item it belongs to.

const TITLES: Record<string, string> = {
  "/": "Dashboard",
  "/subsystems": "Subsystems",
  "/revisions": "Revisions",
  "/resolver": "Resolver",
  "/settings": "Settings",
};

/** The page title for a path; a subsystem's detail page is just "Subsystem". */
export function pageTitle(pathname: string): string {
  if (TITLES[pathname]) return TITLES[pathname];
  if (pathname.startsWith("/subsystems/")) return "Subsystem";
  return "Not found";
}

/** The dashboard is active only on `/`; other items also on their sub-pages. */
export function isNavActive(pathname: string, to: string): boolean {
  return to === "/" ? pathname === "/" : pathname === to || pathname.startsWith(`${to}/`);
}
