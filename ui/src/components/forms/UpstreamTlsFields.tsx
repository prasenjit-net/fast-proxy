import { useQuery } from "@tanstack/react-query";
import { api } from "../../lib/api";
import type { PolicyValues } from "../../lib/policyValidation";
import Badge from "../Badge";
import FormField, { errorProps } from "../FormField";
import Toggle from "../Toggle";

interface Props {
  values: PolicyValues;
  onChange: (patch: Partial<PolicyValues>) => void;
  error: (field: string) => string | undefined;
  idPrefix: string;
}

/** TLS to the endpoints of a group: on or off, verification, the name to check, the trust anchors. */
export default function UpstreamTlsFields({ values, onChange, error, idPrefix }: Props) {
  const bundles = useQuery({ queryKey: ["ca-bundles"], queryFn: () => api.caBundles() });
  const nameId = `${idPrefix}-tls-server-name`;
  const caId = `${idPrefix}-tls-ca`;
  const known = bundles.data?.data ?? [];
  // A saved name that no longer exists stays selectable so it is not silently dropped.
  const missing = values.tlsCa !== "" && !known.some((bundle) => bundle.name === values.tlsCa);

  return (
    <fieldset className="flex flex-col gap-3 rounded-md border border-line p-3">
      <legend className="px-1 text-[0.82rem] font-medium">TLS to the endpoints</legend>
      <Toggle
        checked={values.tlsEnabled}
        onChange={(tlsEnabled) => onChange({ tlsEnabled })}
        label="Connect to the endpoints over TLS"
      />
      {values.tlsEnabled ? (
        <>
          <div className="flex flex-wrap items-center gap-3">
            <Toggle
              checked={values.tlsVerify}
              onChange={(tlsVerify) => onChange({ tlsVerify })}
              label="Verify the endpoint's certificate"
            />
            {values.tlsVerify ? null : <Badge tone="warn">not verified: anyone can impersonate the endpoint</Badge>}
          </div>
          <FormField
            label="Server name"
            htmlFor={nameId}
            error={error("tls.serverName")}
            hint="Optional. The name to check the certificate against; needed when endpoints are IP addresses."
          >
            <input
              id={nameId}
              className="input"
              value={values.tlsServerName}
              onChange={(event) => onChange({ tlsServerName: event.target.value })}
              autoComplete="off"
              {...errorProps(nameId, error("tls.serverName"))}
            />
          </FormField>
          <FormField
            label="CA bundle"
            htmlFor={caId}
            error={error("tls.ca")}
            hint="Upload bundles on the Certificates page. Without one, the operating system's trust store is used."
          >
            <select
              id={caId}
              className="input"
              value={values.tlsCa}
              onChange={(event) => onChange({ tlsCa: event.target.value })}
              {...errorProps(caId, error("tls.ca"))}
            >
              <option value="">Operating system trust store</option>
              {missing ? <option value={values.tlsCa}>{values.tlsCa} (not found)</option> : null}
              {known.map((bundle) => (
                <option key={bundle.id} value={bundle.name}>
                  {bundle.name}
                </option>
              ))}
            </select>
          </FormField>
        </>
      ) : null}
    </fieldset>
  );
}
