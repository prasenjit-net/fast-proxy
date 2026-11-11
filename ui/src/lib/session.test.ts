import { afterEach, describe, expect, it, vi } from "vitest";
import { clearToken, getToken, reportUnauthorized, setToken, setUnauthorizedHandler } from "./session";

afterEach(() => {
  clearToken();
  setUnauthorizedHandler(null);
});

describe("the session token", () => {
  it("is kept for this tab only, in sessionStorage", () => {
    setToken("fp_1.secret");
    expect(getToken()).toBe("fp_1.secret");
    expect(window.sessionStorage.getItem("fast-proxy-token")).toBe("fp_1.secret");
    expect(window.localStorage.getItem("fast-proxy-token")).toBeNull();
  });

  it("is forgotten on clearing", () => {
    setToken("fp_1.secret");
    clearToken();
    expect(getToken()).toBeNull();
    expect(window.sessionStorage.getItem("fast-proxy-token")).toBeNull();
  });

  it("still works in memory when storage is blocked", () => {
    const blocked = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    setToken("fp_2.secret");
    expect(getToken()).toBe("fp_2.secret");
    blocked.mockRestore();
  });

  it("clears the token and tells the app when the server stops accepting it", () => {
    const handler = vi.fn();
    setUnauthorizedHandler(handler);
    setToken("fp_1.secret");
    reportUnauthorized();
    expect(getToken()).toBeNull();
    expect(handler).toHaveBeenCalledTimes(1);
  });

  it("does nothing more than clear when no handler is registered", () => {
    setToken("fp_1.secret");
    expect(() => reportUnauthorized()).not.toThrow();
    expect(getToken()).toBeNull();
  });
});
