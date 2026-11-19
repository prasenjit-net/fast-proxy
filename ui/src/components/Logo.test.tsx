import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import Logo from "./Logo";

describe("Logo", () => {
  it("is decorative and drawn at the requested size", () => {
    const { container } = render(<Logo size={52} />);
    const svg = container.querySelector("svg");
    expect(svg).toHaveAttribute("aria-hidden", "true");
    expect(svg).toHaveAttribute("width", "52");
    expect(svg).toHaveAttribute("height", "52");
  });

  it("uses theme tokens, not fixed colours, so it follows light and dark", () => {
    const { container } = render(<Logo />);
    const colours = Array.from(container.querySelectorAll("[fill], [stroke]")).flatMap((node) =>
      ["fill", "stroke"]
        .map((name) => node.getAttribute(name))
        .filter((value): value is string => value !== null && value !== "none"),
    );
    expect(colours.length).toBeGreaterThan(0);
    for (const colour of colours) expect(colour).toMatch(/^var\(--(accent|on-accent)\)$/);
  });
});
