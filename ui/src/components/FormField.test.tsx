import { renderHook, act } from "@testing-library/react";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ApiError } from "../lib/api";
import { useServerErrors } from "../lib/useServerErrors";
import FormField, { errorProps } from "./FormField";

describe("FormField", () => {
  it("labels its control", () => {
    render(
      <FormField label="Prefix" htmlFor="prefix">
        <input id="prefix" />
      </FormField>,
    );
    expect(screen.getByLabelText("Prefix")).toBeInTheDocument();
  });

  it("shows the hint until there is an error, then only the error", () => {
    const { rerender } = render(
      <FormField label="Prefix" htmlFor="prefix" hint="Starts with a slash">
        <input id="prefix" />
      </FormField>,
    );
    expect(screen.getByText("Starts with a slash")).toBeInTheDocument();

    rerender(
      <FormField label="Prefix" htmlFor="prefix" hint="Starts with a slash" error="must start with '/'">
        <input id="prefix" {...errorProps("prefix", "must start with '/'")} />
      </FormField>,
    );
    expect(screen.getByRole("alert")).toHaveTextContent("must start with '/'");
    expect(screen.queryByText("Starts with a slash")).not.toBeInTheDocument();
    const input = screen.getByLabelText("Prefix");
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(input).toHaveAttribute("aria-describedby", "prefix-error");
  });
});

describe("useServerErrors", () => {
  it("maps server field errors to their inputs", () => {
    const { result } = renderHook(() => useServerErrors());
    act(() =>
      result.current.capture(
        new ApiError("VALIDATION_FAILED", 400, "validation failed", [
          { field: "prefix", message: "bad prefix" },
        ]),
      ),
    );
    expect(result.current.field("prefix")).toBe("bad prefix");
    expect(result.current.field("name")).toBeUndefined();
    expect(result.current.general).toBeUndefined();
  });

  it("shows errors that name no field as a general message", () => {
    const { result } = renderHook(() => useServerErrors());
    act(() => result.current.capture(new ApiError("CONFLICT", 409, "name already exists")));
    expect(result.current.general).toBe("name already exists");
  });

  it("clears everything", () => {
    const { result } = renderHook(() => useServerErrors());
    act(() => result.current.capture(new ApiError("CONFLICT", 409, "dup")));
    act(() => result.current.clear());
    expect(result.current.general).toBeUndefined();
  });

  it("rethrows anything that is not an ApiError", () => {
    const { result } = renderHook(() => useServerErrors());
    expect(() => result.current.capture(new TypeError("a bug"))).toThrow("a bug");
  });
});
