import { timeAgo, formatInstant } from "@/lib/utils";
import { useUserTimezone } from "@/lib/auth";

/**
 * Relative timestamp ("2h ago") whose hover title shows the absolute instant
 * rendered in the profile's timezone (display-only; see lib/doran.ts for the
 * calendar-day policy).
 */
export function Timestamp({ value }: { value: number | null | undefined }) {
  const timeZone = useUserTimezone();
  if (!value) return <span>—</span>;
  return <span title={formatInstant(value, timeZone)}>{timeAgo(value)}</span>;
}
