import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { useToast } from "../../context/ToastContext";
import { api } from "../../lib/api";
import { useRefreshConfigData } from "../../lib/refreshConfigData";
import type { CertificateUpload, HttpsInput, HttpsSettings, SubsystemDetail } from "../../lib/types";
import Badge from "../Badge";
import CertificateUploadForm from "../CertificateUploadForm";
import FormField from "../FormField";
import Modal from "../Modal";
import Toggle from "../Toggle";
import AttachedCertificate from "./AttachedCertificate";
import HttpsPolicyForm from "./HttpsPolicyForm";

type Dialog = "upload" | "choose" | null;

function inputOf(https: HttpsSettings): HttpsInput {
  return {
    enabled: https.enabled,
    certificateId: https.certificateId,
    redirect: https.redirect,
    hsts: https.hsts,
  };
}

/** Serve this subsystem over HTTPS: its certificate, the redirect and HSTS. */
export default function HttpsSection({ subsystem }: { subsystem: SubsystemDetail }) {
  const refresh = useRefreshConfigData();
  const { push, notifyError } = useToast();
  const [dialog, setDialog] = useState<Dialog>(null);
  const [chosen, setChosen] = useState("");
  const https = subsystem.https;
  const tls = useQuery({ queryKey: ["tls"], queryFn: () => api.tls() });
  const certificates = useQuery({
    queryKey: ["certificates"],
    queryFn: () => api.certificates(),
    enabled: dialog === "choose",
  });
  const tlsOff = tls.data !== undefined && !tls.data.data.enabled;
  // Seconds left as of now, from the absolute expiry the server reported.
  const secondsLeft = https.certificate ? Math.floor((https.certificate.notAfterMs - Date.now()) / 1000) : 0;

  async function saveHttps(patch: Partial<HttpsInput>, message: string) {
    await api.updateSubsystem(subsystem.id, {
      name: subsystem.name,
      enabled: subsystem.enabled,
      description: subsystem.description,
      https: { ...inputOf(https), ...patch },
    });
    await refresh();
    push("success", message);
  }

  async function toggle(enabled: boolean) {
    try {
      await saveHttps({ enabled }, enabled ? "HTTPS enabled" : "HTTPS disabled");
    } catch (error) {
      notifyError(error);
    }
  }

  async function upload(input: CertificateUpload) {
    const response = await api.uploadSubsystemCertificate(subsystem.id, input);
    await refresh();
    const warning = response.data.warnings?.[0];
    push("success", warning ? `Certificate ${input.name} attached (${warning})` : `Certificate ${input.name} attached`);
    setDialog(null);
  }

  async function choose() {
    const id = Number(chosen);
    try {
      await saveHttps({ certificateId: id }, "Certificate attached");
      setDialog(null);
    } catch (error) {
      notifyError(error);
    }
  }

  async function useDevelopment() {
    try {
      await api.detachSubsystemCertificate(subsystem.id);
      await refresh();
      push("success", "A development certificate will be used");
    } catch (error) {
      notifyError(error);
    }
  }

  return (
    <section className="card">
      <div className="card-head">
        <h2>HTTPS</h2>
        <span className="card-hint">serve this subsystem over TLS</span>
        <span className="ml-auto">
          {https.effective ? (
            <Badge tone="ok" dot>
              serving HTTPS
            </Badge>
          ) : https.enabled ? (
            <Badge tone="warn" dot>
              saved, not applied
            </Badge>
          ) : (
            <Badge>off</Badge>
          )}
        </span>
      </div>
      {tlsOff ? (
        <p role="note" className="mb-3 rounded-md bg-warn-soft p-3 text-[0.86rem] text-warn">
          TLS is off for this proxy, so these settings are saved but nothing is served over HTTPS yet.
          Start fast-proxy with <span className="font-mono">--tls</span> (or set{" "}
          <span className="font-mono">enabled = true</span> under{" "}
          <span className="font-mono">[data_plane.tls]</span>).
        </p>
      ) : null}
      <div className="flex flex-col gap-4">
        <Toggle checked={https.enabled} onChange={(next) => void toggle(next)} label="Serve over HTTPS" />
        {https.enabled && https.certificate ? (
          <AttachedCertificate
            certificate={https.certificate}
            secondsLeft={secondsLeft}
            onUpload={() => setDialog("upload")}
            onChoose={() => setDialog("choose")}
            onUseDevelopment={() => void useDevelopment()}
          />
        ) : null}
        {https.enabled && !https.certificate ? (
          <div className="flex flex-wrap items-center gap-2 text-[0.86rem] text-ink-muted">
            <span>
              {tlsOff
                ? "No certificate yet; a development certificate is generated when TLS is on."
                : "No certificate is attached."}
            </span>
            <button type="button" className="btn btn-secondary btn-sm" onClick={() => setDialog("upload")}>
              Upload a certificate
            </button>
            <button type="button" className="btn btn-secondary btn-sm" onClick={() => setDialog("choose")}>
              Choose an existing one
            </button>
          </div>
        ) : null}
        {https.enabled ? (
          <HttpsPolicyForm
            // Remount when the saved settings change so the form shows them.
            key={`${https.redirect}|${https.hsts.enabled}|${https.hsts.maxAgeSecs}|${https.hsts.includeSubdomains}`}
            https={https}
            onSave={async (patch) => {
              try {
                await saveHttps(patch, "HTTPS settings saved");
              } catch (error) {
                notifyError(error);
              }
            }}
          />
        ) : null}
      </div>

      {dialog === "upload" ? (
        <Modal title="Upload a certificate" wide onClose={() => setDialog(null)}>
          <CertificateUploadForm
            submitLabel="Upload and attach"
            onSubmit={upload}
            onCancel={() => setDialog(null)}
          />
        </Modal>
      ) : null}
      {dialog === "choose" ? (
        <Modal title="Choose a certificate" onClose={() => setDialog(null)}>
          <div className="flex flex-col gap-4">
            <FormField label="Certificate" htmlFor="choose-certificate">
              <select
                id="choose-certificate"
                className="input"
                value={chosen}
                onChange={(event) => setChosen(event.target.value)}
              >
                <option value="">Select…</option>
                {(certificates.data?.data ?? []).map((certificate) => (
                  <option key={certificate.id} value={certificate.id}>
                    {certificate.name} ({certificate.names.join(", ")})
                  </option>
                ))}
              </select>
            </FormField>
            <div className="flex justify-end gap-2">
              <button type="button" className="btn btn-secondary" onClick={() => setDialog(null)}>
                Cancel
              </button>
              <button type="button" className="btn btn-primary" disabled={chosen === ""} onClick={() => void choose()}>
                Attach
              </button>
            </div>
          </div>
        </Modal>
      ) : null}
    </section>
  );
}
