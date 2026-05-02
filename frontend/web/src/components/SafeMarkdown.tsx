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

// Open links in a new tab so the user doesn't lose their session, AND
// when the visible text equals the href (autolinked plain URLs), replace
// the visible text with the hostname so the chat doesn't get cluttered
// with full URLs.
const defaultLinkOpen =
  md.renderer.rules.link_open ||
  ((tokens, idx, opts, _env, self) => self.renderToken(tokens, idx, opts));

md.renderer.rules.link_open = (tokens, idx, opts, env, self) => {
  const tok = tokens[idx];
  const aIdx = tok.attrIndex("target");
  if (aIdx < 0) tok.attrPush(["target", "_blank"]);
  else tok.attrs![aIdx][1] = "_blank";
  const rIdx = tok.attrIndex("rel");
  if (rIdx < 0) tok.attrPush(["rel", "noopener noreferrer"]);

  // If next token is a text whose content equals the href (i.e. plain
  // URL autolink, not [title](url)), shorten it to a hostname.
  const href = tok.attrGet("href") ?? "";
  const next = tokens[idx + 1];
  if (next && next.type === "text" && next.content && next.content === href) {
    next.content = friendlyHost(href);
  }
  return defaultLinkOpen(tokens, idx, opts, env, self);
};

function friendlyHost(url: string): string {
  try {
    const u = new URL(url);
    let host = u.hostname.replace(/^www\./, "");
    // Add a path hint when it's not just a homepage, so SEC filings stay
    // distinguishable: "sec.gov/Archives/…" not just "sec.gov".
    if (u.pathname && u.pathname !== "/" && u.pathname.length > 1) {
      const segs = u.pathname.split("/").filter(Boolean);
      if (segs.length > 0) {
        const tail = segs[segs.length - 1];
        // truncate long tail segments
        const shortTail = tail.length > 28 ? tail.slice(0, 25) + "…" : tail;
        host += "/" + shortTail;
      }
    }
    return host;
  } catch {
    return url;
  }
}

export function SafeMarkdown(props: { source: string }) {
  const html = createMemo(() => {
    const raw = md.render(props.source ?? "");
    return DOMPurify.sanitize(raw, { ADD_ATTR: ["target", "rel"] });
  });
  return <div class="lk-md" innerHTML={html()} />;
}
