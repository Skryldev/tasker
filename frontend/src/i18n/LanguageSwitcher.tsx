import { useTranslation } from "react-i18next";
import { Languages } from "lucide-react";
import { LOCALES, LOCALE_META, switchLocale, type Locale } from "./locale";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";

/**
 * Language switcher (Phase 15 §6.1). Compact variant for the topbar; the
 * Settings page embeds the same list. Switching navigates to the same route
 * under the new locale prefix (full document navigation — the router
 * basename is rebuilt) and persists to localStorage.
 */
export function LanguageSwitcher({ compact = false }: { compact?: boolean }) {
  const { t, i18n } = useTranslation();
  const active = (i18n.language?.split("-")[0] ?? "en") as Locale;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size={compact ? "icon" : "sm"}
          aria-label={t("lang.switchAria")}
          data-testid="language-switcher"
          className={cn(compact && "px-2")}
        >
          <Languages />
          {compact ? null : <span className="text-xs">{t("lang.label")}</span>}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-44">
        <DropdownMenuLabel>{t("lang.label")}</DropdownMenuLabel>
        {LOCALES.map((locale) => (
          <DropdownMenuItem
            key={locale}
            aria-current={locale === active || undefined}
            onSelect={() => {
              if (locale !== active) switchLocale(locale);
            }}
            className="justify-between"
            data-testid={`lang-${locale}`}
          >
            <span className="flex items-center gap-2">
              <span className="w-4 text-2xs font-semibold text-muted-foreground" aria-hidden>
                {locale.toUpperCase()}
              </span>
              <span className="truncate">{LOCALE_META[locale].name}</span>
            </span>
            {locale === active ? (
              <span className="text-2xs font-medium text-primary" aria-label={t("lang.active")}>
                ✓
              </span>
            ) : null}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
