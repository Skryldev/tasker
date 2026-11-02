import { useState, type ReactNode } from "react";
import { TriangleAlert } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";

/**
 * Accessible replacement for `window.confirm` on destructive or
 * consequential operations. Focus is trapped and restored by the underlying
 * Radix dialog; Escape and the overlay dismiss without confirming. The
 * confirm button stays disabled while the async action is pending, so the
 * operation cannot be submitted twice.
 */
export function ConfirmDialog({
  open,
  onOpenChange,
  title,
  description,
  confirmLabel = "Confirm",
  cancelLabel = "Cancel",
  destructive = true,
  pending = false,
  onConfirm,
  children,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description?: string;
  confirmLabel?: string;
  cancelLabel?: string;
  destructive?: boolean;
  pending?: boolean;
  onConfirm: () => void;
  /** Optional extra content rendered between the description and the actions. */
  children?: ReactNode;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent role="alertdialog" className="max-w-sm">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            {destructive ? (
              <TriangleAlert className="h-4 w-4 shrink-0 text-destructive" aria-hidden />
            ) : null}
            {title}
          </DialogTitle>
          {description ? <DialogDescription>{description}</DialogDescription> : null}
        </DialogHeader>
        {children}
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={pending}>
            {cancelLabel}
          </Button>
          <Button
            variant={destructive ? "destructive" : "default"}
            onClick={onConfirm}
            disabled={pending}
            autoFocus
          >
            {pending ? "Working…" : confirmLabel}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** Hook form of the confirm dialog: returns a wrapper and a request function. */
export function useConfirmDialog() {
  const [state, setState] = useState<{
    open: boolean;
    title: string;
    description?: string;
    confirmLabel?: string;
    destructive: boolean;
    action: (() => void) | null;
  }>({ open: false, title: "", destructive: true, action: null });

  const confirm = (options: {
    title: string;
    description?: string;
    confirmLabel?: string;
    destructive?: boolean;
    action: () => void;
  }) => {
    setState({
      open: true,
      title: options.title,
      description: options.description,
      confirmLabel: options.confirmLabel,
      destructive: options.destructive ?? true,
      action: options.action,
    });
  };

  const element = (
    <ConfirmDialog
      open={state.open}
      onOpenChange={(open) => setState((s) => ({ ...s, open }))}
      title={state.title}
      description={state.description}
      confirmLabel={state.confirmLabel}
      destructive={state.destructive}
      onConfirm={() => {
        setState((s) => ({ ...s, open: false }));
        state.action?.();
      }}
    />
  );

  return { confirm, element };
}
