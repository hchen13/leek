// Main shell. 2 columns (chat | canvas), brain widget at top-right.
// Ported from prototype/leek-workbench.jsx → SolidJS.

import { Show } from "solid-js";
import type { Scene } from "../scenes";
import { Icon } from "./Icon";
import { Composer } from "./Chat";
import { InsightSidebar, type AgentPlanView } from "./BrainWidget";
import { canvasFor } from "./CanvasScenes";
import { transcriptFor } from "./Transcripts";

export function TopBar(props: { route: string }) {
  return (
    <div class="lk-top">
      <span class="lk-top-brand"><b>L.E.E.K</b> · LOGIC-ENHANCED EQUITY KERNEL</span>
      <span class="lk-top-sep">/</span>
      <span class="lk-top-route">{props.route}</span>
      <div class="lk-top-spacer" />
      <span class="lk-top-stat"><span class="lk-pulse" /> MKT OPEN · NYSE</span>
      <span class="lk-top-stat">SPX <b>5,612.41</b> <span style={{ color: "var(--up)" }}>+0.42%</span></span>
      <span class="lk-top-stat">NDX <b>19,884.10</b> <span style={{ color: "var(--up)" }}>+0.71%</span></span>
      <span class="lk-top-stat">VIX <b>14.62</b> <span style={{ color: "var(--down)" }}>−1.8%</span></span>
    </div>
  );
}

export function Rail(_props: { active?: string }) {
  // P2 ships only the chat surface — canvas / corpus / screener / agents
  // pages aren't real yet, so the rail is just the brand mark + avatar.
  // We'll grow this back when those views are wired.
  return (
    <aside class="lk-rail">
      <div class="lk-rail-logo">L</div>
      <button class="lk-rail-btn" data-active="true" title="Chat">
        <Icon name="chat" />
      </button>
      <div class="lk-rail-spacer" />
      <div class="lk-rail-avatar">JC</div>
    </aside>
  );
}

function CanvasArea(props: { scene: Scene }) {
  const headSubtitle = () => {
    switch (props.scene) {
      case "delivered": return "verdict · cached";
      case "deep": return "reasoning · live · 6 panels";
      case "thinking-shallow": return "reasoning · live";
      case "clarify": return "awaiting input";
      default: return "no thread";
    }
  };

  return (
    <div class="lk-canvas">
      <Show when={props.scene !== "idle"}>
        <div class="lk-canvas-head">
          <span class="crumb">
            <b>{props.scene === "delivered" ? "NVDA · Pre-market thesis · v3" : "NVDA · Pre-market thesis"}</b>
            <span class="sep">/</span>
            {headSubtitle()}
          </span>
          <div class="actions">
            <button class="iconbtn" title="Layers"><Icon name="layer" class="ic-sm" /></button>
            <button class="iconbtn" title="Search"><Icon name="search" class="ic-sm" /></button>
            <button class="iconbtn" title="Expand"><Icon name="expand" class="ic-sm" /></button>
          </div>
        </div>
      </Show>

      {canvasFor(props.scene)}

      <InsightSidebar scene={props.scene} corpusTools={[]} plan={demoPlanFor(props.scene)} />
    </div>
  );
}

function demoPlanFor(scene: Scene): AgentPlanView | null {
  if (scene === "idle") return null;
  return {
    explanation: "验证右侧进度栏布局；live 模式会使用 plan_updated 事件里的真实计划。",
    items: [
      { id: "p1", step: "检索 corpus 原则和已有知识", status: scene === "thinking-shallow" ? "in_progress" : "completed" },
      { id: "p2", step: "补齐公司、行业、宏观和资金面事实", status: scene === "delivered" ? "completed" : "pending" },
      { id: "p3", step: "综合反方解释并形成结论", status: scene === "delivered" ? "completed" : "pending" },
    ],
  };
}

export function Workbench(props: { scene: Scene }) {
  const composerVal = () => {
    switch (props.scene) {
      case "thinking-shallow": return "";
      case "clarify": return "Position (3–12m), 1–3% NAV";
      case "deep": return "";
      case "delivered": return "What if Q3 is +4% above guide instead?";
      default: return "";
    }
  };

  const headTitle = () => {
    switch (props.scene) {
      case "idle": return "NEW SESSION";
      case "delivered": return "CHAT · NVDA THESIS · v3";
      default: return "CHAT · NVDA THESIS";
    }
  };

  const headMeta = () => {
    switch (props.scene) {
      case "idle": return "0 turns";
      case "thinking-shallow": return "2 turns · just now";
      case "clarify": return "3 turns · 09:46";
      case "deep": return "2 turns · live";
      case "delivered": return "5 turns · 09:42";
    }
  };

  return (
    <div class="lk-demo-frame">
      <div class="lk-app" data-screen-label={`L.E.E.K · ${props.scene}`}>
        <Rail active="chat" />
        <div class="lk-main">
          <TopBar route={props.scene === "idle" ? "~/sessions/2026-04-29/scratch" : "~/sessions/2026-04-29/nvda-thesis"} />
          <section class="lk-chat">
            <div class="lk-chat-head">
              <span class="lk-chat-head-title">{headTitle()}</span>
              <span class="lk-chat-head-meta">{headMeta()}</span>
            </div>
            <div class="lk-chat-body">
              {transcriptFor(props.scene)}
            </div>
            <Composer value={composerVal()} focus={props.scene === "delivered"} />
          </section>
          <CanvasArea scene={props.scene} />
        </div>
      </div>
    </div>
  );
}
