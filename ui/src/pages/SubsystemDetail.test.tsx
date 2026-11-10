import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { envelope, subsystemDetail, upstream } from "../test/fixtures";
import { apiError, mockApi, type Reply } from "../test/mockApi";
import { renderPage } from "../test/renderPage";
import SubsystemDetailPage from "./SubsystemDetail";

const DETAIL = "GET /api/subsystems/1";
const written: Reply = { body: { version: 5 } };

function renderDetail(handlers: Record<string, Reply>, detail = subsystemDetail()) {
  const api = mockApi({ [DETAIL]: { body: envelope(detail) }, ...handlers });
  const view = renderPage(SubsystemDetailPage, { path: "/subsystems/$id", entry: "/subsystems/1" });
  return { api, ...view };
}

/** The open dialog's queries. */
const dialog = () => within(screen.getByRole("dialog"));

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("SubsystemDetailPage", () => {
  it("shows the subsystem with its hosts, upstreams, and routes in match order", async () => {
    renderDetail({});

    expect(await screen.findByRole("heading", { name: "shop", level: 2 })).toBeInTheDocument();
    const header = screen.getByRole("heading", { name: "shop", level: 2 }).parentElement as HTMLElement;
    expect(within(header).getByText("enabled")).toBeInTheDocument();
    expect(screen.getByText("shop.example.com")).toBeInTheDocument();
    expect(screen.getByText("127.0.0.1:3000")).toBeInTheDocument();
    expect(screen.getByText("used by 2 routes")).toBeInTheDocument();

    const routes = screen.getAllByRole("table")[1];
    const rows = within(routes).getAllByRole("row").slice(1);
    expect(within(rows[0]).getByText("/api")).toBeInTheDocument();
    expect(within(rows[0]).getByText("strip prefix, prepend /v2, rewrite Host")).toBeInTheDocument();
    expect(within(rows[0]).getByText("1s / 2s")).toBeInTheDocument();
    expect(within(rows[1]).getByText("/")).toBeInTheDocument();
    expect(within(rows[1]).getByText("none")).toBeInTheDocument();
    expect(within(rows[1]).getByText("5s / 30s")).toBeInTheDocument();
  });

  it("says so when the subsystem does not exist", async () => {
    mockApi({ [DETAIL]: apiError(404, "NOT_FOUND", "subsystem 1 not found") });
    renderPage(SubsystemDetailPage, { path: "/subsystems/$id", entry: "/subsystems/1" });
    expect(await screen.findByText("This subsystem does not exist")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Back to subsystems" })).toHaveAttribute("href", "/subsystems");
  });

  it("guides a subsystem that has nothing yet", async () => {
    renderDetail({}, subsystemDetail({ hosts: [], upstreams: [], routes: [] }));
    expect(await screen.findByText(/receives no traffic/i)).toBeInTheDocument();
    expect(screen.getByText(/add the address of the backend server first/i)).toBeInTheDocument();
    expect(screen.getByText(/every request to this subsystem's hosts gets a 404/i)).toBeInTheDocument();
  });

  describe("settings", () => {
    it("saves the whole record", async () => {
      const { api } = renderDetail({ "PUT /api/subsystems/1": { body: envelope(subsystemDetail()) } });
      const description = await screen.findByLabelText("Description");

      await userEvent.clear(description);
      await userEvent.type(description, "Now with checkout");
      await userEvent.click(screen.getByRole("button", { name: "Save settings" }));

      await waitFor(() => expect(api.called("PUT", "/api/subsystems/1")).toHaveLength(1));
      expect(api.called("PUT", "/api/subsystems/1")[0].body).toEqual({
        name: "shop",
        description: "Now with checkout",
        enabled: true,
      });
      expect(await screen.findByText("Saved shop")).toBeInTheDocument();
    });

    it("shows a rename that collides with another subsystem", async () => {
      renderDetail({ "PUT /api/subsystems/1": apiError(409, "CONFLICT", "already exists") });
      const name = await screen.findByLabelText("Name");
      await userEvent.clear(name);
      await userEvent.type(name, "blog");
      await userEvent.click(screen.getByRole("button", { name: "Save settings" }));
      expect(await screen.findByRole("alert")).toHaveTextContent("already exists");
    });
  });

  describe("hosts", () => {
    it("adds a host and clears the input", async () => {
      const { api } = renderDetail({
        "POST /api/subsystems/1/hosts": { status: 201, body: envelope({ id: 11, pattern: "*.example.org" }, 5) },
      });
      const input = await screen.findByLabelText("Add a host");

      await userEvent.type(input, "*.example.org");
      await userEvent.click(screen.getByRole("button", { name: "Add host" }));

      expect(await screen.findByText("*.example.org now routes to shop")).toBeInTheDocument();
      expect(api.called("POST", "/api/subsystems/1/hosts")[0].body).toEqual({ pattern: "*.example.org" });
      expect(input).toHaveValue("");
    });

    it("catches an obviously wrong host before sending it", async () => {
      const { api } = renderDetail({});
      await userEvent.type(await screen.findByLabelText("Add a host"), "shop.example.com:8080");
      await userEvent.click(screen.getByRole("button", { name: "Add host" }));
      expect(await screen.findByRole("alert")).toHaveTextContent(/no spaces, scheme, port or path/i);
      expect(api.called("POST", "/api/subsystems/1/hosts")).toHaveLength(0);
    });

    it("shows a host already used by another subsystem", async () => {
      renderDetail({ "POST /api/subsystems/1/hosts": apiError(409, "CONFLICT", "host already in use") });
      await userEvent.type(await screen.findByLabelText("Add a host"), "taken.example.com");
      await userEvent.click(screen.getByRole("button", { name: "Add host" }));
      expect(await screen.findByRole("alert")).toHaveTextContent("host already in use");
    });

    it("asks before removing a host, saying what will happen", async () => {
      const { api } = renderDetail({ "DELETE /api/subsystems/1/hosts/10": written });
      await userEvent.click(await screen.findByRole("button", { name: "Remove shop.example.com" }));

      const confirm = dialog();
      expect(confirm.getByText(/will stop reaching/i)).toHaveTextContent("shop.example.com");
      expect(api.called("DELETE", "/api/subsystems/1/hosts")).toHaveLength(0);

      await userEvent.click(confirm.getByRole("button", { name: "Remove host" }));
      expect(await screen.findByText("Removed shop.example.com")).toBeInTheDocument();
      expect(api.called("DELETE", "/api/subsystems/1/hosts/10")).toHaveLength(1);
    });

    it("does nothing when the removal is cancelled", async () => {
      const { api } = renderDetail({});
      await userEvent.click(await screen.findByRole("button", { name: "Remove shop.example.com" }));
      await userEvent.click(dialog().getByRole("button", { name: "Cancel" }));
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
      expect(api.called("DELETE", "/api/subsystems/1/hosts")).toHaveLength(0);
    });
  });

  describe("upstreams", () => {
    const defaults = {
      lb: { algorithm: "round_robin", hashKey: "client_ip" },
      failurePolicy: "fail_open",
      health: {
        check: "none",
        path: "/",
        expectedStatus: [200, 399],
        intervalMs: 5000,
        timeoutMs: 2000,
        rise: 2,
        fall: 3,
      },
      passive: { consecutiveFailures: 3, baseEjectionMs: 10000, maxEjectionMs: 300000, maxEjectionPercent: 50 },
      retryBudget: { percent: 20, minPerSec: 10 },
    };

    it("checks the form before sending, naming every problem", async () => {
      const { api } = renderDetail({});
      await userEvent.click(await screen.findByRole("button", { name: /add upstream/i }));

      await userEvent.type(dialog().getByLabelText("First endpoint address"), "no-port");
      await userEvent.click(dialog().getByRole("button", { name: "Add upstream" }));

      const alerts = await dialog().findAllByRole("alert");
      expect(alerts.map((alert) => alert.textContent)).toEqual([
        "Must be 1 to 63 characters",
        "Use host:port, with a port from 1 to 65535",
      ]);
      expect(api.called("POST", "/api/subsystems/1/upstreams")).toHaveLength(0);
    });

    it("adds an upstream with the default policy and its first endpoint", async () => {
      const { api } = renderDetail({
        "POST /api/subsystems/1/upstreams": { status: 201, body: envelope(upstream({ id: 8, name: "api" }), 5) },
      });
      await userEvent.click(await screen.findByRole("button", { name: /add upstream/i }));

      await userEvent.type(dialog().getByLabelText("Name"), "api");
      await userEvent.type(dialog().getByLabelText("First endpoint address"), "api.internal:8080");
      await userEvent.click(dialog().getByRole("button", { name: "Add upstream" }));

      expect(await screen.findByText("Added upstream api")).toBeInTheDocument();
      expect(api.called("POST", "/api/subsystems/1/upstreams")[0].body).toEqual({
        name: "api",
        ...defaults,
        endpoints: [{ addr: "api.internal:8080", weight: 1 }],
      });
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });

    it("sends the chosen algorithm, hash key, failure policy and health check", async () => {
      const { api } = renderDetail({
        "POST /api/subsystems/1/upstreams": { status: 201, body: envelope(upstream(), 5) },
      });
      await userEvent.click(await screen.findByRole("button", { name: /add upstream/i }));
      const form = dialog();
      await userEvent.type(form.getByLabelText("Name"), "sticky");
      await userEvent.type(form.getByLabelText("First endpoint address"), "10.0.0.1:80");
      await userEvent.selectOptions(form.getByLabelText("Load balancing"), "consistent_hash");
      await userEvent.selectOptions(form.getByLabelText("Hash on"), "header");
      await userEvent.type(form.getByLabelText("Header name"), "x-user");
      await userEvent.selectOptions(form.getByLabelText("When no endpoint is healthy"), "fail_closed");
      await userEvent.selectOptions(form.getByLabelText("Active health check"), "http");
      await userEvent.clear(form.getByLabelText("Probe path"));
      await userEvent.type(form.getByLabelText("Probe path"), "/healthz");
      await userEvent.clear(form.getByLabelText("Probe interval (ms)"));
      await userEvent.type(form.getByLabelText("Probe interval (ms)"), "3000");
      await userEvent.click(form.getByRole("button", { name: "Add upstream" }));

      await screen.findByText("Added upstream sticky");
      const body = api.called("POST", "/api/subsystems/1/upstreams")[0].body as Record<string, unknown>;
      expect(body.lb).toEqual({ algorithm: "consistent_hash", hashKey: "header:x-user" });
      expect(body.failurePolicy).toBe("fail_closed");
      expect(body.health).toEqual({
        check: "http",
        path: "/healthz",
        expectedStatus: [200, 399],
        intervalMs: 3000,
        timeoutMs: 2000,
        rise: 2,
        fall: 3,
      });
    });

    it("opens the advanced section to show a problem inside it", async () => {
      renderDetail({});
      await userEvent.click(await screen.findByRole("button", { name: /add upstream/i }));
      const form = dialog();
      await userEvent.type(form.getByLabelText("Name"), "api");
      await userEvent.type(form.getByLabelText("First endpoint address"), "a:1");
      await userEvent.click(form.getByText(/advanced: health checks/i));
      await userEvent.clear(form.getByLabelText("Longest ejection (ms)"));
      await userEvent.type(form.getByLabelText("Longest ejection (ms)"), "5");
      // Fold it away again: the error must bring it back into view.
      await userEvent.click(form.getByText(/advanced: health checks/i));
      expect(form.getByLabelText("Longest ejection (ms)")).not.toBeVisible();
      await userEvent.click(form.getByRole("button", { name: "Add upstream" }));

      const alert = await form.findByRole("alert");
      expect(alert).toHaveTextContent("Must be a whole number from 100 to 3600000");
      expect(form.getByLabelText("Longest ejection (ms)")).toBeVisible();
    });

    it("edits an upstream's settings, prefilled, without touching its endpoints", async () => {
      const { api } = renderDetail({ "PUT /api/subsystems/1/upstreams/5": { body: envelope(upstream(), 5) } });
      await userEvent.click(await screen.findByRole("button", { name: "Edit upstream backend" }));

      expect(dialog().getByLabelText("Name")).toHaveValue("backend");
      expect(dialog().getByLabelText("Load balancing")).toHaveValue("round_robin");
      expect(dialog().queryByLabelText("First endpoint address")).not.toBeInTheDocument();
      await userEvent.selectOptions(dialog().getByLabelText("Load balancing"), "p2c_ewma");
      await userEvent.click(dialog().getByRole("button", { name: "Save upstream" }));

      expect(await screen.findByText("Updated upstream backend")).toBeInTheDocument();
      expect(api.called("PUT", "/api/subsystems/1/upstreams/5")[0].body).toEqual({
        name: "backend",
        ...defaults,
        lb: { algorithm: "p2c_ewma", hashKey: "client_ip" },
      });
    });

    it("shows a duplicate upstream name inside the dialog", async () => {
      renderDetail({ "POST /api/subsystems/1/upstreams": apiError(409, "CONFLICT", "name already exists") });
      await userEvent.click(await screen.findByRole("button", { name: /add upstream/i }));
      await userEvent.type(dialog().getByLabelText("Name"), "backend");
      await userEvent.type(dialog().getByLabelText("First endpoint address"), "127.0.0.1:1");
      await userEvent.click(dialog().getByRole("button", { name: "Add upstream" }));
      expect(await dialog().findByRole("alert")).toHaveTextContent("name already exists");
    });

    it("shows a nested field error the server found next to its input", async () => {
      renderDetail({
        "POST /api/subsystems/1/upstreams": apiError(400, "VALIDATION_FAILED", "validation failed", [
          { field: "endpoints.0.addr", message: "port must be from 1 to 65535" },
        ]),
      });
      await userEvent.click(await screen.findByRole("button", { name: /add upstream/i }));
      await userEvent.type(dialog().getByLabelText("Name"), "api");
      await userEvent.type(dialog().getByLabelText("First endpoint address"), "a:1");
      await userEvent.click(dialog().getByRole("button", { name: "Add upstream" }));
      expect(await dialog().findByText("port must be from 1 to 65535")).toBeInTheDocument();
      expect(dialog().getByLabelText("First endpoint address")).toHaveAttribute("aria-invalid", "true");
    });

    it("warns that an upstream in use cannot be deleted, and shows the server's refusal", async () => {
      renderDetail({
        "DELETE /api/subsystems/1/upstreams/5": apiError(409, "CONFLICT", "upstream 5 is used by 2 route(s)"),
      });
      await userEvent.click(await screen.findByRole("button", { name: "Delete upstream backend" }));
      expect(dialog().getByText(/2 route\(s\) still use it/i)).toBeInTheDocument();

      await userEvent.click(dialog().getByRole("button", { name: "Delete upstream" }));
      expect(await screen.findByText("upstream 5 is used by 2 route(s)")).toBeInTheDocument();
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });

    it("deletes an unused upstream", async () => {
      const unused = subsystemDetail({ routes: [] });
      const { api } = renderDetail({ "DELETE /api/subsystems/1/upstreams/5": written }, unused);
      await userEvent.click(await screen.findByRole("button", { name: "Delete upstream backend" }));
      expect(dialog().getByText(/no route uses it/i)).toBeInTheDocument();
      await userEvent.click(dialog().getByRole("button", { name: "Delete upstream" }));
      expect(await screen.findByText("Deleted upstream backend")).toBeInTheDocument();
      expect(api.called("DELETE", "/api/subsystems/1/upstreams/5")).toHaveLength(1);
    });
  });

  describe("routes", () => {
    it("adds a route with every field in the request", async () => {
      const { api } = renderDetail({
        "POST /api/subsystems/1/routes": { status: 201, body: envelope({}, 5) },
      });
      await userEvent.click(await screen.findByRole("button", { name: /add route/i }));

      const form = dialog();
      await userEvent.clear(form.getByLabelText("Path prefix"));
      await userEvent.type(form.getByLabelText("Path prefix"), "/shop/");
      await userEvent.type(form.getByLabelText("Upstream path prefix"), "/v3");
      await userEvent.click(form.getByRole("checkbox", { name: "Strip the matched prefix" }));
      await userEvent.click(form.getByRole("checkbox", { name: "Keep the client's Host header" }));
      await userEvent.clear(form.getByLabelText("Connect timeout (ms)"));
      await userEvent.type(form.getByLabelText("Connect timeout (ms)"), "1500");
      await userEvent.click(form.getByRole("button", { name: "Add route" }));

      expect(await screen.findByText("Added route /shop/")).toBeInTheDocument();
      expect(api.called("POST", "/api/subsystems/1/routes")[0].body).toEqual({
        prefix: "/shop/",
        upstreamGroupId: 5,
        stripPrefix: true,
        upstreamPrefix: "/v3",
        preserveHost: false,
        connectTimeoutMs: 1500,
        responseTimeoutMs: 30000,
        retry: { maxAttempts: 2, idempotentOn5xx: true, onTimeout: false },
        enabled: true,
      });
    });

    it("reports every invalid field at once and sends nothing", async () => {
      const { api } = renderDetail({});
      await userEvent.click(await screen.findByRole("button", { name: /add route/i }));

      const form = dialog();
      await userEvent.clear(form.getByLabelText("Path prefix"));
      await userEvent.type(form.getByLabelText("Path prefix"), "no-slash");
      await userEvent.type(form.getByLabelText("Upstream path prefix"), "bad");
      await userEvent.clear(form.getByLabelText("Response timeout (ms)"));
      await userEvent.type(form.getByLabelText("Response timeout (ms)"), "5");
      await userEvent.click(form.getByRole("button", { name: "Add route" }));

      const alerts = await form.findAllByRole("alert");
      expect(alerts.map((alert) => alert.textContent)).toEqual([
        "Must start with '/'",
        "Must start with '/'",
        "Must be between 100 and 600000 ms",
      ]);
      expect(api.called("POST", "/api/subsystems/1/routes")).toHaveLength(0);
    });

    it("shows a field error the server found", async () => {
      renderDetail({
        "POST /api/subsystems/1/routes": apiError(400, "VALIDATION_FAILED", "validation failed", [
          { field: "upstreamGroupId", message: "must be an upstream of this subsystem" },
        ]),
      });
      await userEvent.click(await screen.findByRole("button", { name: /add route/i }));
      await userEvent.clear(dialog().getByLabelText("Path prefix"));
      await userEvent.type(dialog().getByLabelText("Path prefix"), "/x");
      await userEvent.click(dialog().getByRole("button", { name: "Add route" }));

      expect(await dialog().findByText("must be an upstream of this subsystem")).toBeInTheDocument();
      expect(dialog().getByLabelText("Upstream")).toHaveAttribute("aria-invalid", "true");
    });

    it("refuses a duplicate prefix inside the dialog", async () => {
      renderDetail({ "POST /api/subsystems/1/routes": apiError(409, "CONFLICT", "prefix already used") });
      await userEvent.click(await screen.findByRole("button", { name: /add route/i }));
      await userEvent.click(dialog().getByRole("button", { name: "Add route" }));
      expect(await dialog().findByRole("alert")).toHaveTextContent("prefix already used");
    });

    it("cannot add a route until an upstream exists", async () => {
      renderDetail({}, subsystemDetail({ upstreams: [], routes: [] }));
      await userEvent.click(await screen.findByRole("button", { name: /add route/i }));
      expect(dialog().getByText("Add an upstream to this subsystem first.")).toBeInTheDocument();
      expect(dialog().getByRole("button", { name: "Add route" })).toBeDisabled();
    });

    it("edits a route, prefilled", async () => {
      const { api } = renderDetail({ "PUT /api/subsystems/1/routes/7": { body: envelope({}, 5) } });
      await userEvent.click(await screen.findByRole("button", { name: "Edit route /api" }));

      expect(dialog().getByLabelText("Path prefix")).toHaveValue("/api");
      expect(dialog().getByLabelText("Upstream path prefix")).toHaveValue("/v2");
      expect(dialog().getByLabelText("Connect timeout (ms)")).toHaveValue("1000");
      await userEvent.click(dialog().getByRole("button", { name: "Save route" }));

      expect(await screen.findByText("Updated route /api")).toBeInTheDocument();
      expect(api.called("PUT", "/api/subsystems/1/routes/7")[0].body).toMatchObject({
        prefix: "/api",
        stripPrefix: true,
        upstreamPrefix: "/v2",
        preserveHost: false,
      });
    });

    it("edits a route's retry settings", async () => {
      const { api } = renderDetail({ "PUT /api/subsystems/1/routes/7": { body: envelope({}, 5) } });
      await userEvent.click(await screen.findByRole("button", { name: "Edit route /api" }));

      expect(dialog().getByLabelText("Attempts")).toHaveValue("2");
      expect(dialog().getByRole("checkbox", { name: /retry idempotent requests on 502/i })).toBeChecked();
      expect(dialog().getByRole("checkbox", { name: "Also retry response timeouts" })).not.toBeChecked();
      await userEvent.clear(dialog().getByLabelText("Attempts"));
      await userEvent.type(dialog().getByLabelText("Attempts"), "4");
      await userEvent.click(dialog().getByRole("checkbox", { name: /retry idempotent requests on 502/i }));
      await userEvent.click(dialog().getByRole("checkbox", { name: "Also retry response timeouts" }));
      await userEvent.click(dialog().getByRole("button", { name: "Save route" }));

      expect(await screen.findByText("Updated route /api")).toBeInTheDocument();
      expect(api.called("PUT", "/api/subsystems/1/routes/7")[0].body).toMatchObject({
        retry: { maxAttempts: 4, idempotentOn5xx: false, onTimeout: true },
      });
    });

    it("rejects an out-of-range number of attempts before sending", async () => {
      const { api } = renderDetail({});
      await userEvent.click(await screen.findByRole("button", { name: "Edit route /api" }));
      await userEvent.clear(dialog().getByLabelText("Attempts"));
      await userEvent.type(dialog().getByLabelText("Attempts"), "9");
      await userEvent.click(dialog().getByRole("button", { name: "Save route" }));
      expect(await dialog().findByRole("alert")).toHaveTextContent("Must be a whole number from 1 to 5");
      expect(api.called("PUT", "/api/subsystems/1/routes/7")).toHaveLength(0);
    });

    it("switches a route off with its other settings unchanged", async () => {
      const { api } = renderDetail({ "PUT /api/subsystems/1/routes/7": { body: envelope({}, 5) } });
      const row = (await screen.findByText("/api")).closest("tr") as HTMLElement;
      await userEvent.click(within(row).getByRole("checkbox"));

      expect(await screen.findByText("Disabled route /api")).toBeInTheDocument();
      expect(api.called("PUT", "/api/subsystems/1/routes/7")[0].body).toEqual({
        prefix: "/api",
        upstreamGroupId: 5,
        stripPrefix: true,
        upstreamPrefix: "/v2",
        preserveHost: false,
        connectTimeoutMs: 1000,
        responseTimeoutMs: 2000,
        retry: { maxAttempts: 2, idempotentOn5xx: true, onTimeout: false },
        enabled: false,
      });
    });

    it("asks before deleting a route, saying what requests will do instead", async () => {
      const { api } = renderDetail({ "DELETE /api/subsystems/1/routes/7": written });
      await userEvent.click(await screen.findByRole("button", { name: "Delete route /api" }));
      expect(dialog().getByText(/next-longest matching route, or get a 404/i)).toBeInTheDocument();
      expect(api.called("DELETE", "/api/subsystems/1/routes")).toHaveLength(0);

      await userEvent.click(dialog().getByRole("button", { name: "Delete route" }));
      expect(await screen.findByText("Deleted route /api")).toBeInTheDocument();
      expect(api.called("DELETE", "/api/subsystems/1/routes/7")).toHaveLength(1);
    });
  });

  describe("deleting the subsystem", () => {
    it("states what is removed, then returns to the list", async () => {
      const { api } = renderDetail({ "DELETE /api/subsystems/1": written });
      await userEvent.click(await screen.findByRole("button", { name: /delete subsystem/i }));

      expect(dialog().getByText(/1 host\(s\), 1 upstream\(s\) and 2 route\(s\)/i)).toBeInTheDocument();
      await userEvent.click(dialog().getByRole("button", { name: "Delete subsystem" }));

      expect(await screen.findByText("subsystems page")).toBeInTheDocument();
      expect(api.called("DELETE", "/api/subsystems/1")).toHaveLength(1);
    });

    it("can be cancelled", async () => {
      const { api } = renderDetail({});
      await userEvent.click(await screen.findByRole("button", { name: /delete subsystem/i }));
      await userEvent.click(dialog().getByRole("button", { name: "Cancel" }));
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
      expect(api.called("DELETE", "/api/subsystems/1")).toHaveLength(0);
    });
  });
});
