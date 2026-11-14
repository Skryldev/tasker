import { useQuery } from "@tanstack/react-query";
import { Network, X } from "lucide-react";
import { api, type DependencyGraphNode2, type Task } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/badge";
import { cn, STATUS_LABELS, type TaskStatus } from "@/lib/utils";

/**
 * Dependency graph modal (Phase 14 WS6, ADR-0023): a hand-written SVG of
 * the `blocks` subgraph — blockers on the left, the task in the middle,
 * blocked successors on the right. Code-split from the dependency panel;
 * no graph library (bundle budget).
 */

const NODE_W = 170;
const NODE_H = 44;
const GAP_Y = 14;
const COL_GAP = 70;

function statusDot(status: string): string {
  if (status === "closed") return "bg-status-closed";
  if (status === "in_progress") return "bg-status-in-progress";
  if (status === "resolved") return "bg-status-resolved";
  return "bg-muted-foreground";
}

function Column({
  title,
  nodes,
  align,
}: {
  title: string;
  nodes: DependencyGraphNode2[];
  align: "left" | "right";
}) {
  if (nodes.length === 0) return null;
  return (
    <div className="flex min-w-[190px] flex-col gap-1">
      <p
        className={cn(
          "text-2xs uppercase tracking-wide text-muted-foreground",
          align === "right" && "text-right",
        )}
      >
        {title}
      </p>
      <div className="flex flex-col" style={{ gap: GAP_Y }}>
        {nodes.map((n) => (
          <div
            key={n.task_id}
            data-testid={`graph-node-${n.task_id}`}
            className="rounded-md border bg-card px-2.5 py-2"
            style={{ width: NODE_W, minHeight: NODE_H }}
          >
            <p className="line-clamp-2 text-xs font-medium leading-snug">{n.title}</p>
            <p className="mt-1 flex items-center gap-1 text-2xs text-muted-foreground">
              <span className={cn("h-1.5 w-1.5 rounded-full", statusDot(n.status))} aria-hidden />
              {STATUS_LABELS[n.status as TaskStatus] ?? n.status}
            </p>
          </div>
        ))}
      </div>
    </div>
  );
}

export function DependencyGraphModal({ task, onClose }: { task: Task; onClose: () => void }) {
  const graph = useQuery({
    queryKey: ["dependency-graph", task.id],
    queryFn: () => api.dependencyGraph(task.id),
  });

  const up = graph.data?.blocked_by ?? [];
  const down = graph.data?.blocking ?? [];

  // Edge geometry: left column 0..NODE_W, center after one gap, right
  // column after the center plus another gap. Column height = rows*(NODE_H
  // + GAP_Y) - GAP_Y (no trailing gap); centerY is its vertical middle.
  const rows = Math.max(up.length, down.length, 1);
  const centerY = (rows * (NODE_H + GAP_Y) - GAP_Y) / 2;
  const totalW = NODE_W * 2 + COL_GAP * 2;
  const totalH = rows * (NODE_H + GAP_Y) - GAP_Y;

  const nodePoint = (index: number, side: "left" | "right") => {
    const y = index * (NODE_H + GAP_Y) + NODE_H / 2;
    const x = side === "left" ? NODE_W : NODE_W + 2 * COL_GAP + NODE_W;
    return { x, y };
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4">
      <div
        role="dialog"
        aria-modal
        aria-label={`Dependency graph for ${task.title}`}
        className="w-full max-w-2xl rounded-lg border bg-popover p-4 shadow-lg"
        data-testid="dependency-graph-modal"
      >
        <div className="mb-3 flex items-center justify-between">
          <h2 className="flex items-center gap-1.5 text-sm font-semibold">
            <Network className="h-4 w-4" /> Dependency graph
          </h2>
          <Button variant="ghost" size="icon-sm" aria-label="Close graph" onClick={onClose}>
            <X />
          </Button>
        </div>
        {graph.isLoading ? (
          <div className="flex flex-col gap-2 py-4">
            <Skeleton className="h-12 w-full" />
            <Skeleton className="h-12 w-3/4" />
          </div>
        ) : up.length === 0 && down.length === 0 ? (
          <p className="py-6 text-center text-xs text-muted-foreground">
            No <code>blocks</code> dependencies to draw yet.
          </p>
        ) : (
          <div className="overflow-x-auto scrollbar-thin">
            <div className="relative" style={{ width: totalW, height: totalH }}>
              <svg
                className="pointer-events-none absolute inset-0 h-full w-full"
                aria-hidden
                data-testid="dependency-graph-svg"
              >
                <defs>
                  <marker
                    id="dep-arrow"
                    viewBox="0 0 10 10"
                    refX="9"
                    refY="5"
                    markerWidth="6"
                    markerHeight="6"
                    orient="auto-start-reverse"
                  >
                    <path d="M 0 0 L 10 5 L 0 10 z" className="fill-muted-foreground" />
                  </marker>
                </defs>
                {up.map((n, i) => {
                  const p = nodePoint(i, "left");
                  return (
                    <line
                      key={n.task_id}
                      x1={p.x}
                      y1={p.y}
                      x2={NODE_W + COL_GAP}
                      y2={centerY}
                      className="stroke-muted-foreground"
                      strokeWidth="1.5"
                      markerEnd="url(#dep-arrow)"
                    />
                  );
                })}
                {down.map((n, i) => {
                  const p = nodePoint(i, "right");
                  return (
                    <line
                      key={n.task_id}
                      x1={NODE_W + COL_GAP + NODE_W}
                      y1={centerY}
                      x2={p.x}
                      y2={p.y}
                      className="stroke-muted-foreground"
                      strokeWidth="1.5"
                      markerEnd="url(#dep-arrow)"
                    />
                  );
                })}
              </svg>
              <div className="absolute" style={{ top: centerY - NODE_H / 2, left: NODE_W + COL_GAP }}>
                <div
                  className="rounded-md border-2 border-primary bg-primary/5 px-2.5 py-2"
                  style={{ width: NODE_W, minHeight: NODE_H }}
                  data-testid={`graph-node-${task.id}`}
                >
                  <p className="line-clamp-2 text-xs font-semibold leading-snug">{task.title}</p>
                  <p className="mt-1 flex items-center gap-1 text-2xs text-muted-foreground">
                    <span className={cn("h-1.5 w-1.5 rounded-full", statusDot(task.status))} aria-hidden />
                    {STATUS_LABELS[task.status as TaskStatus] ?? task.status}
                  </p>
                </div>
              </div>
              <div className="absolute" style={{ top: 0, left: 0 }}>
                <Column title="Blocks this task" nodes={up} align="left" />
              </div>
              <div className="absolute" style={{ top: 0, left: NODE_W + 2 * COL_GAP + NODE_W }}>
                <Column title="This task blocks" nodes={down} align="right" />
              </div>
            </div>
          </div>
        )}
        <p className="mt-3 text-2xs text-muted-foreground">
          Arrows flow from dependency to dependent — a predecessor must finish before the arrow
          leaves it. Only <code>blocks</code> edges participate.
        </p>
      </div>
    </div>
  );
}
