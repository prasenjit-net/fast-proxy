import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ThemeProvider, useTheme } from "./ThemeContext";

let defaultTheme = "auto";
vi.mock("./ConfigContext", () => ({
  useConfig: () => ({ ui: { defaultTheme }, version: "0.0.0" }),
}));

function Probe() {
  const { mode, resolved, setMode } = useTheme();
  return (
    <div>
      <span data-testid="mode">{mode}</span>
      <span data-testid="resolved">{resolved}</span>
      <button onClick={() => setMode("light")}>set-light</button>
      <button onClick={() => setMode("dark")}>set-dark</button>
      <button onClick={() => setMode("auto")}>set-auto</button>
    </div>
  );
}

function renderTheme() {
  return render(
    <ThemeProvider>
      <Probe />
    </ThemeProvider>,
  );
}

describe("ThemeContext", () => {
  beforeEach(() => {
    defaultTheme = "auto";
  });

  it("falls back to the server's default_theme when nothing is saved", () => {
    defaultTheme = "dark";
    renderTheme();
    expect(screen.getByTestId("mode")).toHaveTextContent("dark");
    expect(screen.getByTestId("resolved")).toHaveTextContent("dark");
  });

  it("prefers a previously saved choice over the server default", () => {
    window.localStorage.setItem("rt-theme", "light");
    defaultTheme = "dark";
    renderTheme();
    expect(screen.getByTestId("mode")).toHaveTextContent("light");
  });

  it("updates the mode and persists it when setMode is called", async () => {
    const user = userEvent.setup();
    renderTheme();

    await user.click(screen.getByText("set-dark"));
    expect(screen.getByTestId("mode")).toHaveTextContent("dark");
    expect(screen.getByTestId("resolved")).toHaveTextContent("dark");
    expect(window.localStorage.getItem("rt-theme")).toBe("dark");

    await user.click(screen.getByText("set-light"));
    expect(screen.getByTestId("resolved")).toHaveTextContent("light");
    expect(window.localStorage.getItem("rt-theme")).toBe("light");
  });

  it("stamps the resolved theme onto <html data-theme>", async () => {
    const user = userEvent.setup();
    renderTheme();

    await user.click(screen.getByText("set-dark"));
    expect(document.documentElement.dataset.theme).toBe("dark");

    await user.click(screen.getByText("set-light"));
    expect(document.documentElement.dataset.theme).toBe("light");
  });

  it("auto mode resolves against the OS preference", async () => {
    window.matchMedia = vi.fn().mockImplementation((query: string) => ({
      matches: query.includes("dark"),
      media: query,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    }));
    const user = userEvent.setup();
    renderTheme();

    await user.click(screen.getByText("set-auto"));
    expect(screen.getByTestId("resolved")).toHaveTextContent("dark");
  });
});
