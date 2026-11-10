import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { useToast } from "../../context/ToastContext";
import { IconPlus, IconTrash } from "../../icons";
import { api } from "../../lib/api";
import { useRefreshConfigData } from "../../lib/refreshConfigData";
import type { CaBundle } from "../../lib/types";
import ConfirmDialog from "../ConfirmDialog";
import EmptyState from "../EmptyState";
import Modal from "../Modal";
import { CaBundleForm } from "./SmallForms";

/** CA bundles that upstream groups name to verify their TLS backends. */
export default function CaBundlesSection() {
  const refresh = useRefreshConfigData();
  const { push, notifyError } = useToast();
  const [uploading, setUploading] = useState(false);
  const [deleting, setDeleting] = useState<CaBundle | null>(null);
  const [busy, setBusy] = useState(false);
  const bundles = useQuery({ queryKey: ["ca-bundles"], queryFn: () => api.caBundles() });
  const list = bundles.data?.data ?? [];

  async function upload(name: string, pem: string) {
    await api.uploadCaBundle(name, pem);
    await refresh();
    push("success", `Uploaded CA bundle ${name}`);
    setUploading(false);
  }

  async function remove(bundle: CaBundle) {
    setBusy(true);
    try {
      await api.deleteCaBundle(bundle.id);
      await refresh();
      push("success", `Deleted CA bundle ${bundle.name}`);
    } catch (error) {
      notifyError(error);
    } finally {
      setBusy(false);
      setDeleting(null);
    }
  }

  return (
    <section className="card">
      <div className="card-head">
        <h2>CA bundles</h2>
        <span className="card-hint">trust anchors for TLS upstreams</span>
        <button className="btn btn-secondary btn-sm ml-auto" onClick={() => setUploading(true)}>
          <IconPlus size={14} /> Upload a bundle
        </button>
      </div>
      {list.length === 0 ? (
        <EmptyState title="No CA bundles">
          An upstream group that talks TLS to an internal service can verify it against a bundle you
          upload here. Without one it uses the operating system's trust store.
        </EmptyState>
      ) : (
        <ul className="m-0 flex list-none flex-col p-0">
          {list.map((bundle) => (
            <li key={bundle.id} className="flex flex-wrap items-center gap-3 border-t border-line py-2 first:border-t-0">
              <span className="font-mono text-[0.88rem] font-medium">{bundle.name}</span>
              <span className="text-[0.82rem] text-ink-muted">
                {bundle.certCount} certificate{bundle.certCount === 1 ? "" : "s"}:{" "}
                {bundle.certificates.map((certificate) => certificate.subject).join("; ")}
              </span>
              <button
                className="icon-btn ml-auto"
                aria-label={`Delete ${bundle.name}`}
                onClick={() => setDeleting(bundle)}
              >
                <IconTrash size={16} />
              </button>
            </li>
          ))}
        </ul>
      )}
      {uploading ? (
        <Modal title="Upload a CA bundle" wide onClose={() => setUploading(false)}>
          <CaBundleForm onSubmit={upload} onCancel={() => setUploading(false)} />
        </Modal>
      ) : null}
      {deleting ? (
        <ConfirmDialog
          title="Delete CA bundle?"
          confirmLabel="Delete bundle"
          busy={busy}
          onCancel={() => setDeleting(null)}
          onConfirm={() => void remove(deleting)}
        >
          <p>
            Deletes <b>{deleting.name}</b>. An upstream group that still names it blocks the delete.
          </p>
        </ConfirmDialog>
      ) : null}
    </section>
  );
}
