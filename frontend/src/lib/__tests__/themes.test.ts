import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { THEMES, themeById } from "@/lib/theme";

/**
 * Theme engine contracts (Phase 9):
 *  - at least 8 themes shipped, each with a complete token set in index.css;
 *  - every theme satisfies WCAG AA (≥ 4.5:1) on the key pairs: body text on
 *    background and primary on its foreground;
 *  - the registry, the no-flash script in index.html, and the CSS blocks
 *    stay in sync.
 */

const here = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(join(here, "..", "..", "index.css"), "utf8");
const html = readFileSync(join(here, "..", "..", "..", "index.html"), "utf8");

const REQUIRED_TOKENS = [
  "background",
  "foreground",
  "card",
  "card-foreground",
  "popover",
  "popover-foreground",
  "primary",
  "primary-foreground",
  "secondary",
  "secondary-foreground",
  "muted",
  "muted-foreground",
  "accent",
  "accent-foreground",
  "destructive",
  "destructive-foreground",
  "border",
  "input",
  "ring",
  "sidebar",
  "sidebar-foreground",
  "success",
  "success-foreground",
  "warning",
  "warning-foreground",
  "info",
] as const;

/** Extract `selector { … }` blocks from the stylesheet. */
function cssBlock(selector: string): string | undefined {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = new RegExp(`${escaped}\\s*\\{([\\s\\S]*?)\\}`);
  return re.exec(css)?.[1];
}

function token(block: string | undefined, name: string): [number, number, number] | undefined {
  const m = block ? new RegExp(`--${name}:\\s*([\\d.]+)\\s+([\\d.]+)%\\s+([\\d.]+)%`).exec(block) : null;
  if (!m) return undefined;
  return [Number(m[1]), Number(m[2]), Number(m[3])];
}

function hslToRgb(h: number, s: number, l: number): [number, number, number] {
  const sat = s / 100;
  const light = l / 100;
  const k = (n: number) => (n + h / 30) % 12;
  const a = sat * Math.min(light, 1 - light);
  const f = (n: number) => light - a * Math.max(-1, Math.min(k(n) - 3, Math.min(9 - k(n), 1)));
  return [f(0), f(8), f(4)];
}

function channelLuminance(c: number): number {
  return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

function luminance(hsl: [number, number, number]): number {
  const [r, g, b] = hslToRgb(hsl[0], hsl[1], hsl[2]).map(channelLuminance);
  return 0.2126 * (r ?? 0) + 0.7152 * (g ?? 0) + 0.0722 * (b ?? 0);
}

function contrast(a: [number, number, number], b: [number, number, number]): number {
  const la = luminance(a);
  const lb = luminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

describe("theme registry", () => {
  it("ships at least eight themes", () => {
    expect(THEMES.length).toBeGreaterThanOrEqual(8);
  });

  it("uses unique ids and light/dark appearances", () => {
    const ids = new Set(THEMES.map((t) => t.id));
    expect(ids.size).toBe(THEMES.length);
    for (const t of THEMES) {
      expect(["light", "dark"]).toContain(t.appearance);
      expect(t.swatch).toHaveLength(3);
    }
  });

  it("keeps the registry, CSS, and the no-flash script in sync", () => {
    const mapBody = /var darkThemes = \{([\s\S]*?)\};/.exec(html)?.[1] ?? "";
    expect(mapBody).not.toBe("");
    const keyRe = (id: string) =>
      new RegExp(`["']?${id.replaceAll("-", "\\-")}["']?\\s*:`);
    for (const t of THEMES) {
      // Legacy pair uses the :root/.dark blocks instead of data-theme.
      if (t.id !== "light" && t.id !== "dark") {
        expect(cssBlock(`[data-theme="${t.id}"]`), `CSS block for ${t.id}`).toBeDefined();
      }
      // Every dark-appearance theme appears in the inline dark map —
      // light themes must NOT be listed there.
      if (t.appearance === "dark") {
        expect(mapBody).toMatch(keyRe(t.id));
      } else {
        expect(mapBody).not.toMatch(keyRe(t.id));
      }
    }
    expect(themeById("does-not-exist")).toBeUndefined();
  });
});

describe("theme token completeness", () => {
  it("defines every required token in every theme block", () => {
    for (const t of THEMES.filter((t) => t.id !== "light" && t.id !== "dark")) {
      const block = cssBlock(`[data-theme="${t.id}"]`);
      for (const name of REQUIRED_TOKENS) {
        expect(token(block, name), `${t.id}: --${name}`).toBeDefined();
      }
    }
    // The baseline pair stays complete too.
    const rootBlock = cssBlock(":root");
    const darkBlock = cssBlock(".dark");
    for (const name of REQUIRED_TOKENS) {
      expect(token(rootBlock, name), `:root: --${name}`).toBeDefined();
      expect(token(darkBlock, name), `.dark: --${name}`).toBeDefined();
    }
  });
});

describe("WCAG AA contrast on key pairs", () => {
  const cases: { id: string; block?: string }[] = THEMES.map((t) => ({
    id: t.id,
    block: t.id === "light" ? cssBlock(":root") : t.id === "dark" ? cssBlock(".dark") : cssBlock(`[data-theme="${t.id}"]`),
  }));

  it.each(cases)("$id: body text on background", ({ id, block }) => {
    const bg = token(block, "background");
    const fg = token(block, "foreground");
    expect(bg, `${id} background`).toBeDefined();
    expect(fg, `${id} foreground`).toBeDefined();
    expect(contrast(fg!, bg!)).toBeGreaterThanOrEqual(4.5);
  });

  it.each(cases)("$id: foreground on primary", ({ id, block }) => {
    const primary = token(block, "primary");
    const primaryFg = token(block, "primary-foreground");
    expect(primary, `${id} primary`).toBeDefined();
    expect(primaryFg, `${id} primary-foreground`).toBeDefined();
    expect(contrast(primaryFg!, primary!)).toBeGreaterThanOrEqual(4.5);
  });
});
