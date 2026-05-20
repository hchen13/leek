// markdown.ts — one entry point for every markdown render in the app
// (REQUIREMENTS §2 — chat / canvas / right rail all render the same way).
//
// Every render goes through here so configuration (GFM, line breaks, link
// targets, etc.) lives in one place and sanitize is never skipped — LLM
// output is untrusted content (prompt injection could embed `<script>` or
// `onerror=` attributes), so DOMPurify must run on every call.

import { marked } from "marked";
import DOMPurify from "dompurify";

// `breaks: true` lets a single newline become `<br>` — the chat / note voice
// favours visible line wraps over GFM's collapsed-paragraph default.
marked.setOptions({ gfm: true, breaks: true });

// Open every link in a new tab and disown the opener. Hooks run on every
// `DOMPurify.sanitize` call, so registering once at module load is enough.
DOMPurify.addHook("afterSanitizeAttributes", (node) => {
  if (node instanceof HTMLAnchorElement) {
    node.setAttribute("target", "_blank");
    node.setAttribute("rel", "noopener noreferrer");
  }
});

/** Block-level render — paragraphs, lists, headings, fenced code, etc.
 *  Returned HTML always wraps in block elements. */
export function renderMarkdown(text: string | null | undefined): string {
  if (!text) return "";
  const html = marked.parse(text, { async: false }) as string;
  return DOMPurify.sanitize(html);
}

/** Inline-only render — `**bold**`, `[link](url)`, `` `code` `` — without
 *  the wrapping `<p>` `marked.parse` adds. Use for one-line surfaces (plan
 *  steps, tool-result snippets, summary details) where a block `<p>` would
 *  break inline layout. */
export function renderInlineMarkdown(text: string | null | undefined): string {
  if (!text) return "";
  const html = marked.parseInline(text, { async: false }) as string;
  return DOMPurify.sanitize(html);
}
