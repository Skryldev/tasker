import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { SHORTCUTS } from "./shortcuts-list";

/**
 * The keyboard cheatsheet dialog (Phase 12): code-split out of the eager
 * shortcuts manager — it renders only when the user opens the cheatsheet.
 */
export function CheatsheetDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md" aria-describedby={undefined}>
        <DialogHeader>
          <DialogTitle>Keyboard shortcuts</DialogTitle>
          <DialogDescription>
            Shortcuts never fire while typing in a field (except Esc and ⌘K).
          </DialogDescription>
        </DialogHeader>
        <ul className="flex flex-col gap-2" data-testid="shortcuts-list">
          {SHORTCUTS.map((s) => (
            <li key={s.keys} className="flex items-center justify-between gap-4 text-sm">
              <span className="text-muted-foreground">{s.action}</span>
              <kbd className="rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-2xs">
                {s.keys}
              </kbd>
            </li>
          ))}
        </ul>
      </DialogContent>
    </Dialog>
  );
}
