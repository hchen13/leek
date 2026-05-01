// 5 demo scenes from the design prototype. Eventually replaced by real
// app state derived from agent loop events. For now they're hardcoded
// fixtures used to recreate the prototype's visual states.

export type Scene = "idle" | "thinking-shallow" | "clarify" | "deep" | "delivered";

export const ALL_SCENES: Scene[] = ["idle", "thinking-shallow", "clarify", "deep", "delivered"];

export const SCENE_LABELS: Record<Scene, string> = {
  "idle": "A · Idle",
  "thinking-shallow": "B · Light query",
  "clarify": "C · Clarify",
  "deep": "D · Deep research",
  "delivered": "E · Delivered",
};
