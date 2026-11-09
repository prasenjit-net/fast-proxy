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
    expect(screen.getByText("enabled")).toBeInTheDocument();
    expect(screen.getByText("shop.example.com")).toBeInTheDocument();
    expect(screen.getByText("127.0.0.1:3000")).toBeInTheDocument();
    expect(screen.getByText("2 routes")).toBeInTheDocument();

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
    it("checks the form before sending, naming every problem", async () => {
      const { api } = renderDetail({});
      await userEvent.click(await screen.findByRole("button", { name: /add upstream/i }));

      await userEvent.type(dialog().getByLabelText("Address"), "no-port");
      await userEvent.click(dialog().getByRole("button", { name: "Add upstream" }));

      const alerts = await dialog().findAllByRole("alert");
      expect(alerts.map((alert) => alert.textContent)).toEqual([
        "Must be 1 to 63 characters",
        "Use host:port, with a port from 1 to 65535",
      ]);
      expect(api.called("POST", "/api/subsystems/1/upstreams")).toHaveLength(0);
    });

    it("adds an upstream and closes the dialog", async () => {
      const { api } = renderDetail({
        "POST /api/subsystems/1/upstreams": {
          status: 201,
          body: envelope(upstream({ id: 8, name: "api", addr: "api.internal:8080" }), 5),
        },
      });
      await userEvent.click(await screen.findByRole("button", { name: /add upstream/i }));

      await userEvent.type(dialog().getByLabelText("Name"), "api");
      await userEvent.type(dialog().getByLabelText("Address"), "api.internal:8080");
      await userEvent.click(dialog().getByRole("button", { name: "Add upstream" }));

      expect(await screen.findByText("Added upstream api")).toBeInTheDocument();
      expect(api.called("POST", "/api/subsystems/1/upstreams")[0].body).toEqual({
        name: "api",
        addr: "api.internal:8080",
        weight: 1,
      });
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });

    it("edits an upstream, prefilled with its current values", async () => {
      const { api } = renderDetail({ "PUT /api/subsystems/1/upstreams/5": { body: envelope(upstream(), 5) } });
      await userEvent.click(await screen.findByRole("button", { name: "Edit upstream backend" }));

      expect(dialog().getByLabelText("Name")).toHaveValue("backend");
      expect(dialog().getByLabelText("Address")).toHaveValue("127.0.0.1:3000");
      await userEvent.clear(dialog().getByLabelText("Address"));
      await userEvent.type(dialog().getByLabelText("Address"), "127.0.0.1:4000");
      await userEvent.click(dialog().getByRole("button", { name: "Save upstream" }));

      expect(await screen.findByText("Updated upstream backend")).toBeInTheDocument();
      expect(api.called("PUT", "/api/subsystems/1/upstreams/5")[0].body).toEqual({
        name: "backend",
        addr: "127.0.0.1:4000",
        weight: 1,
      });
    });

    it("shows a duplicate upstream name inside the dialog", async () => {
      renderDetail({ "POST /api/subsystems/1/upstreams": apiError(409, "CONFLICT", "name already exists") });
      await userEvent.click(await screen.findByRole("button", { name: /add upstream/i }));
      await userEvent.type(dialog().getByLabelText("Name"), "backend");
      await userEvent.type(dialog().getByLabelText("Address"), "127.0.0.1:1");
      await userEvent.click(dialog().getByRole("button", { name: "Add upstream" }));
      expect(await dialog().findByRole("alert")).toHaveTextContent("name already exists");
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
