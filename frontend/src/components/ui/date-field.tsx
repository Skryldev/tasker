// Doran styles load with the fields that use them (all lazily routed pages).
import "@doranjs/ui/styles.css";
import "@doranjs/react/styles.css";
import { DoranDatePicker, DoranProvider } from "@doranjs/react";
import type { DoranDate } from "@doranjs/core";
import { cn } from "@/lib/utils";
import {
  formatGregorianDateOnly,
  parseGregorianDateOnly,
} from "@/lib/doran";
import { useDialogPortalContainer } from "@/components/ui/dialog";
import { APP_LOCALE, APP_TIME_ZONE } from "@/lib/doran";

/**
 * The application's single date-input control: a thin adapter over Doran's
 * `DoranDatePicker` that keeps the app's Gregorian API contract at the
 * boundary.
 *
 * - `value` / `onChange` speak the canonical API form (`YYYY-MM-DD` Gregorian,
 *   or empty for unset). Nothing Jalali leaves this component.
 * - The user sees and picks Solar Hijri dates; Persian labels and RTL come
 *   from Doran's `faIR` locale (set app-wide via `DoranProvider`).
 * - Styling merges with the app's shadcn/ui tokens through Doran's
 *   `classNames` slots and the token bridge in `index.css`.
 * - Inside a Radix dialog the calendar pop-over portals to the dialog itself
 *   (see `useDialogPortalContainer`) so the focus trap cannot pull focus out
 *   of the open calendar.
 */
export function DateField({
  id,
  value,
  onChange,
  ariaLabel,
  placeholder = "۱۴۰۵/۰۱/۰۱",
  disabled,
  required,
  className,
  invalid,
}: {
  id?: string;
  value: string | null | undefined;
  /** Receives the canonical Gregorian `YYYY-MM-DD` value, or "" when cleared. */
  onChange: (value: string) => void;
  ariaLabel?: string;
  placeholder?: string;
  disabled?: boolean;
  required?: boolean;
  className?: string;
  invalid?: boolean;
}) {
  const portalContainer = useDialogPortalContainer();
  const selected: DoranDate | null = parseGregorianDateOnly(value);
  return (
    <DoranProvider locale={APP_LOCALE} timeZone={APP_TIME_ZONE}>
    <DoranDatePicker
      id={id}
      value={selected}
      onChange={(date) => onChange(formatGregorianDateOnly(date) ?? "")}
      format="YYYY/MM/DD"
      placeholder={placeholder}
      aria-label={ariaLabel}
      disabled={disabled}
      required={required}
      invalid={invalid}
      size="sm"
      portalContainer={portalContainer}
      classNames={{
        trigger: cn(
          "w-full gap-1.5 rounded-md border-input bg-background px-2.5 text-sm shadow-sm",
          "data-[invalid]:border-destructive",
          className,
        ),
        input: "text-sm text-start",
      }}
    />
    </DoranProvider>
  );
}
