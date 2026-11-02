import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { Users } from "lucide-react";
import { api } from "@/api/client";
import { useTeams } from "@/api/hooks";
import { useWorkspace } from "@/lib/workspace";
import { EmptyState, ErrorState } from "@/components/app/EmptyState";
import { Badge } from "@/components/ui/badge";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { DateField } from "@/components/ui/date-field";
import { PRIORITIES, PRIORITY_LABELS, STATUS_LABELS, TASK_STATUSES, cn } from "@/lib/utils";

/** Team workload view: server-side aggregation of assigned open tasks by
 *  member. Counts are task distribution — not effort or capacity. */
export function WorkloadPage() {
  const { teamId: routeTeam } = useParams<{ teamId: string }>();
  const { teamId: workspaceTeam } = useWorkspace();
  const teamId = routeTeam ?? workspaceTeam;
  const teams = useTeams();
  const projects = useQuery({
    queryKey: ["projects", teamId ?? "", false],
    queryFn: () => api.listProjects(teamId!),
    enabled: Boolean(teamId),
  });

  const [projectId, setProjectId] = useState("all");
  const [dueBefore, setDueBefore] = useState("");
  const [dueAfter, setDueAfter] = useState("");

  const workload = useQuery({
    queryKey: ["workload", teamId, projectId, dueBefore, dueAfter],
    queryFn: () =>
      api.workload(teamId!, {
        project_id: projectId === "all" ? undefined : projectId,
        due_before: dueBefore || undefined,
        due_after: dueAfter || undefined,
      }),
    enabled: Boolean(teamId),
  });

  if (!teamId) {
    return (
      <EmptyState
        icon={<Users className="h-8 w-8" />}
        title="No team selected"
        description="Select a team to see its workload."
      />
    );
  }

  const teamName = teams.data?.items.find((t) => t.id === teamId)?.name ?? "Team";

  const taskLink = (userId: string | null, status: string | null) => {
    const params = new URLSearchParams();
    if (userId) params.set("assignee_id", userId);
    else params.set("assignee_id", "none");
    if (status) params.set("status", status);
    if (projectId !== "all") params.set("project", projectId);
    return `/projects?${params.toString()}&team=${teamId}`;
  };

  return (
    <div className="mx-auto max-w-4xl p-6" data-testid="workload-page">
      <header className="mb-5">
        <h1 className="text-lg font-semibold">{teamName} workload</h1>
        <p className="text-xs text-muted-foreground">
          Distribution of open tasks per member. Task counts are not effort,
          productivity, or capacity.
        </p>
      </header>

      <div className="mb-4 flex flex-wrap items-center gap-2" data-testid="workload-filters">
        <Select value={projectId} onValueChange={setProjectId}>
          <SelectTrigger className="w-auto min-w-[10rem]" aria-label="Filter by project">
            <SelectValue placeholder="All projects" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">All projects</SelectItem>
            {(projects.data?.items ?? []).map((p) => (
              <SelectItem key={p.id} value={p.id}>
                {p.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <DateField
          value={dueAfter || null}
          onChange={setDueAfter}
          ariaLabel="Due on or after"
          className="w-40"
        />
        <DateField
          value={dueBefore || null}
          onChange={setDueBefore}
          ariaLabel="Due on or before"
          className="w-40"
        />
      </div>

      {workload.isLoading ? (
        <div className="flex flex-col gap-2">
          {[0, 1, 2].map((i) => (
            <div key={i} className="h-16 animate-pulse rounded bg-muted" />
          ))}
        </div>
      ) : workload.isError ? (
        <ErrorState
          title="Could not load workload"
          detail="You may not be a member of this team."
          onRetry={() => void workload.refetch()}
        />
      ) : (workload.data?.members.length ?? 0) === 0 &&
        (workload.data?.unassigned.total ?? 0) === 0 ? (
        <EmptyState
          icon={<Users className="h-8 w-8" />}
          title="Nothing assigned"
          description="No open tasks match the current filters."
        />
      ) : (
        <div className="overflow-hidden rounded-lg border" data-testid="workload-table">
          <table className="w-full border-collapse text-sm">
            <thead>
              <tr className="border-b bg-muted/50 text-left">
                <th className="px-3 py-2 text-2xs font-semibold uppercase tracking-wide text-muted-foreground">
                  Member
                </th>
                {TASK_STATUSES.map((s) => (
                  <th
                    key={s}
                    className="px-3 py-2 text-2xs font-semibold uppercase tracking-wide text-muted-foreground"
                  >
                    {STATUS_LABELS[s]}
                  </th>
                ))}
                <th className="px-3 py-2 text-2xs font-semibold uppercase tracking-wide text-muted-foreground">
                  Overdue
                </th>
                <th className="px-3 py-2 text-2xs font-semibold uppercase tracking-wide text-muted-foreground">
                  Total
                </th>
              </tr>
            </thead>
            <tbody>
              {workload.data!.members.map((m) => (
                <tr key={m.user.id} className="border-b last:border-0" data-testid={`workload-row-${m.user.id}`}>
                  <td className="px-3 py-2">
                    <span className="font-medium">{m.user.display_name}</span>
                  </td>
                  {TASK_STATUSES.map((s) => {
                    const count = m.by_status[s] ?? 0;
                    return (
                      <td key={s} className="px-3 py-2">
                        {count > 0 ? (
                          <Link
                            to={taskLink(m.user.id, s)}
                            className={cn(
                              "inline-flex h-6 min-w-6 items-center justify-center rounded-full px-1.5 text-xs font-medium",
                              "bg-secondary text-secondary-foreground hover:bg-accent",
                            )}
                            title={`${count} ${STATUS_LABELS[s]} for ${m.user.display_name}`}
                          >
                            {count}
                          </Link>
                        ) : (
                          <span className="text-muted-foreground">–</span>
                        )}
                      </td>
                    );
                  })}
                  <td className="px-3 py-2">
                    {m.overdue > 0 ? (
                      <Badge className="bg-destructive/10 text-destructive">{m.overdue}</Badge>
                    ) : (
                      <span className="text-muted-foreground">–</span>
                    )}
                  </td>
                  <td className="px-3 py-2 font-medium">{m.total}</td>
                </tr>
              ))}
              <tr className="border-t bg-muted/30">
                <td className="px-3 py-2 font-medium">Unassigned</td>
                {TASK_STATUSES.map((s) => {
                  const count = workload.data!.unassigned.by_status[s] ?? 0;
                  return (
                    <td key={s} className="px-3 py-2">
                      {count > 0 ? (
                        <Link
                          to={taskLink(null, s)}
                          className="inline-flex h-6 min-w-6 items-center justify-center rounded-full bg-secondary px-1.5 text-xs font-medium hover:bg-accent"
                        >
                          {count}
                        </Link>
                      ) : (
                        <span className="text-muted-foreground">–</span>
                      )}
                    </td>
                  );
                })}
                <td className="px-3 py-2">
                  {workload.data!.unassigned.overdue > 0 ? (
                    <Badge className="bg-destructive/10 text-destructive">
                      {workload.data!.unassigned.overdue}
                    </Badge>
                  ) : (
                    <span className="text-muted-foreground">–</span>
                  )}
                </td>
                <td className="px-3 py-2 font-medium">{workload.data!.unassigned.total}</td>
              </tr>
            </tbody>
          </table>
        </div>
      )}

      {workload.data ? (
        <div className="mt-4 flex flex-wrap gap-3 text-2xs text-muted-foreground">
          {PRIORITIES.map((p) => {
            const total = workload.data!.members.reduce(
              (sum, m) => sum + (m.by_priority[p] ?? 0),
              0,
            ) + (workload.data!.unassigned.by_priority[p] ?? 0);
            return (
              <span key={p}>
                {PRIORITY_LABELS[p]}: <span className="font-medium text-foreground">{total}</span>
              </span>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}
