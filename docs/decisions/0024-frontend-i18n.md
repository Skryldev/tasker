# ADR-0024: Frontend internationalization — react-i18next, URL-based locales, lazy namespaces

Date: 2026-11-19 · Status: accepted

## Context

Tasker's UI is English-only with Persian calendar support (Phase 6 Doran
integration) and bilingual priority descriptions (Phase 14). Phase 15 makes
the product trilingual — `fa` (RTL, primary market), `en` (source of truth),
`fr` (formal expansion market) — under a hard bundle constraint: the eager
`index.js` chunk must stay ≤ **64.83 kB gzip** (the Phase 14 gate), and no
locale JSON may ship inside it.

## Decision

1. **Library: `react-i18next` + `i18next` 24.** Mature, plugin-based, and
   the only evaluated option whose lazy-loading story fits both requirements
   (per-locale *and* per-namespace chunks) without a build-step extraction
   pass:
   - **Lingui** rejected: compile-time macro extraction couples every string
     addition to a rebuild step and its `.po` toolchain adds workflow weight
     this phase explicitly defers (§5.4 — manual extraction).
   - **Paraglide JS** rejected: zero-runtime is attractive, but new messages
     require a rebuild and there is no runtime locale switching — Tasker
     switches locale at runtime via URL navigation.
   - **FormatJS/react-intl** rejected: no built-in lazy loading, heavier
     setup, and the message-authoring ergonomics are weaker for a codebase
     this size.
2. **No `i18next-icu`.** ICU MessageFormat would add ~20 kB gzip of parser to
   the eager path for capabilities Tasker does not need: the product's
   strings have no gender and no nested selectors. i18next's built-in
   pluralization (Intl.PluralRules-backed suffixes: `one`/`other` for en,
   `one`/`many`/`other` for fr, `other`-only for fa — Persian has no
   plural distinction for count nouns) covers every plural in the app.
   Bundle evidence: the full i18n runtime lands at **~10 kB gzip**; ICU
   would triple it against a hard gate. This satisfies the "no new
   dependencies unless strictly necessary" constraint — the ICU plugin is
   not necessary and the evidence is recorded here.
3. **`i18next-resources-to-backend`** with dynamic `import()` template
   literals: `import(\`./locales/${lng}/${ns}.json\`)`. Vite turns each
   locale × namespace JSON into its own chunk. No `i18next-http-backend`
   (fetch-based loading would bypass Vite's hashing and cache-busting).
   New runtime dependency cost: ~1 kB gzip.
4. **`@radix-ui/react-direction`**: wraps the app in `DirectionProvider` so
   every Radix primitive (DropdownMenu, Select, Dialog, Popover) flips
   arrow-key navigation and popper collision handling in RTL. Already present
   transitively under the installed Radix primitives; now declared as a
   direct dependency because Tasker imports it directly.
5. **URL as source of truth.** The locale is the first path segment
   (`/fa/...`, `/en/...`, `/fr/...`). Implementation: React Router
   `basename = /{locale}`, so every existing `navigate()`/`<Link>` keeps its
   internal path and gains the prefix automatically. Detection order:
   URL segment → `localStorage["tasker:locale"]` → `navigator.language` →
   `en`. First visit with no prefix redirects (replace) to the detected
   locale; explicit switches persist to localStorage and navigate to the
   same route under the new prefix (router remount keyed by locale; the
   QueryClient cache and auth state live above the router and survive).
   `<html lang>` and `<html dir>` are synced by a bootstrap script before
   first paint and by an effect on switch.
6. **Lazy-loading strategy.** Six namespaces (`common`, `tasks`, `settings`,
   `errors`, `notifications`, `goals`). On startup the active locale's
   `common` namespace is loaded before first render; every other
   namespace × locale is fetched on demand when a route mounts. Locale JSON
   is never part of `index.js` — verified by string-grep on the built chunk.

## Consequences

- The eager gate is preserved by keeping the i18n runtime (~10 kB gzip)
  inside `index.js` and extracting an equivalent amount of pre-existing
  shell code into a parallel-loaded `shell` chunk (measured in §Bundle of
  the Phase 15 report; the full initial-load payload is reported alongside
  the gate metric).
- Adding a locale = 6 JSON files + one line in the registry; RTL locales
  additionally need a `dir` entry (the RTL machinery is generic).
- Plural coverage is i18next-native; if true ICU (gender, selects with
  nesting) is ever needed, `i18next-icu` can be added later without
  touching message keys.
