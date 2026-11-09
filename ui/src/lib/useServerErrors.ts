// Routes a failed request into a form: errors that name fields are shown
// next to those inputs, other API errors (a 409 for a duplicate, a 404) are
// shown once above the form. Anything that is not an ApiError is a bug and
// is rethrown.
import { useCallback, useState } from "react";
import { ApiError } from "./http";

export interface ServerErrors {
  /** The message for one input, if the server named it. */
  field: (name: string) => string | undefined;
  /** A message not tied to any input. */
  general: string | undefined;
  /** Records a failed request; rethrows anything that is not an ApiError. */
  capture: (error: unknown) => void;
  clear: () => void;
}

export function useServerErrors(): ServerErrors {
  const [error, setError] = useState<ApiError | null>(null);

  const capture = useCallback((thrown: unknown) => {
    if (!(thrown instanceof ApiError)) throw thrown;
    setError(thrown);
  }, []);
  const clear = useCallback(() => setError(null), []);
  const field = useCallback((name: string) => error?.fieldMessage(name), [error]);

  return {
    field,
    general: error && error.fields.length === 0 ? error.message : undefined,
    capture,
    clear,
  };
}
