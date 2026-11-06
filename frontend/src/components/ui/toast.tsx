import { Toaster as SonnerToaster, toast } from "sonner";
import { useTheme } from "@/lib/theme";

/**
 * Application feedback standard. All user-facing notifications flow through
 * [Sonner](https://sonner.emilkowal.ski) — success, error, and informational
 * messages share one accessible, theme-aware surface with consistent timing
 * (errors linger longer than confirmations). Screens keep inline field-level
 * validation where it is more usable; toasts are for operation outcomes.
 *
 * The `useToast().push(kind, title, description)` signature is preserved so
 * feature code reads the same across the app; it adapts to Sonner beneath.
 */

/** How long a toast stays on screen, in milliseconds. */
const DURATION = {
  success: 4000,
  info: 4000,
  error: 8000,
} as const;

type ToastKind = keyof typeof DURATION;

export function ToastProvider({ children }: { children: React.ReactNode }) {
  const { appearance } = useTheme();
  return (
    <>
      {children}
      <SonnerToaster
        position="bottom-right"
        theme={appearance}
        richColors
        closeButton
        gap={8}
        toastOptions={{
          style: {
            // Align Sonner's surface with the app's popover token.
            background: "hsl(var(--popover))",
            border: "1px solid hsl(var(--border))",
            color: "hsl(var(--popover-foreground))",
          },
        }}
      />
    </>
  );
}

export interface ToastAction {
  label: string;
  onClick: () => void;
}

export interface ToastOptions {
  /** Single action button (e.g. "Undo"), rendered at the toast's right. */
  action?: ToastAction;
  /** Override the default per-kind duration (ms). */
  duration?: number;
}

/** Push a standardized feedback toast. */
export function pushToast(
  kind: ToastKind,
  title: string,
  description?: string,
  options?: ToastOptions,
) {
  const toastOptions = {
    description,
    duration: options?.duration ?? DURATION[kind],
    action: options?.action
      ? { label: options.action.label, onClick: options.action.onClick }
      : undefined,
  };
  switch (kind) {
    case "success":
      toast.success(title, toastOptions);
      break;
    case "error":
      toast.error(title, toastOptions);
      break;
    case "info":
      toast.info(title, toastOptions);
      break;
  }
}

export function useToast() {
  return { push: pushToast };
}

export type { ToastKind };
