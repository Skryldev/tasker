import type { ReactNode } from "react";
import { Button } from "@/components/ui/button";

export function EmptyState({
  icon,
  title,
  description,
  action,
}: {
  icon: ReactNode;
  title: string;
  description: string;
  action?: ReactNode;
}) {
  return (
    <div
      className="flex flex-col items-center justify-center gap-3 rounded-lg border border-dashed p-10 text-center"
      data-testid="empty-state"
    >
      <span className="text-muted-foreground/60">{icon}</span>
      <div>
        <p className="text-sm font-medium">{title}</p>
        <p className="mt-1 max-w-sm text-xs text-muted-foreground">{description}</p>
      </div>
      {action}
    </div>
  );
}

export function ErrorState({
  title = "Something went wrong",
  detail,
  onRetry,
}: {
  title?: string;
  detail?: string;
  onRetry?: () => void;
}) {
  return (
    <div
      className="flex flex-col items-center justify-center gap-3 rounded-lg border border-destructive/30 bg-destructive/5 p-10 text-center"
      role="alert"
      data-testid="error-state"
    >
      <div>
        <p className="text-sm font-medium text-destructive">{title}</p>
        {detail ? <p className="mt-1 max-w-sm text-xs text-muted-foreground">{detail}</p> : null}
      </div>
      {onRetry ? (
        <Button variant="outline" size="sm" onClick={onRetry}>
          Try again
        </Button>
      ) : null}
    </div>
  );
}
