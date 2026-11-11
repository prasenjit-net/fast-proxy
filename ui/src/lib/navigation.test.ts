import { describe, expect, it } from "vitest";
import { isNavActive, pageTitle } from "./navigation";

describe("pageTitle", () => {
  it("names each top-level page", () => {
    expect(pageTitle("/")).toBe("Dashboard");
    expect(pageTitle("/subsystems")).toBe("Subsystems");
    expect(pageTitle("/routes")).toBe("Routes and upstreams");
    expect(pageTitle("/maintenance")).toBe("Maintenance");
    expect(pageTitle("/revisions")).toBe("Revisions");
    expect(pageTitle("/resolver")).toBe("Resolver");
    expect(pageTitle("/settings")).toBe("Settings");
  });

  it("calls a subsystem's detail page Subsystem", () => {
    expect(pageTitle("/subsystems/12")).toBe("Subsystem");
  });

  it("falls back to Not found", () => {
    expect(pageTitle("/nope")).toBe("Not found");
  });
});

describe("isNavActive", () => {
  it("matches the dashboard only on the root", () => {
    expect(isNavActive("/", "/")).toBe(true);
    expect(isNavActive("/subsystems", "/")).toBe(false);
  });

  it("keeps a section active on its sub-pages", () => {
    expect(isNavActive("/subsystems", "/subsystems")).toBe(true);
    expect(isNavActive("/subsystems/3", "/subsystems")).toBe(true);
    expect(isNavActive("/subsystemsX", "/subsystems")).toBe(false);
    expect(isNavActive("/revisions", "/subsystems")).toBe(false);
  });
});
