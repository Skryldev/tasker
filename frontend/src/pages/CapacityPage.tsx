import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Gauge } from "lucide-react";
import { api, type CapacityMember } from "@/api/client";
import { Avatar } from "@/components/ui/avatar";
import { Badge, Skeleton } from "@/components/ui/badge";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { EmptyState } from "@/components/app/EmptyState";
import { useWorkspace } from "@/lib/workspace";
import { cn } from "@/lib/utils";

/**
 * Capacity view (Phase 10.5): forward-looking load per member — open work
 * against each member's configurable capacity limit — as opposed to the
 * Workload view's current-distribution snapshot. Server-computed; the page
 * only renders. Manager+ (the backend enforces it; members get a 404 which
 * renders as the standard unavailable state).
 */

type SortKey = "utilization" | "overdue" | "name";

const STATUS_TONE: Record<string, string> = {
  underutilized: "text-status-resolved",
  balanced: "text-warning",
  overloaded: "text-destructive",
};

const BAR_TONE: Record<string, string> = {
  underutilized: "bg-status-resolved",
  balanced: "bg-warning",
  overloaded: "bg-destructive",
};

export function CapacityPage() {
  const { teamId } = useWorkspace();
  const [window, setWindow] = useState<"week" | "month">("week");
  const [sort, setSort] = useState<SortKey>("utilization");
  const capacity = useQuery({
    queryKey: ["capacity", teamId, window],
    queryFn: () => api.capacity(teamId!, window),
    enabled: Boolean(teamId),
  });

  const members = useMemo(() => {
    const items = [...(capacity.data?.items ?? [])];
    items.sort((a, b) => {
      if (sort === "name") return a.display_name.localeCompare(b.display_name);
      if (sort === "overdue") return b.overdue_tasks - a.overdue_tasks || b.utilization - a.utilization;
      return b.utilization - a.utilization || b.open_tasks - a.open_tasks;
    });
    return items;
  }, [capacity.data, sort]);

  if (!teamId || capacity.isError) {
    return (
      <EmptyState
        icon={<Gauge className="h-8 w-8" />}
        title="Capacity unavailable"
        description="Select a valid team, or ask an administrator for access — capacity is a Manager-level view."
      />
    );
  }

  const summary = capacity.data?.summary;

  return (
    <div className="mx-auto max-w-4xl p-6">
      <header className="mb-6 flex flex-wrap items-end justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">Capacity</h1>
          <p className="text-xs text-muted-foreground">
            How much work each member can take on in the coming window, against their personal limit.
          </p>
        </div>
        <div className="flex items-center gap-2" data-testid="capacity-filters">
          <Select value={window} onValueChange={(v) => setWindow(v as "week" | "month")}>
            <SelectTrigger className="w-auto min-w-[8rem]" aria-label="Capacity window">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="week">This week</SelectItem>
              <SelectItem value="month">This month</SelectItem>
            </SelectContent>
          </Select>
          <Select value={sort} onValueChange={(v) => setSort(v as SortKey)}>
            <SelectTrigger className="w-auto min-w-[9.5rem]" aria-label="Sort members">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="utilization">Sort by utilization</SelectItem>
              <SelectItem value="overdue">Sort by overdue</SelectItem>
              <SelectItem value="name">Sort by name</SelectItem>
            </SelectContent>
          </Select>
        </div>
      </header>

      {capacity.isLoading ? (
        <div className="flex flex-col gap-3">
          <Skeleton className="h-16 w-full" />
          <Skeleton className="h-48 w-full" />
        </div>
      ) : (
        <>
          {/* Summary row */}
          <div
            className="mb-6 grid grid-cols-3 gap-3 rounded-lg border bg-card p-4"
            data-testid="capacity-summary"
          >
            <SummaryTile label="Open tasks" value={summary?.total_open_tasks ?? 0} />
            <SummaryTile label="Total capacity" value={summary?.total_capacity ?? 0} />
            <div>
              <p className="text-2xs text-muted-foreground">Utilization</p>
              <p className="text-lg font-semibold tabular-nums">
                {Math.round((summary?.utilization ?? 0) * 100)}%
              </p>
              <div className="mt-1 h-1.5 w-full overflow-hidden rounded-full bg-muted">
                <div
                  className={cn(
                    "h-full rounded-full",
                    (summary?.utilization ?? 0) > 1
                      ? "bg-destructive"
                      : (summary?.utilization ?? 0) >= 0.6
                        ? "bg-warning"
                        : "bg-status-resolved",
                  )}
                  style={{ width: `${Math.min(100, ((summary?.utilization ?? 0) / 2) * 100)}%` }}
                />
              </div>
            </div>
          </div>

          {members.length === 0 ? (
            <EmptyState
              icon={<Gauge className="h-8 w-8" />}
              title="No members to measure"
              description="Add members to the team to see capacity."
            />
          ) : members.every((m) => m.open_tasks === 0) ? (
            <div data-testid="capacity-empty">
              <EmptyState
                icon={<Gauge className="h-8 w-8" />}
                title="No open tasks"
                description="No open tasks this window — everyone has room to take on work."
              />
            </div>
          ) : (
            <div className="overflow-x-auto" data-testid="capacity-table">
              <table className="w-full min-w-[640px] border-collapse text-sm">
                <thead>
                  <tr className="border-b bg-muted/50 text-left">
                    {["Member", "Open", "Overdue", window === "week" ? "Due this week" : "Due this month", "Capacity", "Utilization"].map((h) => (
                      <th
                        key={h}
                        className="px-3 py-2 text-2xs font-medium uppercase tracking-wide text-muted-foreground"
                      >
                        {h}
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {members.map((member) => (
                    <CapacityRow key={member.user_id} member={member} />
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </>
      )}
    </div>
  );
}

function SummaryTile({ label, value }: { label: string; value: number }) {
  return (
    <div>
      <p className="text-2xs text-muted-foreground">{label}</p>
      <p className="text-lg font-semibold tabular-nums">{value}</p>
    </div>
  );
}

function CapacityRow({ member }: { member: CapacityMember }) {
  const percent = Math.round(member.utilization * 100);
  return (
    <tr className="h-10 border-b last:border-0" data-testid={`capacity-row-${member.user_id}`}>
      <td className="px-3">
        <span className="flex items-center gap-2">
          <Avatar name={member.display_name} size="sm" src={member.avatar_url} />
          <span className="truncate font-medium">{member.display_name}</span>
        </span>
      </td>
      <td className="px-3 tabular-nums">{member.open_tasks}</td>
      <td className={cn("px-3 tabular-nums", member.overdue_tasks > 0 && "font-medium text-destructive")}>
        {member.overdue_tasks}
      </td>
      <td className="px-3 tabular-nums">
        {windowValue(member) ?? <span className="text-muted-foreground">—</span>}
      </td>
      <td className="px-3 tabular-nums text-muted-foreground">{member.capacity_limit}</td>
      <td className="px-3">
        <span className="flex items-center gap-2">
          <span className="h-1.5 w-24 overflow-hidden rounded-full bg-muted" aria-hidden>
            <span
              className={cn("h-full rounded-full", BAR_TONE[member.status])}
              style={{ width: `${Math.min(100, percent / 2)}%` }}
            />
          </span>
          <span className={cn("text-2xs font-medium tabular-nums", STATUS_TONE[member.status])}>
            {percent}%
          </span>
          <Badge variant="muted" className="capitalize">
            {member.status}
          </Badge>
        </span>
      </td>
    </tr>
  );
}

function windowValue(member: CapacityMember): number | null {
  if (member.due_this_week !== null && member.due_this_week !== undefined) return member.due_this_week;
  if (member.due_this_month !== null && member.due_this_month !== undefined) return member.due_this_month;
  return null;
}

