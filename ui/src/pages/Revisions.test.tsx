import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { envelope, httpsSettings, route, revisionPage, upstream } from "../test/fixtures";
import { apiError, mockApi } from "../test/mockApi";
import { renderPage } from "../test/renderPage";
import RevisionsPage from "./Revisions";

const LIST = "GET /api/revisions?limit=25";

function renderRevisions() {
  return renderPage(RevisionsPage, { path: "/revisions" });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("RevisionsPage", () => {
  it("lists revisions newest first, marking the current one", async () => {
    mockApi({ [LIST]: { body: revisionPage() } });
    renderRevisions();

    const rows = (await screen.findAllByRole("row")).slice(1);
    expect(rows).toHaveLength(2);
    expect(within(rows[0]).getByText("4")).toBeInTheDocument();
    expect(within(rows[0]).getByText("current")).toBeInTheDocument();
    expect(within(rows[0]).getByText("route created: / in shop")).toBeInTheDocument();
    expect(within(rows[1]).queryByText("current")).not.toBeInTheDocument();
  });

  it("offers a rollback for every revision except the current one", async () => {
    mockApi({ [LIST]: { body: revisionPage() } });
    renderRevisions();
    const rows = (await screen.findAllByRole("row")).slice(1);
    expect(within(rows[0]).queryByRole("button", { name: /roll back/i })).not.toBeInTheDocument();
    expect(within(rows[1]).getByRole("button", { name: /roll back/i })).toBeInTheDocument();
  });

  it("explains an empty history", async () => {
    mockApi({ [LIST]: { body: revisionPage({ data: [], version: 0 }) } });
    renderRevisions();
    expect(await screen.findByText("No changes yet")).toBeInTheDocument();
  });

  it("loads older revisions on request", async () => {
    const api = mockApi({
      [LIST]: { body: revisionPage({ nextBefore: 3 }) },
      "GET /api/revisions?limit=25&before=3": {
        body: revisionPage({
          data: [{ version: 2, createdAtMs: 1, summary: "host added: a.example.com in shop" }],
          nextBefore: null,
        }),
      },
    });
    renderRevisions();

    await userEvent.click(await screen.findByRole("button", { name: "Load older revisions" }));

    expect(await screen.findByText("host added: a.example.com in shop")).toBeInTheDocument();
    expect(api.called("GET", "/api/revisions?limit=25&before=3")).toHaveLength(1);
    expect(screen.queryByRole("button", { name: "Load older revisions" })).not.toBeInTheDocument();
  });

  it("shows the configuration as of a revision", async () => {
    const view = {
      version: 3,
      createdAtMs: 1,
      summary: "upstream created: backend in shop",
      config: {
        subsystems: [
          {
            id: 1,
            name: "shop",
            enabled: true,
            description: "",
            https: httpsSettings(),
            hosts: ["shop.example.com"],
            upstreams: [upstream()],
            routes: [route({ prefix: "/api" })],
          },
        ],
      },
    };
    mockApi({
      [LIST]: { body: revisionPage() },
      "GET /api/revisions/3": { body: envelope(view) },
    });
    renderRevisions();
    const rows = (await screen.findAllByRole("row")).slice(1);
    await userEvent.click(within(rows[1]).getByRole("button", { name: "View" }));

    const dialog = within(await screen.findByRole("dialog", { name: "Revision 3" }));
    expect(await dialog.findByText("shop")).toBeInTheDocument();
    expect(dialog.getByText("shop.example.com")).toBeInTheDocument();
    expect(dialog.getByText("backend → 127.0.0.1:3000")).toBeInTheDocument();
    expect(dialog.getByText(/\/api →/)).toBeInTheDocument();
  });

  it("shows an error when a revision cannot be loaded", async () => {
    mockApi({
      [LIST]: { body: revisionPage() },
      "GET /api/revisions/3": apiError(404, "NOT_FOUND", "revision 3 not found"),
    });
    renderRevisions();
    const rows = (await screen.findAllByRole("row")).slice(1);
    await userEvent.click(within(rows[1]).getByRole("button", { name: "View" }));
    expect(await screen.findByText(/could not load this revision/i)).toHaveTextContent("revision 3 not found");
  });

  describe("rolling back", () => {
    async function startRollback() {
      const rows = (await screen.findAllByRole("row")).slice(1);
      await userEvent.click(within(rows[1]).getByRole("button", { name: /roll back/i }));
      return within(await screen.findByRole("dialog", { name: "Roll back to revision 3?" }));
    }

    it("asks first, naming the revision and that it can be undone", async () => {
      const api = mockApi({ [LIST]: { body: revisionPage() } });
      renderRevisions();
      const dialog = await startRollback();

      expect(dialog.getByText(/upstream created: backend in shop/)).toBeInTheDocument();
      expect(dialog.getByText(/undo it by rolling back again/i)).toBeInTheDocument();
      expect(api.called("POST", "/api/revisions")).toHaveLength(0);

      await userEvent.click(dialog.getByRole("button", { name: "Cancel" }));
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
      expect(api.called("POST", "/api/revisions")).toHaveLength(0);
    });

    it("rolls back on confirmation and reports the new revision", async () => {
      const api = mockApi({
        [LIST]: { body: revisionPage() },
        "POST /api/revisions/3/rollback": { body: envelope({ restoredFrom: 3 }, 5) },
      });
      renderRevisions();
      const dialog = await startRollback();

      await userEvent.click(dialog.getByRole("button", { name: "Roll back" }));

      expect(await screen.findByText("Rolled back to revision 3 (now revision 5)")).toBeInTheDocument();
      await waitFor(() => expect(api.called("POST", "/api/revisions/3/rollback")).toHaveLength(1));
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });

    it("shows the server's refusal as an error notification", async () => {
      mockApi({
        [LIST]: { body: revisionPage() },
        "POST /api/revisions/3/rollback": apiError(404, "NOT_FOUND", "revision 3 not found"),
      });
      renderRevisions();
      const dialog = await startRollback();
      await userEvent.click(dialog.getByRole("button", { name: "Roll back" }));
      expect(await screen.findByText("revision 3 not found")).toBeInTheDocument();
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });
  });
});
