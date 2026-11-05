import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { useNavigate } from "react-router-dom";
import { useMutation, useQueryClient, type QueryClient } from "@tanstack/react-query";
import type { User } from "@/api/client";
import { api, ApiError } from "@/api/client";
import { useMe } from "@/api/hooks";

interface AuthContextValue {
  user: User | undefined;
  isLoading: boolean;
}

const AuthContext = createContext<AuthContextValue>({ user: undefined, isLoading: true });

// ---------------------------------------------------------------------------
// Signed-out signal
//
// `queryClient.clear()` removes cached queries but does NOT update the
// observers that are still mounted: the `me` observer inside AuthProvider
// keeps serving the last cached user forever (AuthProvider sits above the
// router, so navigation never re-renders it). Without an explicit signal the
// login page sees a stale truthy user, bounces back to "/", and every query
// refetch lands on a 401 "Try again" state — exactly the reported logout bug.
//
// The flag also guards the global 401 handler below: clearing the cache on
// EVERY 401 would re-create the mounted `me` query on each re-render and
// drive an endless refetch loop, so the cache teardown happens once, on the
// transition into the signed-out state.
// ---------------------------------------------------------------------------

let signedOut = false;
const signedOutListeners = new Set<() => void>();

/** True once the session has been torn down locally (logout or a 401). */
export function isSignedOut(): boolean {
  return signedOut;
}

/** Mark the session as gone and notify the auth context. */
export function emitSignedOut(): void {
  signedOut = true;
  for (const listener of signedOutListeners) listener();
}

/** Clear the signed-out state (a fresh session has been established). */
export function resetSignedOut(): void {
  signedOut = false;
}

function subscribeSignedOut(listener: () => void): () => void {
  signedOutListeners.add(listener);
  return () => {
    signedOutListeners.delete(listener);
  };
}

export function AuthProvider({ children }: { children: ReactNode }) {
  const { data: user, isLoading, dataUpdatedAt } = useMe();
  // Once the server rejects the session, the last cached user must never be
  // trusted again — the observer may still hold it after `queryClient.clear()`.
  // The mask lifts only when a NEWER successful /me fetch arrives (login flow),
  // detected via dataUpdatedAt so a cached-object identity can never unmask it.
  const [tornDownAt, setTornDownAt] = useState<number | null>(isSignedOut() ? Date.now() : null);

  useEffect(
    () =>
      subscribeSignedOut(() => {
        // Always advance the timestamp: a second teardown (logout after a
        // re-login) must re-arm the mask even though tornDownAt was already
        // set by an earlier 401.
        setTornDownAt(Date.now());
      }),
    [],
  );

  const forcedSignOut = tornDownAt !== null && !(user && dataUpdatedAt >= tornDownAt);

  // A new session re-arms the global 401 teardown (cache clear on first 401).
  useEffect(() => {
    if (!forcedSignOut) resetSignedOut();
  }, [forcedSignOut]);

  const value = useMemo<AuthContextValue>(
    () => ({ user: forcedSignOut ? undefined : user, isLoading }),
    [user, isLoading, forcedSignOut],
  );

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth(): AuthContextValue {
  return useContext(AuthContext);
}

/**
 * The profile timezone for displaying absolute instants. Calendar days (due
 * dates) stay anchored to the product's Persian-calendar policy (see
 * lib/doran.ts); only instant rendering honors this value.
 */
export function useUserTimezone(): string | undefined {
  return useContext(AuthContext).user?.timezone ?? undefined;
}

/**
 * Global query-failure handler: a 401 means the session is gone — tear down
 * all cached server state and land on the login screen. Non-401 errors (5xx,
 * network, validation) are left to the caller's error UI and never redirect.
 *
 * Wired as `QueryCache.onError` in `main.tsx` and unit-tested directly.
 */
export function handleUnauthorizedError(
  error: unknown,
  queryClient: QueryClient,
  navigate: (path: string) => void,
): void {
  if (!(error instanceof ApiError) || error.status !== 401) return;
  // First 401 of a dead session: clear the cache exactly once. Repeat
  // 401s only navigate — re-clearing would rebuild mounted queries and
  // refetch-loop (see the signed-out signal above).
  if (!signedOut) {
    queryClient.clear();
  }
  emitSignedOut();
  navigate("/login");
}

/** Log out and reset the client cache. */
export function useLogout() {
  const navigate = useNavigate();
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => api.logout(),
    onSuccess: () => {
      qc.clear();
      // Drop the cached user from the auth context BEFORE the redirect, so
      // the login page cannot bounce back to "/" on a stale value.
      emitSignedOut();
      navigate("/login", { replace: true });
    },
  });
}
