import { createContext, useContext } from "react";
import type { ToolId } from "../model/catalog.ts";

export type Section =
  | "overview"
  | "agents"
  | "tasks"
  | "jobs"
  | "projects"
  | "memory"
  | "automations"
  | "tools"
  | "models"
  | "activity"
  | "settings";

export interface NavTarget {
  section: Section;
  /** Tasks: the task to open; `null` starts a new one. */
  taskId?: string | null;
  /** Tasks and Memory: start filtered to this project. */
  projectId?: string;
  tool?: ToolId;
  /** Jobs: the record to open. */
  recordId?: string;
}

export const NavContext = createContext<(target: NavTarget) => void>(() => undefined);

export function useGoTo() {
  return useContext(NavContext);
}
