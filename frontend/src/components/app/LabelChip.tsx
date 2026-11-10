import { useTheme } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { chipBackground, chipTextColor } from "@/lib/chip-color";
import { LabelIcon } from "./TaskBadges";

/**
 * Reusable label chip (Phase 13.5 Bug 9): the label color tints the
 * background at 15% and the text/icon share a same-hue shade computed at
 * render time to meet WCAG AA (≥ 4.5:1) — theme-aware, never hardcoded.
 * Used in the label manager, task detail, kanban cards, list rows, and
 * dialogs so every surface renders identically.
 */
export function LabelChip({
  name,
  color,
  icon,
  className,
}: {
  name: string;
  color: string;
  icon?: string | null;
  className?: string;
}) {
  const { appearance } = useTheme();
  const dark = appearance === "dark";
  const bg = chipBackground(color, dark);
  const text = chipTextColor(color, dark);
  return (
    <span
      className={cn(
        "inline-flex max-w-[10rem] items-center gap-1 rounded-full border px-2 py-0.5 text-2xs font-medium transition-colors duration-100 hover:brightness-95",
        className,
      )}
      style={{
        backgroundColor: bg,
        borderColor: `${color}55`,
        color: text,
      }}
    >
      {icon ? <LabelIcon icon={icon} className="h-3 w-3" /> : null}
      <span className="truncate">{name}</span>
    </span>
  );
}
