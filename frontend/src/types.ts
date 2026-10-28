import type { Task } from "@/api/client";

/** A task row in the My Tasks view carries team/project context. */
export interface MyTaskRow extends Task {
  team_name: string;
  project_name: string;
}
