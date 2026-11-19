import type { TFunction } from "i18next";
import { ApiError } from "@/api/client";

/**
 * API error localization (Phase 15 §6.4). Backend messages arrive in English;
 * the UI never translates them verbatim — it maps the RFC 9457 problem
 * `type` slug (https://tasker.dev/problems/<slug>) to a localized message
 * from the `errors` namespace and falls back generically for unknown slugs.
 */

/** The last path segment of a problem type URI, or null. */
export function problemSlug(problemType?: string): string | null {
  if (!problemType) return null;
  const slug = problemType.split("/").pop();
  return slug || null;
}

/**
 * Localized, user-facing message for any thrown error. ApiErrors map by
 * type; validation problems additionally list the field messages (which
 * stay backend-authored — see docs/i18n.md §Errors for the boundary).
 */
export function localizedApiError(error: unknown, t: TFunction): string {
  if (error instanceof ApiError) {
    const slug = problemSlug(error.problem.type) ?? "internal-error";
    const key = `errors:types.${slug}`;
    const base = t(key, { defaultValue: "" });
    const message = base || t("errors:types.unknown");
    const fieldErrors = error.problem.errors;
    if (fieldErrors && fieldErrors.length > 0) {
      return `${message} ${fieldErrors.map((f) => f.message).join(" ")}`;
    }
    return message;
  }
  if (error instanceof Error && error.message) {
    return error.message;
  }
  return t("errors:types.unknown");
}
