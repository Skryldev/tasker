import { DoranDate, faIR } from "@doranjs/core";

/**
 * Gregorian/Jalali boundary for the whole application.
 *
 * ## Canonical representation
 * The API speaks Gregorian date-only strings (`YYYY-MM-DD`, e.g. `2026-10-31`)
 * for every date field (`due_date`, `start_date`, `due_before`, `due_after`).
 * Nothing Jalali ever crosses the network boundary — this module is the single
 * place where the two calendars meet.
 *
 * ## Timezone policy
 * The app previously had no explicit timezone policy: `<input type="date">`
 * worked in the browser's local zone. Phase 6 makes the policy explicit and
 * deterministic: **date-only values are interpreted as `Asia/Tehran` calendar
 * days**, matching the Persian-calendar product. A picked Jalali day therefore
 * always serializes to the same Gregorian calendar day regardless of the
 * viewer's machine timezone. Date-TIME instants (epoch milliseconds on
 * activity, comments, notifications) are never altered — they are read-only
 * displays and keep their exact instant; relative phrasing ("2h ago") stays
 * in English alongside the app's English chrome and is not a calendar date.
 *
 * Date-only strings are parsed by field extraction — never via
 * `new Date("YYYY-MM-DD")` (UTC) or `toISOString()` on a local Date (both can
 * shift the calendar day).
 */

/** IANA zone that defines the app's calendar-day boundary. */
export const APP_TIME_ZONE = "Asia/Tehran";

/** The application's display locale (Persian labels, Persian digits). */
export const APP_LOCALE = faIR;

const DATE_ONLY = /^(\d{4})-(\d{2})-(\d{2})/;

/**
 * Parse a canonical API date-only string (`YYYY-MM-DD`) into the Doran
 * representation of the same calendar day. Returns `null` for null/empty or
 * malformed input so optional fields stay optional. Rejects values with a
 * time component — date-only fields are calendar dates, not timestamps.
 */
export function parseGregorianDateOnly(value: string | null | undefined): DoranDate | null {
  if (!value) return null;
  const match = DATE_ONLY.exec(value.trim());
  // Exactly YYYY-MM-DD: a time suffix means this is not a date-only field.
  if (!match || match[0] !== value.trim()) return null;
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  if (month < 1 || month > 12 || day < 1 || day > 31) return null;
  // Field extraction + Tehran anchoring: no UTC round-trip, no local shift.
  return DoranDate.fromGregorianParts({ year, month, day }, { timeZone: APP_TIME_ZONE });
}

/**
 * Serialize a picked Jalali date back to the canonical API form: the Gregorian
 * calendar day of the picked date, `YYYY-MM-DD`, Latin digits. `null`/undefined
 * round-trips to `null` so optional fields can be cleared.
 */
export function formatGregorianDateOnly(date: DoranDate | null | undefined): string | null {
  if (!date) return null;
  // Gregorian wall-clock fields in the date's own zone (Tehran), Latin digits —
  // explicitly documented as backend-safe.
  return date.formatGregorian("YYYY-MM-DD");
}

/**
 * Display form of a date-only API value in the Persian calendar, e.g.
 * `۱۴۰۵/۰۸/۰۹`. Falls back to the raw string when it cannot be parsed
 * (defensive; the API contract only produces valid `YYYY-MM-DD`).
 */
export function formatJalaliDate(value: string | null | undefined): string {
  const date = parseGregorianDateOnly(value);
  if (!date) return value?.trim() ? value : "—";
  return date.format("YYYY/MM/DD");
}

/**
 * The Jalali (Solar Hijri) year/month/day for a Gregorian API date — used by
 * non-Doran UI that needs the calendar fields directly (e.g. month labels).
 * Returns `null` for unparseable input.
 */
export function jalaliParts(value: string | null | undefined): {
  year: number;
  month: number;
  day: number;
} | null {
  const date = parseGregorianDateOnly(value);
  if (!date) return null;
  const p = date.toObject();
  return { year: p.year, month: p.month, day: p.day };
}

/**
 * Today as the canonical API date-only string, anchored to the app calendar
 * day (Tehran) rather than the viewer's machine.
 */
export function todayGregorianISO(): string {
  return formatGregorianDateOnly(DoranDate.now({ timeZone: APP_TIME_ZONE })) as string;
}
