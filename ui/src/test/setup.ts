import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, beforeEach, vi } from "vitest";

afterEach(() => {
  cleanup();
  // Bare `localStorage` can resolve to Node's own experimental global
  // (stable since Node 22) instead of jsdom's — go through `window`
  // explicitly so this always clears the storage the app actually uses.
  window.localStorage.clear();
  vi.restoreAllMocks();
});

// jsdom doesn't implement matchMedia. ThemeContext (prefers-color-scheme)
// and Layout's mobile-breakpoint check both call it, so every test needs a
// stand-in. Reset to the same default ("no match" — light / desktop) before
// each test; a test that needs different behavior overrides it afterward.
beforeEach(() => {
  window.matchMedia = vi.fn().mockImplementation((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    addListener: vi.fn(),
    removeListener: vi.fn(),
    dispatchEvent: vi.fn(),
  }));
  // jsdom doesn't implement scrollTo either — TanStack Router's scroll
  // restoration calls it on every navigation.
  window.scrollTo = vi.fn();
});
