// Rail — the 48px vertical nav (DESIGN.md §5.4 / §4.3 layout grid).
//
// Owns the workbench's primary navigation. Phase 1 has a single chat tab
// (sessions live inside the drawer, not as rail buttons). Bottom slot
// holds the settings gear that swaps the main area for the Settings page.
//
// State coverage:
//   - tab active → accent indicator bar on the left + accent foreground
//   - tab idle   → muted icon, no bar
//   - settings page open → settings button is "active" instead of chat
//   - all buttons preserve their hit area (36×36) on small viewports

import { For, Show } from "solid-js";

import { Icon, type IconName } from "./Icon";

export type RailTab = "chat" | "settings";

type Props = {
  /** Which workspace is currently shown — chat or settings. */
  active: () => RailTab;
  /** Click a top-row tab (Phase 1: only chat). */
  onTab: (t: RailTab) => void;
};

type TopTab = { id: RailTab; icon: IconName; label: string };
const TOP_TABS: TopTab[] = [
  { id: "chat", icon: "chat", label: "聊天" },
];

export function Rail(props: Props) {
  return (
    <nav class="lk-rail" aria-label="主导航">
      {/* Brand mark — square, accent-tinted on hover but not interactive. */}
      <div class="lk-rail-brand" title="L.E.E.K">
        L
      </div>
      <div class="lk-rail-spacer-sm" />
      <For each={TOP_TABS}>
        {(tab) => {
          const isActive = () => props.active() === tab.id;
          return (
            <button
              classList={{
                "lk-rail-btn": true,
                "lk-rail-btn--active": isActive(),
              }}
              onClick={() => props.onTab(tab.id)}
              type="button"
              title={tab.label}
              aria-label={tab.label}
              aria-current={isActive() ? "page" : undefined}
            >
              <Show when={isActive()}>
                <span class="lk-rail-indicator" aria-hidden="true" />
              </Show>
              <Icon name={tab.icon} size={18} />
            </button>
          );
        }}
      </For>
      <div class="lk-rail-spacer" />
      <button
        classList={{
          "lk-rail-btn": true,
          "lk-rail-btn--active": props.active() === "settings",
        }}
        onClick={() => props.onTab("settings")}
        type="button"
        title="设置"
        aria-label="设置"
        aria-current={props.active() === "settings" ? "page" : undefined}
      >
        <Show when={props.active() === "settings"}>
          <span class="lk-rail-indicator" aria-hidden="true" />
        </Show>
        <Icon name="settings" size={18} />
      </button>
    </nav>
  );
}
