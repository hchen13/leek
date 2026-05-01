// L.E.E.K shell — currently switches between 5 demo scenes for design review.
// Eventually scene becomes derived from real agent loop state; for now the
// scene is selected via URL hash for development convenience.

import { createSignal, onCleanup, onMount } from "solid-js";
import { Workbench } from "./components/Workbench";
import { ALL_SCENES, SCENE_LABELS, type Scene } from "./scenes";

function readSceneFromHash(): Scene {
  const h = window.location.hash.replace(/^#/, "");
  return (ALL_SCENES as string[]).includes(h) ? (h as Scene) : "idle";
}

export function App() {
  const [scene, setScene] = createSignal<Scene>(readSceneFromHash());

  onMount(() => {
    const handler = () => setScene(readSceneFromHash());
    window.addEventListener("hashchange", handler);
    onCleanup(() => window.removeEventListener("hashchange", handler));
  });

  const setSceneAndHash = (s: Scene) => {
    window.location.hash = s;
    setScene(s);
  };

  return (
    <div style={{
      width: "100vw", height: "100vh",
      display: "flex", "flex-direction": "column",
      background: "var(--bg-0)",
    }}>
      <div class="lk-scene-picker">
        {ALL_SCENES.map((s) => (
          <button
            class={"lk-scene-btn " + (scene() === s ? "active" : "")}
            onClick={() => setSceneAndHash(s)}
          >
            {SCENE_LABELS[s]}
          </button>
        ))}
      </div>
      <div style={{ flex: 1, display: "grid", "place-items": "center", padding: "16px", overflow: "auto" }}>
        <Workbench scene={scene()} />
      </div>
    </div>
  );
}
