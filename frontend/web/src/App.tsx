// L.E.E.K shell — defaults to the LIVE workbench (real session). URL hash
// `#demo:<scene>` (e.g. `#demo:deep`) renders the fixture variant instead,
// kept around as a visual reference / dev preview, NOT as a parallel page.
// The 5 scenes are forms of the same UI under different session states;
// the LIVE workbench evolves through them based on real data.

import { Show, createSignal, onCleanup, onMount } from "solid-js";
import { LiveChat } from "./components/LiveChat";
import { Workbench } from "./components/Workbench";
import { ALL_SCENES, type Scene } from "./scenes";

type View =
  | { mode: "live" }
  | { mode: "demo"; scene: Scene };

function readViewFromHash(): View {
  const h = window.location.hash.replace(/^#/, "");
  if (h.startsWith("demo:")) {
    const s = h.slice(5);
    if ((ALL_SCENES as string[]).includes(s)) {
      return { mode: "demo", scene: s as Scene };
    }
  }
  return { mode: "live" };
}

export function App() {
  const [view, setView] = createSignal<View>(readViewFromHash());

  onMount(() => {
    const handler = () => setView(readViewFromHash());
    window.addEventListener("hashchange", handler);
    onCleanup(() => window.removeEventListener("hashchange", handler));
  });

  return (
    <div style={{
      width: "100vw", height: "100vh",
      display: "flex", "flex-direction": "column",
      background: "var(--bg-0)",
    }}>
      <Show when={view().mode === "demo"}>
        <DemoBanner scene={(view() as { mode: "demo"; scene: Scene }).scene} />
      </Show>
      <div style={{ flex: 1, overflow: "auto" }}>
        <Show
          when={view().mode === "live"}
          fallback={<Workbench scene={(view() as { mode: "demo"; scene: Scene }).scene} />}
        >
          <LiveChat />
        </Show>
      </div>
    </div>
  );
}

function DemoBanner(props: { scene: Scene }) {
  return (
    <div style={{
      display: "flex",
      "align-items": "center",
      gap: "12px",
      padding: "6px 14px",
      background: "rgba(217, 119, 87, 0.08)",
      "border-bottom": "1px solid rgba(217, 119, 87, 0.2)",
      "font-family": "var(--font-mono)",
      "font-size": "11px",
      color: "var(--ink-2)",
    }}>
      <span><b>DEMO</b> · {props.scene}</span>
      <span style={{ color: "var(--ink-3)" }}>fixture data, not live session</span>
      <span style={{ "margin-left": "auto" }}>
        <a href="#" style={{ color: "var(--ink-2)" }}>← back to LIVE</a>
      </span>
    </div>
  );
}
