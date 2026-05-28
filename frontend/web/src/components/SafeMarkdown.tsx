// SafeMarkdown — markdown render component.
//
// Thin wrapper around the project's existing `markdown.ts` (marked + DOMPurify
// configured once at module load). Two reasons not to fork main's
// markdown-it implementation:
//   1. Phase 1 directive: "no new deps" — markdown-it would be one.
//   2. markdown.ts already centralises sanitize / link target hooks; a
//      second parser would mean a second config drift surface.
//
// The component renders to `<div class="lk-markdown">`; component-level
// typography styling lives in styles.css under `.lk-markdown` so canvas
// cards, chat bubbles, and plan rows can opt in by reusing the same
// class wherever markdown lands.

import type { JSX } from "solid-js";
import { renderInlineMarkdown, renderMarkdown } from "../markdown";

type Props = {
  text: string | null | undefined;
  /** True = inline render (no wrapping `<p>`), for one-liners (plan step,
   *  summary detail, chat bubble that should sit inline). Default false. */
  inline?: boolean;
  /** Extra class — appended after `lk-markdown` so per-surface tweaks
   *  (e.g. compact line-height in a Note card) can override defaults. */
  class?: string;
  style?: JSX.CSSProperties;
};

export function SafeMarkdown(props: Props) {
  const html = () => {
    if (props.inline) return renderInlineMarkdown(props.text);
    return renderMarkdown(props.text);
  };
  const cls = () =>
    props.class ? `lk-markdown ${props.class}` : "lk-markdown";
  return <div class={cls()} style={props.style} innerHTML={html()} />;
}
