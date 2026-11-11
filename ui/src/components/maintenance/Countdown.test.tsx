import { act, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import Countdown from "./Countdown";

afterEach(() => {
  vi.useRealTimers();
});

describe("Countdown", () => {
  it("counts down each second and says ending at zero", async () => {
    vi.useFakeTimers();
    render(<Countdown expiresAtMs={Date.now() + 65_000} />);
    expect(screen.getByText("ends in 1:05")).toBeInTheDocument();
    await act(() => vi.advanceTimersByTimeAsync(10_000));
    expect(screen.getByText("ends in 55s")).toBeInTheDocument();
    await act(() => vi.advanceTimersByTimeAsync(60_000));
    expect(screen.getByText("ending")).toBeInTheDocument();
  });

  it("says until turned off when there is no end", () => {
    render(<Countdown expiresAtMs={null} />);
    expect(screen.getByText("until turned off")).toBeInTheDocument();
  });
});
