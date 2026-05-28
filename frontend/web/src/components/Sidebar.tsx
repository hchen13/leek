// Sidebar — right column (280px). Hosts the corpus minimap and the
// per-turn plan. Both are passive surfaces — the user reads them but
// doesn't drive the agent from here.
//
// Layout (DESIGN.md §5.6 / §5.8):
//   - Top: corpus minimap 256×256 square (click → fullscreen overlay)
//   - Middle: --space-3 gap divider
//   - Bottom: Plan list (HIDDEN when no plan or 0 steps)
//
// State coverage:
//   - corpus loading/error/empty handled inside <CorpusMinimap />
//   - plan empty → not rendered (per spec: "无 plan → 整个 Plan 区域不渲染")
//   - plan running → in_progress dot pulses
//   - plan all completed → green ok dot

import { For, Show } from "solid-js";

import { CorpusMinimap } from "./CorpusMinimap";
import { SafeMarkdown } from "./SafeMarkdown";
import type { Activation, Plan, PlanStep } from "../types";

type Props = {
  activation: () => Activation;
  plan: () => Plan | null;
  onFullscreenCorpus: () => void;
};

function stepStatusClass(status: PlanStep["status"]): string {
  if (status === "completed") return "lk-plan-step--completed";
  if (status === "in_progress") return "lk-plan-step--in-progress";
  return "lk-plan-step--pending";
}

export function Sidebar(props: Props) {
  const hasPlan = () => {
    const p = props.plan();
    return p != null && p.steps.length > 0;
  };

  return (
    <aside class="lk-sidebar" aria-label="Sidebar">
      <section class="lk-sidebar-section lk-sidebar-section--minimap">
        <header class="lk-sidebar-section-head">
          <span class="lk-sidebar-section-title">corpus 知识图谱</span>
        </header>
        <CorpusMinimap
          activation={props.activation}
          onFullscreen={props.onFullscreenCorpus}
        />
      </section>

      <Show when={hasPlan()}>
        <section class="lk-sidebar-section lk-sidebar-section--plan">
          <header class="lk-sidebar-section-head">
            <span class="lk-sidebar-section-title">Plan / TODO</span>
          </header>
          <Show when={props.plan()!.explanation}>
            <p class="lk-plan-note">{props.plan()!.explanation}</p>
          </Show>
          <ul class="lk-plan-list">
            <For each={props.plan()!.steps}>
              {(s) => (
                <li class={`lk-plan-step ${stepStatusClass(s.status)}`}>
                  <span class="lk-plan-dot" aria-hidden="true">
                    <Show when={s.status === "in_progress"}>
                      <span class="lk-pulse lk-plan-dot-inner" />
                    </Show>
                  </span>
                  <SafeMarkdown text={s.step} inline class="lk-plan-text" />
                </li>
              )}
            </For>
          </ul>
        </section>
      </Show>
    </aside>
  );
}
