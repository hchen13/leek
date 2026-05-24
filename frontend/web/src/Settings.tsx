// Settings — the user-facing knob panel (M2.6, spec §B).
//
// Renders a modal overlay over the workbench (the App.tsx ⚙ button toggles
// it). One form for the seven guard fields in `Config` — for each field we
// show:
//
//   - the input bound to `config.<field>` (raw file value; null = "unset"),
//   - the effective value the gateway actually applies after env-var
//     layering (so a sleepy user can spot a stale env var), and
//   - a "⚠ 被环境变量 override" badge when the resolver's
//     `overridden_by_env` flag for that field is true.
//
// Submit PATCHes a partial document (only the fields the user actually
// changed) so a partial save can't accidentally clear other knobs. Reset
// PATCHes `{}` after first clearing every field locally — the backend
// merge semantics mean "send null" wins over "send absent", which we want
// here. Save / reset both re-`GET` the response so the effective values
// and override badges reflect what the server is now applying.
//
// Modal layout chosen over sidebar or a new page: 7 fields fit a single
// vertical scroll, the workbench underneath stays visible (so a user can
// glance at the warning bar that prompted them to open this), and we add
// zero dependencies (no router, no portal library). The escape key + the
// backdrop both close the panel.

import { createSignal, For, onCleanup, Show } from "solid-js";

import type { SettingsConfig, SettingsResponse } from "./types";

type SettingsProps = {
  /** Close handler — App owns the visible signal. */
  onClose: () => void;
};

// ── one declarative row per field ─────────────────────────────────────────
//
// Keeping the field list as data (not 7 hand-rolled JSX blocks) lets us
// reuse one "row" renderer and keeps the labels / placeholders one screen
// scroll instead of nine. `step` controls the HTML number spinner; `min`
// is informative only — we do not block invalid submissions client-side,
// the gateway's `Config::validate` returns per-field errors we then show.
type FieldSpec = {
  /** Key in `SettingsConfig` and in `effective`. */
  key: keyof SettingsConfig;
  label: string;
  /** Hint under the input — defaults + what `null` / `0` mean. */
  hint: string;
  /** Unit suffix shown after the effective number (`秒` / `USD` / `tokens`). */
  unit: string;
  /** HTML number step (1 for ints, "any" for floats). */
  step: string;
};

const FIELDS: FieldSpec[] = [
  {
    key: "cost_cap_usd",
    label: "成本上限 (USD per turn)",
    hint: "0 = 不限制；超过后该 turn 软停，标记 cost_cap_exceeded",
    unit: "USD",
    step: "0.01",
  },
  {
    key: "idle_timeout_secs",
    label: "Stream 空闲超时 (秒)",
    hint: "默认 90；0 = 关闭。SSE 长时间没字节就 abort 当前 iteration。",
    unit: "秒",
    step: "1",
  },
  {
    key: "wall_clock_secs",
    label: "每 turn 墙钟预算 (秒)",
    hint: "默认 1800；0 = 关闭。",
    unit: "秒",
    step: "1",
  },
  {
    key: "max_iterations",
    label: "最大 iteration 数",
    hint: "留空 = 不限。",
    unit: "次",
    step: "1",
  },
  {
    key: "doom_loop_threshold",
    label: "Doom-loop 阈值",
    hint: "连续相同 (tool, args) 调用多少次后视为死循环。≥ 2，默认 3。",
    unit: "次",
    step: "1",
  },
  {
    key: "auto_compact_threshold",
    label: "自动 compaction 触发比例",
    hint: "context 占用比例 ≥ 该值时触发 compaction。(0, 1]，默认 0.90。",
    unit: "",
    step: "0.01",
  },
  {
    key: "context_window",
    label: "Context window (tokens)",
    hint: "留空 = 用 model 默认。仅用于测试或老 model 微调。",
    unit: "tokens",
    step: "1",
  },
];

/** Parse one input value into the JSON we'd PATCH. Empty string → null
 *  (= "unset this field"); anything else passes through `Number`. The
 *  gateway re-validates and 400s on garbage. */
function parseInputValue(raw: string): number | null {
  if (raw.trim() === "") return null;
  const n = Number(raw);
  return Number.isFinite(n) ? n : null;
}

/** Render `null` → "—", number → toString with the unit. Used to display
 *  the effective value next to each input. */
function formatEffective(v: number | null, unit: string): string {
  if (v == null) return "未启用";
  const s = String(v);
  return unit ? `${s} ${unit}` : s;
}

/** One validation error from the gateway — `{ field, message }`. */
type FieldError = { field: string; message: string };

/** Decode the gateway's 400 body. The settings handler wraps a JSON
 *  string (`{kind, errors}`) inside `ApiError`'s `{error}` envelope; we
 *  parse twice. Falls back to one generic error if the shape is unknown. */
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
    // Not the validation shape — fall through.
  }
  return [{ field: "_", message: raw }];
}

export function Settings(props: SettingsProps) {
  const [data, setData] = createSignal<SettingsResponse | null>(null);
  // The form's per-field draft text. We hold strings (not numbers) so the
  // user can clear a field to mean null — `Number("") === 0` would lose
  // that signal.
  const [draft, setDraft] = createSignal<Record<string, string>>({});
  const [loading, setLoading] = createSignal(true);
  const [saving, setSaving] = createSignal(false);
  const [errors, setErrors] = createSignal<FieldError[]>([]);
  const [topError, setTopError] = createSignal<string | null>(null);
  const [savedAt, setSavedAt] = createSignal<number | null>(null);

  // Pull the field value out of a SettingsConfig, coerced to a draft
  // string (number → "n", null/undefined → "").
  const draftOf = (cfg: SettingsConfig, key: keyof SettingsConfig): string => {
    const v = cfg[key];
    return v == null ? "" : String(v);
  };

  const fillDraftFrom = (resp: SettingsResponse) => {
    const next: Record<string, string> = {};
    for (const f of FIELDS) next[f.key] = draftOf(resp.config, f.key);
    // M3 vendor token (string field, not in FIELDS — handled inline)
    next.tushare_token = resp.config.tushare_token ?? "";
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
      setTopError(`无法加载设置：${(e as Error).message}`);
    } finally {
      setLoading(false);
    }
  };

  /** Build the partial PATCH body — only fields whose draft differs from
   *  the loaded `config` go in. This keeps a save from re-asserting fields
   *  the user didn't touch, which matters for the env-override case (we
   *  don't want to "save" the env-resolved value back as the file value). */
  const buildPatchBody = (): Record<string, number | string | null> => {
    const cfg = data()?.config ?? {};
    const out: Record<string, number | string | null> = {};
    const d = draft();
    for (const f of FIELDS) {
      const raw = d[f.key] ?? "";
      const parsed = parseInputValue(raw);
      const current = cfg[f.key] ?? null;
      if (parsed !== current) {
        out[f.key as string] = parsed;
      }
    }
    // M3 vendor token — string field, separate handling.
    const tokRaw = (d.tushare_token ?? "").trim();
    const tokDraft = tokRaw === "" ? null : tokRaw;
    const tokCurrent = cfg.tushare_token ?? null;
    if (tokDraft !== tokCurrent) {
      out.tushare_token = tokDraft;
    }
    return out;
  };

  /** PATCH with the given body — `{}` is the "reset to defaults" path
   *  after `clearAllFields` writes nulls. */
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
          setTopError("有字段不通过校验，请按提示修正后重新保存。");
        } else {
          setTopError(`保存失败：HTTP ${res.status}`);
        }
        return;
      }
      const body2 = (await res.json()) as SettingsResponse;
      setData(body2);
      fillDraftFrom(body2);
      setSavedAt(Date.now());
    } catch (e) {
      setTopError(`保存失败：${(e as Error).message}`);
    } finally {
      setSaving(false);
    }
  };

  const save = async () => {
    const body = buildPatchBody();
    if (Object.keys(body).length === 0) {
      // Nothing changed — still flash a saved confirmation so the click
      // doesn't feel silent, but skip the network round-trip.
      setSavedAt(Date.now());
      return;
    }
    await patchAndRefresh(body);
  };

  /** Reset = set every field back to the gateway's built-in default. The
   *  backend `Config::merge` only overwrites fields where the patch is
   *  `Some`, so a `null` patch is a no-op — we cannot "clear" a stored
   *  value through PATCH. Instead we PATCH the defaults explicitly, which
   *  is observationally identical for the resolver (a stored `cost_cap_usd
   *  = 0` and an unstored one both disable the cap).
   *
   *  `max_iterations` and `context_window` have no "0 = unset" idiom —
   *  the validator rejects 0 for both — so we send them as `null` (a
   *  no-op if the user had set them) and tell the user to clear them by
   *  hand if needed. The defaults below match
   *  `gateway::agent::guards::resolve`'s built-ins. */
  const reset = async () => {
    if (
      !confirm(
        "将所有字段重置为内置默认值。\n" +
          "注意：max_iterations 与 context_window 无法通过 reset 清除（后端 merge 不支持清空字段），" +
          "如已设置请手动留空并保存单独字段。",
      )
    )
      return;
    const body: Record<string, number | null> = {
      cost_cap_usd: 0, // 0 = disabled (same as unset)
      idle_timeout_secs: 90,
      wall_clock_secs: 1800,
      doom_loop_threshold: 3,
      auto_compact_threshold: 0.9,
      max_iterations: null, // backend merge ignores null; advised in confirm()
      context_window: null,
    };
    await patchAndRefresh(body);
  };

  // Escape closes the modal — easier on a keyboard-driven user than
  // hunting for the X. We attach to `keydown` on window so it fires
  // regardless of focus (the inputs don't `preventDefault`).
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") props.onClose();
  };
  window.addEventListener("keydown", onKey);
  onCleanup(() => window.removeEventListener("keydown", onKey));

  // Kick off the initial load. We don't await it — Solid renders the
  // loading shell first and the form once `data()` populates.
  void load();

  // ── per-field render helpers (kept inline so they read state via the
  //    `data` / `errors` signals without prop-drilling) ─────────────────
  const errorFor = (key: string): string | null => {
    const hit = errors().find((e) => e.field === key);
    return hit?.message ?? null;
  };

  const effectiveCellFor = (f: FieldSpec) => {
    const d = data();
    if (!d) return null;
    const eff = d.effective[f.key as keyof typeof d.effective] as
      | { value: number | null; overridden_by_env: boolean }
      | undefined;
    if (!eff) return null;
    return (
      <div class="settings-effective">
        <span class="settings-eff-label">当前生效：</span>
        <span class="settings-eff-value">{formatEffective(eff.value, f.unit)}</span>
        <Show when={eff.overridden_by_env}>
          <span class="settings-env-badge" title="同名环境变量已设置，会覆盖此字段">
            ⚠ 被环境变量 override
          </span>
        </Show>
      </div>
    );
  };

  return (
    <div class="settings-overlay" onClick={(e) => e.target === e.currentTarget && props.onClose()}>
      <div class="settings-modal" role="dialog" aria-label="Settings">
        <header class="settings-head">
          <h2>设置</h2>
          <button class="settings-close" onClick={props.onClose} aria-label="关闭设置">
            ×
          </button>
        </header>

        <Show
          when={!loading()}
          fallback={<div class="settings-loading">加载中…</div>}
        >
          <Show when={topError()}>
            <div class="settings-banner err">{topError()}</div>
          </Show>
          <Show when={savedAt() != null && topError() == null}>
            <div class="settings-banner ok">已保存到 {data()?.config_path}</div>
          </Show>

          <div class="settings-body">
            <For each={FIELDS}>
              {(f) => (
                <div class="settings-field" data-field={f.key}>
                  <label class="settings-label" for={`settings-${f.key}`}>
                    {f.label}
                  </label>
                  <input
                    id={`settings-${f.key}`}
                    class="settings-input"
                    type="number"
                    step={f.step}
                    placeholder="留空 = 不设置"
                    value={draft()[f.key as string] ?? ""}
                    disabled={saving()}
                    onInput={(e) => {
                      setDraft({ ...draft(), [f.key]: e.currentTarget.value });
                    }}
                  />
                  <div class="settings-hint">{f.hint}</div>
                  {effectiveCellFor(f)}
                  <Show when={errorFor(f.key as string)}>
                    <div class="settings-field-err">{errorFor(f.key as string)}</div>
                  </Show>
                </div>
              )}
            </For>

            {/* M3 vendor 凭证 section — single string field, separate from numeric FIELDS */}
            <div class="settings-field" data-field="tushare_token">
              <label class="settings-label" for="settings-tushare_token">
                Tushare Token (A 股数据主源)
              </label>
              <input
                id="settings-tushare_token"
                class="settings-input"
                type="password"
                placeholder="留空 = 仅用 fallback(新浪/东方财富)"
                value={draft().tushare_token ?? ""}
                disabled={saving()}
                onInput={(e) => {
                  setDraft({ ...draft(), tushare_token: e.currentTarget.value });
                }}
              />
              <div class="settings-hint">
                在 <a href="https://tushare.pro/register" target="_blank" rel="noopener noreferrer">tushare.pro</a> 注册免费 token,粘贴到此处启用 market_quote / get_financials / get_capital_flow 等工具的主数据源。空时自动走新浪 + 东方财富 fallback(覆盖率较低)。
              </div>
              <div class="settings-effective">
                <span class="settings-eff-label">当前生效:</span>
                <span class="settings-eff-value">
                  {(() => {
                    const eff = data()?.effective?.tushare_token;
                    if (!eff || eff.value == null) return "未设置";
                    return `已设置(${String(eff.value).slice(0, 4)}••••${String(eff.value).slice(-4)})`;
                  })()}
                </span>
                <Show when={data()?.effective?.tushare_token?.overridden_by_env}>
                  <span class="settings-env-badge" title="LEEK_TUSHARE_TOKEN 环境变量优先于此字段">⚠ 被环境变量 override</span>
                </Show>
              </div>
              <Show when={errorFor("tushare_token")}>
                <div class="settings-field-err">{errorFor("tushare_token")}</div>
              </Show>
            </div>
          </div>

          <footer class="settings-foot">
            <div class="settings-path">
              配置文件：<code>{data()?.config_path ?? "~/.leek/config.json"}</code>
            </div>
            <div class="settings-actions">
              <button class="settings-btn ghost" disabled={saving()} onClick={() => void reset()}>
                重置为默认
              </button>
              <button class="settings-btn primary" disabled={saving()} onClick={() => void save()}>
                {saving() ? "保存中…" : "保存"}
              </button>
            </div>
          </footer>
        </Show>
      </div>
    </div>
  );
}
