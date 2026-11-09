/**
 * Cross-module shortcut publishers (Phase 13).
 *
 * The lazy `shortcuts.tsx` chunk reads these refs; eager shell code
 * (AppShell, ProjectPage) writes them. Keeping the setters in this tiny
 * module keeps `shortcuts.tsx` itself out of the eager bundle — anything
 * that statically imported shortcuts.tsx for a setter would have pulled
 * the whole manager back into index.
 */

// Phase 13 Bug 1: the quick view publishes its full-page opener here.
const openFullPageRef: { current: (() => void) | null } = { current: null };
export function setOpenFullPageHandler(fn: (() => void) | null) {
  openFullPageRef.current = fn;
}

// Phase 13 Bug 5: ⌘/Ctrl+, opens Settings (published by the shell).
const openSettingsRef: { current: (() => void) | null } = { current: null };
export function setOpenSettingsHandler(fn: (() => void) | null) {
  openSettingsRef.current = fn;
}

/** Internal: read by the lazy shortcuts chunk. */
export function getFullPageHandler(): (() => void) | null {
  return openFullPageRef.current;
}

/** Internal: read by the lazy shortcuts chunk. */
export function getSettingsHandler(): (() => void) | null {
  return openSettingsRef.current;
}
