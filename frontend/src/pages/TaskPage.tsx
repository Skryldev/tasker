import { useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { ArrowLeft } from "lucide-react";
import { api } from "@/api/client";
import { Button } from "@/components/ui/button";
import { ErrorState } from "@/components/app/EmptyState";
import { TaskDetailPanel } from "@/components/app/TaskDetailPanel";
import { ApiError } from "@/api/client";

/** Standalone task view reached from notifications. Handles deleted or
 * inaccessible tasks gracefully — notifications may outlive access. */
export function TaskPage() {
  const { taskId } = useParams<{ taskId: string }>();
  const query = useQuery({
    queryKey: ["task", taskId],
    queryFn: () => api.getTask(taskId!),
    retry: (count, error) => error instanceof ApiError && error.status >= 500 && count < 2,
  });

  return (
    <div className="flex h-full flex-col">
      <div className="border-b p-4">
        <Button variant="ghost" size="sm" onClick={() => window.history.back()}>
          <ArrowLeft /> Back
        </Button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto scrollbar-thin">
        {query.isLoading ? (
          <div className="p-6">
            <div className="h-5 w-64 animate-pulse rounded bg-muted" />
            <div className="mt-3 h-24 w-full animate-pulse rounded bg-muted" />
          </div>
        ) : query.isError ? (
          <div className="p-6">
            <ErrorState
              title="This task is no longer available"
              detail="It may have been deleted, or you no longer have access to its team."
              onRetry={() => void query.refetch()}
            />
          </div>
        ) : (
          <div className="mx-auto max-w-3xl p-6">
            <TaskDetailPanel task={query.data} onClose={() => window.history.back()} />
          </div>
        )}
      </div>
    </div>
  );
}
