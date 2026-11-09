import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { trafficSample } from "../test/traffic";
import LiveChart, { niceMax } from "./LiveChart";

describe("niceMax", () => {
  it("rounds up to 1, 2, 5, 10 times a power of ten", () => {
    expect(niceMax(0)).toBe(1);
    expect(niceMax(0.3)).toBe(1);
    expect(niceMax(1.5)).toBe(2);
    expect(niceMax(3)).toBe(5);
    expect(niceMax(7)).toBe(10);
    expect(niceMax(12.5)).toBe(20);
    expect(niceMax(430)).toBe(500);
    expect(niceMax(5000)).toBe(5000);
  });
});

describe("LiveChart", () => {
  it("waits for at least two samples", () => {
    render(<LiveChart history={[trafficSample()]} />);
    expect(screen.getByText(/waiting for live data/i)).toBeInTheDocument();
  });

  it("draws the series and shows the latest rate", () => {
    const history = [
      trafficSample({ total: { requestsTotal: 10, requestsPerSec: 4, errorRate: 0 } }),
      trafficSample({ total: { requestsTotal: 30, requestsPerSec: 12.5, errorRate: 0 } }),
    ];
    render(<LiveChart history={history} />);
    expect(
      screen.getByRole("img", { name: /requests per second/i }),
    ).toBeInTheDocument();
    expect(screen.getByText("Requests/s")).toBeInTheDocument();
    // The latest sample is the one in the legend (12.5 rounds to 13 above 10/s).
    expect(screen.getByText("13")).toBeInTheDocument();
  });
});
