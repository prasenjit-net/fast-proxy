import { useState, type FormEvent } from "react";
import { HSTS_PRESETS } from "../../lib/tlsValidation";
import type { HttpsSettings, RedirectMode } from "../../lib/types";
import ConfirmDialog from "../ConfirmDialog";
import FormField from "../FormField";
import Toggle from "../Toggle";

interface Props {
  https: HttpsSettings;
  onSave: (patch: Pick<HttpsSettings, "redirect" | "hsts">) => Promise<void>;
}

/** The redirect and HSTS settings, with a confirmation that says what will happen. */
export default function HttpsPolicyForm({ https, onSave }: Props) {
  const [redirect, setRedirect] = useState<RedirectMode>(https.redirect);
  const [hstsEnabled, setHstsEnabled] = useState(https.hsts.enabled);
  const [maxAge, setMaxAge] = useState(https.hsts.maxAgeSecs);
  const [includeSubdomains, setIncludeSubdomains] = useState(https.hsts.includeSubdomains);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);

  const turningOnRedirect = redirect !== "off" && https.redirect === "off";
  const turningOnHsts = hstsEnabled && !https.hsts.enabled;
  const preset = HSTS_PRESETS.find((entry) => entry.secs === maxAge);
  const changed =
    redirect !== https.redirect ||
    hstsEnabled !== https.hsts.enabled ||
    maxAge !== https.hsts.maxAgeSecs ||
    includeSubdomains !== https.hsts.includeSubdomains;

  async function save() {
    setBusy(true);
    try {
      await onSave({
        redirect,
        hsts: { enabled: hstsEnabled, maxAgeSecs: maxAge, includeSubdomains },
      });
    } finally {
      setBusy(false);
      setConfirming(false);
    }
  }

  function handleSubmit(event: FormEvent) {
    event.preventDefault();
    if (turningOnRedirect || turningOnHsts) setConfirming(true);
    else void save();
  }

  return (
    <form onSubmit={handleSubmit} className="flex flex-col gap-4">
      <FormField
        label="Plain HTTP requests"
        htmlFor="https-redirect"
        hint="Sent to the HTTPS address of the same host, path and query."
      >
        <select
          id="https-redirect"
          className="input"
          value={redirect}
          onChange={(event) => setRedirect(event.target.value as RedirectMode)}
        >
          <option value="off">Served as usual (no redirect)</option>
          <option value="308">Redirect with 308 (keeps the method)</option>
          <option value="301">Redirect with 301</option>
        </select>
      </FormField>
      <div className="flex flex-col gap-2">
        <Toggle checked={hstsEnabled} onChange={setHstsEnabled} label="Send Strict-Transport-Security (HSTS)" />
        {hstsEnabled ? (
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <FormField label="Remember HTTPS for" htmlFor="hsts-max-age">
              <select
                id="hsts-max-age"
                className="input"
                value={maxAge}
                onChange={(event) => setMaxAge(Number(event.target.value))}
              >
                {preset ? null : <option value={maxAge}>{maxAge} seconds</option>}
                {HSTS_PRESETS.map((entry) => (
                  <option key={entry.secs} value={entry.secs}>
                    {entry.label}
                  </option>
                ))}
              </select>
            </FormField>
            <div className="flex items-end pb-1">
              <Toggle
                checked={includeSubdomains}
                onChange={setIncludeSubdomains}
                label="Include subdomains"
              />
            </div>
          </div>
        ) : null}
      </div>
      <div className="flex justify-end">
        <button type="submit" className="btn btn-primary" disabled={busy || !changed}>
          Save HTTPS settings
        </button>
      </div>
      {confirming ? (
        <ConfirmDialog
          title="Apply these HTTPS settings?"
          confirmLabel="Apply"
          busy={busy}
          onCancel={() => setConfirming(false)}
          onConfirm={() => void save()}
        >
          {turningOnRedirect ? (
            <p>
              Plain-HTTP requests for this subsystem will be answered with a {redirect} redirect to
              HTTPS. Make sure HTTPS works for every host first.
            </p>
          ) : null}
          {turningOnHsts ? (
            <p className="mt-2">
              Browsers will remember for {preset?.label ?? `${maxAge} seconds`}
              {includeSubdomains ? ", for all subdomains too," : ""} that this host is HTTPS only, and
              will refuse plain HTTP even if you turn this off later. A wrong setting is hard to undo.
            </p>
          ) : null}
        </ConfirmDialog>
      ) : null}
    </form>
  );
}
