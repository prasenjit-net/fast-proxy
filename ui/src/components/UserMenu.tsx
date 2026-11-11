import { useSession } from "../context/SessionContext";
import { IconLogOut } from "../icons";
import Badge from "./Badge";

/** Who is signed in, with their role, and a way to sign out. */
export default function UserMenu() {
  const { who, signOut } = useSession();
  const open = who.auth === "off";
  return (
    <div className="flex items-center gap-2">
      <span className="hidden items-center gap-2 text-[0.84rem] sm:inline-flex">
        <span className="font-medium">{who.name}</span>
        <Badge tone={who.role === "admin" ? "accent" : "info"}>
          {who.role === "viewer" ? "viewer (read-only)" : who.role}
        </Badge>
      </span>
      {open ? null : (
        <button className="icon-btn" onClick={signOut} aria-label="Sign out" title="Sign out">
          <IconLogOut size={18} />
        </button>
      )}
    </div>
  );
}
