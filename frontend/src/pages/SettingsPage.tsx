import { useCallback, useEffect, useRef, useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { KeyRound, UserRound, Users } from "lucide-react";
import { lazy, Suspense } from "react";
import { useAuth } from "@/lib/auth";
import { Skeleton } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

/**
 * Settings hub (Phase 13 Bug 5; Phase 13.5 Bug 2): hash-driven sections.
 *
 * Scroll mechanism: the page scrolls inside AppShell's `main` container
 * (`#main-content`), not the window, so native `#hash` anchors do nothing.
 * Menu items scroll the target section into view within that container
 * (smooth, instant under prefers-reduced-motion), update the hash, and a
 * scroll spy highlights the active section while scrolling.
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

function scrollToSection(id: string): void {
  const el = document.getElementById(id);
  if (!el) return;
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  el.scrollIntoView({ behavior: reduced ? "auto" : "smooth", block: "start" });
}

export default function SettingsPage() {
  const navigate = useNavigate();
  const location = useLocation();
  const { user } = useAuth();
  const isOwner = user?.instance_role === "owner";
  const [active, setActive] = useState("account");
  const spyLock = useRef(false);
  const spyTimer = useRef<number | null>(null);

  const visible = SECTIONS.filter((s) => !s.ownerOnly || isOwner);

  const go = useCallback(
    (id: string) => {
      // Optimistic: the clicked item is the active one for the duration of
      // the smooth scroll (the spy's 600ms scroll-lock would otherwise
      // leave the previous item highlighted mid-animation and after it).
      setActive(id);
      spyLock.current = true;
      if (spyTimer.current) window.clearTimeout(spyTimer.current);
      spyTimer.current = window.setTimeout(() => {
        spyLock.current = false;
      }, 900);
      scrollToSection(id);
      navigate(`/settings#${id}`, { replace: true });
    },
    [navigate],
  );

  // Direct navigation to /settings#users scrolls on mount (and after the
  // lazy sections have rendered).
  useEffect(() => {
    const hash = location.hash.replace("#", "");
    if (!hash) return undefined;
    const t = window.setTimeout(() => scrollToSection(hash), 250);
    return () => window.clearTimeout(t);
  }, [location.hash]);

  // Scroll spy: the section intersecting the upper band of the scroll
  // container becomes the active menu item.
  useEffect(() => {
    const container = document.getElementById("main-content");
    if (!container) return undefined;
    const observer = new IntersectionObserver(
      (entries) => {
        if (spyLock.current) return;
        for (const entry of entries) {
          if (entry.isIntersecting) {
            setActive(entry.target.id);
          }
        }
      },
      { root: container, rootMargin: "-20% 0px -60% 0px" },
    );
    for (const s of visible) {
      const el = document.getElementById(s.id);
      if (el) observer.observe(el);
    }
    return () => observer.disconnect();
  }, [isOwner]);

  // Manual scrolling suppresses spy updates for a moment so a scrolled-to
  // section stays highlighted through intermediate sections.
  useEffect(() => {
    const container = document.getElementById("main-content");
    if (!container) return undefined;
    const onScroll = () => {
      spyLock.current = true;
      if (spyTimer.current) window.clearTimeout(spyTimer.current);
      spyTimer.current = window.setTimeout(() => {
        spyLock.current = false;
      }, 600);
    };
    container.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      container.removeEventListener("scroll", onScroll);
      if (spyTimer.current) window.clearTimeout(spyTimer.current);
    };
  }, []);

  return (
    <div className="mx-auto flex w-full max-w-5xl gap-6 p-4" data-testid="settings-page">
      {/* Section rail */}
      <nav aria-label="Settings sections" className="hidden w-48 shrink-0 md:block">
        <ul className="sticky top-4 flex flex-col gap-0.5">
          {visible.map((s) => (
            <li key={s.id}>
              <button
                type="button"
                onClick={() => go(s.id)}
                aria-current={active === s.id ? "true" : undefined}
                className={cn(
                  "flex h-9 w-full items-center gap-2.5 rounded-md px-2 text-sm transition-colors duration-100",
                  active === s.id
                    ? "bg-primary/10 font-medium text-primary"
                    : "text-muted-foreground hover:bg-accent hover:text-foreground",
                )}
                data-testid={`settings-nav-${s.id}`}
              >
                <s.icon className="h-4 w-4" />
                {s.label}
              </button>
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

        <section id="account" aria-label="Account" className="scroll-mt-4">
          <Suspense fallback={<Skeleton className="h-64 w-full" />}>
            <AccountPage />
          </Suspense>
        </section>

        {isOwner ? (
          <section
            id="users"
            aria-label="Users"
            className="mt-10 border-t pt-8 scroll-mt-4"
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
