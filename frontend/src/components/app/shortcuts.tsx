import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { Keyboard } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useWorkspace } from "@/lib/workspace";

// Phase 12: the cheatsheet dialog is code-split — the eager manager keeps
// only the trigger button and the global key listener.
const CheatsheetDialog = lazy(() =>
  import("./CheatsheetDialog").then((m) => ({ default: m.CheatsheetDialog })),
);

/**
 * Global keyboard shortcuts (Phase 10.5 §6.2). All handlers ignore events
 * from inputs/textareas/contenteditables — except Esc and Cmd/Ctrl+K, which
 * always work. Sequences (G → …) reset after one second of inactivity.
 */

function isTypingTarget(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el) return false;
  return (
    el.tagName === "INPUT" ||
    el.tagName === "TEXTAREA" ||
    el.tagName === "SELECT" ||
    el.isContentEditable
  );
}

/** Wire the global listener; returns nothing (side-effect hook). */
export function useGlobalShortcuts(handlers: {
  onPalette: () => void;
  onCheatsheet: () => void;
  onCreateTask: () => void;
  onFocusSearch: () => void;
  onNavigate: (path: string) => void;
}) {
  const gPending = useRef(false);
  const gTimer = useRef<number | null>(null);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      const typing = isTypingTarget(e.target);
      // Cmd/Ctrl+K works everywhere.
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        handlers.onPalette();
        return;
      }
      if (typing || e.metaKey || e.ctrlKey || e.altKey) return;
      if (gPending.current) {
        const target = e.key.toLowerCase();
        const { teamId } = { teamId: windowTeamIdRef.current };
        const base = teamId ? `?team=${teamId}` : "";
        const routes: Record<string, string> = {
          t: `/${base}`,
          p: `/projects${base}`,
          g: teamId ? `/teams/${teamId}/goals${base}` : "",
          c: teamId ? `/teams/${teamId}/capacity${base}` : "",
        };
        const path = routes[target];
        gPending.current = false;
        if (gTimer.current) window.clearTimeout(gTimer.current);
        if (path) {
          e.preventDefault();
          handlers.onNavigate(path);
        }
        return;
      }
      if (e.key.toLowerCase() === "g") {
        gPending.current = true;
        if (gTimer.current) window.clearTimeout(gTimer.current);
        gTimer.current = window.setTimeout(() => {
          gPending.current = false;
        }, 1000);
        return;
      }
      if (e.key === "c") {
        e.preventDefault();
        handlers.onCreateTask();
      } else if (e.key === "/") {
        e.preventDefault();
        handlers.onFocusSearch();
      } else if (e.key === "?") {
        e.preventDefault();
        handlers.onCheatsheet();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [handlers]);
}

// The active team id changes per route; shortcuts read it via a ref set by
// the shell each render (simplest channel that avoids stale closures).
const windowTeamIdRef = { current: undefined as string | undefined };

export function ShortcutsManager() {
  const navigate = useNavigate();
  const { teamId } = useWorkspace();
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [cheatsheetOpen, setCheatsheetOpen] = useState(false);
  const [Palette, setPalette] = useState<React.ComponentType<{
    open: boolean;
    onOpenChange: (v: boolean) => void;
  }> | null>(null);

  windowTeamIdRef.current = teamId;

  // The palette is code-split; load it on first use so the initial bundle
  // carries none of it.
  const loadPalette = () => {
    if (!Palette) {
      void import("./CommandPalette").then((m) => setPalette(() => m.CommandPalette));
    }
    setPaletteOpen(true);
  };

  const handlers = useMemo(
    () => ({
      onPalette: loadPalette,
      onCheatsheet: () => setCheatsheetOpen(true),
      onCreateTask: () => {
        const onProjectPage = window.location.pathname.startsWith("/projects/");
        navigate(onProjectPage ? `${window.location.pathname}?new=task` : "/projects?new=project");
      },
      onFocusSearch: () => {
        if (window.location.pathname !== "/") navigate("/");
        window.dispatchEvent(new CustomEvent("tasker:focus-search"));
      },
      onNavigate: (path: string) => navigate(path),
    }),
    // loadPalette reads state; keep the handlers stable across renders.
    [navigate, teamId],
  );

  useGlobalShortcuts(handlers);

  return (
    <>
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label="Keyboard shortcuts"
        title="Keyboard shortcuts (?)"
        onClick={() => setCheatsheetOpen(true)}
        data-testid="shortcuts-help"
      >
        <Keyboard />
      </Button>
      {Palette ? <Palette open={paletteOpen} onOpenChange={setPaletteOpen} /> : null}
      {cheatsheetOpen ? (
        <Suspense fallback={null}>
          <CheatsheetDialog open={cheatsheetOpen} onOpenChange={setCheatsheetOpen} />
        </Suspense>
      ) : null}
    </>
  );
}

/** Focus the My Tasks search input when the global "/" fires. */
export function useFocusSearchOnEvent(ref: React.RefObject<HTMLInputElement | null>) {
  useEffect(() => {
    const focus = () => ref.current?.focus();
    window.addEventListener("tasker:focus-search", focus);
    return () => window.removeEventListener("tasker:focus-search", focus);
  }, [ref]);
}
