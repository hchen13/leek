// Render markdown safely. We use markdown-it for the parser (good GFM
// support: tables, fenced code, autolinks) and dompurify to strip any
// HTML the LLM might emit. Output goes through innerHTML.

import DOMPurify from "dompurify";
import MarkdownIt from "markdown-it";
import { createEffect, createMemo, onCleanup } from "solid-js";

/** Pretty-print a wikilink id's last segment ("long-term-debt-cycle" →
 *  "Long-term debt cycle") so the chat doesn't expose internal paths. */
function prettyFromId(id: string): string {
  const last = id.split("/").pop() || id;
  // Keep first-letter capitalization but leave the rest lowercased so we
  // don't randomly Title-Case English filler ("compound-interest" stays
  // "Compound interest", not "Compound Interest").
  const spaced = last.replace(/[-_]/g, " ");
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

/** Rewrite raw corpus path references inside markdown text so they appear
 *  as clean `[Title](leek-wiki:id)` links instead of leaking
 *  `wikis/principles/...` or `[[…]]` wikilinks at the user. */
export function rewriteCorpusPaths(text: string): string {
  if (!text) return text;
  let out = text;
  // [[wikis/foo/bar]] form (Obsidian / wiki convention)
  out = out.replace(/\[\[((?:wikis|sources)\/[^\]|]+)(?:\|[^\]]+)?\]\]/g,
    (_full, id: string) => `[${prettyFromId(id)}](leek-wiki:${id})`);
  // `wikis/foo/bar` inside backticks
  out = out.replace(/`((?:wikis|sources)\/[^`]+)`/g,
    (_full, raw: string) => {
      const id = raw.replace(/[.,)\]]+$/, "");
      return `[${prettyFromId(id)}](leek-wiki:${id})`;
    });
  // Bare paths — only after a non-word char so we don't damage URLs etc.
  out = out.replace(/(^|[\s(])(wikis|sources)\/([A-Za-z0-9_\-./]+?)(?=[\s.,)\]]|$)/g,
    (_full, lead: string, base: string, rest: string) => {
      const id = `${base}/${rest}`.replace(/[.,)\]]+$/, "");
      return `${lead}[${prettyFromId(id)}](leek-wiki:${id})`;
    });
  return out;
}

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
  const href = tok.attrGet("href") ?? "";

  // leek-wiki: links navigate inside the corpus modal — don't open in a
  // new tab and don't rewrite the visible text.
  if (href.startsWith("leek-wiki:")) {
    return defaultLinkOpen(tokens, idx, opts, env, self);
  }

  const aIdx = tok.attrIndex("target");
  if (aIdx < 0) tok.attrPush(["target", "_blank"]);
  else tok.attrs![aIdx][1] = "_blank";
  const rIdx = tok.attrIndex("rel");
  if (rIdx < 0) tok.attrPush(["rel", "noopener noreferrer"]);

  // If next token is a text whose content equals the href (i.e. plain URL
  // autolink, not [title](url)), prefer the page title we cached from a
  // prior web_fetch on this URL; fall back to a friendly hostname.
  const next = tokens[idx + 1];
  if (next && next.type === "text" && next.content && next.content === href) {
    const titles = (env as { urlTitles?: Record<string, string> })?.urlTitles;
    const title = titles?.[href];
    next.content = title || friendlyHost(href);
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

export function SafeMarkdown(props: {
  source: string;
  /** When set, leek-wiki: links are intercepted and routed through this
   *  callback (preventDefault on the original click) so the host app
   *  controls navigation. Without it, clicks fall through. */
  onWikiOpen?: (id: string) => void;
  /** Map of url → page title harvested from prior web_fetch tool calls.
   *  Plain URL autolinks render with the cached title instead of a bare
   *  hostname (so chat shows "Warren Buffett - Wikipedia" not "en.wikipedia.org"). */
  urlTitles?: Record<string, string>;
}) {
  const html = createMemo(() => {
    const rewritten = rewriteCorpusPaths(props.source ?? "");
    const env = { urlTitles: props.urlTitles };
    const raw = md.render(rewritten, env);
    return DOMPurify.sanitize(raw, {
      ADD_ATTR: ["target", "rel"],
      // Same as the DOMPurify default plus the `leek-wiki:` scheme used
      // by CorpusDocModal to keep wiki↔wiki navigation in-app.
      ALLOWED_URI_REGEXP:
        /^(?:(?:https?|mailto|tel|ftp|leek-wiki):|[^a-z]|[a-z+.\-]+(?:[^a-z+.\-:]|$))/i,
    });
  });

  let rootEl!: HTMLDivElement;
  createEffect(() => {
    const cb = props.onWikiOpen;
    if (!rootEl || !cb) return;
    const handler = (e: MouseEvent) => {
      const t = e.target as HTMLElement | null;
      const a = t?.closest("a") as HTMLAnchorElement | null;
      if (!a) return;
      const href = a.getAttribute("href") ?? "";
      if (href.startsWith("leek-wiki:")) {
        e.preventDefault();
        cb(href.slice("leek-wiki:".length));
      }
    };
    rootEl.addEventListener("click", handler);
    onCleanup(() => rootEl.removeEventListener("click", handler));
  });

  return <div class="lk-md" ref={(el) => (rootEl = el)} innerHTML={html()} />;
}
