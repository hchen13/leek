// SettingsPage — Phase 1 page (DESIGN.md §5.6 settings page).
//
// Replaces the M2.6 modal Settings.tsx with a page that takes over the
// main area (rail stays visible). Same field set, same PATCH wiring,
// re-skinned with design-system tokens. One new section: Theme toggle
// (dark / light).
//
// Logic is a port of Settings.tsx — same validation parser, same partial-
// PATCH semantics, same env-override badge. The visual shell is the
// novelty: section groups (Limits / API Keys / Behavior / Theme),
// each field a row with effective value + hint.
//
// State coverage:
//   - loading        → skeleton placeholder
//   - load error     → "无法加载设置" banner + retry button
//   - validation 400 → per-field error pinned to its row
//   - save ok        → "已保存" banner with config_path
//   - reset          → confirms then PATCHes null over every key

import { createSignal, For, onCleanup, Show } from "solid-js";

import { Icon } from "./Icon";
import { applyTheme, currentTheme, type Theme } from "../lib/theme";
import type { SettingsConfig, SettingsResponse } from "../types";

type SettingsPageProps = {
  onBack: () => void;
};

type FieldSpec = {
  key: keyof SettingsConfig;
  label: string;
  hint: string;
  unit: string;
  step: string;
  group: "limits" | "behavior";
};

const FIELDS: FieldSpec[] = [
  {
    key: "cost_cap_usd",
    label: "成本上限 (USD/turn)",
    hint: "0 = 不限制; 超过后该 turn 软停, 标记 cost_cap_exceeded",
    unit: "USD",
    step: "0.01",
    group: "limits",
  },
  {
    key: "idle_timeout_secs",
    label: "Stream 空闲超时",
    hint: "默认 180 (M3.6 加倍, xhigh + 长 tool turn 经常超 90s silence); 0 = 关闭",
    unit: "秒",
    step: "1",
    group: "limits",
  },
  {
    key: "wall_clock_secs",
    label: "每 turn 墙钟预算",
    hint: "默认 1800; 0 = 关闭",
    unit: "秒",
    step: "1",
    group: "limits",
  },
  {
    key: "max_iterations",
    label: "最大 iteration 数",
    hint: "留空 = 不限",
    unit: "次",
    step: "1",
    group: "limits",
  },
  {
    key: "doom_loop_threshold",
    label: "Doom-loop 阈值",
    hint: "连续相同 (tool, args) 多少次视为死循环。≥ 2, 默认 3",
    unit: "次",
    step: "1",
    group: "behavior",
  },
  {
    key: "auto_compact_threshold",
    label: "自动 compaction 触发比例",
    hint: "context 占用 ≥ 该值时触发 compaction。(0, 1], 默认 0.90",
    unit: "",
    step: "0.01",
    group: "behavior",
  },
  {
    key: "context_window",
    label: "Context window (tokens)",
    hint: "留空 = 用 model 默认",
    unit: "tokens",
    step: "1",
    group: "behavior",
  },
  {
    key: "builtin_url_warn_threshold",
    label: "Codex 重复 URL 警告阈值",
    hint: "同一 (action, URL) open ≥ 该次数时, 出 warning。默认 3, 0 = 关闭",
    unit: "次",
    step: "1",
    group: "behavior",
  },
  {
    key: "builtin_url_abort_threshold",
    label: "Codex 重复 URL 中止阈值",
    hint: "同一 (action, URL) open ≥ 该次数时, 强制 abort。默认 0 = 关闭",
    unit: "次",
    step: "1",
    group: "behavior",
  },
];

function parseInputValue(raw: string): number | null {
  if (raw.trim() === "") return null;
  const n = Number(raw);
  return Number.isFinite(n) ? n : null;
}

function formatEffective(v: number | null, unit: string): string {
  if (v == null) return "未启用";
  const s = String(v);
  return unit ? `${s} ${unit}` : s;
}

type FieldError = { field: string; message: string };

function parseValidationErrors(body: unknown): FieldError[] {
  if (!body || typeof body !== "object") return [];
  const env = body as Record<string, unknown>;
  const raw = env.error;
  if (typeof raw !== "string") return [];
  try {
    const inner = JSON.parse(raw) as { kind?: string; errors?: FieldError[] };
    if (inner.kind === "validation_failed" && Array.isArray(inner.errors)) {
      return inner.errors;
    }
  } catch {
    // fall through
  }
  return [{ field: "_", message: raw }];
}

export function SettingsPage(props: SettingsPageProps) {
  const [data, setData] = createSignal<SettingsResponse | null>(null);
  const [draft, setDraft] = createSignal<Record<string, string>>({});
  const [loading, setLoading] = createSignal(true);
  const [saving, setSaving] = createSignal(false);
  const [errors, setErrors] = createSignal<FieldError[]>([]);
  const [topError, setTopError] = createSignal<string | null>(null);
  const [savedAt, setSavedAt] = createSignal<number | null>(null);
  const [theme, setTheme] = createSignal<Theme>(currentTheme());

  const draftOf = (cfg: SettingsConfig, key: keyof SettingsConfig): string => {
    const v = cfg[key];
    return v == null ? "" : String(v);
  };

  const fillDraftFrom = (resp: SettingsResponse) => {
    const next: Record<string, string> = {};
    for (const f of FIELDS) next[f.key as string] = draftOf(resp.config, f.key);
    next.tushare_token = resp.config.tushare_token ?? "";
    next.reasoning_effort = resp.config.reasoning_effort ?? "";
    setDraft(next);
  };

  const load = async () => {
    setLoading(true);
    setTopError(null);
    try {
      const res = await fetch("/api/v1/settings");
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const body = (await res.json()) as SettingsResponse;
      setData(body);
      fillDraftFrom(body);
    } catch (e) {
      setTopError(`无法加载设置: ${(e as Error).message}`);
    } finally {
      setLoading(false);
    }
  };

  const buildPatchBody = (): Record<string, number | string | null> => {
    const cfg = data()?.config ?? {};
    const out: Record<string, number | string | null> = {};
    const d = draft();
    for (const f of FIELDS) {
      const raw = d[f.key as string] ?? "";
      const parsed = parseInputValue(raw);
      const current = cfg[f.key] ?? null;
      if (parsed !== current) out[f.key as string] = parsed;
    }
    const tokRaw = (d.tushare_token ?? "").trim();
    const tokDraft = tokRaw === "" ? null : tokRaw;
    const tokCurrent = cfg.tushare_token ?? null;
    if (tokDraft !== tokCurrent) out.tushare_token = tokDraft;
    const reRaw = (d.reasoning_effort ?? "").trim();
    const reDraft = reRaw === "" ? null : reRaw;
    const reCurrent = cfg.reasoning_effort ?? null;
    if (reDraft !== reCurrent) out.reasoning_effort = reDraft;
    return out;
  };

  const patchAndRefresh = async (body: Record<string, number | string | null>) => {
    setSaving(true);
    setErrors([]);
    setTopError(null);
    try {
      const res = await fetch("/api/v1/settings", {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      });
      if (!res.ok) {
        const errBody = await res.json().catch(() => null);
        const parsed = parseValidationErrors(errBody);
        if (parsed.length > 0) {
          setErrors(parsed);
          setTopError("有字段不通过校验, 请按提示修正后重新保存。");
        } else {
          setTopError(`保存失败: HTTP ${res.status}`);
        }
        return;
      }
      const body2 = (await res.json()) as SettingsResponse;
      setData(body2);
      fillDraftFrom(body2);
      setSavedAt(Date.now());
    } catch (e) {
      setTopError(`保存失败: ${(e as Error).message}`);
    } finally {
      setSaving(false);
    }
  };

  const save = async () => {
    const body = buildPatchBody();
    if (Object.keys(body).length === 0) {
      setSavedAt(Date.now());
      return;
    }
    await patchAndRefresh(body);
  };

  const reset = async () => {
    if (!confirm("将所有字段重置为内置默认值, 清空 config.json 中所有已存储字段。")) return;
    const body: Record<string, number | string | null> = {
      cost_cap_usd: null,
      idle_timeout_secs: null,
      wall_clock_secs: null,
      doom_loop_threshold: null,
      auto_compact_threshold: null,
      max_iterations: null,
      context_window: null,
      builtin_url_warn_threshold: null,
      builtin_url_abort_threshold: null,
      reasoning_effort: null,
    };
    await patchAndRefresh(body);
  };

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") props.onBack();
  };
  window.addEventListener("keydown", onKey);
  onCleanup(() => window.removeEventListener("keydown", onKey));

  void load();

  const errorFor = (key: string): string | null => {
    const hit = errors().find((e) => e.field === key);
    return hit?.message ?? null;
  };

  const switchTheme = (t: Theme) => {
    setTheme(t);
    applyTheme(t);
  };

  const effectiveCellFor = (f: FieldSpec) => {
    const d = data();
    if (!d) return null;
    const eff = d.effective[f.key as keyof typeof d.effective] as
      | { value: number | null; overridden_by_env: boolean }
      | undefined;
    if (!eff) return null;
    return (
      <div class="lk-set-eff">
        <span class="lk-set-eff-label">当前生效:</span>
        <span class="lk-set-eff-value lk-num">{formatEffective(eff.value, f.unit)}</span>
        <Show when={eff.overridden_by_env}>
          <span class="lk-set-env-badge" title="同名环境变量已设置, 会覆盖此字段">
            被环境变量 override
          </span>
        </Show>
      </div>
    );
  };

  const renderField = (f: FieldSpec) => (
    <div class="lk-set-field" data-field={f.key}>
      <label class="lk-set-label" for={`set-${f.key}`}>{f.label}</label>
      <input
        id={`set-${f.key}`}
        class="lk-input"
        type="number"
        step={f.step}
        placeholder="留空 = 不设置"
        value={draft()[f.key as string] ?? ""}
        disabled={saving()}
        onInput={(e) => setDraft({ ...draft(), [f.key as string]: e.currentTarget.value })}
      />
      <div class="lk-set-hint">{f.hint}</div>
      {effectiveCellFor(f)}
      <Show when={errorFor(f.key as string)}>
        <div class="lk-set-err">{errorFor(f.key as string)}</div>
      </Show>
    </div>
  );

  return (
    <section class="lk-settings" aria-label="Settings">
      <header class="lk-settings-head">
        <button
          class="lk-btn lk-btn--ghost lk-btn--sm"
          onClick={() => props.onBack()}
          type="button"
        >
          <Icon name="chevronL" size={14} />
          <span>返回</span>
        </button>
        <h1 class="lk-settings-title">设置</h1>
        <span class="lk-settings-head-spacer" />
      </header>

      <Show
        when={!loading()}
        fallback={<div class="lk-settings-loading">加载中…</div>}
      >
        <Show when={topError()}>
          <div class="lk-bar lk-bar--danger">{topError()}</div>
        </Show>
        <Show when={savedAt() != null && topError() == null}>
          <div class="lk-bar lk-bar--ok">
            已保存到 <code class="lk-mono">{data()?.config_path ?? "~/.leek/config.json"}</code>
          </div>
        </Show>

        <div class="lk-settings-body">
          <section class="lk-settings-section">
            <h2 class="lk-settings-section-title">Limits</h2>
            <For each={FIELDS.filter((f) => f.group === "limits")}>
              {(f) => renderField(f)}
            </For>
          </section>

          <section class="lk-settings-section">
            <h2 class="lk-settings-section-title">Behavior</h2>
            <For each={FIELDS.filter((f) => f.group === "behavior")}>
              {(f) => renderField(f)}
            </For>

            <div class="lk-set-field" data-field="reasoning_effort">
              <label class="lk-set-label" for="set-reasoning_effort">
                主 agent reasoning effort
              </label>
              <select
                id="set-reasoning_effort"
                class="lk-input"
                value={draft().reasoning_effort ?? ""}
                disabled={saving()}
                onInput={(e) =>
                  setDraft({ ...draft(), reasoning_effort: e.currentTarget.value })
                }
              >
                <option value="">(默认: medium)</option>
                <option value="minimal">minimal</option>
                <option value="low">low</option>
                <option value="medium">medium</option>
                <option value="high">high</option>
                <option value="xhigh">xhigh</option>
              </select>
              <div class="lk-set-hint">
                xhigh 更深思但更易撞 codex 长 stream 不稳; medium 推荐; low/minimal 让简单问题快答。
                deep-review subagent 仍以 xhigh 跑(独立 context), 不受此设置影响。
              </div>
              <div class="lk-set-eff">
                <span class="lk-set-eff-label">当前生效:</span>
                <span class="lk-set-eff-value lk-mono">
                  {String(data()?.effective?.reasoning_effort?.value ?? "medium")}
                </span>
                <Show when={data()?.effective?.reasoning_effort?.overridden_by_env}>
                  <span class="lk-set-env-badge">被环境变量 override</span>
                </Show>
              </div>
            </div>
          </section>

          <section class="lk-settings-section">
            <h2 class="lk-settings-section-title">API Keys</h2>
            <div class="lk-set-field" data-field="tushare_token">
              <label class="lk-set-label" for="set-tushare_token">
                Tushare Token (A 股数据主源)
              </label>
              <input
                id="set-tushare_token"
                class="lk-input"
                type="password"
                placeholder="留空 = 仅用 fallback(新浪/东方财富)"
                value={draft().tushare_token ?? ""}
                disabled={saving()}
                onInput={(e) =>
                  setDraft({ ...draft(), tushare_token: e.currentTarget.value })
                }
              />
              <div class="lk-set-hint">
                在 tushare.pro 注册免费 token, 粘贴此处启用 market_quote / get_financials /
                get_capital_flow 等工具的主数据源。
              </div>
              <div class="lk-set-eff">
                <span class="lk-set-eff-label">当前生效:</span>
                <span class="lk-set-eff-value lk-mono">
                  {(() => {
                    const eff = data()?.effective?.tushare_token;
                    if (!eff || eff.value == null) return "未设置";
                    const v = String(eff.value);
                    return `${v.slice(0, 4)}••••${v.slice(-4)}`;
                  })()}
                </span>
                <Show when={data()?.effective?.tushare_token?.overridden_by_env}>
                  <span class="lk-set-env-badge">被环境变量 override</span>
                </Show>
              </div>
            </div>
          </section>

          <section class="lk-settings-section">
            <h2 class="lk-settings-section-title">Theme</h2>
            <div class="lk-set-field">
              <label class="lk-set-label">主题</label>
              <div class="lk-theme-toggle" role="radiogroup" aria-label="主题">
                <button
                  classList={{
                    "lk-theme-btn": true,
                    "lk-theme-btn--active": theme() === "dark",
                  }}
                  type="button"
                  onClick={() => switchTheme("dark")}
                  role="radio"
                  aria-checked={theme() === "dark"}
                >
                  深色 (默认)
                </button>
                <button
                  classList={{
                    "lk-theme-btn": true,
                    "lk-theme-btn--active": theme() === "light",
                  }}
                  type="button"
                  onClick={() => switchTheme("light")}
                  role="radio"
                  aria-checked={theme() === "light"}
                >
                  浅色
                </button>
              </div>
              <div class="lk-set-hint">
                设计原生暗色; 浅色模式 Phase 4 polish。选择会立即生效, 偏好保存在浏览器。
              </div>
            </div>
          </section>
        </div>

        <footer class="lk-settings-foot">
          <div class="lk-settings-path">
            配置文件: <code class="lk-mono">{data()?.config_path ?? "~/.leek/config.json"}</code>
          </div>
          <div class="lk-settings-actions">
            <button
              class="lk-btn lk-btn--ghost"
              disabled={saving()}
              onClick={() => void reset()}
              type="button"
            >
              重置为默认
            </button>
            <button
              class="lk-btn lk-btn--primary"
              disabled={saving()}
              onClick={() => void save()}
              type="button"
            >
              {saving() ? "保存中…" : "保存"}
            </button>
          </div>
        </footer>
      </Show>
    </section>
  );
}
