// Session switcher dropdown rendered in the chat-head. List existing
// sessions, create new ones, rename/delete inline.

import { For, Show, createEffect, createSignal, onCleanup } from "solid-js";

export interface SessionRow {
  id: string;
  title: string | null;
  status: string;
  pinned: number;
  created_at: string;
  last_active_at: string;
  /** Live runtime flag from the gateway — an agent reply is in flight. */
  running?: boolean;
}

export function SessionMenu(props: {
  sessions: SessionRow[];
  currentId: string;
  onSelect: (id: string) => void;
  onCreate: () => void;
  onRename: (id: string, title: string) => void;
  onDelete: (id: string) => void;
}) {
  const [open, setOpen] = createSignal(false);
  const [renaming, setRenaming] = createSignal<string | null>(null);
  const [renameVal, setRenameVal] = createSignal("");
  let rootEl!: HTMLDivElement;

  const current = () => props.sessions.find((s) => s.id === props.currentId);
  const display = () => current()?.title?.trim() || props.currentId;

  // Close dropdown on outside click / Esc.
  createEffect(() => {
    if (!open()) return;
    const onDocClick = (e: MouseEvent) => {
      if (rootEl && !rootEl.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDocClick);
    document.addEventListener("keydown", onKey);
    onCleanup(() => {
      document.removeEventListener("mousedown", onDocClick);
      document.removeEventListener("keydown", onKey);
    });
  });

  function startRename(s: SessionRow) {
    setRenameVal(s.title ?? "");
    setRenaming(s.id);
  }
  function commitRename(id: string) {
    const v = renameVal().trim();
    if (v) props.onRename(id, v);
    setRenaming(null);
  }

  return (
    <div ref={rootEl} style={{ position: "relative" }}>
      <button
        onClick={() => setOpen((v) => !v)}
        style={{
          background: open() ? "var(--bg-2)" : "transparent",
          border: "1px solid var(--bg-2)",
          color: "var(--ink-1)",
          "border-radius": "6px",
          padding: "3px 10px",
          cursor: "pointer",
          "font-family": "var(--font-mono)",
          "font-size": "11px",
          display: "flex",
          "align-items": "center",
          gap: "6px",
        }}
      >
        <Show when={current()?.running}>
          <span class="lk-live-dot" title="该会话正在生成" />
        </Show>
        <span>{display()}</span>
        <span style={{ color: "var(--ink-3)", "font-size": "9px" }}>▾</span>
      </button>
      <Show when={open()}>
        <div style={{
          position: "absolute",
          top: "100%",
          left: 0,
          "margin-top": "6px",
          width: "320px",
          "max-height": "60vh",
          overflow: "auto",
          background: "var(--bg-1)",
          border: "1px solid var(--bg-2)",
          "border-radius": "8px",
          "box-shadow": "0 8px 24px rgba(0,0,0,0.4)",
          "z-index": 50,
          padding: "6px",
        }}>
          <button
            onClick={() => { props.onCreate(); setOpen(false); }}
            style={{
              width: "100%",
              padding: "8px 10px",
              background: "transparent",
              border: "1px dashed var(--bg-2)",
              "border-radius": "6px",
              color: "var(--clay-soft)",
              cursor: "pointer",
              "font-family": "var(--font-mono)",
              "font-size": "11px",
              "text-align": "left",
              "margin-bottom": "6px",
            }}
          >+ new session</button>
          <Show when={props.sessions.length === 0}>
            <div style={{
              padding: "10px",
              color: "var(--ink-3)",
              "font-size": "11px",
              "text-align": "center",
            }}>No sessions yet.</div>
          </Show>
          <For each={props.sessions}>{(s) => {
            const isCurrent = () => s.id === props.currentId;
            const renamingThis = () => renaming() === s.id;
            return (
              <div style={{
                display: "flex",
                "align-items": "center",
                gap: "6px",
                padding: "6px 8px",
                "border-radius": "6px",
                background: isCurrent() ? "rgba(217,119,87,0.1)" : "transparent",
                cursor: "pointer",
              }}
              onClick={() => {
                if (renamingThis()) return;
                props.onSelect(s.id);
                setOpen(false);
              }}>
                <Show when={s.running}>
                  <span class="lk-live-dot" title="正在生成" />
                </Show>
                <Show
                  when={renamingThis()}
                  fallback={
                    <div style={{ flex: 1, "min-width": 0 }}>
                      <div style={{
                        "font-size": "12.5px",
                        color: isCurrent() ? "var(--ink-0)" : "var(--ink-1)",
                        "font-weight": isCurrent() ? 600 : 400,
                        overflow: "hidden",
                        "text-overflow": "ellipsis",
                        "white-space": "nowrap",
                      }}>{(s.title?.trim() || s.id)}</div>
                      <div style={{
                        "font-family": "var(--font-mono)",
                        "font-size": "10px",
                        color: "var(--ink-3)",
                      }}>{s.last_active_at.slice(0, 10)} · {s.id}</div>
                    </div>
                  }
                >
                  <input
                    type="text"
                    value={renameVal()}
                    autofocus
                    onClick={(e) => e.stopPropagation()}
                    onInput={(e) => setRenameVal(e.currentTarget.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") commitRename(s.id);
                      if (e.key === "Escape") setRenaming(null);
                    }}
                    style={{
                      flex: 1,
                      padding: "4px 6px",
                      background: "var(--bg-0)",
                      border: "1px solid var(--bg-2)",
                      "border-radius": "4px",
                      color: "var(--ink-0)",
                      "font-size": "12.5px",
                    }}
                  />
                </Show>
                <Show when={!renamingThis()}>
                  <button
                    onClick={(e) => { e.stopPropagation(); startRename(s); }}
                    title="Rename"
                    style={smallBtnStyle()}
                  >✎</button>
                  <button
                    onClick={(e) => {
                      e.stopPropagation();
                      if (confirm(`Delete session "${s.title || s.id}"? This cannot be undone.`)) {
                        props.onDelete(s.id);
                      }
                    }}
                    title="Delete"
                    style={smallBtnStyle()}
                  >×</button>
                </Show>
              </div>
            );
          }}</For>
        </div>
      </Show>
    </div>
  );
}

function smallBtnStyle() {
  return {
    background: "transparent",
    border: "0",
    color: "var(--ink-3)",
    cursor: "pointer",
    "font-size": "13px",
    padding: "2px 6px",
    "border-radius": "4px",
  };
}
