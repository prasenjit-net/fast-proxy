import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import FilterActions from "./FilterActions";

describe("FilterActions", () => {
  it("offers Apply always and Clear only once a filter is applied", async () => {
    const onClear = vi.fn();
    const { rerender } = render(<FilterActions filtered={false} onClear={onClear} />);
    expect(screen.getByRole("button", { name: "Apply" })).toHaveAttribute("type", "submit");
    expect(screen.queryByRole("button", { name: "Clear" })).not.toBeInTheDocument();

    rerender(<FilterActions filtered onClear={onClear} />);
    await userEvent.setup().click(screen.getByRole("button", { name: "Clear" }));
    expect(onClear).toHaveBeenCalledTimes(1);
  });
});
