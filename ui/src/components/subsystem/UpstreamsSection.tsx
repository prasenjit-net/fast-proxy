import { useState } from "react";
import { useToast } from "../../context/ToastContext";
import { IconPlus } from "../../icons";
import { api } from "../../lib/api";
import { useRefreshConfigData } from "../../lib/refreshConfigData";
import type { EndpointInput, SubsystemDetail, UpstreamSettings } from "../../lib/types";
import UpstreamForm from "../forms/UpstreamForm";
import Modal from "../Modal";
import WriteOnly from "../WriteOnly";
import UpstreamCard from "./UpstreamCard";

/** The backends a subsystem's routes can send traffic to, with live health. */
export default function UpstreamsSection({ subsystem }: { subsystem: SubsystemDetail }) {
  const refresh = useRefreshConfigData();
  const { push } = useToast();
  const [adding, setAdding] = useState(false);

  async function create(settings: UpstreamSettings, firstEndpoint: EndpointInput | null) {
    await api.createUpstream(subsystem.id, { ...settings, endpoints: firstEndpoint ? [firstEndpoint] : [] });
    await refresh();
    push("success", `Added upstream ${settings.name}`);
    setAdding(false);
  }

  return (
    <section className="card">
      <div className="card-head">
        <h2>Upstreams</h2>
        <span className="card-hint">where routes send traffic</span>
        <WriteOnly>
          <button className="btn btn-secondary btn-sm ml-auto" onClick={() => setAdding(true)}>
            <IconPlus size={14} /> Add upstream
          </button>
        </WriteOnly>
      </div>
      {subsystem.upstreams.length === 0 ? (
        <p className="text-[0.88rem] text-ink-muted">
          No upstreams yet. Add the address of the backend server first; routes point at it.
        </p>
      ) : (
        <div className="flex flex-col gap-3">
          {subsystem.upstreams.map((upstream) => (
            <UpstreamCard key={upstream.id} subsystem={subsystem} upstream={upstream} />
          ))}
        </div>
      )}
      {adding ? (
        <Modal title="Add upstream" onClose={() => setAdding(false)} wide>
          <UpstreamForm submitLabel="Add upstream" onSubmit={create} onCancel={() => setAdding(false)} />
        </Modal>
      ) : null}
    </section>
  );
}
