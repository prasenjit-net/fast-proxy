import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import Badge from "../components/Badge";
import CaBundlesSection from "../components/certificates/CaBundlesSection";
import { SourceBadge, StateBadge } from "../components/certificates/CertificateBadges";
import { RenameForm, SelfSignedForm } from "../components/certificates/SmallForms";
import CertificateUploadForm from "../components/CertificateUploadForm";
import ConfirmDialog from "../components/ConfirmDialog";
import EmptyState from "../components/EmptyState";
import Modal from "../components/Modal";
import { useToast } from "../context/ToastContext";
import { IconEdit, IconPlus, IconTrash } from "../icons";
import { api } from "../lib/api";
import { expiryLabel } from "../lib/certificates";
import { useRefreshConfigData } from "../lib/refreshConfigData";
import type { Certificate, CertificateUpload } from "../lib/types";

type Dialog =
  | { kind: "upload" }
  | { kind: "selfsigned" }
  | { kind: "replace"; certificate: Certificate }
  | { kind: "rename"; certificate: Certificate }
  | { kind: "delete"; certificate: Certificate }
  | null;

export default function CertificatesPage() {
  const refresh = useRefreshConfigData();
  const { push, notifyError } = useToast();
  const [dialog, setDialog] = useState<Dialog>(null);
  const [busy, setBusy] = useState(false);
  const certificates = useQuery({ queryKey: ["certificates"], queryFn: () => api.certificates() });
  const tls = useQuery({ queryKey: ["tls"], queryFn: () => api.tls() });
  const subsystems = useQuery({ queryKey: ["subsystems"], queryFn: () => api.subsystems() });
  const list = certificates.data?.data ?? [];
  const tlsState = tls.data?.data;
  const nameOf = (id: number) => subsystems.data?.data.find((entry) => entry.id === id)?.name ?? `#${id}`;
  const onlyDevelopment = tlsState?.enabled === true && list.length > 0 && list.every((c) => c.source === "self_signed");

  async function done(message: string, warnings?: string[]) {
    await refresh();
    push("success", warnings?.[0] ? `${message} (${warnings[0]})` : message);
    setDialog(null);
  }

  async function upload(input: CertificateUpload) {
    const response = await api.uploadCertificate(input);
    await done(`Uploaded certificate ${input.name}`, response.data.warnings);
  }

  async function replace(certificate: Certificate, input: CertificateUpload) {
    const response = await api.changeCertificate(certificate.id, {
      certPem: input.certPem.trim() === "" ? undefined : input.certPem,
      keyPem: input.keyPem.trim() === "" ? undefined : input.keyPem,
    });
    await done(`Replaced certificate ${certificate.name}`, response.data.warnings);
  }

  async function remove(certificate: Certificate) {
    setBusy(true);
    try {
      await api.deleteCertificate(certificate.id);
      await done(`Deleted certificate ${certificate.name}`);
    } catch (error) {
      notifyError(error);
      setDialog(null);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <section className="card">
        <div className="card-head">
          <h2>TLS</h2>
          <span className="card-hint">process-wide switch and key vault</span>
        </div>
        {tlsState ? (
          <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1.5 text-[0.88rem]">
            <dt className="text-ink-muted">Status</dt>
            <dd>
              <Badge tone={tlsState.enabled ? "ok" : "neutral"} dot>
                {tlsState.enabled ? "on" : "off"}
              </Badge>
            </dd>
            {tlsState.enabled ? (
              <>
                <dt className="text-ink-muted">HTTPS listener</dt>
                <dd className="font-mono">{tlsState.listen}</dd>
                <dt className="text-ink-muted">Minimum version</dt>
                <dd className="font-mono">
                  TLS {tlsState.minVersion}, HTTP/2 {tlsState.http2 ? "on" : "off"}
                </dd>
              </>
            ) : null}
            <dt className="text-ink-muted">Key vault</dt>
            <dd>
              <Badge tone={tlsState.vault === "key mismatch" ? "err" : "neutral"} dot>
                {tlsState.vault}
              </Badge>
            </dd>
          </dl>
        ) : (
          <p className="text-[0.88rem] text-ink-faint">{tls.isError ? "TLS status is unavailable." : "Loading…"}</p>
        )}
        {tlsState && !tlsState.enabled ? (
          <p role="note" className="mt-3 text-[0.86rem] text-ink-muted">
            TLS is off, so no HTTPS listener runs. Certificates can still be uploaded ahead of time. Start
            fast-proxy with <span className="font-mono">--tls</span> to serve them.
          </p>
        ) : null}
        {tlsState?.vault === "key mismatch" ? (
          <p role="alert" className="mt-3 rounded-md bg-err-soft p-3 text-[0.86rem] text-err">
            The master key does not match this database, so stored private keys cannot be read and no
            certificate can be served from it. Restore the key this database was sealed with
            (FAST_PROXY_MASTER_KEY or the master key file), or upload the certificates again.
          </p>
        ) : null}
      </section>

      {onlyDevelopment ? (
        <p role="note" className="rounded-md bg-warn-soft p-3 text-[0.86rem] text-warn">
          Only development (self-signed) certificates exist. Browsers will warn visitors: upload a real
          certificate before using this in production.
        </p>
      ) : null}

      <section className="card">
        <div className="card-head">
          <h2>Certificates</h2>
          <span className="card-hint">private keys are write-only and never shown</span>
          <div className="ml-auto flex gap-2">
            <button className="btn btn-secondary btn-sm" onClick={() => setDialog({ kind: "selfsigned" })}>
              Generate self-signed
            </button>
            <button className="btn btn-primary btn-sm" onClick={() => setDialog({ kind: "upload" })}>
              <IconPlus size={14} /> Upload a certificate
            </button>
          </div>
        </div>
        {list.length === 0 ? (
          <EmptyState title="No certificates yet">
            Upload a certificate and key, or turn on HTTPS for a subsystem to get a development
            certificate generated for it.
          </EmptyState>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-left text-[0.88rem]">
              <thead className="font-mono text-[0.7rem] tracking-wide text-ink-faint uppercase">
                <tr>
                  <th className="py-2 pr-4 font-medium">Name</th>
                  <th className="py-2 pr-4 font-medium">Covers</th>
                  <th className="py-2 pr-4 font-medium">Expires</th>
                  <th className="py-2 pr-4 font-medium">State</th>
                  <th className="py-2 pr-4 font-medium">Used by</th>
                  <th className="py-2 font-medium" />
                </tr>
              </thead>
              <tbody>
                {list.map((certificate) => (
                  <tr key={certificate.id} className="border-t border-line align-top">
                    <td className="py-2 pr-4">
                      <div className="font-medium">{certificate.name}</div>
                      <SourceBadge source={certificate.source} />
                    </td>
                    <td className="py-2 pr-4 font-mono text-[0.8rem]">{certificate.names.join(", ")}</td>
                    <td className="py-2 pr-4">{expiryLabel(certificate.secondsLeft)}</td>
                    <td className="py-2 pr-4">
                      <StateBadge state={certificate.state} />
                    </td>
                    <td className="py-2 pr-4">
                      {certificate.usedBy.length === 0 ? (
                        <span className="text-ink-faint">unused</span>
                      ) : (
                        certificate.usedBy.map(nameOf).join(", ")
                      )}
                    </td>
                    <td className="py-2 text-right whitespace-nowrap">
                      <button
                        className="icon-btn"
                        aria-label={`Rename ${certificate.name}`}
                        onClick={() => setDialog({ kind: "rename", certificate })}
                      >
                        <IconEdit size={16} />
                      </button>
                      <button
                        className="btn btn-secondary btn-sm mx-1"
                        onClick={() => setDialog({ kind: "replace", certificate })}
                      >
                        Replace
                      </button>
                      <button
                        className="icon-btn"
                        aria-label={`Delete ${certificate.name}`}
                        onClick={() => setDialog({ kind: "delete", certificate })}
                      >
                        <IconTrash size={16} />
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>

      <CaBundlesSection />

      {dialog?.kind === "upload" ? (
        <Modal title="Upload a certificate" wide onClose={() => setDialog(null)}>
          <CertificateUploadForm submitLabel="Upload" onSubmit={upload} onCancel={() => setDialog(null)} />
        </Modal>
      ) : null}
      {dialog?.kind === "selfsigned" ? (
        <Modal title="Generate a self-signed certificate" onClose={() => setDialog(null)}>
          <SelfSignedForm
            onSubmit={async (name, names) => {
              await api.generateSelfSigned(name, names);
              await done(`Generated certificate ${name}`);
            }}
            onCancel={() => setDialog(null)}
          />
        </Modal>
      ) : null}
      {dialog?.kind === "replace" ? (
        <Modal title={`Replace ${dialog.certificate.name}`} wide onClose={() => setDialog(null)}>
          <CertificateUploadForm
            submitLabel="Replace"
            fixedName={dialog.certificate.name}
            optionalMaterial
            onSubmit={(input) => replace(dialog.certificate, input)}
            onCancel={() => setDialog(null)}
          />
        </Modal>
      ) : null}
      {dialog?.kind === "rename" ? (
        <Modal title="Rename certificate" onClose={() => setDialog(null)}>
          <RenameForm
            current={dialog.certificate.name}
            onSubmit={async (name) => {
              await api.changeCertificate(dialog.certificate.id, { name });
              await done(`Renamed to ${name}`);
            }}
            onCancel={() => setDialog(null)}
          />
        </Modal>
      ) : null}
      {dialog?.kind === "delete" ? (
        <ConfirmDialog
          title="Delete certificate?"
          confirmLabel="Delete certificate"
          busy={busy}
          onCancel={() => setDialog(null)}
          onConfirm={() => void remove(dialog.certificate)}
        >
          <p>
            Deletes <b>{dialog.certificate.name}</b> and its private key. A subsystem that still uses it
            blocks the delete.
          </p>
        </ConfirmDialog>
      ) : null}
    </div>
  );
}
