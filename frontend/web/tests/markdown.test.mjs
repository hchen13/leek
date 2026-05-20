// Sanity test for markdown.ts (M2-polish §B "renderMarkdown 单测").
//
// Mirrors the configuration in src/markdown.ts and asserts that:
//   1. Basic GFM constructs (bold, link) render to expected HTML tags.
//   2. DOMPurify strips dangerous content (script tag).
//   3. Inline render returns no wrapping <p>.
//   4. The link target hook adds target="_blank" rel="noopener noreferrer".
//
// Run with `node tests/markdown.test.mjs` from `frontend/web/`.

import { JSDOM } from "jsdom";
import { marked } from "marked";
import createDOMPurify from "dompurify";

const window = new JSDOM("").window;
const DOMPurify = createDOMPurify(window);

marked.setOptions({ gfm: true, breaks: true });

DOMPurify.addHook("afterSanitizeAttributes", (node) => {
  if (node.tagName === "A") {
    node.setAttribute("target", "_blank");
    node.setAttribute("rel", "noopener noreferrer");
  }
});

function renderMarkdown(text) {
  if (!text) return "";
  const html = marked.parse(text, { async: false });
  return DOMPurify.sanitize(html);
}

function renderInlineMarkdown(text) {
  if (!text) return "";
  const html = marked.parseInline(text, { async: false });
  return DOMPurify.sanitize(html);
}

let pass = 0;
let fail = 0;

function check(name, predicate) {
  const ok = !!predicate();
  if (ok) {
    pass += 1;
    console.log(`  ok  ${name}`);
  } else {
    fail += 1;
    console.log(`  FAIL ${name}`);
  }
}

console.log("renderMarkdown:");

const inputBoldLink = "**bold** [link](https://x)";
const outBoldLink = renderMarkdown(inputBoldLink);
check("bold renders to <strong>", () => outBoldLink.includes("<strong>bold</strong>"));
check("link renders to <a href>", () => outBoldLink.includes('href="https://x"'));
check("link gets target=_blank from hook", () =>
  outBoldLink.includes('target="_blank"') && outBoldLink.includes('rel="noopener noreferrer"'),
);

const inputScript = "<script>alert(1)</script>";
const outScript = renderMarkdown(inputScript);
check("script tag stripped", () => !outScript.includes("<script"));
check("script tag stripped (no inner alert exec)", () => !outScript.includes("alert(1)") || !outScript.includes("<script"));

const inputOnerror = '<img src=x onerror="alert(2)">';
const outOnerror = renderMarkdown(inputOnerror);
check("onerror handler stripped", () => !outOnerror.includes("onerror"));

const inputJsUrl = "[evil](javascript:alert(3))";
const outJsUrl = renderMarkdown(inputJsUrl);
check("javascript: URL stripped", () => !outJsUrl.includes("javascript:"));

const outEmpty = renderMarkdown("");
check("empty input returns empty string", () => outEmpty === "");
check("null input returns empty string", () => renderMarkdown(null) === "");

const outHeader = renderMarkdown("# Heading\n\nbody text");
check("heading renders to <h1>", () => outHeader.includes("<h1>Heading</h1>"));

const outList = renderMarkdown("- one\n- two");
check("list renders to <ul><li>", () => outList.includes("<ul>") && outList.includes("<li>one</li>"));

const outCode = renderMarkdown("```\ncode block\n```");
check("fenced code renders to <pre><code>", () => outCode.includes("<pre><code>") && outCode.includes("code block"));

console.log("\nrenderInlineMarkdown:");

const outInline = renderInlineMarkdown("**bold** text");
check("inline bold renders to <strong>", () => outInline.includes("<strong>bold</strong>"));
check("inline does NOT wrap in <p>", () => !outInline.includes("<p>"));

const outInlineScript = renderInlineMarkdown("<script>alert(1)</script>");
check("inline script stripped", () => !outInlineScript.includes("<script"));

console.log(`\n${pass} passed, ${fail} failed`);
if (fail > 0) process.exit(1);
