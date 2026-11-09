import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { PanelRightClose, PanelRightOpen } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/**
 * Phase 12 WS6: resizable, collapsible right sidebar for the task detail
 * panel. Cmd/Ctrl+. toggles it; the width persists in localStorage and a
 * drag handle resizes between 320 and 640 px. Collapsing keeps the content
 * mounted (drafts survive) but out of the tab order.
 */

const WIDTH_KEY = "tasker.detailSidebar.width";
const MIN_WIDTH = 320;
const MAX_WIDTH = 640;
const DEFAULT_WIDTH = 380;

function storedWidth(): number {
  const raw = Number(localStorage.getItem(WIDTH_KEY));
  if (!Number.isFinite(raw) || raw <= 0) return DEFAULT_WIDTH;
  return Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, raw));
}

export function DetailSidebar({
  children,
  testId = "task-panel",
  hidden,
}: {
  children: ReactNode;
  testId?: string;
  /** Bug 3: the full task page is open — the quick view yields the layout. */
  hidden?: boolean;
}) {
  const [width, setWidth] = useState(storedWidth);
  const [collapsed, setCollapsed] = useState(false);
  const regionRef = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);

  // WS6: Cmd/Ctrl+. toggles the sidebar (global, even while typing — it is
  // a layout command like Cmd+K).
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === ".") {
        e.preventDefault();
        setCollapsed((c) => !c);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  // Focus management: opening the sidebar hands focus to the region so
  // keyboard users land inside it without hunting through the tab order.
  useEffect(() => {
    if (!collapsed) regionRef.current?.focus({ preventScroll: true });
  }, [collapsed]);

  const startDrag = useCallback((e: React.PointerEvent) => {
    e.preventDefault();
    dragging.current = true;
    const startX = e.clientX;
    const startWidth = width;
    const onMove = (ev: PointerEvent) => {
      if (!dragging.current) return;
      const delta = startX - ev.clientX;
      setWidth(Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, startWidth + delta)));
    };
    const onUp = () => {
      dragging.current = false;
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      setWidth((w) => {
        localStorage.setItem(WIDTH_KEY, String(w));
        return w;
      });
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  }, [width]);

  if (collapsed) {
    return (
      <div className="hidden shrink-0 border-l lg:block" data-testid={`${testId}-collapsed`}>
        <Button
          variant="ghost"
          size="icon-sm"
          className="m-1"
          aria-label="Show detail sidebar"
          onClick={() => setCollapsed(false)}
          data-testid="sidebar-expand"
        >
          <PanelRightOpen />
        </Button>
      </div>
    );
  }

  return (
    <aside
      className={cn(
        "relative hidden shrink-0 border-l lg:block",
        hidden && "hidden",
      )}
      style={{ width: `${width}px` }}
      data-testid={testId}
      aria-hidden={hidden || undefined}
    >
      {/* Resize handle: 6px interactive strip on the border. */}
      <div
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize sidebar"
        onPointerDown={startDrag}
        className="absolute inset-y-0 left-0 z-10 w-1.5 cursor-col-resize transition-colors hover:bg-primary/30"
        data-testid="sidebar-resize"
      />
      {/* Collapse control: bottom-right corner — the panel header's top-right
          hosts the task actions menu, so a top placement would intercept its
          clicks (found by E2E). */}
      <div className="absolute bottom-2 right-2 z-10">
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Hide detail sidebar (⌘.)"
          onClick={() => setCollapsed(true)}
          data-testid="sidebar-collapse"
        >
          <PanelRightClose />
        </Button>
      </div>
      <div
        ref={regionRef}
        tabIndex={-1}
        role="complementary"
        aria-label="Task detail"
        className={cn("h-full outline-none", dragging.current && "select-none")}
      >
        {children}
      </div>
    </aside>
  );
}
