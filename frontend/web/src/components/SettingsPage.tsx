import { For, Show, createSignal, onMount } from "solid-js";

type SurfaceKey = "main" | "routing" | "compaction" | "subagent";

interface SurfaceTuning {
  reasoning_effort: string;
  verbosity: string;
}

type Tuning = Record<SurfaceKey, SurfaceTuning>;

interface SettingsPayload {
  tuning: Tuning;
  options: {
    reasoning_effort: string[];
    verbosity: string[];
  };
}

const SURFACE_LABELS: Record<SurfaceKey, { title: string; sub: string }> = {
  main: {
    title: "主 agent · main",
    sub: "合成最终回答的主调用。默认 xhigh，分析最深。",
  },
  routing: {
    title: "路由 · routing",
    sub: "分类 user message 是 chat 还是 task。轻量 classifier。",
  },
  compaction: {
    title: "上下文压缩 · compaction",
    sub: "对长会话生成结构化 handoff summary。不需要深度推理。",
  },
  subagent: {
    title: "子代理 · subagent",
    sub: "delegate_research 派出的二次视角。当前默认 minimal — 提高会显著增加 token。",
  },
};

const SURFACE_ORDER: SurfaceKey[] = ["main", "routing", "compaction", "subagent"];

export function SettingsPage() {
  const [tuning, setTuning] = createSignal<Tuning | null>(null);
  const [options, setOptions] = createSignal<SettingsPayload["options"] | null>(null);
  const [dirty, setDirty] = createSignal(false);
  const [saving, setSaving] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [savedAt, setSavedAt] = createSignal<number | null>(null);

  onMount(async () => {
    try {
      const r = await fetch("/api/v1/settings");
      if (!r.ok) {
        setError(`加载失败 (HTTP ${r.status})`);
        return;
      }
      const j: SettingsPayload = await r.json();
      setTuning(j.tuning);
      setOptions(j.options);
    } catch (e) {
      setError(`加载失败: ${e}`);
    }
  });

  function updateField(surface: SurfaceKey, field: keyof SurfaceTuning, value: string) {
    const t = tuning();
    if (!t) return;
    setTuning({ ...t, [surface]: { ...t[surface], [field]: value } });
    setDirty(true);
    setSavedAt(null);
  }

  async function save() {
    const t = tuning();
    if (!t) return;
    setSaving(true);
    setError(null);
    try {
      const r = await fetch("/api/v1/settings", {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(t),
      });
      if (!r.ok) {
        const body = await r.text();
        setError(`保存失败 (HTTP ${r.status}): ${body}`);
        return;
      }
      const j = await r.json();
      setTuning(j.tuning);
      setDirty(false);
      setSavedAt(Date.now());
    } catch (e) {
      setError(`保存失败: ${e}`);
    } finally {
      setSaving(false);
    }
  }

  return (
    <div style={{ padding: "24px 32px", "max-width": "780px", margin: "0 auto", "overflow-y": "auto" }}>
      <h1 style={{ "font-family": "var(--font-stencil)", "font-size": "20px", "letter-spacing": "0.04em", "text-transform": "uppercase", margin: "0 0 4px 0" }}>
        Settings
      </h1>
      <div style={{ "font-size": "12px", color: "var(--ink-3)", "margin-bottom": "24px" }}>
        各 surface 的 reasoning effort + verbosity。改完点保存，下一个 turn 立即生效。
      </div>

      <Show when={error()}>
        {(msg) => (
          <div style={{ background: "rgba(217, 119, 87, 0.12)", color: "var(--clay-deep)", padding: "10px 14px", "border-radius": "6px", "margin-bottom": "16px", "font-size": "12px" }}>
            {msg()}
          </div>
        )}
      </Show>

      <Show when={tuning() && options()} fallback={<div style={{ color: "var(--ink-3)", "font-size": "13px" }}>加载中…</div>}>
        <div style={{ display: "flex", "flex-direction": "column", gap: "20px" }}>
          <For each={SURFACE_ORDER}>
            {(key) => (
              <SurfaceRow
                surfaceKey={key}
                surface={tuning()![key]}
                options={options()!}
                onChange={(field, value) => updateField(key, field, value)}
              />
            )}
          </For>
        </div>

        <div style={{ display: "flex", gap: "12px", "margin-top": "28px", "align-items": "center" }}>
          <button
            disabled={!dirty() || saving()}
            onClick={save}
            style={{
              background: "linear-gradient(180deg, var(--clay), var(--clay-deep))",
              color: "#2a1810",
              "font-weight": "600",
              "font-family": "var(--font-mono)",
              "font-size": "12px",
              padding: "8px 18px",
              border: "none",
              "border-radius": "4px",
              cursor: !dirty() || saving() ? "default" : "pointer",
              opacity: !dirty() || saving() ? 0.55 : 1,
              "min-width": "92px",
            }}
          >
            {saving() ? "保存中…" : "保存"}
          </button>
          <Show when={savedAt() !== null && !dirty()}>
            <span style={{ "font-size": "11px", color: "var(--ink-3)" }}>已保存</span>
          </Show>
          <Show when={dirty()}>
            <span style={{ "font-size": "11px", color: "var(--clay-soft)" }}>● 未保存</span>
          </Show>
        </div>
      </Show>
    </div>
  );
}

function SurfaceRow(props: {
  surfaceKey: SurfaceKey;
  surface: SurfaceTuning;
  options: SettingsPayload["options"];
  onChange: (field: keyof SurfaceTuning, value: string) => void;
}) {
  const meta = SURFACE_LABELS[props.surfaceKey];
  return (
    <div style={{
      border: "1px solid var(--ink-line)",
      "border-radius": "8px",
      padding: "16px 18px",
      background: "var(--bg-1)",
    }}>
      <div style={{ "font-family": "var(--font-stencil)", "font-size": "13px", "letter-spacing": "0.04em", "text-transform": "uppercase", "margin-bottom": "4px" }}>
        {meta.title}
      </div>
      <div style={{ "font-size": "11px", color: "var(--ink-3)", "margin-bottom": "12px" }}>
        {meta.sub}
      </div>
      <div style={{ display: "grid", "grid-template-columns": "1fr 1fr", gap: "12px" }}>
        <SelectField
          label="reasoning_effort"
          value={props.surface.reasoning_effort}
          options={props.options.reasoning_effort}
          onChange={(v) => props.onChange("reasoning_effort", v)}
        />
        <SelectField
          label="verbosity"
          value={props.surface.verbosity}
          options={props.options.verbosity}
          onChange={(v) => props.onChange("verbosity", v)}
        />
      </div>
    </div>
  );
}

function SelectField(props: { label: string; value: string; options: string[]; onChange: (v: string) => void }) {
  return (
    <label style={{ display: "flex", "flex-direction": "column", gap: "4px" }}>
      <span style={{ "font-size": "10px", color: "var(--ink-3)", "letter-spacing": "0.05em", "text-transform": "uppercase" }}>
        {props.label}
      </span>
      <select
        value={props.value}
        onChange={(e) => props.onChange(e.currentTarget.value)}
        style={{
          padding: "6px 8px",
          background: "var(--bg-0)",
          color: "var(--ink-1)",
          border: "1px solid var(--ink-line)",
          "border-radius": "4px",
          "font-family": "var(--font-mono)",
          "font-size": "12px",
        }}
      >
        <For each={props.options}>{(opt) => <option value={opt}>{opt}</option>}</For>
      </select>
    </label>
  );
}
