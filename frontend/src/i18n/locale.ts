/**
 * Locale registry and detection (Phase 15, ADR-0024).
 *
 * The URL is the source of truth: the locale is the first path segment
 * (`/fa/...`, `/en/...`, `/fr/...`). Detection order for prefixless URLs:
 * localStorage (`tasker:locale`) → navigator.language → `en`.
 */

export const LOCALES = ["fa", "en", "fr"] as const;
export type Locale = (typeof LOCALES)[number];
export const DEFAULT_LOCALE: Locale = "en";
/** localStorage key — mirrors the `tasker:*` convention (sidebar, theme). */
export const LOCALE_STORAGE_KEY = "tasker:locale";

export interface LocaleMeta {
  /** Native name shown in the switcher. */
  readonly name: string;
  readonly dir: "ltr" | "rtl";
  /** BCP-47 tag for Intl formatting. */
  readonly intl: string;
  /** Primary self-hosted font family for the locale. */
  readonly font: string;
  /** Preload hints for the locale's primary fonts (public/ paths). */
  readonly preloadFonts: readonly string[];
}

export const LOCALE_META: Record<Locale, LocaleMeta> = {
  fa: {
    name: "فارسی",
    dir: "rtl",
    intl: "fa-IR",
    font: "Vazirmatn UI",
    preloadFonts: ["/fonts/Vazirmatn-UI-Regular.woff2", "/fonts/Vazirmatn-UI-Medium.woff2"],
  },
  en: {
    name: "English",
    dir: "ltr",
    intl: "en",
    font: "Inter",
    preloadFonts: ["/fonts/Inter-Variable-latin.woff2"],
  },
  fr: {
    name: "Français",
    dir: "ltr",
    intl: "fr-FR",
    font: "Inter",
    preloadFonts: ["/fonts/Inter-Variable-latin.woff2"],
  },
};

export function isLocale(value: string | null | undefined): value is Locale {
  return value !== null && value !== undefined && (LOCALES as readonly string[]).includes(value);
}

/** The `/{locale}` prefix for a locale — empty never (en is not prefix-less). */
export function localePrefix(locale: Locale): string {
  return `/${locale}`;
}

/** Pull the locale out of a URL path's first segment, if present. */
export function localeFromPath(pathname: string): Locale | null {
  const segment = pathname.split("/")[1];
  return isLocale(segment) ? segment : null;
}

/** Remove the locale prefix (and collapse the leading slash). */
export function stripLocalePrefix(pathname: string): string {
  if (localeFromPath(pathname) !== null) {
    const rest = pathname.split("/").slice(2).join("/");
    return `/${rest}`;
  }
  return pathname;
}

function fromNavigator(): Locale | null {
  for (const tag of navigator.languages ?? []) {
    const base = tag.toLowerCase().split("-")[0];
    if (isLocale(base)) return base;
  }
  return null;
}

/** Read the persisted preference (no URL involvement). */
export function storedLocale(): Locale | null {
  try {
    const value = localStorage.getItem(LOCALE_STORAGE_KEY);
    return isLocale(value) ? value : null;
  } catch {
    return null;
  }
}

/** Persist an explicit switch. */
export function storeLocale(locale: Locale): void {
  try {
    localStorage.setItem(LOCALE_STORAGE_KEY, locale);
  } catch {
    /* storage may be unavailable (private mode) — the URL stays authoritative */
  }
}

/** URL → localStorage → navigator → default. */
export function detectLocale(): Locale {
  return storedLocale() ?? fromNavigator() ?? DEFAULT_LOCALE;
}

/**
 * Resolve the locale for the current location. Called once at bootstrap
 * before the router mounts: a prefixless URL is detected (and the caller
 * rewrites the address), a prefixed URL wins as-is.
 */
export function resolveInitialLocale(): Locale {
  const fromUrl = localeFromPath(window.location.pathname);
  if (fromUrl) return fromUrl;
  return detectLocale();
}

/** Full-page navigation to the same route under another locale. */
export function switchLocale(next: Locale): void {
  storeLocale(next);
  const path = stripLocalePrefix(window.location.pathname);
  const search = window.location.search;
  const hash = window.location.hash;
  window.location.assign(`/${next}${path === "/" ? "/" : path}${search}${hash}`);
}

/** Keep `<html lang>` and `<html dir>` in sync with the active locale. */
export function applyDocumentLocale(locale: Locale): void {
  const meta = LOCALE_META[locale];
  document.documentElement.lang = locale;
  document.documentElement.dir = meta.dir;
  // Typography tokens switch with the locale (see index.css).
  document.documentElement.dataset.locale = locale;
}
