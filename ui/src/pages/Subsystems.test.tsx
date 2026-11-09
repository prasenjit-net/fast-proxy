import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { envelope, subsystemDetail, subsystemSummary } from "../test/fixtures";
import { apiError, mockApi } from "../test/mockApi";
import { renderPage } from "../test/renderPage";
import SubsystemsPage from "./Subsystems";

function renderList() {
  return renderPage(SubsystemsPage, { path: "/subsystems" });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("SubsystemsPage", () => {
  it("explains the first steps when there are no subsystems", async () => {
    mockApi({ "GET /api/subsystems": { body: envelope([], 0) } });
    renderList();

    expect(await screen.findByText("No subsystems yet")).toBeInTheDocument();
    expect(screen.getByText(/one host name served by the proxy/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /new subsystem/i })).toBeInTheDocument();
  });

  it("lists subsystems with host chips, route counts and links to their pages", async () => {
    mockApi({
      "GET /api/subsystems": {
        body: envelope([
          subsystemSummary({
            hosts: ["a.example.com", "b.example.com", "c.example.com", "d.example.com", "e.example.com"],
          }),
          subsystemSummary({ id: 2, name: "blog", description: "", hosts: [], routeCount: 0, enabled: false }),
        ]),
      },
    });
    renderList();

    const link = await screen.findByRole("link", { name: "shop" });
    expect(link).toHaveAttribute("href", "/subsystems/1");
    expect(screen.getByRole("link", { name: "blog" })).toHaveAttribute("href", "/subsystems/2");
    expect(screen.getByText("a.example.com")).toBeInTheDocument();
    expect(screen.getByText("c.example.com")).toBeInTheDocument();
    expect(screen.queryByText("d.example.com")).not.toBeInTheDocument();
    expect(screen.getByText("+2 more")).toBeInTheDocument();
    expect(screen.getByText("no hosts")).toBeInTheDocument();
    expect(screen.getByText("The shop")).toBeInTheDocument();
  });

  it("switches a subsystem off, sending the whole record", async () => {
    const api = mockApi({
      "GET /api/subsystems": { body: envelope([subsystemSummary()]) },
      "PUT /api/subsystems/1": { body: envelope(subsystemDetail({ enabled: false })) },
    });
    renderList();

    await userEvent.click(await screen.findByRole("checkbox"));

    await waitFor(() => expect(api.called("PUT", "/api/subsystems/1")).toHaveLength(1));
    expect(api.called("PUT", "/api/subsystems/1")[0].body).toEqual({
      name: "shop",
      description: "The shop",
      enabled: false,
    });
    expect(await screen.findByText("shop disabled")).toBeInTheDocument();
  });

  it("shows a message when the list cannot be loaded", async () => {
    mockApi({ "GET /api/subsystems": apiError(500, "INTERNAL", "an internal error occurred") });
    renderList();
    expect(await screen.findByText(/could not load subsystems/i)).toHaveTextContent(
      "an internal error occurred",
    );
  });

  describe("creating a subsystem", () => {
    async function openDialog() {
      await userEvent.click(await screen.findByRole("button", { name: /new subsystem/i }));
      return screen.findByRole("dialog", { name: "New subsystem" });
    }

    it("checks the name before sending anything", async () => {
      const api = mockApi({ "GET /api/subsystems": { body: envelope([], 0) } });
      renderList();
      const dialog = within(await openDialog());

      await userEvent.type(dialog.getByLabelText("Name"), "Bad Name");
      await userEvent.click(dialog.getByRole("button", { name: "Create subsystem" }));

      expect(await dialog.findByRole("alert")).toHaveTextContent(/lowercase letters, digits/i);
      expect(api.called("POST", "/api/subsystems")).toHaveLength(0);
    });

    it("sends the form and opens the new subsystem", async () => {
      const api = mockApi({
        "GET /api/subsystems": { body: envelope([], 0) },
        "POST /api/subsystems": { status: 201, body: envelope(subsystemDetail({ id: 9, name: "shop" }), 1) },
      });
      renderList();
      const dialog = within(await openDialog());

      await userEvent.type(dialog.getByLabelText("Name"), "shop");
      await userEvent.type(dialog.getByLabelText("Description"), "The shop");
      await userEvent.click(dialog.getByRole("button", { name: "Create subsystem" }));

      expect(await screen.findByText("subsystem detail page")).toBeInTheDocument();
      expect(api.called("POST", "/api/subsystems")[0].body).toEqual({
        name: "shop",
        description: "The shop",
        enabled: true,
      });
    });

    it("shows a duplicate name from the server inside the dialog", async () => {
      mockApi({
        "GET /api/subsystems": { body: envelope([], 0) },
        "POST /api/subsystems": apiError(409, "CONFLICT", "already exists (UNIQUE constraint failed)"),
      });
      renderList();
      const dialog = within(await openDialog());

      await userEvent.type(dialog.getByLabelText("Name"), "shop");
      await userEvent.click(dialog.getByRole("button", { name: "Create subsystem" }));

      expect(await dialog.findByRole("alert")).toHaveTextContent("already exists");
      expect(screen.getByRole("dialog", { name: "New subsystem" })).toBeInTheDocument();
    });

    it("shows server field errors next to their inputs", async () => {
      mockApi({
        "GET /api/subsystems": { body: envelope([], 0) },
        "POST /api/subsystems": apiError(400, "VALIDATION_FAILED", "validation failed", [
          { field: "name", message: "server says the name is bad" },
        ]),
      });
      renderList();
      const dialog = within(await openDialog());

      await userEvent.type(dialog.getByLabelText("Name"), "shop");
      await userEvent.click(dialog.getByRole("button", { name: "Create subsystem" }));

      expect(await dialog.findByText("server says the name is bad")).toBeInTheDocument();
      expect(dialog.getByLabelText("Name")).toHaveAttribute("aria-invalid", "true");
    });

    it("can be cancelled", async () => {
      mockApi({ "GET /api/subsystems": { body: envelope([], 0) } });
      renderList();
      const dialog = within(await openDialog());
      await userEvent.click(dialog.getByRole("button", { name: "Cancel" }));
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
  });
});
