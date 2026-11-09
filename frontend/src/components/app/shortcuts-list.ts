/** Global keyboard shortcuts registry (Phase 10.5 §6.2, Phase 12 additions). */
export const SHORTCUTS: { keys: string; action: string }[] = [
  { keys: "⌘/Ctrl + K", action: "Open the command palette" },
  { keys: "C", action: "Create a new task" },
  { keys: "/", action: "Focus search" },
  { keys: "G then T", action: "Go to My Tasks" },
  { keys: "G then P", action: "Go to Projects" },
  { keys: "G then G", action: "Go to Goals" },
  { keys: "G then C", action: "Go to Capacity" },
  { keys: "⌘/Ctrl + .", action: "Toggle the detail sidebar" },
  { keys: "⌘/Ctrl + ⇧ + O", action: "Open the focused task as a full page" },
  { keys: "⌘/Ctrl + B", action: "Toggle the navigation sidebar" },
  { keys: "⌘/Ctrl + ,", action: "Open Settings" },
  { keys: "?", action: "Keyboard shortcuts cheatsheet" },
  { keys: "Esc", action: "Close dialogs and panels" },
];
