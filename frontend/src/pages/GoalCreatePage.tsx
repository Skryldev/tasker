import { useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { api } from "@/api/client";
import { GoalFormDialog } from "@/pages/GoalsPage";
import { GoalDetailPage } from "@/pages/GoalDetailPage";

/** Route wrapper: /teams/:teamId/goals/new opens the create dialog on top of
 *  the team goals list. */
export function GoalCreatePage() {
  const { teamId } = useParams<{ teamId: string }>();
  const navigate = useNavigate();
  const [open, setOpen] = useState(true);
  if (!teamId) return null;
  return (
    <GoalFormDialog
      open={open}
      onOpenChange={(v) => {
        setOpen(v);
        if (!v) navigate(`/teams/${teamId}/goals`);
      }}
      teamId={teamId}
    />
  );
}

/** Route wrapper exposing the shared GoalDetailPage on /goals/:goalId. */
export function GoalDetailRoute() {
  return <GoalDetailPage />;
}

// Re-export api for convenience in tests
export { api };
