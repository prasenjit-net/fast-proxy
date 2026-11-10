import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { endpoint, route, upstream } from "../test/fixtures";
import ConfigViewer from "./ConfigViewer";

const subsystems = (upstreams: ReturnType<typeof upstream>[]) => [
  {
    id: 1,
    name: "shop",
    enabled: true,
    description: "",
    hosts: ["shop.example.com"],
    upstreams,
    routes: [route()],
  },
];

describe("ConfigViewer", () => {
  it("shows every endpoint of an upstream", () => {
    render(
      <ConfigViewer
        subsystems={subsystems([
          upstream({ endpoints: [endpoint({ addr: "10.0.0.1:80" }), endpoint({ id: 12, addr: "10.0.0.2:80" })] }),
        ])}
      />,
    );
    expect(screen.getByText("backend → 10.0.0.1:80 10.0.0.2:80")).toBeInTheDocument();
  });

  it("says none when a subsystem has no upstream", () => {
    render(<ConfigViewer subsystems={subsystems([])} />);
    expect(screen.getByText("none")).toBeInTheDocument();
  });
});
