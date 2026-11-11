import type { SubsystemDetail } from "../../lib/types";
import MaintenanceControl from "../maintenance/MaintenanceControl";

/** Turn a whole subsystem into a temporary 503 page, or back. */
export default function MaintenanceSection({ subsystem }: { subsystem: SubsystemDetail }) {
  return (
    <section className="card">
      <div className="card-head">
        <h2>Maintenance</h2>
        <span className="card-hint">visitors get a 503 with a message; routes can be closed on their own below</span>
      </div>
      <MaintenanceControl
        target={{ scope: "subsystem", subsystemId: subsystem.id, name: subsystem.name }}
      />
    </section>
  );
}
