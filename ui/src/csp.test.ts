// The admin plane serves a Content-Security-Policy with `script-src 'self'`
// (it is what makes keeping the token in sessionStorage acceptable), so the
// page must carry no inline script and no inline event handler.
import { describe, expect, it } from "vitest";
import html from "../index.html?raw";
import theme from "../public/theme-init.js?raw";

describe("index.html under the admin plane's Content-Security-Policy", () => {
  it("has no inline script: every script names a file", () => {
    const scripts = [...html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/g)];
    expect(scripts.length).toBeGreaterThan(0);
    for (const [, attributes, body] of scripts) {
      expect(attributes, "a script needs src").toMatch(/\bsrc=/);
      expect(body.trim(), "an external script has no body").toBe("");
    }
  });

  it("has no inline event handler and no javascript: URL", () => {
    expect(html).not.toMatch(/\son[a-z]+\s*=/i);
    expect(html).not.toMatch(/javascript:/i);
  });

  it("loads the theme bootstrap from the page's own origin", () => {
    expect(html).toContain('src="/theme-init.js"');
    expect(theme).toContain("dataset.theme");
  });
});
