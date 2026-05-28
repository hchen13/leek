// CardShell — the framework every Canvas card uses (DESIGN.md §5.1).
//
// Provides the consistent header (icon + title + status pill + optional
// actions) and content slot. Width comes from a `size` class (lk-card--sm
// / md / lg / xl / full) — never inline. Status maps to a pill that picks
// up the right token (`--info` / `--ok` / `--warn` / `--danger`).
//
// Phase 1 doesn't implement collapse on header click — that's Phase 2.
// `raw-toggle` action is also Phase-2 wiring; the action slot accepts a
// list of buttons and the caller renders them.

import { For, Show, type JSX } from "solid-js";

import { Icon, type IconName } from "./Icon";

export type CardSize = "sm" | "md" | "lg" | "xl" | "full";
export type CardStatus = "running" | "ok" | "warn" | "danger" | "info";

export type CardAction = {
  /** Stable id so `For` keys correctly when the action list changes. */
  id: string;
  icon: IconName;
  label: string;
  onClick: () => void;
  /** Disabled state — applies the standard disabled token + cursor. */
  disabled?: boolean;
};

type CardShellProps = {
  size?: CardSize; // default "md"
  /** Icon for the card head — choose based on the tool / artifact kind. */
  icon?: IconName;
  /** The visible card title — e.g. "stock_overview · 600519.SH". */
  title: string;
  /** Optional small subtitle next to the title (right of `·`). */
  subtitle?: string;
  /** Status semantics — pills picks the right token automatically. */
  status?: CardStatus;
  /** Human label for the status pill — defaults to a status-name in CN. */
  statusLabel?: string;
  /** Action buttons rendered on the right side of the header. */
  actions?: CardAction[];
  /** Stable id (for #card-id deep-linking from chat summary). */
  id?: string;
  /** True = visually flash on highlight; toggled by parent via class. */
  highlighted?: boolean;
  /** Children — the actual content area (Phase 1 = GenericToolCard). */
  children: JSX.Element;
};

const STATUS_LABEL_DEFAULT: Record<CardStatus, string> = {
  running: "运行中",
  ok: "完成",
  warn: "警告",
  danger: "失败",
  info: "信息",
};

export function CardShell(props: CardShellProps) {
  const size = () => props.size ?? "md";
  const status = () => props.status;
  return (
    <article
      id={props.id ? `card-${props.id}` : undefined}
      classList={{
        "lk-card": true,
        [`lk-card--${size()}`]: true,
        "lk-card--highlighted": Boolean(props.highlighted),
        [`lk-card--status-${status()}`]: status() != null,
      }}
    >
      <header class="lk-card-head">
        <Show when={props.icon}>
          <span class="lk-card-head-icon" aria-hidden="true">
            <Icon name={props.icon!} size={18} />
          </span>
        </Show>
        <span class="lk-card-head-title" title={props.title}>
          {props.title}
        </span>
        <Show when={props.subtitle}>
          <span class="lk-card-head-sub lk-mono">{props.subtitle}</span>
        </Show>
        <Show when={status() != null}>
          <span
            classList={{
              "lk-card-status": true,
              [`lk-card-status--${status()}`]: true,
            }}
          >
            <Show when={status() === "running"}>
              <span class="lk-card-status-dot lk-pulse" aria-hidden="true" />
            </Show>
            {props.statusLabel ?? STATUS_LABEL_DEFAULT[status()!]}
          </span>
        </Show>
        <span class="lk-card-head-spacer" />
        <Show when={props.actions && props.actions.length > 0}>
          <div class="lk-card-actions">
            <For each={props.actions ?? []}>
              {(a) => (
                <button
                  class="lk-icon-btn lk-icon-btn--sm"
                  type="button"
                  onClick={() => a.onClick()}
                  disabled={a.disabled}
                  title={a.label}
                  aria-label={a.label}
                >
                  <Icon name={a.icon} size={14} />
                </button>
              )}
            </For>
          </div>
        </Show>
      </header>
      <div class="lk-card-body">{props.children}</div>
    </article>
  );
}
