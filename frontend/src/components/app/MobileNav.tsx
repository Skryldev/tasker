import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { SidebarNav } from "@/components/app/AppShell";

/** Slide-in navigation for widths below `sm`, where the sidebar is hidden.
    Phase 13: code-split out of the eager shell — loads on first open. */
export function MobileNav({
  open,
  onOpenChange,
  collapsed,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  collapsed?: boolean;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="left-0 top-0 h-full max-h-none w-72 max-w-[85vw] translate-x-0 translate-y-0 rounded-none rounded-r-lg border-r p-3 data-[state=open]:slide-in-from-left data-[state=closed]:slide-out-from-left"
        aria-describedby={undefined}
      >
        <DialogTitle className="text-sm font-semibold">Navigation</DialogTitle>
        <nav className="mt-3 flex flex-col" aria-label="Mobile">
          <SidebarNav onNavigate={() => onOpenChange(false)} collapsed={collapsed} />
        </nav>
        <div className="mt-auto pb-1 pl-2 text-2xs text-muted-foreground">Tasker 0.1</div>
      </DialogContent>
    </Dialog>
  );
}
