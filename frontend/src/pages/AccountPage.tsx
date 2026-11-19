import { useEffect, useRef, useState } from "react";
import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { useMutation } from "@tanstack/react-query";
import { Check, KeyRound, Loader2, MonitorSmartphone } from "lucide-react";
import {
  api,
  ApiError,
  type NotificationPrefs,
  type SessionInfo,
  type User,
} from "@/api/client";
import {
  useMySummary,
  useRevokeSession,
  useSessions,
  useUpdateNotificationPrefs,
  useUpdateProfile,
} from "@/api/hooks";
import { Avatar, Separator } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import { Input, Label } from "@/components/ui/input";
import { TimezoneSelect } from "@/components/app/TimezoneSelect";
import { strengthLabel, strengthTone } from "@/lib/password-strength";
import { passwordStrength } from "@/lib/password-strength";
import { useToast } from "@/components/ui/toast";
import { useAuth, useLogout } from "@/lib/auth";
import { fileToAvatarDataUrl, AvatarError } from "@/lib/avatar";
import { STATUS_LABELS, TASK_STATUSES, timeAgo } from "@/lib/utils";
import { MfaSettings } from "@/components/app/MfaSettings";

/** The six notification events a user can toggle (contract vocabulary). */
const PREF_EVENTS: { key: keyof NotificationPrefs; labelKey: string; hintKey: string }[] = [
  { key: "task_assigned", labelKey: "prefs.task_assigned.label", hintKey: "prefs.task_assigned.hint" },
  { key: "comment_mention", labelKey: "prefs.comment_mention.label", hintKey: "prefs.comment_mention.hint" },
  { key: "task_submitted", labelKey: "prefs.task_submitted.label", hintKey: "prefs.task_submitted.hint" },
  { key: "task_accepted", labelKey: "prefs.task_accepted.label", hintKey: "prefs.task_accepted.hint" },
  { key: "task_rejected", labelKey: "prefs.task_rejected.label", hintKey: "prefs.task_rejected.hint" },
  { key: "task_status_changed", labelKey: "prefs.task_status_changed.label", hintKey: "prefs.task_status_changed.hint" },
];

/** Contract default: every event enabled (pre-Phase-9 behavior). */
const DEFAULT_PREFS: NotificationPrefs = {
  task_assigned: true,
  comment_mention: true,
  task_submitted: true,
  task_accepted: true,
  task_rejected: true,
  task_status_changed: true,
};

function fieldError(error: unknown, field: string): string | undefined {
  if (error instanceof ApiError) {
    const messages = error.fieldMessages();
    if (messages[field]) return messages[field];
  }
  return undefined;
}

export function AccountPage() {
  const { t } = useTranslation("settings");
  const { user } = useAuth();
  const { push } = useToast();
  const [currentPassword, setCurrentPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [formError, setFormError] = useState<string | null>(null);

  const change = useMutation({
    mutationFn: () => api.changePassword(currentPassword, newPassword),
    onSuccess: () => {
      push("success", t("password.changedTitle"), t("password.changedBody"));
      setCurrentPassword("");
      setNewPassword("");
      setFormError(null);
    },
    onError: (err) =>
      setFormError(err instanceof ApiError ? err.message : t("password.errors.change")),
  });

  return (
    <div className="mx-auto max-w-2xl p-6">
      <h1 className="mb-1 text-lg font-semibold">{t("profile.title")}</h1>
      <p className="mb-6 text-xs text-muted-foreground">
        {t("profile.subtitle")}
      </p>

      {user ? <WorkIdentitySection user={user} /> : null}

      <Separator className="my-8" />

      {user ? (
        <NotificationPrefsSection
          prefs={user.notification_prefs ?? DEFAULT_PREFS}
        />
      ) : null}

      <Separator className="my-8" />

      <MyWorkSummarySection />

      <Separator className="my-8" />

      <section id="security" aria-label={t("sections.security")} className="mb-8 scroll-mt-16">
        <h2 className="mb-3 text-sm font-semibold">{t("sections.security")}</h2>
        <div className="mb-6">
          <MfaSettings enabled={Boolean(user?.mfa_enabled)} />
        </div>
        <Separator className="mb-6" />
        <SessionsSection />
        <Separator className="my-6" />
        <form
          className="flex max-w-sm flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            change.mutate();
          }}
          noValidate
          data-testid="change-password-form"
        >
          <h3 className="flex items-center gap-1.5 text-sm font-semibold">
            <KeyRound className="h-4 w-4" /> {t("password.change")}
          </h3>
          <p className="text-xs text-muted-foreground">
            {t("password.changeHint")}
          </p>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="current-password">{t("password.current")}</Label>
            <Input
              id="current-password"
              type="password"
              autoComplete="current-password"
              value={currentPassword}
              onChange={(e) => setCurrentPassword(e.target.value)}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="new-password">{t("password.new")}</Label>
            <Input
              id="new-password"
              type="password"
              autoComplete="new-password"
              value={newPassword}
              onChange={(e) => setNewPassword(e.target.value)}
            />
            {newPassword ? (
              <PasswordStrengthMeter password={newPassword} />
            ) : null}
            <p className="text-2xs text-muted-foreground">
              {t("password.rules")}
            </p>
          </div>
          {formError ? <p className="text-2xs text-destructive" role="alert">{formError}</p> : null}
          <Button
            type="submit"
            size="sm"
            className="self-start"
            disabled={!currentPassword || !newPassword || change.isPending}
            data-testid="change-password-submit"
          >
            {change.isPending ? t("actions.saving") : t("password.change")}
          </Button>
        </form>
      </section>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Work identity
// ---------------------------------------------------------------------------

function WorkIdentitySection({ user }: { user: User }) {
  const { t } = useTranslation("settings");
  const { push } = useToast();
  const fileInput = useRef<HTMLInputElement>(null);

  const [displayName, setDisplayName] = useState(user.display_name);
  const [username, setUsername] = useState(user.username ?? "");
  const [bio, setBio] = useState(user.bio ?? "");
  const [timezone, setTimezone] = useState(user.timezone ?? "none");
  const [capacity, setCapacity] = useState(String(user.capacity_limit ?? 10));
  const [localError, setLocalError] = useState<string | null>(null);

  const update = useUpdateProfile();

  const save = () => {
    setLocalError(null);
    const usernameTrimmed = username.trim();
    if (usernameTrimmed && !/^[a-z0-9_]{3,32}$/.test(usernameTrimmed)) {
      setLocalError(t("profile.errors.usernameFormat"));
      return;
    }
    if (bio.length > 200) {
      setLocalError(t("profile.errors.bioTooLong"));
      return;
    }
    update.mutate(
      {
        display_name: displayName.trim(),
        username: usernameTrimmed || null,
        bio: bio.trim() || null,
        timezone: timezone === "none" ? null : timezone,
        capacity_limit: Number(capacity) || undefined,
      },
      {
        onSuccess: () => push("success", t("profile.saved")),
        onError: (error) => {
          const message =
            error instanceof ApiError ? error.message : t("profile.errors.save");
          setLocalError(message);
        },
      },
    );
  };

  const onPickFile = async (file: File | undefined) => {
    if (!file) return;
    setLocalError(null);
    try {
      const dataUrl = await fileToAvatarDataUrl(file);
      update.mutate(
        { avatar: dataUrl },
        {
          onSuccess: () => push("success", t("profile.avatar.updated")),
          onError: (error) =>
            setLocalError(
              error instanceof ApiError ? error.message : t("avatar.errors.upload"),
            ),
        },
      );
    } catch (error) {
      setLocalError(error instanceof AvatarError ? error.message : t("avatar.errors.read"));
    }
  };

  const usernameError = fieldError(update.error, "username");
  const bioError = fieldError(update.error, "bio");
  const timezoneError = fieldError(update.error, "timezone");
  return (
    <section aria-label={t("profile.workIdentity")}>
      <h2 className="mb-3 text-sm font-semibold">{t("profile.workIdentity")}</h2>
      <form
        className="flex flex-col gap-4"
        onSubmit={(e) => {
          e.preventDefault();
          save();
        }}
        noValidate
        data-testid="profile-identity-form"
      >
        <div className="flex items-center gap-4">
          <Avatar
            name={user.display_name}
            size="md"
            className="h-16 w-16 rounded-full text-lg"
            src={api.userAvatarUrl(user.id)}
          />
          <div className="flex flex-col gap-1">
            <Label htmlFor="avatar-upload" className="text-xs">
              {t("profile.avatar.label")}
            </Label>
            <input
              id="avatar-upload"
              ref={fileInput}
              type="file"
              accept="image/jpeg,image/png,image/webp"
              className="hidden"
              onChange={(e) => {
                void onPickFile(e.target.files?.[0]);
                e.target.value = "";
              }}
            />
            <div className="flex gap-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => fileInput.current?.click()}
                data-testid="avatar-pick"
              >
                {t("actions.upload")}
              </Button>
              {user.id ? (
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  disabled={update.isPending}
                  onClick={() => update.mutate({ avatar: null }, { onSuccess: () => push("success", t("profile.avatar.removed")) })}
                >
                  {t("actions.remove")}
                </Button>
              ) : null}
            </div>
          </div>
        </div>

        <div className="grid grid-cols-2 gap-3">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="display-name">{t("profile.displayName")}</Label>
            <Input
              id="display-name"
              value={displayName}
              onChange={(e) => setDisplayName(e.target.value)}
              className="max-w-xs"
            />
            {fieldError(update.error, "display_name") ? (
              <p className="text-2xs text-destructive" role="alert">
                {fieldError(update.error, "display_name")}
              </p>
            ) : null}
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="username">{t("profile.username")}</Label>
            <Input
              id="username"
              value={username}
              onChange={(e) => setUsername(e.target.value.toLowerCase())}
              placeholder="ada_lovelace"
              className="max-w-xs font-mono"
              aria-invalid={Boolean(usernameError)}
              data-testid="username-input"
            />
            <p className="text-2xs text-muted-foreground">
              {t("profile.usernameHint")}
            </p>
            {usernameError ? (
              <p className="text-2xs text-destructive" role="alert">{usernameError}</p>
            ) : null}
          </div>
        </div>

        <div className="flex flex-col gap-1.5">
          <Label htmlFor="bio">{t("profile.bio")}</Label>
          <textarea
            id="bio"
            value={bio}
            maxLength={200}
            rows={2}
            onChange={(e) => setBio(e.target.value)}
            className="max-w-md rounded-md border border-input bg-background px-3 py-2 text-sm shadow-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            aria-invalid={Boolean(bioError)}
            data-testid="bio-input"
          />
          <p className="text-2xs text-muted-foreground">
            {t("profile.bioHint", { length: bio.length })}
          </p>
          {bioError ? (
            <p className="text-2xs text-destructive" role="alert">{bioError}</p>
          ) : null}
        </div>

        <div className="flex flex-col gap-1.5">
          <Label htmlFor="timezone">{t("profile.timezone")}</Label>
          <div className="max-w-xs">
            <TimezoneSelect
              id="timezone"
              value={timezone}
              onChange={setTimezone}
              ariaInvalid={Boolean(timezoneError)}
            />
          </div>
          <p className="text-2xs text-muted-foreground">
            {t("profile.timezoneHint")}
          </p>
          {timezoneError ? (
            <p className="text-2xs text-destructive" role="alert">{timezoneError}</p>
          ) : null}
        </div>

        <div className="flex flex-col gap-1.5">
          <Label htmlFor="capacity-limit">{t("profile.capacity")}</Label>
          <Input
            id="capacity-limit"
            type="number"
            min={1}
            max={100}
            value={capacity}
            onChange={(e) => setCapacity(e.target.value)}
            className="max-w-[7rem]"
            data-testid="capacity-input"
          />
          <p className="text-2xs text-muted-foreground">
            {t("profile.capacityHint")}
          </p>
          {fieldError(update.error, "capacity_limit") ? (
            <p className="text-2xs text-destructive" role="alert">
              {fieldError(update.error, "capacity_limit")}
            </p>
          ) : null}
        </div>

        {localError ? (
          <p className="text-2xs text-destructive" role="alert">{localError}</p>
        ) : null}

        <Button
          type="submit"
          size="sm"
          className="self-start"
          disabled={update.isPending || !displayName.trim()}
          data-testid="profile-save"
        >
          {update.isPending ? t("actions.saving") : t("profile.save")}
        </Button>
      </form>
    </section>
  );
}

// ---------------------------------------------------------------------------
// Notification preferences
// ---------------------------------------------------------------------------

function NotificationPrefsSection({ prefs }: { prefs: NotificationPrefs }) {
  const { t } = useTranslation("settings");
  const { push } = useToast();
  const update = useUpdateNotificationPrefs();
  // Optimistic local state; the mutation is debounced (§6.4) so rapid
  // toggling collapses into one request. On failure the override is dropped,
  // which visually reverts the toggle to the server state.
  const [overrides, setOverrides] = useState<Partial<Record<keyof NotificationPrefs, boolean>>>({});
  const [savedFlash, setSavedFlash] = useState(false);
  const timer = useRef<number | null>(null);

  useEffect(() => () => {
    if (timer.current) window.clearTimeout(timer.current);
  }, []);

  const toggle = (key: keyof NotificationPrefs) => {
    const current = overrides[key] ?? prefs[key];
    const next = !current;
    setOverrides((o) => ({ ...o, [key]: next }));
    if (timer.current) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      update.mutate(
        { ...prefs, ...overrides, [key]: next },
        {
          onSuccess: () => {
            setOverrides({});
            setSavedFlash(true);
            window.setTimeout(() => setSavedFlash(false), 1500);
          },
          onError: (error) => {
            setOverrides((o) => {
              const reverted = { ...o };
              delete reverted[key];
              return reverted;
            });
            push(
              "error",
              t("prefs.errors.save"),
              error instanceof ApiError ? error.message : undefined,
            );
          },
        },
      );
    }, 300);
  };

  const resolved = (key: keyof NotificationPrefs) => overrides[key] ?? prefs[key];

  return (
    <section aria-label={t("prefs.title")}>
      <h2 className="mb-1 text-sm font-semibold">{t("prefs.title")}</h2>
      <p className="mb-3 text-xs text-muted-foreground">
        {t("prefs.subtitle")}
      </p>
      <p
        className="mb-2 flex items-center gap-1 text-2xs text-success transition-opacity duration-150"
        data-testid="prefs-saved"
        style={{ opacity: savedFlash ? 1 : 0 }}
        aria-live="polite"
      >
        <Check className="h-3 w-3" aria-hidden /> {t("prefs.saved")}
      </p>
      <ul className="max-w-md overflow-hidden rounded-sg border" data-testid="notification-prefs">
        {PREF_EVENTS.map(({ key, labelKey, hintKey }) => (
          <li key={key} className="flex items-center justify-between gap-3 border-b px-4 py-2.5 last:border-0">
            <div>
              <p className="text-sm font-medium">{t(labelKey)}</p>
              <p className="text-2xs text-muted-foreground">{t(hintKey)}</p>
            </div>
            <button
              type="button"
              role="switch"
              aria-checked={resolved(key)}
              aria-label={t(labelKey)}
              disabled={update.isPending}
              onClick={() => toggle(key)}
              data-testid={`pref-${key.replaceAll("_", "-")}`}
              className={`relative h-5 w-9 shrink-0 rounded-full transition-colors ${
                resolved(key) ? "bg-primary" : "bg-input"
              }`}
            >
              <span
                className={`absolute top-0.5 h-4 w-4 rounded-full bg-background shadow transition-[inset-inline-start] ${
                  resolved(key) ? "end-1" : "start-0.5"
                }`}
              />
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}

// ---------------------------------------------------------------------------
// My Work Summary
// ---------------------------------------------------------------------------

function MyWorkSummarySection() {
  const { t } = useTranslation("settings");
  const summary = useMySummary();

  if (summary.isError) {
    return (
      <section aria-label={t("summary.aria")}>
        <h2 className="mb-3 text-sm font-semibold">{t("summary.title")}</h2>
        <p className="text-xs text-destructive" role="alert">
          {t("summary.errors.load")}
        </p>
      </section>
    );
  }

  const byStatus = summary.data?.by_status;

  const tileLink = (to: string, label: string, count: number | undefined, testId: string) => (
    <Link
      to={to}
      className="flex flex-col items-center gap-1 rounded-sg border p-3 transition-colors hover:bg-accent"
      data-testid={testId}
    >
      <span className="text-lg font-semibold">{count ?? "…"}</span>
      <span className="text-2xs text-muted-foreground">{label}</span>
    </Link>
  );

  return (
    <section aria-label={t("summary.aria")}>
      <h2 className="mb-1 text-sm font-semibold">{t("summary.title")}</h2>
      <p className="mb-3 text-xs text-muted-foreground">
        {t("summary.subtitle")}
      </p>
      <div className="grid grid-cols-4 gap-2 sm:grid-cols-8" data-testid="my-work-summary">
        {TASK_STATUSES.map((status) =>
          tileLink(
            `/?status=${status}`,
            STATUS_LABELS[status],
            byStatus?.[status],
            `summary-${status.replaceAll("_", "-")}`,
          ),
        )}
      </div>
      <div className="mt-2 grid max-w-md grid-cols-2 gap-2">
        {tileLink("/?overdue=1", t("summary.overdue"), summary.data?.overdue, "summary-overdue")}
        {tileLink(
          "/?mentioned=1",
          t("summary.unreadMentions"),
          summary.data?.unread_mentions_7d,
          "summary-mentions",
        )}
      </div>
    </section>
  );
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

/** Best-effort device label from a stored User-Agent string. */
function deviceLabel(
  userAgent: string | null | undefined,
  unknownLabel: string,
  joiner: string,
): string {
  if (!userAgent) return unknownLabel;
  const ua = userAgent;
  const os = /Windows/.test(ua)
    ? "Windows"
    : /Android/.test(ua)
      ? "Android"
      : /iPhone|iPad/.test(ua)
        ? "iOS"
        : /Mac OS X/.test(ua)
          ? "macOS"
          : /Linux/.test(ua)
            ? "Linux"
            : null;
  const browser = /Edg\//.test(ua)
    ? "Edge"
    : /OPR\//.test(ua)
      ? "Opera"
      : /Chrome\//.test(ua)
        ? "Chrome"
        : /Firefox\//.test(ua)
          ? "Firefox"
          : /Safari\//.test(ua)
            ? "Safari"
            : null;
  return [browser, os].filter(Boolean).join(joiner) || unknownLabel;
}

function SessionsSection() {
  const { t } = useTranslation("settings");
  const sessions = useSessions();
  const revoke = useRevokeSession();
  const logout = useLogout();
  const { push } = useToast();

  const onRevoke = (session: SessionInfo) => {
    if (session.current) {
      // Revoking the current session is a sign-out: run the full logout
      // teardown (cache clear + auth context reset + redirect). The backend
      // logout handler still clears the cookie for an already-dead session.
      logout.mutate();
      return;
    }
    revoke.mutate(session.id, {
      onSuccess: () => push("success", t("sessions.revoked")),
      onError: (error) =>
        push(
          "error",
          t("sessions.errors.revoke"),
          error instanceof ApiError ? error.message : undefined,
        ),
    });
  };

  return (
    <div>
      <h3 className="mb-1 flex items-center gap-1.5 text-sm font-semibold">
        <MonitorSmartphone className="h-4 w-4" /> {t("sessions.title")}
      </h3>
      <p className="mb-3 text-xs text-muted-foreground">
        {t("sessions.subtitle")}
      </p>
      {sessions.isLoading ? (
        <div className="flex items-center gap-2 text-xs text-muted-foreground" data-testid="sessions-loading">
          <Loader2 className="h-4 w-4 animate-spin" /> {t("sessions.loading")}
        </div>
      ) : (
        <ul className="max-w-md overflow-hidden rounded-sg border" data-testid="session-list">
          {sessions.data?.items.length === 0 ? (
            <li className="px-4 py-3 text-xs text-muted-foreground">{t("sessions.empty")}</li>
          ) : null}
          {sessions.data?.items.map((session) => (
            <li key={session.id} className="flex items-center justify-between gap-3 border-b px-4 py-2.5 last:border-0">
              <div className="min-w-0">
                <p className="text-sm font-medium">
                  {deviceLabel(session.user_agent, t("sessions.unknownDevice"), t("sessions.onJoiner"))}
                  {session.current ? (
                    <span className="ms-2 rounded bg-primary/10 px-1.5 py-0.5 text-2xs font-semibold text-primary">
                      {t("sessions.thisDevice")}
                    </span>
                  ) : null}
                </p>
                <p className="truncate text-2xs text-muted-foreground">
                  {t("sessions.lastSeen", { time: timeAgo(session.last_seen_at) })}
                  {session.ip_address ? ` · ${session.ip_address}` : ""}
                </p>
              </div>
              <Button
                variant="outline"
                size="sm"
                disabled={revoke.isPending}
                onClick={() => onRevoke(session)}
                data-testid={`revoke-session-${session.current ? "current" : "other"}`}
              >
                {t("actions.revoke")}
              </Button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/**
 * Live strength meter (Phase 12.5 Bug 8): advisory only — the server
 * policy (ADR-0016) is the enforcement point. 0-4 scale.
 */
function PasswordStrengthMeter({ password }: { password: string }) {
  const score = passwordStrength(password);
  return (
    <div className="flex items-center gap-2" data-testid="password-strength">
      <span className="flex flex-1 gap-0.5" aria-hidden>
        {[0, 1, 2, 3].map((i) => (
          <span
            key={i}
            className={`h-1.5 flex-1 rounded-full ${i < score ? strengthTone(score) : "bg-muted"}`}
          />
        ))}
      </span>
      <span className="text-2xs text-muted-foreground">{strengthLabel(score)}</span>
    </div>
  );
}
