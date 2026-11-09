import { Link } from "@tanstack/react-router";

export default function NotFoundPage() {
  return (
    <div className="flex flex-col items-center gap-3 px-4 py-12 text-center">
      <div className="text-7xl leading-none font-bold tracking-wider text-accent opacity-35">
        404
      </div>
      <h2 className="text-lg font-semibold">Page not found</h2>
      <p className="max-w-md text-ink-muted">
        The server's SPA fallback delivered the app for this URL, but the client
        router has no matching route.
      </p>
      <Link className="btn btn-primary" to="/">
        Back to dashboard
      </Link>
    </div>
  );
}
