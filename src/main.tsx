import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { applyTheme, useUi } from "./state/ui";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/ui.css";
import "./styles/layout.css";

// Before the first render, so a light theme does not flash dark.
applyTheme(useUi.getState().theme);
useUi.subscribe((s, prev) => s.theme !== prev.theme && applyTheme(s.theme));

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
