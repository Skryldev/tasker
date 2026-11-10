/**
 * Label chip color computation (Phase 13.5 Bug 9).
 *
 * A chip renders its color at 15% opacity over the card surface and must
 * pick a text/icon shade of the SAME hue with WCAG AA (≥ 4.5:1) contrast
 * against that tinted background — computed at render time, not hardcoded
 * per color, and theme-aware (the tint blends with a light surface in
 * light themes and a dark surface in dark themes).
 */

export function hexToRgb(hex: string): [number, number, number] {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m || !m[1]) return [0, 0, 0];
  const int = parseInt(m[1], 16);
  return [(int >> 16) & 255, (int >> 8) & 255, int & 255];
}

export function rgbToHex(rgb: [number, number, number]): string {
  const r = Math.max(0, Math.min(255, Math.round(rgb[0])));
  const g = Math.max(0, Math.min(255, Math.round(rgb[1])));
  const b = Math.max(0, Math.min(255, Math.round(rgb[2])));
  return `#${((1 << 24) | (r << 16) | (g << 8) | b).toString(16).slice(1)}`;
}

/** Linear mix of two colors: t=0 → a, t=1 → b. */
export function mixHex(a: string, b: string, t: number): string {
  const [r1, g1, b1] = hexToRgb(a);
  const [r2, g2, b2] = hexToRgb(b);
  return rgbToHex([
    (r1 ?? 0) + ((r2 ?? 0) - (r1 ?? 0)) * t,
    (g1 ?? 0) + ((g2 ?? 0) - (g1 ?? 0)) * t,
    (b1 ?? 0) + ((b2 ?? 0) - (b1 ?? 0)) * t,
  ]);
}

/** WCAG relative luminance. */
export function luminance(hex: string): number {
  const [r, g, b] = hexToRgb(hex).map((v) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  });
  return 0.2126 * (r ?? 0) + 0.7152 * (g ?? 0) + 0.0722 * (b ?? 0);
}

/** WCAG contrast ratio between two colors (≥ 1, ≤ 21). */
export function contrast(a: string, b: string): number {
  const l1 = luminance(a);
  const l2 = luminance(b);
  const [hi, lo] = l1 >= l2 ? [l1, l2] : [l2, l1];
  return (hi + 0.05) / (lo + 0.05);
}

/** Approximate card surfaces for the bundled themes (light / dark). */
const SURFACE_LIGHT = "#ffffff";
const SURFACE_DARK = "#1a1b20";

/** The chip background: the label color at 15% opacity over the card. */
export function chipBackground(hex: string, dark: boolean): string {
  return mixHex(hex, dark ? SURFACE_DARK : SURFACE_LIGHT, 0.85);
}

/**
 * The text/icon color: the same hue pushed far enough toward black (light
 * surfaces) or white (dark surfaces) that contrast against the tinted
 * background is ≥ 4.5:1. Returns the first candidate that passes; the
 * last candidate is a near-neutral of the correct polarity so a result
 * always exists.
 */
export function chipTextColor(hex: string, dark: boolean): string {
  const bg = chipBackground(hex, dark);
  const target = dark ? "#ffffff" : "#000000";
  const steps = dark ? [0.45, 0.6, 0.75, 0.92] : [0.4, 0.55, 0.7, 0.88];
  let best = target;
  let bestRatio = 0;
  for (const t of steps) {
    const candidate = mixHex(hex, target, t);
    const ratio = contrast(candidate, bg);
    if (ratio >= 4.5) return candidate;
    if (ratio > bestRatio) {
      bestRatio = ratio;
      best = candidate;
    }
  }
  // No candidate reached 4.5 (pathological hues): guarantee polarity with
  // an extreme mix, which always clears 4.5 for any mid-tone tint.
  const extreme = mixHex(hex, target, 0.97);
  return contrast(extreme, bg) >= bestRatio ? extreme : best;
}
