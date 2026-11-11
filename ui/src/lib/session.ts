// The admin token for this browser tab. It lives in sessionStorage (gone when
// the tab closes, never shared with other tabs or kept on disk) and in memory,
// is sent as a bearer header on every request, and is cleared on sign-out or
// when the server stops accepting it. The page's Content-Security-Policy
// forbids inline script and third-party origins, which is what makes keeping a
// token here acceptable (ADR 0015).

const STORAGE_KEY = "fast-proxy-token";

function readStored(): string | null {
  try {
    return window.sessionStorage.getItem(STORAGE_KEY);
  } catch {
    return null; // storage blocked: the token then lives only in memory
  }
}

let token: string | null = readStored();
let onUnauthorized: (() => void) | null = null;

export function getToken(): string | null {
  return token;
}

export function setToken(value: string): void {
  token = value;
  try {
    window.sessionStorage.setItem(STORAGE_KEY, value);
  } catch {
    /* kept in memory only */
  }
}

export function clearToken(): void {
  token = null;
  try {
    window.sessionStorage.removeItem(STORAGE_KEY);
  } catch {
    /* nothing to clear */
  }
}

/** Registers what happens when the server answers 401 to a signed-in request. */
export function setUnauthorizedHandler(handler: (() => void) | null): void {
  onUnauthorized = handler;
}

/** Called by the HTTP layer on a 401 that the sign-in form did not ask for. */
export function reportUnauthorized(): void {
  clearToken();
  onUnauthorized?.();
}
