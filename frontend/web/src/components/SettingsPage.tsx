import { createSignal, onMount, Show } from "solid-js";

interface DataProviderStatus {
  provider_name: string;
  source?: string;
  configured: boolean;
  enabled: boolean;
  token_last4?: string | null;
  updated_at?: string;
  last_error?: string | null;
  last_error_at?: string | null;
}

interface SettingsResponse {
  data_providers: DataProviderStatus[];
}

type UpDownScheme = "cn" | "intl";

export function SettingsPage() {
  const [status, setStatus] = createSignal<DataProviderStatus | null>(null);
  const [token, setToken] = createSignal("");
  const [enabled, setEnabled] = createSignal(true);
  const [busy, setBusy] = createSignal(false);
  const [message, setMessage] = createSignal("");
  const [error, setError] = createSignal("");
  const [scheme, setSchemeSignal] = createSignal<UpDownScheme>(
    localStorage.getItem("lk-updown") === "intl" ? "intl" : "cn",
  );

  // Applies instantly (no save button) — pure client preference. Flips the
  // --up / --down tokens app-wide via the documentElement attribute.
  function setScheme(next: UpDownScheme) {
    setSchemeSignal(next);
    localStorage.setItem("lk-updown", next);
    if (next === "intl") document.documentElement.dataset.updown = "intl";
    else delete document.documentElement.dataset.updown;
  }

  async function load() {
    setError("");
    const r = await fetch("/api/v1/settings");
    if (!r.ok) {
      setError("读取 settings 失败");
      return;
    }
    const data = (await r.json()) as SettingsResponse;
    const tushare = data.data_providers.find((p) => p.provider_name === "tushare") ?? null;
    setStatus(tushare);
    setEnabled(tushare?.enabled ?? true);
  }

  async function save() {
    setBusy(true);
    setMessage("");
    setError("");
    try {
      const body: { token?: string; enabled: boolean } = { enabled: enabled() };
      if (token().trim()) body.token = token().trim();
      const r = await fetch("/api/v1/settings/tushare", {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      });
      const data = await r.json().catch(() => null);
      if (!r.ok) {
        setError(data?.error?.message ?? "保存 Tushare token 失败");
        return;
      }
      setStatus(data.provider);
      setToken("");
      setMessage("已保存");
    } finally {
      setBusy(false);
    }
  }

  onMount(() => {
    void load();
  });

  return (
    <main class="lk-settings">
      <div class="lk-settings-head">
        <div>
          <div class="lk-settings-kicker">SETTINGS</div>
          <h1>设置</h1>
        </div>
        <button class="lk-settings-refresh" onClick={() => void load()} disabled={busy()}>
          refresh
        </button>
      </div>

      <section class="lk-settings-card">
        <div class="lk-settings-card-head">
          <div>
            <div class="lk-settings-title">涨跌配色</div>
            <div class="lk-settings-sub">行情涨跌的红绿习惯,即时生效</div>
          </div>
        </div>
        <div class="lk-settings-segment" role="radiogroup" aria-label="涨跌配色">
          <button
            type="button"
            role="radio"
            aria-checked={scheme() === "cn"}
            class={scheme() === "cn" ? "active" : ""}
            onClick={() => setScheme("cn")}
          >
            <span class="lk-seg-swatch"><i style={{ background: "#e35b4e" }} /><i style={{ background: "#5fb98f" }} /></span>
            红涨绿跌 · 中国大陆
          </button>
          <button
            type="button"
            role="radio"
            aria-checked={scheme() === "intl"}
            class={scheme() === "intl" ? "active" : ""}
            onClick={() => setScheme("intl")}
          >
            <span class="lk-seg-swatch"><i style={{ background: "#5fb98f" }} /><i style={{ background: "#e35b4e" }} /></span>
            绿涨红跌 · 国际惯例
          </button>
        </div>
      </section>

      <section class="lk-settings-card">
        <div class="lk-settings-card-head">
          <div>
            <div class="lk-settings-title">Tushare Pro</div>
            <div class="lk-settings-sub">A 股行情、财务、资金、行业、宏观数据源</div>
          </div>
          <span class="lk-settings-status" data-ok={status()?.configured === true}>
            {status()?.configured ? `configured${status()?.source === "env" ? " from env" : ""} · ****${status()?.token_last4 ?? ""}` : "not configured"}
          </span>
        </div>

        <label class="lk-settings-field">
          <span>Token</span>
          <input
            type="password"
            value={token()}
            placeholder={status()?.configured ? "留空则只更新启用状态" : "粘贴 Tushare token"}
            autocomplete="off"
            onInput={(e) => setToken(e.currentTarget.value)}
          />
        </label>

        <label class="lk-settings-toggle">
          <input
            type="checkbox"
            checked={enabled()}
            onChange={(e) => setEnabled(e.currentTarget.checked)}
          />
          <span>启用 Tushare 数据源</span>
        </label>

        <Show when={status()?.last_error}>
          <div class="lk-settings-error">
            {status()?.last_error}
          </div>
        </Show>

        <div class="lk-settings-actions">
          <button class="lk-settings-save" onClick={() => void save()} disabled={busy()}>
            {busy() ? "saving..." : "save"}
          </button>
          <Show when={message()}>
            <span class="lk-settings-ok">{message()}</span>
          </Show>
          <Show when={error()}>
            <span class="lk-settings-error-inline">{error()}</span>
          </Show>
        </div>
      </section>
    </main>
  );
}
