import { Fragment } from "react";
import { Check } from "lucide-react";
import {
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from "@/components/ui/dropdown-menu";
import { THEMES, type ThemeId } from "@/lib/theme";

/**
 * Theme picker content (Phase 12.5 bundle offset): code-split out of the
 * eager shell — the 16-theme registry, swatches, and grouping markup render
 * only when the menu first opens. Hover preview is handled by the eager
 * trigger via callbacks.
 */
export function ThemeSwitcherContent({
  theme,
  onPreview,
  onSelect,
}: {
  theme: ThemeId;
  onPreview: (id: ThemeId | null) => void;
  onSelect: (id: ThemeId) => void;
}) {
  return (
    <DropdownMenuContent align="end" className="w-52">
      <DropdownMenuLabel>Theme — hover to preview</DropdownMenuLabel>
      {/* WS1: themes grouped by appearance, light first. */}
      {(["light", "dark"] as const).map((group) => (
        <Fragment key={group}>
          <DropdownMenuSeparator />
          <DropdownMenuLabel className="text-2xs uppercase tracking-wide text-muted-foreground">
            {group === "light" ? "Light" : "Dark"}
          </DropdownMenuLabel>
          {THEMES.filter((d) => d.appearance === group).map((definition) => (
            <DropdownMenuItem
              key={definition.id}
              onSelect={() => onSelect(definition.id)}
              onMouseEnter={() => onPreview(definition.id)}
              onMouseLeave={() => onPreview(null)}
              onFocus={() => onPreview(definition.id)}
              onBlur={() => onPreview(null)}
              className="justify-between"
              data-testid={`theme-option-${definition.id}`}
            >
              <span className="flex items-center gap-2">
                {/* Preview swatch: background, primary, accent. */}
                <span className="flex -space-x-0.5" aria-hidden>
                  {definition.swatch.map((color) => (
                    <span
                      key={color}
                      className="h-3.5 w-3.5 rounded-full border border-border"
                      style={{ backgroundColor: color }}
                    />
                  ))}
                </span>
                {definition.label}
              </span>
              {theme === definition.id ? (
                <Check className="h-3.5 w-3.5" aria-label="Active theme" />
              ) : null}
            </DropdownMenuItem>
          ))}
        </Fragment>
      ))}
    </DropdownMenuContent>
  );
}
