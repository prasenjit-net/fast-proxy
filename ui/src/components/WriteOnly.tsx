import type { ReactNode } from "react";
import { useCanWrite } from "../context/SessionContext";

/** Shows its children only to someone who may change configuration; a viewer sees none of them. */
export default function WriteOnly({ children }: { children: ReactNode }) {
  return useCanWrite() ? <>{children}</> : null;
}
