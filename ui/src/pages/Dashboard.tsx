import ActivityFeed from "../components/ActivityFeed";
import LiveChart from "../components/LiveChart";
import StatCard from "../components/StatCard";
import { Link } from "@tanstack/react-router";
import { useLive } from "../context/LiveContext";
import { IconActivity, IconAlertTriangle, IconServer, IconUsers } from "../icons";
import { formatNumber, formatPercent, formatRate } from "../lib/format";

export default function DashboardPage() {
  const { traffic, history, activities } = useLive();
  const rateSeries = history.map((sample) => sample.total.requestsPerSec);
  const errorSeries = history.map((sample) => sample.total.errorRate);

  return (
    <div className="flex flex-col gap-4">
      <div className="grid grid-cols-1 gap-4 min-[560px]:grid-cols-2 xl:grid-cols-4">
        <StatCard
          icon={<IconActivity size={18} />}
          label="Requests / s"
          value={traffic ? formatRate(traffic.total.requestsPerSec) : "—"}
          sub={
            traffic
              ? `${formatNumber(traffic.total.requestsTotal)} total`
              : "waiting for data"
          }
          series={rateSeries}
        />
        <StatCard
          icon={<IconAlertTriangle size={18} />}
          label="Error rate"
          value={traffic ? formatPercent(traffic.total.errorRate) : "—"}
          sub="share of responses that were 5xx"
          series={errorSeries}
          max={1}
          color="var(--chart-2)"
        />
        <StatCard
          icon={<IconUsers size={18} />}
          label="Connections"
          value={traffic ? formatNumber(traffic.activeConnections) : "—"}
          sub="open client connections"
        />
        <StatCard
          icon={<IconServer size={18} />}
          label="In flight"
          value={traffic ? formatNumber(traffic.activeRequests) : "—"}
          sub="requests being handled"
        />
      </div>

      <section className="card">
        <div className="card-head">
          <h2>Live traffic</h2>
          <span className="card-hint">pushed over WebSocket every 2 s</span>
        </div>
        <LiveChart history={history} />
      </section>

      <section className="card">
        <div className="card-head">
          <h2>By subsystem</h2>
          <span className="card-hint">rates over the last sample window</span>
        </div>
        {traffic && traffic.subsystems.length > 0 ? (
          <div className="overflow-x-auto">
            <table className="w-full text-left text-[0.88rem]">
              <thead className="font-mono text-[0.7rem] tracking-wide text-ink-faint uppercase">
                <tr>
                  <th className="py-2 pr-4 font-medium">Subsystem</th>
                  <th className="py-2 pr-4 text-right font-medium">Requests / s</th>
                  <th className="py-2 pr-4 text-right font-medium">Error rate</th>
                  <th className="py-2 text-right font-medium">Total</th>
                </tr>
              </thead>
              <tbody>
                {traffic.subsystems.map((subsystem) => (
                  <tr key={subsystem.name} className="border-t border-line">
                    <td className="py-2 pr-4 font-medium">
                      {subsystem.name.startsWith("_") ? (
                        <span title="Requests that matched no subsystem" className="text-ink-muted">
                          {subsystem.name}
                        </span>
                      ) : (
                        <Link to="/subsystems" className="text-accent hover:underline">
                          {subsystem.name}
                        </Link>
                      )}
                    </td>
                    <td className="py-2 pr-4 text-right font-mono">{formatRate(subsystem.requestsPerSec)}</td>
                    <td className="py-2 pr-4 text-right font-mono">{formatPercent(subsystem.errorRate)}</td>
                    <td className="py-2 text-right font-mono text-ink-muted">
                      {formatNumber(subsystem.requestsTotal)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : (
          <p className="text-[0.88rem] text-ink-faint">
            {traffic ? "No traffic has reached the proxy yet." : "Waiting for data…"}
          </p>
        )}
      </section>

      <section className="card">
        <div className="card-head">
          <h2>Activity</h2>
          <span className="card-hint">server events</span>
        </div>
        <ActivityFeed activities={activities} />
      </section>
    </div>
  );
}
