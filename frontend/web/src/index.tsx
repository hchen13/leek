/* @refresh reload */
import { render } from "solid-js/web";
import "./styles.css";
import { App } from "./App";

const root = document.getElementById("app");
if (!root) throw new Error("#app element not found");
render(() => <App />, root);
