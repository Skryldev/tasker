import { useEffect } from "react";
import { useNavigate } from "react-router-dom";
import { KeyRound, UserRound, Users } from "lucide-react";
import { lazy, Suspense } from "react";
import { useAuth } from "@/lib/auth";
import { Skeleton } from "@/components/ui/badge";

/**
 * Phase 13 Bug 5: a coherent Settings hub. Instance-scoped (team
 * independent): Account / Security live here (moved from /account);
 * Users (Owner-only) relocates from the left sidebar.
 *
 * Sections render in-page (anchors), so ⌘, lands on the hub and every
 * section has a stable URL (#account, #security, #users).
 */

const AccountPage = lazy(() =>
  import("@/pages/AccountPage").then((m) => ({ default: m.AccountPage })),
);
const AdminUsersPage = lazy(() =>
  import("@/pages/AdminUsersPage").then((m) => ({ default: m.default })),
);

interface SettingsSection {
  id: string;
  label: string;
  icon: typeof UserRound;
  ownerOnly?: boolean;
}

const SECTIONS: SettingsSection[] = [
  { id: "account", label: "Account", icon: UserRound },
  { id: "security", label: "Security", icon: KeyRound },
  { id: "users", label: "Users", icon: Users, ownerOnly: true },
];

export default function SettingsPage() {
  const navigate = useNavigate();
  const { user } = useAuth();
  const isOwner = user?.instance_role === "owner";

  // Default hash → #account on mount (no scroll hijack afterwards).
  useEffect(() => {
    if (!window.location.hash) {
      navigate("/settings#account", { replace: true });
    }
  }, [navigate]);

  const visible = SECTIONS.filter((s) => !s.ownerOnly || isOwner);

  return (
    <div className="mx-auto flex w-full max-w-5xl gap-6 p-4" data-testid="settings-page">
      {/* Section rail */}
      <nav aria-label="Settings sections" className="hidden w-48 shrink-0 md:block">
        <ul className="sticky top-4 flex flex-col gap-0.5">
          {visible.map((s) => (
            <li key={s.id}>
              <a
                href={`#/settings#${s.id}`}
                className="flex h-9 items-center gap-2.5 rounded-md px-2 text-sm text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
              >
                <s.icon className="h-4 w-4" />
                {s.label}
              </a>
            </li>
          ))}
        </ul>
      </nav>

      <div className="min-w-0 flex-1">
        <header className="mb-6">
          <h1 className="text-lg font-semibold">Settings</h1>
          <p className="text-2xs text-muted-foreground">
            Instance-wide — independent of the selected team.
          </p>
        </header>

        <section id="account" aria-label="Account" className="scroll-mt-16">
          <Suspense fallback={<Skeleton className="h-64 w-full" />}>
            <AccountPage />
          </Suspense>
        </section>

        {isOwner ? (
          <section
            id="users"
            aria-label="Users"
            className="mt-10 border-t pt-8 scroll-mt-16"
          >
            <div className="mb-4 flex items-center gap-2">
              <Users className="h-4 w-4 text-muted-foreground" />
              <h2 className="text-sm font-semibold">Users</h2>
              <span className="rounded-full border bg-muted/50 px-2 py-0.5 text-2xs text-muted-foreground">
                Owner only
              </span>
            </div>
            <Suspense fallback={<Skeleton className="h-48 w-full" />}>
              <AdminUsersPage />
            </Suspense>
          </section>
        ) : null}
      </div>
    </div>
  );
}
