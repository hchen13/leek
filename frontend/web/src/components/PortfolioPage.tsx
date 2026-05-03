import { For, Show, createSignal, onMount } from "solid-js";

interface HoldingRow {
  snapshot_at: string;
  ticker: string;
  qty: number;
  avg_cost: number | null;
  account: string;
  notes: string | null;
  source: string;
}

interface EditableRow {
  ticker: string;
  qty: string;
  avg_cost: string;
  account: string;
  notes: string;
}

function rowToEditable(r: HoldingRow): EditableRow {
  return {
    ticker: r.ticker,
    qty: String(r.qty),
    avg_cost: r.avg_cost != null ? String(r.avg_cost) : "",
    account: r.account,
    notes: r.notes ?? "",
  };
}

function emptyEditable(): EditableRow {
  return { ticker: "", qty: "", avg_cost: "", account: "main", notes: "" };
}

export function PortfolioPage() {
  const [items, setItems] = createSignal<HoldingRow[]>([]);
  const [snapshotAt, setSnapshotAt] = createSignal<string | null>(null);
  const [rows, setRows] = createSignal<EditableRow[]>([]);
  const [addingRow, setAddingRow] = createSignal(false);
  const [newRow, setNewRow] = createSignal<EditableRow>(emptyEditable());
  const [nlText, setNlText] = createSignal("");
  const [saving, setSaving] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [dirty, setDirty] = createSignal(false);

  const [charterText, setCharterText] = createSignal("");
  const [charterDirty, setCharterDirty] = createSignal(false);
  const [charterSaving, setCharterSaving] = createSignal(false);

  onMount(async () => {
    try {
      const cr = await fetch("/api/v1/charter");
      if (cr.ok) {
        const cj = await cr.json();
        setCharterText(cj.charter?.text ?? "");
      }
    } catch { /* ignore */ }

    try {
      const r = await fetch("/api/v1/portfolio");
      if (r.ok) {
        const j = await r.json();
        setItems(j.items ?? []);
        setSnapshotAt(j.snapshot_at ?? null);
        setRows((j.items ?? []).map(rowToEditable));
      }
    } catch {
      setError("加载持仓失败");
    }
  });

  function updateRow(i: number, field: keyof EditableRow, val: string) {
    setRows((prev) => {
      const next = [...prev];
      next[i] = { ...next[i], [field]: val };
      return next;
    });
    setDirty(true);
  }

  function deleteRow(i: number) {
    setRows((prev) => prev.filter((_, idx) => idx !== i));
    setDirty(true);
  }

  function commitNewRow() {
    const r = newRow();
    if (!r.ticker.trim()) return;
    setRows((prev) => [...prev, { ...r }]);
    setNewRow(emptyEditable());
    setAddingRow(false);
    setDirty(true);
  }

  async function save() {
    setSaving(true);
    setError(null);
    try {
      const holdings = rows().map((r) => ({
        ticker: r.ticker.trim(),
        qty: parseFloat(r.qty) || 0,
        avg_cost: r.avg_cost !== "" ? parseFloat(r.avg_cost) || null : null,
        account: r.account.trim() || "main",
        notes: r.notes.trim() || null,
      }));
      const resp = await fetch("/api/v1/portfolio", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ holdings }),
      });
      if (!resp.ok) {
        const body = await resp.text();
        setError(`保存失败 ${resp.status}: ${body.slice(0, 200)}`);
        return;
      }
      const j = await resp.json();
      setSnapshotAt(j.snapshot_at);
      setDirty(false);
    } catch {
      setError("网络错误");
    } finally {
      setSaving(false);
    }
  }

  async function saveCharter() {
    setCharterSaving(true);
    try {
      const resp = await fetch("/api/v1/charter", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ text: charterText() }),
      });
      if (resp.ok) setCharterDirty(false);
      else setError(`Charter 保存失败 ${resp.status}`);
    } catch {
      setError("Charter 保存失败：网络错误");
    } finally {
      setCharterSaving(false);
    }
  }

  const fmtSnap = () => {
    const s = snapshotAt();
    if (!s) return "无快照";
    const d = new Date(s);
    if (Number.isNaN(d.getTime())) return s;
    return d.toLocaleString("zh-CN", {
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
    });
  };

  return (
    <div style={{
      display: "flex",
      "flex-direction": "column",
      height: "100%",
      background: "var(--bg-1)",
      color: "var(--ink-0)",
      "font-family": "var(--font-sans)",
      overflow: "hidden",
    }}>
      <div style={{
        display: "flex",
        "align-items": "center",
        padding: "0 20px",
        height: "44px",
        "border-bottom": "1px solid var(--line-1)",
        background: "rgba(26,23,20,0.6)",
        "backdrop-filter": "blur(8px)",
        gap: "12px",
        "flex-shrink": "0",
      }}>
        <span style={{
          "font-family": "var(--font-stencil)",
          "font-size": "12px",
          "letter-spacing": "0.18em",
          color: "var(--ink-1)",
        }}>
          <b style={{ color: "var(--clay-soft)", "font-weight": 400 }}>L.E.E.K</b>
          {" · "}PORTFOLIO
        </span>
        <span style={{ color: "var(--ink-4)" }}>/</span>
        <span style={{
          "font-family": "var(--font-mono)",
          "font-size": "11px",
          color: "var(--ink-2)",
        }}>~/portfolio</span>
        <div style={{ flex: 1 }} />
        <span style={{
          "font-family": "var(--font-mono)",
          "font-size": "10px",
          color: "var(--ink-3)",
        }}>snapshot: {fmtSnap()}</span>
      </div>

      <div style={{
        flex: 1,
        overflow: "auto",
        padding: "20px",
        display: "flex",
        "flex-direction": "column",
        gap: "20px",
      }}>
        <div style={{
          background: "var(--bg-2)",
          border: "1px solid var(--line-1)",
          "border-radius": "10px",
          overflow: "hidden",
        }}>
          <div style={{
            display: "flex",
            "align-items": "center",
            padding: "10px 14px",
            "border-bottom": "1px solid var(--line-0)",
            gap: "8px",
          }}>
            <span style={{
              "font-family": "var(--font-mono)",
              "font-size": "11px",
              color: "var(--ink-2)",
              "letter-spacing": "0.06em",
              "text-transform": "uppercase",
            }}>
              持仓 Holdings
              <Show when={rows().length > 0}>
                <span style={{ color: "var(--ink-3)", "margin-left": "6px" }}>
                  ({rows().length})
                </span>
              </Show>
            </span>
            <div style={{ flex: 1 }} />
            <Show when={dirty()}>
              <button
                onClick={save}
                disabled={saving()}
                style={{
                  background: "linear-gradient(180deg, var(--clay), var(--clay-deep))",
                  color: "#2a1810",
                  "font-weight": 600,
                  "font-size": "11px",
                  padding: "4px 12px",
                  "border-radius": "6px",
                  "letter-spacing": "0.04em",
                  "text-transform": "uppercase",
                  "font-family": "var(--font-mono)",
                  cursor: saving() ? "wait" : "pointer",
                  opacity: saving() ? 0.7 : 1,
                  border: "none",
                }}
              >
                {saving() ? "保存中…" : "保存快照"}
              </button>
            </Show>
            <button
              onClick={() => { setAddingRow(true); setNewRow(emptyEditable()); }}
              style={{
                "font-family": "var(--font-mono)",
                "font-size": "10.5px",
                color: "var(--ink-2)",
                background: "transparent",
                border: "1px solid var(--line-1)",
                "border-radius": "5px",
                padding: "3px 10px",
                cursor: "pointer",
              }}
            >
              + 添加
            </button>
          </div>

          <Show
            when={rows().length > 0 || addingRow()}
            fallback={
              <div style={{
                padding: "32px",
                "text-align": "center",
                "font-family": "var(--font-mono)",
                "font-size": "12px",
                color: "var(--ink-3)",
              }}>
                暂无持仓。点击「+ 添加」手动录入，或通过自然语言更新。
              </div>
            }
          >
            <table style={{
              width: "100%",
              "border-collapse": "collapse",
              "font-family": "var(--font-mono)",
              "font-size": "12px",
            }}>
              <thead>
                <tr style={{ "border-bottom": "1px solid var(--line-1)" }}>
                  {["代码", "数量", "均价", "账户", "备注", ""].map((h) => (
                    <th style={{
                      padding: "6px 12px",
                      "text-align": "left",
                      color: "var(--ink-3)",
                      "font-weight": 400,
                      "font-size": "10px",
                      "text-transform": "uppercase",
                      "letter-spacing": "0.06em",
                    }}>{h}</th>
                  ))}
                </tr>
              </thead>
              <tbody>
                <For each={rows()}>{(row, i) => (
                  <tr style={{ "border-bottom": "1px solid var(--line-0)" }}>
                    <td style={{ padding: "6px 12px" }}>
                      <input
                        value={row.ticker}
                        onInput={(e) => updateRow(i(), "ticker", e.currentTarget.value)}
                        style={cellInput}
                      />
                    </td>
                    <td style={{ padding: "6px 12px" }}>
                      <input
                        type="number"
                        value={row.qty}
                        onInput={(e) => updateRow(i(), "qty", e.currentTarget.value)}
                        style={cellInput}
                      />
                    </td>
                    <td style={{ padding: "6px 12px" }}>
                      <input
                        type="number"
                        value={row.avg_cost}
                        placeholder="—"
                        onInput={(e) => updateRow(i(), "avg_cost", e.currentTarget.value)}
                        style={cellInput}
                      />
                    </td>
                    <td style={{ padding: "6px 12px" }}>
                      <input
                        value={row.account}
                        onInput={(e) => updateRow(i(), "account", e.currentTarget.value)}
                        style={cellInput}
                      />
                    </td>
                    <td style={{ padding: "6px 12px" }}>
                      <input
                        value={row.notes}
                        placeholder="—"
                        onInput={(e) => updateRow(i(), "notes", e.currentTarget.value)}
                        style={{ ...cellInput, "min-width": "120px" }}
                      />
                    </td>
                    <td style={{ padding: "6px 8px", "text-align": "right" }}>
                      <button
                        onClick={() => deleteRow(i())}
                        title="删除"
                        style={{
                          color: "var(--ink-3)",
                          "font-size": "12px",
                          background: "transparent",
                          border: "none",
                          cursor: "pointer",
                          padding: "2px 6px",
                          "border-radius": "4px",
                        }}
                      >
                        ✕
                      </button>
                    </td>
                  </tr>
                )}</For>

                <Show when={addingRow()}>
                  <tr style={{
                    "border-bottom": "1px solid var(--line-0)",
                    background: "rgba(217,119,87,0.04)",
                  }}>
                    <td style={{ padding: "6px 12px" }}>
                      <input
                        placeholder="AAPL"
                        value={newRow().ticker}
                        onInput={(e) => setNewRow((r) => ({ ...r, ticker: e.currentTarget.value }))}
                        style={{ ...cellInput, color: "var(--ink-0)" }}
                        autofocus
                      />
                    </td>
                    <td style={{ padding: "6px 12px" }}>
                      <input
                        type="number"
                        placeholder="0"
                        value={newRow().qty}
                        onInput={(e) => setNewRow((r) => ({ ...r, qty: e.currentTarget.value }))}
                        style={cellInput}
                      />
                    </td>
                    <td style={{ padding: "6px 12px" }}>
                      <input
                        type="number"
                        placeholder="—"
                        value={newRow().avg_cost}
                        onInput={(e) => setNewRow((r) => ({ ...r, avg_cost: e.currentTarget.value }))}
                        style={cellInput}
                      />
                    </td>
                    <td style={{ padding: "6px 12px" }}>
                      <input
                        placeholder="main"
                        value={newRow().account}
                        onInput={(e) => setNewRow((r) => ({ ...r, account: e.currentTarget.value }))}
                        style={cellInput}
                      />
                    </td>
                    <td style={{ padding: "6px 12px" }}>
                      <input
                        placeholder="—"
                        value={newRow().notes}
                        onInput={(e) => setNewRow((r) => ({ ...r, notes: e.currentTarget.value }))}
                        style={{ ...cellInput, "min-width": "120px" }}
                      />
                    </td>
                    <td style={{ padding: "6px 8px", "text-align": "right", display: "flex", gap: "4px", "align-items": "center" }}>
                      <button
                        onClick={commitNewRow}
                        style={{
                          color: "var(--clay-soft)",
                          "font-size": "12px",
                          background: "transparent",
                          border: "none",
                          cursor: "pointer",
                          padding: "2px 6px",
                        }}
                      >
                        ✓
                      </button>
                      <button
                        onClick={() => setAddingRow(false)}
                        style={{
                          color: "var(--ink-3)",
                          "font-size": "12px",
                          background: "transparent",
                          border: "none",
                          cursor: "pointer",
                          padding: "2px 6px",
                        }}
                      >
                        ✕
                      </button>
                    </td>
                  </tr>
                </Show>
              </tbody>
            </table>
          </Show>
        </div>

        <div style={{
          background: "var(--bg-2)",
          border: "1px solid var(--line-1)",
          "border-radius": "10px",
          padding: "14px 16px",
          display: "flex",
          "flex-direction": "column",
          gap: "10px",
        }}>
          <div style={{
            "font-family": "var(--font-mono)",
            "font-size": "11px",
            color: "var(--ink-2)",
            "letter-spacing": "0.06em",
            "text-transform": "uppercase",
          }}>
            自然语言更新持仓
          </div>
          <textarea
            placeholder="例：我今天买了 100 股苹果，均价 190 美元，账户是主账户"
            value={nlText()}
            onInput={(e) => setNlText(e.currentTarget.value)}
            rows={3}
            style={{
              background: "var(--bg-3)",
              border: "1px solid var(--line-2)",
              "border-radius": "6px",
              padding: "10px 12px",
              "font-size": "13px",
              "line-height": "1.5",
              color: "var(--ink-0)",
              resize: "vertical",
              width: "100%",
            }}
          />
          <div style={{
            display: "flex",
            "align-items": "center",
            gap: "10px",
          }}>
            <button
              disabled
              style={{
                "font-family": "var(--font-mono)",
                "font-size": "11px",
                color: "var(--ink-3)",
                background: "transparent",
                border: "1px solid var(--line-1)",
                "border-radius": "6px",
                padding: "5px 14px",
                cursor: "not-allowed",
                opacity: 0.5,
              }}
            >
              解析并更新
            </button>
            <span style={{
              "font-family": "var(--font-mono)",
              "font-size": "10px",
              color: "var(--ink-3)",
            }}>
              自然语言解析功能即将接入 leek agent，解析结果需确认后生效
            </span>
          </div>
        </div>

        <div style={{
          background: "var(--bg-2)",
          border: "1px solid var(--line-1)",
          "border-radius": "10px",
          overflow: "hidden",
        }}>
          <div style={{
            display: "flex",
            "align-items": "center",
            padding: "10px 14px",
            "border-bottom": "1px solid var(--line-0)",
            gap: "8px",
          }}>
            <span style={{
              "font-family": "var(--font-mono)",
              "font-size": "11px",
              color: "var(--ink-2)",
              "letter-spacing": "0.06em",
              "text-transform": "uppercase",
            }}>Investment Philosophy · Team Charter</span>
            <div style={{ flex: 1 }} />
            <Show when={charterDirty()}>
              <button
                onClick={saveCharter}
                disabled={charterSaving()}
                style={{
                  background: "linear-gradient(180deg, var(--clay), var(--clay-deep))",
                  color: "#2a1810",
                  "font-weight": 600,
                  "font-size": "11px",
                  padding: "4px 12px",
                  "border-radius": "6px",
                  "letter-spacing": "0.04em",
                  "text-transform": "uppercase",
                  "font-family": "var(--font-mono)",
                  cursor: charterSaving() ? "wait" : "pointer",
                  opacity: charterSaving() ? 0.7 : 1,
                  border: "none",
                }}
              >
                {charterSaving() ? "保存中…" : "保存"}
              </button>
            </Show>
          </div>
          <div style={{ padding: "14px" }}>
            <textarea
              value={charterText()}
              onInput={(e) => { setCharterText(e.currentTarget.value); setCharterDirty(true); }}
              rows={8}
              placeholder="在此描述你的投资哲学、信仰体系、长期目标……leek 将在每次对话中遵循这些原则"
              style={{
                background: "var(--bg-3)",
                border: "1px solid var(--line-2)",
                "border-radius": "6px",
                padding: "10px 12px",
                "font-size": "13px",
                "line-height": "1.6",
                color: "var(--ink-0)",
                resize: "vertical",
                width: "100%",
                "font-family": "var(--font-sans)",
              }}
            />
          </div>
        </div>

        <Show when={error()}>
          <div style={{
            "font-family": "var(--font-mono)",
            "font-size": "12px",
            color: "#d97070",
            background: "rgba(217,112,112,0.08)",
            border: "1px solid rgba(217,112,112,0.2)",
            "border-radius": "6px",
            padding: "8px 12px",
          }}>
            {error()}
          </div>
        </Show>
      </div>
    </div>
  );
}

const cellInput: Record<string, string> = {
  background: "transparent",
  border: "none",
  outline: "none",
  color: "var(--ink-0)",
  "font-family": "var(--font-mono)",
  "font-size": "12px",
  width: "100%",
  padding: "0",
};
