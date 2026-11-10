import { describe, expect, it } from "vitest";
import { chipBackground, chipTextColor, contrast, mixHex } from "../chip-color";

// Phase 13.5 Bug 9: every label color must render a chip whose text/icon
// shade reaches WCAG AA (≥ 4.5:1) against the 15%-tinted background in
// BOTH themes.
import { LABEL_PALETTE } from "../label-palette";

describe("chip color computation", () => {
  it("palette colors reach AA contrast in light mode", () => {
    for (const color of LABEL_PALETTE) {
      const bg = chipBackground(color, false);
      const text = chipTextColor(color, false);
      const ratio = contrast(text, bg);
      expect(ratio, `${color} light: ${text} on ${bg} = ${ratio}`).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("palette colors reach AA contrast in dark mode", () => {
    for (const color of LABEL_PALETTE) {
      const bg = chipBackground(color, true);
      const text = chipTextColor(color, true);
      const ratio = contrast(text, bg);
      expect(ratio, `${color} dark: ${text} on ${bg} = ${ratio}`).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("background is the color at 15% over the surface", () => {
    // 15% of #ef4444 over white = mostly white with a red tint.
    const bg = chipBackground("#ef4444", false);
    expect(bg).not.toBe("#ef4444");
    expect(bg.startsWith("#")).toBe(true);
    // The blended background leans heavily toward the surface.
    const ratioToWhite = contrast(bg, "#ffffff");
    expect(ratioToWhite).toBeLessThan(1.4);
  });

  it("text shade keeps the same hue family (not a neutral swap)", () => {
    const text = chipTextColor("#3b82f6", false);
    // Blue hue pushed toward black — still bluish, not gray.
    const r = parseInt(text.slice(1, 3), 16);
    const b = parseInt(text.slice(5, 7), 16);
    expect(b).toBeGreaterThan(r);
  });

  it("mixHex interpolates linearly", () => {
    expect(mixHex("#000000", "#ffffff", 0)).toBe("#000000");
    expect(mixHex("#000000", "#ffffff", 1)).toBe("#ffffff");
    expect(mixHex("#000000", "#ffffff", 0.5)).toBe("#808080");
  });
});
