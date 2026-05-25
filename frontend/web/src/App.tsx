// L.E.E.K shell — defaults to the LIVE workbench (real session). URL hash
// `#demo:<scene>` (e.g. `#demo:deep`) renders the fixture variant instead,
// kept around as a visual reference / dev preview, NOT as a parallel page.
// The 5 scenes are forms of the same UI under different session states;
// the LIVE workbench evolves through them based on real data.

import { Match, Show, Switch, createSignal, onCleanup, onMount } from "solid-js";
import { LiveChat } from "./components/LiveChat";
import { Workbench } from "./components/Workbench";
import { PortfolioPage } from "./components/PortfolioPage";
import { SettingsPage } from "./components/SettingsPage";
import { ALL_SCENES, type Scene } from "./scenes";

type Page = "chat" | "portfolio" | "settings";

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
  const [page, setPage] = createSignal<Page>("chat");

  onMount(() => {
    const handler = () => setView(readViewFromHash());
    window.addEventListener("hashchange", handler);
    onCleanup(() => window.removeEventListener("hashchange", handler));
  });

  return (
    <div style={{
      width: "100vw",
      height: "100vh",
      display: "flex",
      "flex-direction": "column",
      background: "var(--bg-0)",
    }}>
      <Show when={view().mode === "demo"}>
        <DemoBanner scene={(view() as { mode: "demo"; scene: Scene }).scene} />
      </Show>
      <div style={{ flex: 1, overflow: "hidden" }}>
        <Show
          when={view().mode === "live"}
          fallback={<Workbench scene={(view() as { mode: "demo"; scene: Scene }).scene} />}
        >
          <LivePage page={page} onNavigate={setPage} />
        </Show>
      </div>
    </div>
  );
}

function LivePage(props: { page: () => Page; onNavigate: (p: Page) => void }) {
  return (
    <Switch>
      <Match when={props.page() === "portfolio"}>
        <PortfolioWithRail onNavigate={props.onNavigate} />
      </Match>
      <Match when={props.page() === "settings"}>
        <SettingsWithRail onNavigate={props.onNavigate} />
      </Match>
      <Match when={props.page() === "chat"}>
        <LiveChat onNavigate={props.onNavigate} />
      </Match>
    </Switch>
  );
}

function PortfolioWithRail(props: { onNavigate: (p: Page) => void }) {
  return (
    <div class="lk-app" style={{ width: "100%", height: "100%" }}>
      <NavRail page="portfolio" onNavigate={props.onNavigate} />
      <div class="lk-main" style={{ "grid-template-rows": "1fr", "grid-template-columns": "1fr" }}>
        <PortfolioPage />
      </div>
    </div>
  );
}

function SettingsWithRail(props: { onNavigate: (p: Page) => void }) {
  return (
    <div class="lk-app" style={{ width: "100%", height: "100%" }}>
      <NavRail page="settings" onNavigate={props.onNavigate} />
      <div class="lk-main" style={{ "grid-template-rows": "1fr", "grid-template-columns": "1fr" }}>
        <SettingsPage />
      </div>
    </div>
  );
}

export function NavRail(props: { page: Page; onNavigate: (p: Page) => void }) {
  return (
    <aside class="lk-rail">
      <div class="lk-rail-logo">L</div>
      <RailNavBtn label="C" sub="chat" target="chat" active={props.page === "chat"} onNavigate={props.onNavigate} />
      <RailNavBtn label="P" sub="port" target="portfolio" active={props.page === "portfolio"} onNavigate={props.onNavigate} />
      <div class="lk-rail-spacer" />
      <button
        class="lk-rail-btn"
        data-active={props.page === "settings"}
        onClick={() => props.onNavigate("settings")}
        title="settings"
      >
        <span style={{ width: "18px", height: "18px", display: "inline-flex" }}>
          <svg class="ic" viewBox="0 0 24 24">
            <circle cx="12" cy="12" r="3" />
            <path d="M19 12a7 7 0 0 0-.1-1.2l2-1.5-2-3.5-2.4.9a7 7 0 0 0-2-1.2L14 3h-4l-.5 2.5a7 7 0 0 0-2 1.2l-2.4-.9-2 3.5 2 1.5A7 7 0 0 0 5 12c0 .4 0 .8.1 1.2l-2 1.5 2 3.5 2.4-.9a7 7 0 0 0 2 1.2L10 21h4l.5-2.5a7 7 0 0 0 2-1.2l2.4.9 2-3.5-2-1.5c.1-.4.1-.8.1-1.2Z" />
          </svg>
        </span>
      </button>
      <div class="lk-rail-avatar">JC</div>
    </aside>
  );
}

function RailNavBtn(props: {
  label: string;
  sub: string;
  target: Page;
  active: boolean;
  onNavigate: (p: Page) => void;
}) {
  return (
    <button
      class="lk-rail-btn"
      data-active={props.active}
      onClick={() => props.onNavigate(props.target)}
      title={props.target}
      style={{
        display: "flex",
        "flex-direction": "column",
        "align-items": "center",
        "justify-content": "center",
        gap: "2px",
        height: "44px",
        width: "36px",
      }}
    >
      <span style={{
        "font-family": "var(--font-stencil)",
        "font-size": "13px",
        "font-weight": "700",
        "line-height": "1",
        color: props.active ? "var(--clay-soft)" : "var(--ink-2)",
      }}>{props.label}</span>
      <span style={{
        "font-family": "var(--font-mono)",
        "font-size": "8px",
        color: props.active ? "var(--clay-soft)" : "var(--ink-3)",
        "letter-spacing": "0.04em",
        "text-transform": "uppercase",
      }}>{props.sub}</span>
    </button>
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
