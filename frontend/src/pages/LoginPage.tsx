import { useState } from "react";
import { Navigate, useNavigate } from "react-router-dom";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { useQueryClient } from "@tanstack/react-query";
import { ClipboardList } from "lucide-react";
import { api, ApiError } from "@/api/client";
import { useAuth } from "@/lib/auth";
import { Button } from "@/components/ui/button";
import { Input, Label } from "@/components/ui/input";
import { useToast } from "@/components/ui/toast";

const loginSchema = z.object({
  email: z.string().min(1, "Email is required").email("Enter a valid email address"),
  password: z.string().min(1, "Password is required"),
});
type LoginValues = z.infer<typeof loginSchema>;

function LoginForm() {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { push } = useToast();
  const [formError, setFormError] = useState<string | null>(null);
  // If a valid session cookie exists, skip the form (single auth source).
  const { user, isLoading } = useAuth();
  const {
    register,
    handleSubmit,
    formState: { errors, isSubmitting },
  } = useForm<LoginValues>({
    resolver: zodResolver(loginSchema),
    defaultValues: { email: "", password: "" },
  });

  if (!isLoading && user) {
    return <Navigate to="/" replace />;
  }

  const onSubmit = async (values: LoginValues) => {
    setFormError(null);
    try {
      await api.login(values.email, values.password);
      await qc.invalidateQueries();
      navigate("/", { replace: true });
    } catch (error) {
      if (error instanceof ApiError) {
        if (error.status === 401) {
          setFormError("Incorrect email or password.");
        } else if (error.status === 429) {
          setFormError("Too many attempts. Please wait a minute and try again.");
        } else {
          setFormError(error.message);
        }
      } else {
        push("error", "Sign-in failed", error instanceof Error ? error.message : undefined);
      }
    }
  };

  return (
    <div className="flex min-h-screen items-center justify-center bg-background p-4">
      <div className="w-full max-w-sm">
        <div className="mb-8 flex flex-col items-center gap-2">
          <span className="flex h-11 w-11 items-center justify-center rounded-xl bg-primary text-primary-foreground shadow-sm">
            <ClipboardList className="h-5 w-5" />
          </span>
          <h1 className="text-lg font-semibold">Sign in to Tasker</h1>
          <p className="text-xs text-muted-foreground">Self-hosted team work management</p>
        </div>
        <form
          onSubmit={handleSubmit(onSubmit)}
          className="flex flex-col gap-4 rounded-xl border bg-card p-6 shadow-sm"
          noValidate
          data-testid="login-form"
        >
          {formError ? (
            <p className="rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive" role="alert" data-testid="login-error">
              {formError}
            </p>
          ) : null}
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="email">Email</Label>
            <Input
              id="email"
              type="email"
              autoComplete="email"
              placeholder="you@company.com"
              aria-invalid={Boolean(errors.email)}
              {...register("email")}
            />
            {errors.email ? (
              <p className="text-2xs text-destructive" role="alert">{errors.email.message}</p>
            ) : null}
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="password">Password</Label>
            <Input
              id="password"
              type="password"
              autoComplete="current-password"
              placeholder="••••••••••"
              aria-invalid={Boolean(errors.password)}
              {...register("password")}
            />
            {errors.password ? (
              <p className="text-2xs text-destructive" role="alert">{errors.password.message}</p>
            ) : null}
          </div>
          <Button type="submit" disabled={isSubmitting} data-testid="login-submit">
            {isSubmitting ? "Signing in…" : "Sign in"}
          </Button>
        </form>
        <p className="mt-4 text-center text-2xs text-muted-foreground">
          No account? Your administrator can add you from Team Settings.
        </p>
      </div>
    </div>
  );
}

export function LoginPage() {
  return <LoginForm />;
}
