// L.E.E.K shell — switches between the live (real backend) view and the
// 5 hardcoded design fixtures. View is selected via URL hash for dev convenience.

import { Show, createSignal, onCleanup, onMount } from "solid-js";
import { LiveChat } from "./components/LiveChat";
import { Workbench } from "./components/Workbench";
import { ALL_SCENES, SCENE_LABELS, type Scene } from "./scenes";

type View = Scene | "live";

const ALL_VIEWS: View[] = ["live", ...ALL_SCENES];

const VIEW_LABELS: Record<View, string> = {
  "live": "● LIVE",
  "idle": SCENE_LABELS["idle"],
  "thinking-shallow": SCENE_LABELS["thinking-shallow"],
  "clarify": SCENE_LABELS["clarify"],
  "deep": SCENE_LABELS["deep"],
  "delivered": SCENE_LABELS["delivered"],
};

function readViewFromHash(): View {
  const h = window.location.hash.replace(/^#/, "");
  return (ALL_VIEWS as string[]).includes(h) ? (h as View) : "live";
}

export function App() {
  const [view, setView] = createSignal<View>(readViewFromHash());

  onMount(() => {
    const handler = () => setView(readViewFromHash());
    window.addEventListener("hashchange", handler);
    onCleanup(() => window.removeEventListener("hashchange", handler));
  });

  const setViewAndHash = (v: View) => {
    window.location.hash = v;
    setView(v);
  };

  return (
    <div style={{
      width: "100vw", height: "100vh",
      display: "flex", "flex-direction": "column",
      background: "var(--bg-0)",
    }}>
      <div class="lk-scene-picker">
        {ALL_VIEWS.map((v) => (
          <button
            class={"lk-scene-btn " + (view() === v ? "active" : "")}
            onClick={() => setViewAndHash(v)}
          >
            {VIEW_LABELS[v]}
          </button>
        ))}
      </div>
      <div style={{ flex: 1, display: "grid", "place-items": "center", padding: "16px", overflow: "auto" }}>
        <Show
          when={view() === "live"}
          fallback={<Workbench scene={view() as Scene} />}
        >
          <LiveChat />
        </Show>
      </div>
    </div>
  );
}
