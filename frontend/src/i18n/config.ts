import i18next, { type i18n as I18nInstance } from "i18next";
import { initReactI18next } from "react-i18next";
import resourcesToBackend from "i18next-resources-to-backend";
import { LOCALES, type Locale } from "./locale";

/** Message namespaces (ADR-0024 §5): one JSON file per locale × namespace. */
export const NAMESPACES = [
  "common",
  "tasks",
  "settings",
  "errors",
  "notifications",
  "goals",
] as const;
export type Namespace = (typeof NAMESPACES)[number];
/** Loaded before first render so the shell paints translated. */
export const EAGER_NAMESPACE: Namespace = "common";

/**
 * Per locale × namespace dynamic import — Vite emits one chunk per JSON
 * file, so only the active locale's messages ever reach the client, and
 * `index.js` contains no locale JSON at all (ADR-0024 §5.5).
 */
const resourceBackend = resourcesToBackend((lng: string, ns: string) =>
  import(`../locales/${lng}/${ns}.json`),
);

export async function initI18n(locale: Locale): Promise<I18nInstance> {
  await i18next.use(resourceBackend).use(initReactI18next).init({
    lng: locale,
    fallbackLng: "en",
    supportedLngs: [...LOCALES],
    ns: NAMESPACES as unknown as string[],
    defaultNS: "common",
    // The shell preloads `common`; everything else loads per route mount.
    partialBundledLanguages: true,
    // Plurals come from Intl.PluralRules via i18next core (ADR-0024 —
    // no ICU plugin). `.` in keys is the nesting/separator convention.
    nsSeparator: ":",
    keySeparator: ".",
    interpolation: { escapeValue: false },
    react: { useSuspense: false },
    returnEmptyString: false,
  });
  // Preload the eager namespace so the first paint is fully translated.
  await i18next.loadNamespaces([EAGER_NAMESPACE]);
  return i18next;
}

