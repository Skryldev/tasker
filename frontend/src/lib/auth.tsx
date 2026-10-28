import { createContext, useContext, type ReactNode } from "react";
import { useNavigate } from "react-router-dom";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { User } from "@/api/client";
import { api } from "@/api/client";
import { useMe } from "@/api/hooks";

interface AuthContextValue {
  user: User | undefined;
  isLoading: boolean;
}

const AuthContext = createContext<AuthContextValue>({ user: undefined, isLoading: true });

export function AuthProvider({ children }: { children: ReactNode }) {
  const { data: user, isLoading } = useMe();
  return (
    <AuthContext.Provider value={{ user, isLoading }}>{children}</AuthContext.Provider>
  );
}

export function useAuth(): AuthContextValue {
  return useContext(AuthContext);
}

/** Log out and reset the client cache. */
export function useLogout() {
  const navigate = useNavigate();
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => api.logout(),
    onSuccess: () => {
      qc.clear();
      navigate("/login", { replace: true });
    },
  });
}
