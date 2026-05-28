import { render } from "solid-js/web";

import App from "./App";
import { initTheme } from "./lib/theme";
// Design tokens load FIRST so every override in styles.css references a
// var() that resolves; theme preference applies to <html> before mount so
// initial paint matches user choice.
import "./design-system/tokens.css";
import "./styles.css";

initTheme();

const root = document.getElementById("app");
if (root) {
  render(() => <App />, root);
}
