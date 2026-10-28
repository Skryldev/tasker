import * as React from "react";
import { cn } from "@/lib/utils";

export function Avatar({
  name,
  size = "sm",
  className,
}: {
  name: string;
  size?: "xs" | "sm" | "md";
  className?: string;
}) {
  const { initials, tone } = React.useMemo(() => {
    const parts = name.trim().split(/\s+/).slice(0, 2);
    const chars = parts.map((p) => p.charAt(0).toUpperCase()).join("") || "?";
    let hash = 0;
    for (const ch of name) hash = (hash * 31 + ch.charCodeAt(0)) | 0;
    return { initials: chars, tone: `hsl(${Math.abs(hash) % 360} 48% 42%)` };
  }, [name]);

  const sizes = {
    xs: "h-5 w-5 text-[9px]",
    sm: "h-6 w-6 text-[10px]",
    md: "h-8 w-8 text-xs",
  } as const;

  return (
    <span
      title={name}
      style={{ backgroundColor: tone }}
      className={cn(
        "inline-flex shrink-0 select-none items-center justify-center rounded-full font-medium text-white",
        sizes[size],
        className,
      )}
    >
      {initials}
    </span>
  );
}

export function Separator({
  className,
  orientation = "horizontal",
}: {
  className?: string;
  orientation?: "horizontal" | "vertical";
}) {
  return (
    <div
      role="separator"
      aria-orientation={orientation}
      className={cn(
        "shrink-0 bg-border",
        orientation === "horizontal" ? "h-px w-full" : "h-full w-px",
        className,
      )}
    />
  );
}
