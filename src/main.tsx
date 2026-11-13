import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App";
import Calibration from "./Calibration";
import History from "./History";
import Target from "./Target";
import "./styles.css";

// A window that throws before rendering shows as a blank page, which is
// indistinguishable from a window whose webview never loaded. Print the failure
// so it can actually be read.
function showFatal(error: unknown) {
  const text =
    error instanceof Error
      ? `${error.message}\n\n${error.stack ?? ""}`
      : String(error);
  const escaped = text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
  document.body.classList.add("page-body");
  document.body.innerHTML =
    `<pre style="margin:0;padding:16px;color:#f87171;` +
    `font:12px ui-monospace,monospace;white-space:pre-wrap">` +
    `MScout window failed to start:\n\n${escaped}</pre>`;
}

window.addEventListener("error", (event) =>
  showFatal(event.error ?? event.message),
);
window.addEventListener("unhandledrejection", (event) =>
  showFatal(event.reason),
);

try {
  const label = getCurrentWindow().label;
  const isPage =
    label === "history" || label === "target" || label === "calibration";

  document.body.classList.toggle("page-body", isPage);

  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      {label === "history" ? (
        <History />
      ) : label === "target" ? (
        <Target />
      ) : label === "calibration" ? (
        <Calibration />
      ) : (
        <App />
      )}
    </React.StrictMode>,
  );
} catch (error) {
  showFatal(error);
}
