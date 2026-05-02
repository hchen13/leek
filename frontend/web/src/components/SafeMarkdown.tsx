// Render markdown safely. We use markdown-it for the parser (good GFM
// support: tables, fenced code, autolinks) and dompurify to strip any
// HTML the LLM might emit. Output goes through innerHTML.

import DOMPurify from "dompurify";
import MarkdownIt from "markdown-it";
import { createMemo } from "solid-js";

const md = new MarkdownIt({
  html: false,        // never trust raw HTML from the model
  linkify: true,
  breaks: true,       // single \n → <br/>, matches chat conventions
  typographer: false,
});

// Open links in a new tab so the user doesn't lose their session.
const defaultRender =
  md.renderer.rules.link_open ||
  ((tokens, idx, opts, _env, self) => self.renderToken(tokens, idx, opts));
md.renderer.rules.link_open = (tokens, idx, opts, env, self) => {
  const tok = tokens[idx];
  const aIdx = tok.attrIndex("target");
  if (aIdx < 0) tok.attrPush(["target", "_blank"]);
  else tok.attrs![aIdx][1] = "_blank";
  const rIdx = tok.attrIndex("rel");
  if (rIdx < 0) tok.attrPush(["rel", "noopener noreferrer"]);
  return defaultRender(tokens, idx, opts, env, self);
};

export function SafeMarkdown(props: { source: string }) {
  const html = createMemo(() => {
    const raw = md.render(props.source ?? "");
    return DOMPurify.sanitize(raw, { ADD_ATTR: ["target", "rel"] });
  });
  return <div class="lk-md" innerHTML={html()} />;
}
