import ActivityFeed from "../components/ActivityFeed";
import LiveChart from "../components/LiveChart";
import StatCard from "../components/StatCard";
import TasksCard from "../components/TasksCard";
import { useLive } from "../context/LiveContext";
import { IconActivity, IconCpu, IconDatabase, IconUsers } from "../icons";
import { formatNumber, formatUptime } from "../lib/format";

export default function DashboardPage() {
  const { metrics, history, activities } = useLive();
  const cpuSeries = history.map((m) => m.cpu);
  const memSeries = history.map((m) => m.memory);

  return (
    <div className="flex flex-col gap-4">
      <div className="grid grid-cols-1 gap-4 min-[560px]:grid-cols-2 xl:grid-cols-4">
        <StatCard
          icon={<IconCpu size={18} />}
          label="CPU"
          value={metrics ? `${metrics.cpu.toFixed(0)}%` : "—"}
          sub="simulated load"
          series={cpuSeries}
          max={100}
        />
        <StatCard
          icon={<IconDatabase size={18} />}
          label="Memory"
          value={metrics ? `${metrics.memory.toFixed(0)}%` : "—"}
          sub="simulated usage"
          series={memSeries}
          max={100}
          color="var(--chart-2)"
        />
        <StatCard
          icon={<IconActivity size={18} />}
          label="Requests / min"
          value={metrics ? formatNumber(metrics.requestsPerMin) : "—"}
          sub={metrics ? `${formatNumber(metrics.requestsTotal)} total` : "waiting for data"}
        />
        <StatCard
          icon={<IconUsers size={18} />}
          label="Clients online"
          value={metrics ? String(metrics.wsClients) : "—"}
          sub={metrics ? `server up ${formatUptime(metrics.uptimeSecs)}` : "waiting for data"}
        />
      </div>

      <section className="card">
        <div className="card-head">
          <h2>Live metrics</h2>
          <span className="card-hint">pushed over WebSocket every 2 s</span>
        </div>
        <LiveChart history={history} />
      </section>

      <div className="grid grid-cols-1 items-start gap-4 lg:grid-cols-[3fr_2fr]">
        <TasksCard />
        <section className="card">
          <div className="card-head">
            <h2>Activity</h2>
            <span className="card-hint">server events</span>
          </div>
          <ActivityFeed activities={activities} />
        </section>
      </div>
    </div>
  );
}
