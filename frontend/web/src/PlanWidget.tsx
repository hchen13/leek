// Plan / TODO widget — bottom-right of the workbench (REQUIREMENTS §2.6).
//
// Shows the plan the agent explicitly maintains via `update_plan`. It is
// display-only: it appears solely when a plan exists and never gates a
// reply. Each `plan_updated` event replaces the whole plan.

import { For, Show } from "solid-js";

import type { Plan, PlanStep } from "./types";

type PlanWidgetProps = { plan: () => Plan | null };

function stepMark(status: PlanStep["status"]): string {
  return status === "completed" ? "✓" : status === "in_progress" ? "▸" : "○";
}

export function PlanWidget(props: PlanWidgetProps) {
  const has = () => {
    const p = props.plan();
    return p != null && p.steps.length > 0;
  };

  return (
    <Show when={has()}>
      <aside class="plan-widget">
        <header class="plan-head">
          <h2>Plan / TODO</h2>
        </header>
        <Show when={props.plan()!.explanation}>
          <p class="plan-note">{props.plan()!.explanation}</p>
        </Show>
        <ul class="plan-steps">
          <For each={props.plan()!.steps}>
            {(s) => (
              <li classList={{ "plan-step": true, [`p-${s.status}`]: true }}>
                <span class="plan-mark">{stepMark(s.status)}</span>
                <span class="plan-text">{s.step}</span>
              </li>
            )}
          </For>
        </ul>
      </aside>
    </Show>
  );
}
