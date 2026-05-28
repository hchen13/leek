// Theme — dark / light toggle.
//
// L.E.E.K is dark-mode native; the light mode is the Phase 4 polish layer.
// Phase 1 still ships the toggle so the cascade (every var(--*) flipping
// from dark to light) is visible and tested even before light is polished.
//
// Source of truth = `<html data-theme="...">`; tokens.css already defines
// `:root[data-theme="light"]` overrides. We just persist the user choice
// in localStorage so a refresh keeps it, and expose a tiny API for the
// Settings page.

const STORAGE_KEY = "leek.theme";

export type Theme = "dark" | "light";

function isTheme(v: unknown): v is Theme {
  return v === "dark" || v === "light";
}

function readStored(): Theme | null {
  try {
    const v = localStorage.getItem(STORAGE_KEY);
    return isTheme(v) ? v : null;
  } catch {
    // localStorage may be unavailable (private mode, etc.) — degrade silently.
    return null;
  }
}

function writeStored(theme: Theme) {
  try {
    localStorage.setItem(STORAGE_KEY, theme);
  } catch {
    // Same as above — silent.
  }
}

/** Apply a theme to `<html>` AND persist it. Call from anywhere; the
 *  cascade does the rest (tokens.css `:root[data-theme="light"]` overrides). */
export function applyTheme(theme: Theme) {
  document.documentElement.setAttribute("data-theme", theme);
  writeStored(theme);
}

/** Read the current theme — what `<html data-theme=…>` says, falling
 *  back to "dark" (the design default). */
export function currentTheme(): Theme {
  const attr = document.documentElement.getAttribute("data-theme");
  return isTheme(attr) ? attr : "dark";
}

/** Initialise on app start — pulls the user's prior choice out of
 *  localStorage and stamps `<html>`. Called once from `index.tsx`
 *  before mount. No-op if no stored value (default = dark, which is
 *  the unset state). */
export function initTheme() {
  const stored = readStored();
  if (stored) document.documentElement.setAttribute("data-theme", stored);
}
