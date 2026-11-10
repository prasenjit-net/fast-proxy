import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { endpoint, httpsSettings, route, upstream } from "../test/fixtures";
import ConfigViewer from "./ConfigViewer";

const subsystems = (upstreams: ReturnType<typeof upstream>[]) => [
  {
    id: 1,
    name: "shop",
    enabled: true,
    description: "",
    https: httpsSettings(),
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

  it("shows the HTTPS settings a revision held", () => {
    const view = (https: ReturnType<typeof httpsSettings>) => [{ ...subsystems([upstream()])[0], https }];
    const { rerender } = render(<ConfigViewer subsystems={view(httpsSettings())} />);
    expect(screen.getByText("off")).toBeInTheDocument();
    rerender(
      <ConfigViewer
        subsystems={view(
          httpsSettings({
            enabled: true,
            certificateId: 7,
            redirect: "308",
            hsts: { enabled: true, maxAgeSecs: 600, includeSubdomains: true },
          }),
        )}
      />,
    );
    expect(
      screen.getByText("on, redirect 308, HSTS 600s + subdomains, certificate #7"),
    ).toBeInTheDocument();
    rerender(<ConfigViewer subsystems={view(httpsSettings({ enabled: true }))} />);
    expect(screen.getByText("on, no certificate")).toBeInTheDocument();
  });

  it("summarizes each group's policy, including upstream TLS (the policies the viewer used to leave out)", () => {
    render(
      <ConfigViewer
        subsystems={subsystems([
          upstream({
            lb: { algorithm: "least_connections", hashKey: "client_ip" },
            failurePolicy: "fail_closed",
            health: { ...upstream().health, check: "http" },
            tls: { enabled: true, verify: false, serverName: "", ca: "corp" },
          }),
        ])}
      />,
    );
    expect(
      screen.getByText(
        "Least connections · fail closed · http health check · eject after 3 failures · TLS (not verified, CA corp)",
      ),
    ).toBeInTheDocument();
  });

  it("keeps a plain group's policy line short", () => {
    render(<ConfigViewer subsystems={subsystems([upstream()])} />);
    expect(screen.getByText("Round robin · fail open · eject after 3 failures")).toBeInTheDocument();
  });
});
