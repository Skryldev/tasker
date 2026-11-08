import { useState } from "react";
import {
  Activity,
  CalendarClock,
  Gauge,
  Hourglass,
  Inbox,
  ListChecks,
  Timer,
  TrendingDown,
  TrendingUp,
} from "lucide-react";
import { useDashboard } from "@/api/hooks";
import type { DashboardWindow } from "@/api/client";
import { EmptyState } from "@/components/app/EmptyState";
import { Skeleton } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { useWorkspace } from "@/lib/workspace";
import { cn, PRIORITY_LABELS, type Priority } from "@/lib/utils";

/**
 * Phase 12 WS14: team dashboard. Server-computed KPIs (Manager+ only) with
 * window selection, hand-rolled SVG charts, and a review-load table.
 */

const WINDOWS: { id: DashboardWindow; label: string }[] = [
  { id: "7d", label: "Last 7 days" },
  { id: "30d", label: "Last 30 days" },
  { id: "90d", label: "Last 90 days" },
];

function delta(current: number, previous: number): { pct: string; up: boolean } | null {
  if (previous === 0) return current > 0 ? { pct: "new", up: true } : null;
  const diff = ((current - previous) / previous) * 100;
  return { pct: `${diff >= 0 ? "+" : ""}${Math.round(diff)}%`, up: diff >= 0 };
}

function KpiCard({
  icon,
  label,
  value,
  sub,
  tone,
  testId,
}: {
  icon: React.ReactNode;
  label: string;
  value: string;
  sub?: React.ReactNode;
  tone?: "positive" | "negative" | "neutral";
  testId: string;
}) {
  return (
    <div
      className="rounded-lg border bg-card p-3 shadow-sm"
      data-testid={testId}
    >
      <div className="flex items-center gap-1.5 text-2xs font-medium uppercase tracking-wide text-muted-foreground">
        {icon}
        {label}
      </div>
      <p
        className={cn(
          "mt-1 text-2xl font-semibold tabular-nums",
          tone === "negative" && "text-destructive",
          tone === "positive" && "text-success",
        )}
      >
        {value}
      </p>
      {sub ? <div className="mt-0.5 text-2xs text-muted-foreground">{sub}</div> : null}
    </div>
  );
}

export default function DashboardPage({ teamId: teamIdProp }: { teamId?: string }) {
  const { teamId: workspaceTeamId } = useWorkspace();
  const teamId = teamIdProp ?? workspaceTeamId;
  const [window, setWindow] = useState<DashboardWindow>("7d");
  const { data, isLoading, isError, error } = useDashboard(teamId, window);

  if (!teamId) {
    return (
      <div className="p-6">
        <EmptyState
          icon={<Gauge className="h-6 w-6" />}
          title="No team selected"
          description="Pick a team to see its dashboard."
        />
      </div>
    );
  }

  if (isLoading) {
    return (
      <div className="mx-auto grid w-full max-w-5xl grid-cols-2 gap-3 p-4 md:grid-cols-4" aria-busy="true">
        {Array.from({ length: 8 }).map((_, i) => (
          <Skeleton key={i} className="h-24 w-full" />
        ))}
      </div>
    );
  }

  if (isError || !data) {
    const status = (error as { status?: number } | null)?.status;
    return (
      <div className="p-6">
        <EmptyState
          icon={<Gauge className="h-6 w-6" />}
          title={status === 403 ? "Manager access required" : "Dashboard unavailable"}
          description={
            status === 403
              ? "The dashboard is available to managers and admins of this team."
              : "Try again in a moment."
          }
        />
      </div>
    );
  }

  const k = data.kpis;
  const cap = k.capacity_utilization ?? null;
  const throughputDelta = delta(k.throughput, k.throughput_prev);
  const mixEntries = Object.entries(k.priority_mix as Record<string, number>);
  const mixMax = Math.max(1, ...mixEntries.map(([, n]) => n));

  return (
    <div className="mx-auto w-full max-w-5xl p-4" data-testid="dashboard-page">
      <div className="mb-4 flex flex-wrap items-center justify-between gap-2">
        <h1 className="text-lg font-semibold">Team dashboard</h1>
        <div className="flex gap-1" role="tablist" aria-label="Dashboard window">
          {WINDOWS.map((w) => (
            <Button
              key={w.id}
              role="tab"
              aria-selected={window === w.id}
              variant={window === w.id ? "secondary" : "ghost"}
              size="sm"
              onClick={() => setWindow(w.id)}
              data-testid={`dashboard-window-${w.id}`}
            >
              {w.label}
            </Button>
          ))}
        </div>
      </div>

      <div className="grid grid-cols-2 gap-3 md:grid-cols-4" data-testid="dashboard-kpis">
        <KpiCard
          testId="kpi-throughput"
          icon={<Activity className="h-3 w-3" />}
          label="Throughput"
          value={String(k.throughput)}
          tone={k.throughput >= k.throughput_prev ? "positive" : "neutral"}
          sub={
            throughputDelta ? (
              <span className="inline-flex items-center gap-1">
                {throughputDelta.up ? (
                  <TrendingUp className="h-3 w-3 text-success" aria-hidden />
                ) : (
                  <TrendingDown className="h-3 w-3 text-destructive" aria-hidden />
                )}
                {throughputDelta.pct} vs previous {window === "7d" ? "7" : window === "30d" ? "30" : "90"} days
              </span>
            ) : (
              "no previous-window data"
            )
          }
        />
        <KpiCard
          testId="kpi-cycle"
          icon={<Timer className="h-3 w-3" />}
          label="Cycle time"
          value={k.cycle_time_days !== null ? `${k.cycle_time_days}d` : "—"}
          sub="start → close, closed tasks"
        />
        <KpiCard
          testId="kpi-lead"
          icon={<Hourglass className="h-3 w-3" />}
          label="Lead time"
          value={k.lead_time_days !== null ? `${k.lead_time_days}d` : "—"}
          sub="creation → close"
        />
        <KpiCard
          testId="kpi-wip"
          icon={<ListChecks className="h-3 w-3" />}
          label="WIP"
          value={String(k.wip)}
          sub="in progress or in review"
        />
        <KpiCard
          testId="kpi-overdue"
          icon={<CalendarClock className="h-3 w-3" />}
          label="Overdue"
          value={String(k.overdue)}
          tone={k.overdue > 0 ? "negative" : "positive"}
          sub="open tasks past due"
        />
        <KpiCard
          testId="kpi-deadline-risk"
          icon={<CalendarClock className="h-3 w-3" />}
          label="Deadline risk"
          value={String(k.deadline_risk)}
          tone={k.deadline_risk > 0 ? "negative" : undefined}
          sub="deadline within 7 days"
        />
        <KpiCard
          testId="kpi-review-load"
          icon={<Inbox className="h-3 w-3" />}
          label="Review load"
          value={String(k.review_load)}
          sub="awaiting review"
        />
        <KpiCard
          testId="kpi-capacity"
          icon={<Gauge className="h-3 w-3" />}
          label="Capacity"
          value={cap !== null ? `${Math.round(cap * 100)}%` : "—"}
          tone={cap !== null && cap > 1 ? "negative" : undefined}
          sub="assigned open / capacity"
        />
      </div>

      <div className="mt-4 grid gap-3 md:grid-cols-2">
        {/* Throughput: current vs previous window, hand-rolled SVG. */}
        <section className="rounded-lg border bg-card p-4" aria-label="Throughput chart" data-testid="dashboard-throughput">
          <h2 className="mb-3 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            Throughput — current vs previous
          </h2>
          <svg viewBox="0 0 220 90" className="h-24 w-full" role="img" aria-label={`Bar chart: ${k.throughput} closed in this window versus ${k.throughput_prev} in the previous one`}>
            {[k.throughput, k.throughput_prev].map((v, i) => {
              const max = Math.max(1, k.throughput, k.throughput_prev);
              const h = (v / max) * 60;
              const x = 40 + i * 90;
              return (
                <g key={i}>
                  <rect x={x} y={70 - h} width={44} height={h} rx={4} className="fill-primary" opacity={i === 0 ? 1 : 0.45} />
                  <text x={x + 22} y={82} textAnchor="middle" className="fill-muted-foreground text-[8px]">
                    {i === 0 ? "current" : "previous"}
                  </text>
                  <text x={x + 22} y={70 - h - 4} textAnchor="middle" className="fill-foreground text-[9px] font-semibold">
                    {v}
                  </text>
                </g>
              );
            })}
          </svg>
        </section>

        {/* Priority mix: horizontal bars. */}
        <section className="rounded-lg border bg-card p-4" aria-label="Priority mix" data-testid="dashboard-priority-mix">
          <h2 className="mb-3 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
            Open tasks by priority
          </h2>
          <div className="flex flex-col gap-1.5">
            {mixEntries.map(([p, n]) => (
              <div key={p} className="flex items-center gap-2 text-2xs">
                <span className="w-14 shrink-0 text-muted-foreground">
                  {PRIORITY_LABELS[p as Priority] ?? p}
                </span>
                <span className="h-2 flex-1 overflow-hidden rounded-full bg-muted">
                  <span
                    className="block h-full rounded-full bg-primary/70"
                    style={{ width: `${(n / mixMax) * 100}%` }}
                  />
                </span>
                <span className="w-6 text-right tabular-nums">{n}</span>
              </div>
            ))}
            {mixEntries.every(([, n]) => n === 0) ? (
              <p className="text-2xs text-muted-foreground">No open tasks.</p>
            ) : null}
          </div>
        </section>
      </div>

      {/* Review load per reporter. */}
      <section className="mt-4 rounded-lg border bg-card p-4" aria-label="Review load per reporter" data-testid="dashboard-review-table">
        <h2 className="mb-3 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          Tasks awaiting review, by reporter
        </h2>
        {data.review_by_reporter.length === 0 ? (
          <p className="text-2xs text-muted-foreground">Nothing in review.</p>
        ) : (
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b text-left text-2xs uppercase tracking-wide text-muted-foreground">
                <th className="py-1.5 pr-3 font-medium">Reporter</th>
                <th className="py-1.5 font-medium">In review</th>
              </tr>
            </thead>
            <tbody>
              {data.review_by_reporter.map((r) => (
                <tr key={r.reporter_id} className="border-b last:border-0">
                  <td className="py-1.5 pr-3">{r.display_name}</td>
                  <td className="py-1.5 tabular-nums">{r.count}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>
    </div>
  );
}
