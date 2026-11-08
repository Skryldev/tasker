import React from "react";

/**
 * Avatar cache-busting (Phase 12.5 Bug 1).
 *
 * Root cause: `GET /users/{id}/avatar` is served with
 * `Cache-Control: private, max-age=300` at a *stable* URL, so after an
 * upload every surface kept rendering the browser-cached old image until
 * the entry expired.
 *
 * Fix: a per-user version counter. Every user-avatar URL carries `?v=`
 * once a version exists; a successful upload bumps the version, which
 * changes the URL, which forces a fresh fetch. The versions map is
 * persisted to localStorage so a second tab picks the bump up through the
 * `storage` event (documented cross-tab choice: storage event, not polling
 * — it is instant, free, and needs no server round-trip).
 */

const STORAGE_KEY = "tasker.avatarVersions";

type Versions = Record<string, number>;

const listeners = new Set<() => void>();

function readStorage(): Versions {
  try {
    return JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "{}") as Versions;
  } catch {
    return {};
  }
}

function writeStorage(versions: Versions): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(versions));
  } catch {
    // Private-mode quota errors: in-tab busting still works.
  }
}

let versions: Versions = readStorage();

function notify(): void {
  for (const listener of listeners) listener();
}

if (typeof window !== "undefined") {
  // Another tab bumped a version: rehydrate and re-render subscribed avatars.
  window.addEventListener("storage", (event) => {
    if (event.key !== STORAGE_KEY) return;
    versions = readStorage();
    notify();
  });
}

/** Monotonic per-user version; 0 means "no bump known — plain URL". */
export function getAvatarVersion(userId: string): number {
  return versions[userId] ?? 0;
}

/** Called after a successful avatar upload (or cross-tab sync). */
export function setAvatarVersion(userId: string, version: number): void {
  const current = versions[userId] ?? 0;
  // Never go backwards: two uploads in a row must each produce a new URL.
  versions = { ...versions, [userId]: Math.max(current + 1, version) };
  writeStorage(versions);
  notify();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** `/api/v1/users/{id}/avatar` with the cache-busting `?v=` when known. */
export function avatarUrl(userId: string): string {
  const base = `/api/v1/users/${userId}/avatar`;
  const version = getAvatarVersion(userId);
  return version > 0 ? `${base}?v=${version}` : base;
}

/**
 * Resolves a user-avatar URL reactively: swaps a plain user-avatar path for
 * its versioned form and re-renders when the version changes (same tab via
 * the store, other tabs via the storage event). Non-avatar URLs pass
 * through untouched, so every existing Avatar call site benefits without
 * modification.
 */
export function useAvatarSrc(src: string | undefined): string | undefined {
  const match = src?.match(/^\/api\/v1\/users\/([^/?]+)\/avatar$/);
  const userId = match?.[1];
  const version = React.useSyncExternalStore(
    subscribe,
    () => (userId ? getAvatarVersion(userId) : 0),
    () => 0,
  );
  if (!userId) return src;
  return version > 0 ? `${src}?v=${version}` : src;
}
