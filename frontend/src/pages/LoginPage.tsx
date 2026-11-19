import { useRef, useState } from "react";
import { Navigate, useNavigate } from "react-router-dom";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ClipboardList, ShieldCheck } from "lucide-react";
import { api, ApiError } from "@/api/client";
import { useAuth } from "@/lib/auth";
import { Button } from "@/components/ui/button";
import { Input, Label } from "@/components/ui/input";
import { useToast } from "@/components/ui/toast";

// Validation messages live in the common namespace and are resolved through
// t() at render time (zod messages are stored keys, not literals).
const loginSchema = (t: (k: string) => string) =>
  z.object({
    email: z
      .string()
      .min(1, t("login.validation.emailRequired"))
      .email(t("login.validation.emailInvalid")),
    password: z.string().min(1, t("login.validation.passwordRequired")),
  });
type LoginValues = { email: string; password: string };

const mfaSchema = (t: (k: string) => string) =>
  z.object({
    code: z
      .string()
      .min(6, t("login.validation.codeShort"))
      .max(9, t("login.validation.codeLong")),
  });
type MfaValues = { code: string };

function LoginForm() {
  const navigate = useNavigate();
  const { t } = useTranslation("common");
  const qc = useQueryClient();
  const { push } = useToast();
  const [formError, setFormError] = useState<string | null>(null);
  // MFA-pending credential from the password step. Held only in component
  // state (never persisted) and cleared when consumed or abandoned.
  const [mfaToken, setMfaToken] = useState<string | null>(null);
  const [pendingEmail, setPendingEmail] = useState("");
  const mfaTokenRef = useRef<string | null>(null);
  // If a valid session cookie exists, skip the form (single auth source).
  const { user, isLoading } = useAuth();
  const {
    register,
    handleSubmit,
    formState: { errors, isSubmitting },
  } = useForm<LoginValues>({
    resolver: zodResolver(loginSchema((k) => t(k))),
    defaultValues: { email: "", password: "" },
  });
  const {
    register: registerMfa,
    handleSubmit: handleMfaSubmit,
    formState: { errors: mfaErrors, isSubmitting: mfaSubmitting },
  } = useForm<MfaValues>({
    resolver: zodResolver(mfaSchema((k) => t(k))),
    defaultValues: { code: "" },
  });

  if (!isLoading && user) {
    return <Navigate to="/" replace />;
  }

  const onLogin = async (values: LoginValues) => {
    setFormError(null);
    try {
      const result = await api.login(values.email, values.password);
      if ("mfa_required" in result && result.mfa_required) {
        // Two-factor user: hold the single-use pending token in memory only.
        setMfaToken(result.mfa_token);
        mfaTokenRef.current = result.mfa_token;
        setPendingEmail(values.email);
        return;
      }
      await qc.invalidateQueries();
      navigate("/", { replace: true });
    } catch (error) {
      if (error instanceof ApiError) {
        if (error.status === 401) {
          setFormError(t("login.error.badCredentials"));
        } else if (error.status === 429) {
          setFormError(t("login.error.rateLimited"));
        } else {
          setFormError(error.message);
        }
      } else {
        push("error", t("login.error.failed"), error instanceof Error ? error.message : undefined);
      }
    }
  };

  const onMfaVerify = async (values: MfaValues) => {
    setFormError(null);
    const token = mfaToken ?? mfaTokenRef.current;
    if (!token) {
      setMfaToken(null);
      mfaTokenRef.current = null;
      return;
    }
    try {
      await api.mfaVerify(token, values.code.trim());
      // The pending token is consumed server-side; drop it locally first so a
      // failed cache refresh can never resubmit it.
      setMfaToken(null);
      mfaTokenRef.current = null;
      await qc.invalidateQueries();
      navigate("/", { replace: true });
    } catch (error) {
      if (error instanceof ApiError) {
        if (error.status === 401) {
          setFormError(t("login.error.badCode"));
        } else if (error.status === 429) {
          setFormError(t("login.error.codeRateLimited"));
        } else {
          setFormError(error.message);
        }
      } else {
        push("error", t("login.error.verifyFailed"), error instanceof Error ? error.message : undefined);
      }
    }
  };

  const exitMfa = () => {
    // Abandon the pending credential: back to the password step.
    setMfaToken(null);
    mfaTokenRef.current = null;
    setFormError(null);
  };

  const cardClassName =
    "flex flex-col gap-4 rounded-xl border bg-card p-6 shadow-sm";
  const errorBanner = formError ? (
    <p
      className="rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive"
      role="alert"
      data-testid="login-error"
    >
      {formError}
    </p>
  ) : null;

  if (mfaToken) {
    return (
      <div className="flex min-h-screen items-center justify-center bg-background p-4">
        <div className="w-full max-w-sm">
          <div className="mb-8 flex flex-col items-center gap-2">
            <span className="flex h-11 w-11 items-center justify-center rounded-xl bg-primary text-primary-foreground shadow-sm">
              <ShieldCheck className="h-5 w-5" />
            </span>
            <h1 className="text-lg font-semibold">{t("login.mfa.title")}</h1>
            <p className="text-xs text-muted-foreground">
              {pendingEmail
                ? t("login.mfa.subtitle", { email: pendingEmail })
                : t("login.mfa.subtitlePlain")}
            </p>
          </div>
          <form
            onSubmit={handleMfaSubmit(onMfaVerify)}
            className={cardClassName}
            noValidate
            data-testid="mfa-verify-form"
          >
            {errorBanner}
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="mfa-login-code">{t("login.mfa.codeLabel")}</Label>
              <Input
                id="mfa-login-code"
                inputMode="numeric"
                dir="ltr"
                autoComplete="one-time-code"
                placeholder="000000 or XXXX-XXXX"
                aria-invalid={Boolean(mfaErrors.code)}
                className="font-mono tracking-widest"
                autoFocus
                data-testid="mfa-login-code"
                {...registerMfa("code")}
              />
              {mfaErrors.code ? (
                <p className="text-2xs text-destructive" role="alert">
                  {mfaErrors.code.message}
                </p>
              ) : null}
            </div>
            <Button type="submit" disabled={mfaSubmitting} data-testid="mfa-verify-submit">
              {mfaSubmitting ? t("login.mfa.verifying") : t("login.mfa.verify")}
            </Button>
            <button
              type="button"
              className="text-2xs text-muted-foreground underline underline-offset-2"
              onClick={exitMfa}
              data-testid="mfa-back"
            >
              {t("login.mfa.useOtherAccount")}
            </button>
          </form>
        </div>
      </div>
    );
  }

  return (
    <div className="flex min-h-screen items-center justify-center bg-background p-4">
      <div className="w-full max-w-sm">
        <div className="mb-8 flex flex-col items-center gap-2">
          <span className="flex h-11 w-11 items-center justify-center rounded-xl bg-primary text-primary-foreground shadow-sm">
            <ClipboardList className="h-5 w-5" />
          </span>
          <h1 className="text-lg font-semibold">{t("login.title")}</h1>
          <p className="text-xs text-muted-foreground">{t("login.subtitle")}</p>
        </div>
        <form
          onSubmit={handleSubmit(onLogin)}
          className={cardClassName}
          noValidate
          data-testid="login-form"
        >
          {errorBanner}
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="email">{t("login.email")}</Label>
            <Input
              id="email"
              type="email"
              dir="ltr"
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
            <Label htmlFor="password">{t("login.password")}</Label>
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
            {isSubmitting ? t("login.signingIn") : t("login.signIn")}
          </Button>
        </form>
        <p className="mt-4 text-center text-2xs text-muted-foreground">
          {t("login.noAccount")}
        </p>
      </div>
    </div>
  );
}

export function LoginPage() {
  return <LoginForm />;
}
