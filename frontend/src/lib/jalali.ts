/**
 * Pure Gregorian ↔ Jalali (Solar Hijri) conversion — zero dependencies.
 *
 * Phase 10: the display path (`formatJalaliDate`) previously went through
 * `@doranjs/core`, which dragged the whole Doran calendar runtime into the
 * eager bundle just to render date strings in the task table. This module
 * ports the standard jalaali algorithm (Behrooz Nikoosokhan's jalaali-js,
 * MIT — the reference implementation used across the ecosystem), so the
 * eager path can render Persian calendar dates without that runtime.
 *
 * The Doran picker boundary (`lib/doran.ts`) still owns interactive picking;
 * this module owns string rendering. Correctness anchors are shared with
 * `lib/__tests__/doran.test.ts` (independently published Nowruz dates).
 */

const PERSIAN_DIGITS = ["۰", "۱", "۲", "۳", "۴", "۵", "۶", "۷", "۸", "۹"] as const;

function toPersianDigits(s: string): string {
  return s.replace(/\d/g, (d) => PERSIAN_DIGITS[Number(d)] ?? d);
}

function div(a: number, b: number): number {
  return Math.trunc(a / b);
}

function mod(a: number, b: number): number {
  return a - Math.trunc(a / b) * b;
}

/** Jalali-era break points (jalaali-js `breaks`). */
const BREAKS = [
  -61, 9, 38, 199, 426, 686, 756, 818, 1111, 1181, 1210, 1635, 2060, 2097, 2192,
  2262, 2324, 2394, 2456, 3178,
] as const;

/** Days in the Jalali year and leap status, from the era's break structure. */
function jalCal(jy: number): { leap: number; gy: number; march: number } {
  const bl = BREAKS.length;
  const gy = jy + 621;
  let leapJ = -14;
  let jp: number = BREAKS[0] ?? jy;
  const last = BREAKS[bl - 1] ?? jy;
  if (jy < jp || jy >= last) {
    throw new Error(`invalid Jalali year ${jy}`);
  }
  let jump = 0;
  for (let i = 1; i < bl; i += 1) {
    const jm: number = BREAKS[i] ?? last;
    jump = jm - jp;
    if (jy < jm) break;
    leapJ = leapJ + div(jump, 33) * 8 + div(mod(jump, 33), 4);
    jp = jm;
  }
  let n = jy - jp;
  leapJ = leapJ + div(n, 33) * 8 + div(mod(n, 33) + 3, 4);
  if (mod(jump, 33) === 4 && jump - n === 4) leapJ += 1;
  const leapG = div(gy, 4) - div((div(gy, 100) + 1) * 3, 4) - 150;
  const march = 20 + leapJ - leapG;
  if (jump - n < 6) n = n - jump + div(jump + 4, 33) * 33;
  let leap = mod(mod(n + 1, 33) - 1, 4);
  if (leap === -1) leap = 4;
  return { leap, gy, march };
}

/** Gregorian date → JDN (Julian day number, noon-based integer). */
function g2d(gy: number, gm: number, gd: number): number {
  let d =
    div((gy + div(gm - 8, 6) + 100100) * 1461, 4) +
    div(153 * mod(gm + 9, 12) + 2, 5) +
    gd -
    34840408;
  d = d - div(div(gy + 100100 + div(gm - 8, 6), 100) * 3, 4) + 752;
  return d;
}

/** JDN → Gregorian calendar fields. */
function d2g(jdn: number): { gy: number; gm: number; gd: number } {
  let j = 4 * jdn + 139361631;
  j = j + div(div(4 * jdn + 183187720, 146097) * 3, 4) * 4 - 3908;
  const i = div(mod(j, 1461), 4) * 5 + 308;
  const gd = div(mod(i, 153), 5) + 1;
  const gm = mod(div(i, 153), 12) + 1;
  const gy = div(j, 1461) - 100100 + div(8 - gm, 6);
  return { gy, gm, gd };
}

/** Jalali date → JDN. */
function j2d(jy: number, jm: number, jd: number): number {
  const r = jalCal(jy);
  return g2d(r.gy, 3, r.march) + (jm - 1) * 31 - div(jm, 7) * (jm - 7) + jd - 1;
}

/** JDN → Jalali fields. */
function d2j(jdn: number): { jy: number; jm: number; jd: number } {
  const gy = d2g(jdn).gy;
  let jy = gy - 621;
  const r = jalCal(jy);
  const jdn1f = g2d(gy, 3, r.march);
  let k = jdn - jdn1f;
  if (k >= 0) {
    if (k <= 185) {
      return { jy, jm: 1 + div(k, 31), jd: mod(k, 31) + 1 };
    }
    k -= 186;
  } else {
    jy -= 1;
    k += 179;
    if (r.leap === 1) k += 1;
  }
  return { jy, jm: 7 + div(k, 30), jd: mod(k, 30) + 1 };
}

/** Jalali ↔ Gregorian, in calendar fields (no time, no zone — field math). */
export function gregorianToJalali(
  gy: number,
  gm: number,
  gd: number,
): { year: number; month: number; day: number } {
  const { jy, jm, jd } = d2j(g2d(gy, gm, gd));
  return { year: jy, month: jm, day: jd };
}

export function jalaliToGregorian(
  jy: number,
  jm: number,
  jd: number,
): { year: number; month: number; day: number } {
  const { gy, gm, gd } = d2g(j2d(jy, jm, jd));
  return { year: gy, month: gm, day: gd };
}

export function isLeapJalaaliYear(jy: number): boolean {
  return jalCal(jy).leap === 0;
}

export function jalaliMonthLength(jy: number, jm: number): number {
  if (jm <= 6) return 31;
  if (jm <= 11) return 30;
  return isLeapJalaaliYear(jy) ? 30 : 29;
}

const DATE_ONLY = /^(\d{4})-(\d{2})-(\d{2})$/;

function parseDateOnly(value: string | null | undefined): [number, number, number] | null {
  if (!value) return null;
  const trimmed = value.trim();
  const match = DATE_ONLY.exec(trimmed);
  if (!match) return null;
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  if (month < 1 || month > 12 || day < 1 || day > 31) return null;
  return [year, month, day];
}

/**
 * Display form of a date-only API value in the Persian calendar, e.g.
 * `۱۴۰۵/۰۸/۰۹` — output-identical to the previous Doran-backed formatter.
 * Falls back to the raw string when it cannot be parsed (defensive; the API
 * contract only produces valid `YYYY-MM-DD`).
 */
export function formatJalaliDate(value: string | null | undefined): string {
  const parsed = parseDateOnly(value);
  if (!parsed) return value?.trim() ? value.trim() : "—";
  const { year, month, day } = gregorianToJalali(parsed[0], parsed[1], parsed[2]);
  return toPersianDigits(
    `${year}/${String(month).padStart(2, "0")}/${String(day).padStart(2, "0")}`,
  );
}

/**
 * The Jalali (Solar Hijri) fields for a Gregorian API date. Returns `null`
 * for unparseable input.
 */
export function jalaliParts(value: string | null | undefined): {
  year: number;
  month: number;
  day: number;
} | null {
  const parsed = parseDateOnly(value);
  if (!parsed) return null;
  return gregorianToJalali(parsed[0], parsed[1], parsed[2]);
}
