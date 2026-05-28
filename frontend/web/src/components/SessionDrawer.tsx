// SessionDrawer — left-sliding 320px panel with the session list
// (DESIGN.md §5.5).
//
// Trigger: ChatColumn ≡ button. Closes on Esc / backdrop / re-press ≡.
// Sessions group by recency: today / 本周 / 本月 / 更早. The current
// session is highlighted with a tinted background + accent indicator bar.
//
// State coverage:
//   - sessions loading      → loading placeholder
//   - no sessions yet       → empty placeholder + "新建" button
//   - no search match       → "没有匹配的会话"
//   - normal                → grouped list

import { createMemo, createSignal, For, onCleanup, Show } from "solid-js";

import { Icon } from "./Icon";
import type { Session } from "../types";

type Props = {
  open: () => boolean;
  /** Sessions list (most-recent-first per the API). */
  sessions: () => Session[];
  loading: () => boolean;
  /** Currently-selected session id (or null). */
  currentId: () => string | null;
  onClose: () => void;
  onPick: (id: string) => void;
  onNew: () => void;
  /** Optional — Phase 2 may wire delete/rename. */
  onDelete?: (id: string) => void;
};

type Bucket = { id: string; label: string; items: Session[] };

/** Group sessions by `last_active_at` recency. "today" = same calendar
 *  day; "本周" = same ISO week; "本月" = same year+month; else "更早". */
function groupSessions(sessions: Session[]): Bucket[] {
  const now = new Date();
  const today = startOfDay(now).getTime();
  const weekStart = startOfWeek(now).getTime();
  const monthStart = startOfMonth(now).getTime();

  const buckets: Record<string, Bucket> = {
    today: { id: "today", label: "今天", items: [] },
    week: { id: "week", label: "本周", items: [] },
    month: { id: "month", label: "本月", items: [] },
    older: { id: "older", label: "更早", items: [] },
  };

  for (const s of sessions) {
    const ts = Date.parse(s.last_active_at);
    if (Number.isNaN(ts)) {
      buckets.older.items.push(s);
      continue;
    }
    if (ts >= today) buckets.today.items.push(s);
    else if (ts >= weekStart) buckets.week.items.push(s);
    else if (ts >= monthStart) buckets.month.items.push(s);
    else buckets.older.items.push(s);
  }
  return [buckets.today, buckets.week, buckets.month, buckets.older].filter(
    (b) => b.items.length > 0,
  );
}

function startOfDay(d: Date): Date {
  const out = new Date(d);
  out.setHours(0, 0, 0, 0);
  return out;
}
function startOfWeek(d: Date): Date {
  const out = startOfDay(d);
  const day = (out.getDay() + 6) % 7; // Monday=0, Sunday=6
  out.setDate(out.getDate() - day);
  return out;
}
function startOfMonth(d: Date): Date {
  const out = startOfDay(d);
  out.setDate(1);
  return out;
}

function relativeTime(iso: string): string {
  const ts = Date.parse(iso);
  if (Number.isNaN(ts)) return "";
  const diffSec = (Date.now() - ts) / 1000;
  if (diffSec < 60) return "刚刚";
  if (diffSec < 3600) return `${Math.floor(diffSec / 60)} 分钟前`;
  if (diffSec < 86400) return `${Math.floor(diffSec / 3600)} 小时前`;
  if (diffSec < 7 * 86400) return `${Math.floor(diffSec / 86400)} 天前`;
  return new Date(ts).toISOString().slice(0, 10);
}

export function SessionDrawer(props: Props) {
  const [query, setQuery] = createSignal("");

  // Esc to close. We attach to window; ChatColumn's keydown is for the
  // textarea only so there's no conflict.
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape" && props.open()) props.onClose();
  };
  window.addEventListener("keydown", onKey);
  onCleanup(() => window.removeEventListener("keydown", onKey));

  const filtered = createMemo(() => {
    const q = query().trim().toLowerCase();
    if (!q) return props.sessions();
    return props.sessions().filter((s) => {
      const t = (s.title ?? "").toLowerCase();
      return t.includes(q) || s.id.includes(q);
    });
  });

  const buckets = createMemo(() => groupSessions(filtered()));

  return (
    <Show when={props.open()}>
      <div
        class="lk-drawer-backdrop"
        onClick={() => props.onClose()}
        aria-hidden="true"
      />
      <aside
        class="lk-drawer"
        role="dialog"
        aria-label="会话列表"
      >
        <header class="lk-drawer-head">
          <h2 class="lk-drawer-title">会话</h2>
          <button
            class="lk-icon-btn"
            onClick={() => props.onClose()}
            type="button"
            title="关闭"
            aria-label="关闭"
          >
            <Icon name="x" size={14} />
          </button>
        </header>

        <div class="lk-drawer-search">
          <span class="lk-drawer-search-icon" aria-hidden="true">
            <Icon name="search" size={14} />
          </span>
          <input
            class="lk-drawer-search-input"
            placeholder="搜索会话标题或 id"
            value={query()}
            onInput={(e) => setQuery(e.currentTarget.value)}
          />
          <Show when={query()}>
            <button
              class="lk-icon-btn lk-icon-btn--sm"
              onClick={() => setQuery("")}
              type="button"
              title="清除"
              aria-label="清除搜索"
            >
              <Icon name="x" size={12} />
            </button>
          </Show>
        </div>

        <div class="lk-drawer-body">
          <Show
            when={!props.loading()}
            fallback={
              <div class="lk-empty">
                <span class="lk-empty-title">加载中…</span>
              </div>
            }
          >
            <Show
              when={props.sessions().length > 0}
              fallback={
                <div class="lk-empty">
                  <span class="lk-empty-title">还没有会话</span>
                  <span class="lk-empty-hint">点击下方"新建"开始第一次研究。</span>
                </div>
              }
            >
              <Show
                when={filtered().length > 0}
                fallback={
                  <div class="lk-empty">
                    <span class="lk-empty-title">没有匹配的会话</span>
                    <span class="lk-empty-hint">尝试别的关键字。</span>
                  </div>
                }
              >
                <For each={buckets()}>
                  {(b) => (
                    <section class="lk-drawer-bucket">
                      <header class="lk-drawer-bucket-head">{b.label}</header>
                      <ul class="lk-drawer-list">
                        <For each={b.items}>
                          {(s) => {
                            const isCurrent = () => s.id === props.currentId();
                            return (
                              <li
                                classList={{
                                  "lk-drawer-item": true,
                                  "lk-drawer-item--current": isCurrent(),
                                }}
                                onClick={() => {
                                  props.onPick(s.id);
                                  props.onClose();
                                }}
                              >
                                <Show when={isCurrent()}>
                                  <span
                                    class="lk-drawer-item-indicator"
                                    aria-hidden="true"
                                  />
                                </Show>
                                <div class="lk-drawer-item-row">
                                  <span class="lk-drawer-item-title">
                                    {s.title ?? "(未命名)"}
                                  </span>
                                  <span class="lk-drawer-item-time lk-num">
                                    {relativeTime(s.last_active_at)}
                                  </span>
                                </div>
                                <span class="lk-drawer-item-id lk-mono">
                                  {s.id.slice(0, 8)}
                                </span>
                              </li>
                            );
                          }}
                        </For>
                      </ul>
                    </section>
                  )}
                </For>
              </Show>
            </Show>
          </Show>
        </div>

        <footer class="lk-drawer-foot">
          <button
            class="lk-btn lk-btn--secondary lk-btn--block"
            onClick={() => {
              props.onNew();
              props.onClose();
            }}
            type="button"
          >
            <Icon name="plus" size={14} />
            <span>新建会话</span>
          </button>
        </footer>
      </aside>
    </Show>
  );
}
