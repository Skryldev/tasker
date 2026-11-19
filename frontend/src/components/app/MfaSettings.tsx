import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { QRCodeSVG } from "qrcode.react";
import { KeyRound, ShieldCheck, ShieldOff } from "lucide-react";
import { api, ApiError } from "@/api/client";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Input, Label } from "@/components/ui/input";
import { useToast } from "@/components/ui/toast";
import { cn } from "@/lib/utils";

/**
 * Two-factor authentication section for the account page. The QR code is
 * rendered locally from the provisioning URI — nothing leaves the browser.
 * Recovery codes are shown exactly once, held only in component state (never
 * persisted), and re-authentication with the current password guards
 * disable/regenerate. QR + Doran-free path: this component is imported by the
 * lazily routed AccountPage, so qrcode.react never reaches the initial chunk.
 */

type SetupState = { uri: string; secret: string } | null;

function formatCode(value: string): string {
  const digits = value.replace(/\D/g, "").slice(0, 6);
  return digits;
}

export function MfaSettings({ enabled }: { enabled: boolean }) {
  const { t } = useTranslation("settings");
  const { push } = useToast();
  const qc = useQueryClient();
  const [setup, setSetup] = useState<SetupState>(null);
  const [manualEntry, setManualEntry] = useState(false);
  const [code, setCode] = useState("");
  const [recoveryCodes, setRecoveryCodes] = useState<string[] | null>(null);
  const [acknowledged, setAcknowledged] = useState(false);
  const [password, setPassword] = useState("");
  const [disableOpen, setDisableOpen] = useState(false);
  const [regenOpen, setRegenOpen] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);

  const resetSetup = () => {
    setSetup(null);
    setManualEntry(false);
    setCode("");
    setFormError(null);
  };

  const startSetup = useMutation({
    mutationFn: () => api.mfaSetup(),
    onSuccess: (data) => {
      setSetup({ uri: data.uri, secret: data.secret });
      setFormError(null);
    },
    onError: (err) =>
      push("error", t("mfa.errors.start"), err instanceof ApiError ? err.message : undefined),
  });

  const enable = useMutation({
    mutationFn: () => api.mfaEnable(formatCode(code)),
    onSuccess: (data) => {
      setRecoveryCodes(data.recovery_codes);
      setAcknowledged(false);
      resetSetup();
      void qc.invalidateQueries({ queryKey: ["me"] });
    },
    onError: (err) =>
      setFormError(
        err instanceof ApiError && err.status === 401
          ? t("mfa.errors.invalidCode")
          : err.message,
      ),
  });

  const disable = useMutation({
    mutationFn: () => api.mfaDisable(password),
    onSuccess: () => {
      push("success", t("mfa.disabledToast"));
      setPassword("");
      setDisableOpen(false);
      void qc.invalidateQueries({ queryKey: ["me"] });
    },
    onError: (err) =>
      setFormError(err instanceof ApiError ? err.message : t("mfa.errors.disable")),
  });

  const regenerate = useMutation({
    mutationFn: () => api.mfaRecoveryCodes(password),
    onSuccess: (data) => {
      setRecoveryCodes(data.recovery_codes);
      setAcknowledged(false);
      setPassword("");
      setRegenOpen(false);
      setFormError(null);
    },
    onError: (err) =>
      setFormError(err instanceof ApiError ? err.message : t("mfa.errors.regenerate")),
  });

  return (
    <section aria-label={t("mfa.title")} className="rounded-sg border p-4">
      <h2 className="mb-1 flex items-center gap-1.5 text-sm font-semibold">
        <ShieldCheck className="h-4 w-4" /> {t("mfa.title")}
      </h2>
      <p className="mb-3 text-xs text-muted-foreground">
        {enabled
          ? t("mfa.subtitleEnabled")
          : t("mfa.subtitleDisabled")}
      </p>

      {enabled && !recoveryCodes ? (
        <div className="flex flex-col gap-2" data-testid="mfa-enabled">
          <div className="flex flex-wrap gap-2">
            <Button size="sm" variant="outline" onClick={() => { setFormError(null); setRegenOpen(true); }} data-testid="mfa-regenerate">
              <KeyRound /> {t("mfa.newRecoveryCodes")}
            </Button>
            <Button
              size="sm"
              variant="ghost"
              className="text-destructive hover:bg-destructive/10 hover:text-destructive"
              onClick={() => { setFormError(null); setDisableOpen(true); }}
              data-testid="mfa-disable"
            >
              <ShieldOff /> {t("mfa.disable2fa")}
            </Button>
          </div>
        </div>
      ) : null}

      {!enabled && !setup && !recoveryCodes ? (
        <Button
          size="sm"
          onClick={() => startSetup.mutate()}
          disabled={startSetup.isPending}
          data-testid="mfa-start"
        >
          {startSetup.isPending ? t("mfa.preparing") : t("mfa.enable2fa")}
        </Button>
      ) : null}

      {setup ? (
        <div className="flex flex-col gap-3" data-testid="mfa-setup">
          <div className="flex flex-col items-start gap-3 sm:flex-row sm:items-center">
            <div className="rounded-sg border bg-white p-2" aria-hidden>
              <QRCodeSVG value={setup.uri} size={148} level="M" data-testid="mfa-qr" />
            </div>
            <div className="min-w-0 flex-1 text-xs">
              <p className="font-medium">{t("mfa.scanTitle")}</p>
              <p className="mt-1 text-muted-foreground">
                {t("mfa.manualEntry")}
                <button
                  type="button"
                  className="ms-1 underline underline-offset-2"
                  onClick={() => setManualEntry((v) => !v)}
                >
                  {manualEntry ? t("mfa.hide") : t("mfa.show")}
                </button>
              </p>
              {manualEntry ? (
                <code
                  className="mt-1 block break-all rounded bg-muted px-1.5 py-0.5 font-mono text-2xs"
                  data-testid="mfa-secret"
                >
                  {setup.secret}
                </code>
              ) : null}
            </div>
          </div>
          <form
            className="flex max-w-xs flex-col gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              if (formatCode(code).length !== 6) {
                setFormError(t("mfa.errors.enterCode"));
                return;
              }
              enable.mutate();
            }}
            noValidate
          >
            <Label htmlFor="mfa-code">{t("mfa.codeLabel")}</Label>
            <Input
              id="mfa-code"
              inputMode="numeric"
              autoComplete="one-time-code"
              value={code}
              onChange={(e) => setCode(formatCode(e.target.value))}
              placeholder="000000"
              className="w-32 font-mono tracking-widest"
              autoFocus
              data-testid="mfa-code-input"
            />
            {formError ? (
              <p className="text-2xs text-destructive" role="alert">
                {formError}
              </p>
            ) : null}
            <div className="flex gap-2">
              <Button
                type="submit"
                size="sm"
                disabled={formatCode(code).length !== 6 || enable.isPending}
                data-testid="mfa-enable-submit"
              >
                {enable.isPending ? t("mfa.verifying") : t("mfa.verifyEnable")}
              </Button>
              <Button type="button" size="sm" variant="ghost" onClick={resetSetup}>
                {t("actions.cancel")}
              </Button>
            </div>
          </form>
        </div>
      ) : null}

      {recoveryCodes ? (
        <div className="flex flex-col gap-3" data-testid="mfa-recovery">
          <div
            className="rounded-md border border-warning/40 bg-warning/5 p-3 text-xs"
            role="alert"
          >
            <p className="font-medium text-warning-foreground">
              {t("mfa.recoveryWarning")}
            </p>
            <p className="mt-1 text-muted-foreground">
              {t("mfa.recoveryInfo")}
            </p>
          </div>
          <ul
            className={cn(
              "grid w-fit grid-cols-2 gap-x-6 gap-y-1 rounded-md border bg-muted/40 px-4 py-3",
              "font-mono text-sm",
            )}
          >
            {recoveryCodes.map((c) => (
              <li key={c} data-testid="mfa-recovery-code">
                {c}
              </li>
            ))}
          </ul>
          <label className="flex max-w-md items-start gap-2 text-xs text-muted-foreground">
            <input
              type="checkbox"
              checked={acknowledged}
              onChange={(e) => setAcknowledged(e.target.checked)}
              className="mt-0.5 h-3.5 w-3.5 accent-[hsl(var(--primary))]"
              data-testid="mfa-recovery-ack"
            />
            {t("mfa.ackLabel")}
          </label>
          <Button
            size="sm"
            className="self-start"
            disabled={!acknowledged}
            onClick={() => setRecoveryCodes(null)}
            data-testid="mfa-recovery-done"
          >
            {t("mfa.ackButton")}
          </Button>
        </div>
      ) : null}

      {/* Password re-authentication dialogs */}
      <ConfirmDialog
        open={disableOpen}
        onOpenChange={setDisableOpen}
        title={t("mfa.disableConfirm.title")}
        description={t("mfa.disableConfirm.description")}
        confirmLabel={t("mfa.disable2fa")}
        pending={disable.isPending}
        onConfirm={() => disable.mutate()}
      >
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="mfa-disable-password">{t("password.current")}</Label>
          <Input
            id="mfa-disable-password"
            type="password"
            autoComplete="current-password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoFocus
            data-testid="mfa-disable-password"
          />
          {formError ? (
            <p className="text-2xs text-destructive" role="alert">
              {formError}
            </p>
          ) : null}
        </div>
      </ConfirmDialog>
      <ConfirmDialog
        open={regenOpen}
        onOpenChange={setRegenOpen}
        title={t("mfa.regenConfirm.title")}
        description={t("mfa.regenConfirm.description")}
        confirmLabel={t("mfa.regenConfirm.confirm")}
        destructive={false}
        pending={regenerate.isPending}
        onConfirm={() => regenerate.mutate()}
      >
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="mfa-regen-password">{t("password.current")}</Label>
          <Input
            id="mfa-regen-password"
            type="password"
            autoComplete="current-password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoFocus
            data-testid="mfa-regen-password"
          />
          {formError ? (
            <p className="text-2xs text-destructive" role="alert">
              {formError}
            </p>
          ) : null}
        </div>
      </ConfirmDialog>
    </section>
  );
}
