// Inline SVG icon set. Stroke-only, 1.5px monoline, currentColor — so
// `color: var(--accent)` on a parent flows into the icon without an
// extra prop. Ported + extended from origin/main's Icon.tsx; new entries
// (history, send, x, chevronL, dot, brain, more) are flagged inline.

import type { JSX } from "solid-js";

export type IconName =
  // Rail / nav
  | "chat"
  | "settings"
  | "brand"
  // Composer / actions
  | "send"
  | "plus"
  | "search"
  | "x"
  | "more"
  // Chevrons
  | "chevronR"
  | "chevronD"
  | "chevronL"
  // Card / state
  | "expand"
  | "copy"
  | "dot"
  // Session drawer
  | "history"
  | "trash"
  | "pencil"
  // Misc (kept for Phase 2 cards)
  | "eye"
  | "branch"
  | "spark";

const PATHS: Record<IconName, JSX.Element> = {
  chat: (
    <path d="M3 12c0-4.4 3.6-8 8-8s8 3.6 8 8-3.6 8-8 8c-1 0-2-.2-2.9-.5L4 21l1.5-3.4C4 16 3 14 3 12Z" />
  ),
  settings: (
    <>
      <circle cx="12" cy="12" r="3" />
      <path d="M19 12a7 7 0 0 0-.1-1.2l2-1.5-2-3.5-2.4.9a7 7 0 0 0-2-1.2L14 3h-4l-.5 2.5a7 7 0 0 0-2 1.2l-2.4-.9-2 3.5 2 1.5A7 7 0 0 0 5 12c0 .4 0 .8.1 1.2l-2 1.5 2 3.5 2.4-.9a7 7 0 0 0 2 1.2L10 21h4l.5-2.5a7 7 0 0 0 2-1.2l2.4.9 2-3.5-2-1.5c.1-.4.1-.8.1-1.2Z" />
    </>
  ),
  // brand "L" — boxed, slight stroke (extends main's Icon set; the Rail
  // renders this in a 36×36 rounded square so the L sits as a brand mark
  // rather than a plain glyph).
  brand: <path d="M7 5v14h10" />,
  send: <path d="m4 12 16-8-6 16-2-7-8-1Z" />,
  plus: <path d="M12 5v14M5 12h14" />,
  search: (
    <>
      <circle cx="11" cy="11" r="7" />
      <path d="m20 20-3.5-3.5" />
    </>
  ),
  x: <path d="m6 6 12 12M18 6 6 18" />,
  more: (
    <>
      <circle cx="5" cy="12" r="1.4" />
      <circle cx="12" cy="12" r="1.4" />
      <circle cx="19" cy="12" r="1.4" />
    </>
  ),
  chevronR: <path d="m9 6 6 6-6 6" />,
  chevronD: <path d="m6 9 6 6 6-6" />,
  chevronL: <path d="m15 6-6 6 6 6" />,
  expand: (
    <path d="M4 9V5a1 1 0 0 1 1-1h4M20 9V5a1 1 0 0 0-1-1h-4M4 15v4a1 1 0 0 0 1 1h4M20 15v4a1 1 0 0 1-1 1h-4" />
  ),
  copy: (
    <>
      <rect x="9" y="9" width="11" height="11" rx="2" />
      <path d="M5 15V5a1 1 0 0 1 1-1h10" />
    </>
  ),
  dot: <circle cx="12" cy="12" r="3" />,
  // history — clock-with-arrow (new for Session drawer trigger)
  history: (
    <>
      <path d="M3 12a9 9 0 1 0 3-6.7" />
      <path d="M3 4v5h5" />
      <path d="M12 8v4l3 2" />
    </>
  ),
  trash: (
    <>
      <path d="M4 7h16" />
      <path d="M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" />
      <path d="M6 7v12a2 2 0 0 0 2 2h8a2 2 0 0 0 2-2V7" />
      <path d="M10 11v6M14 11v6" />
    </>
  ),
  pencil: (
    <>
      <path d="M4 20h4l10-10a2.8 2.8 0 1 0-4-4L4 16Z" />
      <path d="m13 7 4 4" />
    </>
  ),
  eye: (
    <>
      <path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12Z" />
      <circle cx="12" cy="12" r="3" />
    </>
  ),
  branch: (
    <>
      <circle cx="6" cy="6" r="2" />
      <circle cx="6" cy="18" r="2" />
      <circle cx="18" cy="12" r="2" />
      <path d="M6 8v8M8 18a6 6 0 0 0 6-6 6 6 0 0 1 4-6" />
    </>
  ),
  spark: (
    <>
      <path d="M12 4v4M12 16v4M4 12h4M16 12h4" />
      <path d="m6 6 2 2M16 16l2 2M18 6l-2 2M8 16l-2 2" />
    </>
  ),
};

type IconProps = {
  name: IconName;
  /** Extra CSS class — base styling is `.lk-ic` set in styles.css. */
  class?: string;
  /** Stroke override. Defaults to currentColor so the parent's text color
   *  decides; pass when an icon's color should diverge from its container
   *  (e.g. a status dot inside a neutral row). */
  size?: number;
  /** ARIA label. Default decorative — icons are paired with text or
   *  buttons that carry their own labels, so the icon itself is
   *  `aria-hidden`. Pass a value to make it announce. */
  ariaLabel?: string;
};

export function Icon(props: IconProps) {
  const isDecorative = () => props.ariaLabel == null;
  const size = () => props.size ?? 20;
  return (
    <svg
      class={props.class ?? "lk-ic"}
      width={size()}
      height={size()}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden={isDecorative() ? "true" : undefined}
      aria-label={isDecorative() ? undefined : props.ariaLabel}
      role={isDecorative() ? undefined : "img"}
    >
      {PATHS[props.name]}
    </svg>
  );
}
