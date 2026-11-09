import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import Badge from "./Badge";

describe("Badge", () => {
  it("renders its children", () => {
    render(<Badge>Operational</Badge>);
    expect(screen.getByText("Operational")).toBeInTheDocument();
  });

  it("renders a status dot only when dot is true", () => {
    const { container, rerender } = render(<Badge tone="ok">Up</Badge>);
    expect(container.querySelector("i")).not.toBeInTheDocument();

    rerender(
      <Badge tone="ok" dot>
        Up
      </Badge>,
    );
    expect(container.querySelector("i")).toBeInTheDocument();
  });
});
