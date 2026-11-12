import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import App from "./App";
import Calibration from "./Calibration";
import History from "./History";
import Target from "./Target";
import "./styles.css";

const label = getCurrentWindow().label;
const isHistory = label === "history";
const isTarget = label === "target";
const isCalibration = label === "calibration";

document.body.classList.toggle("page-body", isHistory || isTarget || isCalibration);

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {isHistory ? (
      <History />
    ) : isTarget ? (
      <Target />
    ) : isCalibration ? (
      <Calibration />
    ) : (
      <App />
    )}
  </React.StrictMode>,
);
