// Modal showing a single corpus wiki / source document. Pulled out so that
// both BrainWidget node clicks and ArtifactPanel hit-tile clicks can pop it.
//
// Wikilink-aware rendering is handled by SafeMarkdown (rewriteCorpusPaths +
// onWikiOpen) so the modal stays a thin shell.

import { Show } from "solid-js";
import { SafeMarkdown } from "./SafeMarkdown";

export interface CorpusDoc {
  id: string;
  title: string;
  tier: string;
  layer: string;
  tags: string[];
  body: string;
}

export function CorpusDocModal(props: {
  doc: CorpusDoc;
  loading: boolean;
  error: string | null;
  onClose: () => void;
  /** Called when a wikilink inside this doc is clicked. */
  onOpenDoc: (id: string) => void;
}) {
  return (
    <div
      onClick={props.onClose}
      style={{
        position: "fixed",
        inset: 0,
        background: "rgba(8, 6, 4, 0.55)",
        display: "flex",
        "align-items": "center",
        "justify-content": "center",
        "z-index": 1000,
      }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          width: "min(820px, 92vw)",
          "max-height": "85vh",
          background: "var(--bg-1)",
          border: "1px solid var(--bg-2)",
          "border-radius": "12px",
          padding: "24px 28px",
          display: "flex",
          "flex-direction": "column",
          gap: "12px",
          color: "var(--ink-1)",
          "font-family": "var(--font-sans)",
        }}
      >
        <div style={{
          display: "flex",
          "align-items": "baseline",
          "justify-content": "space-between",
          gap: "16px",
        }}>
          <div>
            <div style={{ "font-size": "18px", "font-weight": 600 }}>
              {props.doc.title || (props.doc.id.split("/").pop() ?? "")}
            </div>
            <Show when={props.doc.tier && props.doc.layer}>
              <div style={{
                "font-family": "var(--font-mono)",
                "font-size": "11px",
                color: "var(--ink-3)",
                "margin-top": "4px",
              }}>
                {props.doc.tier} · {props.doc.layer}
              </div>
            </Show>
          </div>
          <button
            onClick={props.onClose}
            style={{
              background: "transparent",
              border: "1px solid var(--bg-2)",
              color: "var(--ink-2)",
              "border-radius": "6px",
              padding: "4px 12px",
              cursor: "pointer",
              "font-family": "var(--font-mono)",
              "font-size": "11px",
            }}
          >Esc · close</button>
        </div>
        <Show when={props.doc.tags.length > 0}>
          <div style={{
            display: "flex",
            "flex-wrap": "wrap",
            gap: "6px",
            "font-family": "var(--font-mono)",
            "font-size": "10px",
            color: "var(--ink-3)",
          }}>
            {props.doc.tags.map((t) => (
              <span style={{
                padding: "2px 8px",
                background: "var(--bg-2)",
                "border-radius": "10px",
              }}>{t}</span>
            ))}
          </div>
        </Show>
        <div style={{
          flex: 1,
          overflow: "auto",
          "padding-right": "8px",
          "font-size": "13px",
          "line-height": 1.6,
        }}>
          <Show
            when={!props.loading && !props.error}
            fallback={
              <div style={{ color: "var(--ink-3)", "font-style": "italic" }}>
                {props.loading ? "loading…" : `error: ${props.error}`}
              </div>
            }
          >
            <SafeMarkdown source={props.doc.body} onWikiOpen={props.onOpenDoc} />
          </Show>
        </div>
      </div>
    </div>
  );
}
