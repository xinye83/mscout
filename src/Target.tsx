import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { TargetStatus, WindowInfo } from "./types";

const SCAN_MS = 1500;

function statusLine(status: TargetStatus | null): string {
  if (!status) return "checking…";
  if (!status.configured) return "No target selected — pick the game window below.";
  if (status.found && status.minimized) {
    return "Attached, but the window is minimized. Restore the game to capture.";
  }
  if (status.found) {
    const client = status.client;
    return client
      ? `Attached · client ${client.w}×${client.h} at ${status.screen?.x ?? 0},${status.screen?.y ?? 0}`
      : "Attached";
  }
  const wanted = status.selector;
  if (!wanted) return "Target window not open — waiting.";
  return `Waiting for ${wanted.process || wanted.title} — it reattaches by name when the game returns.`;
}

export default function Target() {
  const [windows, setWindows] = useState<WindowInfo[] | null>(null);
  const [status, setStatus] = useState<TargetStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  const scan = useCallback(() => {
    invoke<WindowInfo[]>("list_windows")
      .then(setWindows)
      .catch((cause) => {
        setWindows([]);
        setError(String(cause));
      });
    invoke<TargetStatus>("get_target_status")
      .then(setStatus)
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    scan();
    const id = window.setInterval(scan, SCAN_MS);
    const unlisten = listen<TargetStatus>("target-status", (event) =>
      setStatus(event.payload),
    );

    return () => {
      window.clearInterval(id);
      void unlisten.then((fn) => fn());
    };
  }, [scan]);

  const attach = (window: WindowInfo) => {
    setError(null);
    invoke<TargetStatus>("set_target", {
      title: window.title,
      process: window.process,
    })
      .then(setStatus)
      .catch((cause) => setError(String(cause)));
  };

  const detach = () => {
    setError(null);
    invoke<TargetStatus>("clear_target").catch((cause) => setError(String(cause)));
  };

  const current = status?.selector ?? null;
  const dotClass = !status?.configured
    ? "off"
    : status.found
      ? status.minimized
        ? "degraded"
        : "reading"
      : "lost";

  const rows = windows ?? [];

  return (
    <div className="page">
      <header className="page-head">
        <div>
          <h1>Target window</h1>
          <p className="muted">
            Attach by title and process. The choice survives game restarts.
          </p>
        </div>
        <div className="presets">
          <button className="ghost" onClick={scan}>
            Rescan
          </button>
          <button
            className="ghost"
            onClick={() => void invoke("open_calibration")}
          >
            Calibrate
          </button>
        </div>
      </header>

      <section className="target-card">
        <span className={`dot ${dotClass}`} />
        <div className="target-meta">
          <strong>
            {status?.configured
              ? status.found
                ? status.title
                : current?.title || current?.process || "target"
              : "No target"}
          </strong>
          <span className="muted small">{statusLine(status)}</span>
        </div>
        {status?.configured && (
          <button className="ghost" onClick={detach}>
            Detach
          </button>
        )}
      </section>

      <p className="muted small">
        Windows with a title, excluding this app and tool windows. The game must run
        windowed or borderless — exclusive fullscreen cannot be captured.
      </p>

      <div className="win-list">
        {windows === null && <div className="empty">scanning…</div>}
        {windows !== null && rows.length === 0 && (
          <div className="empty">No capturable windows found.</div>
        )}
        {rows.map((window) => {
          const selected =
            current !== null &&
            current.title === window.title &&
            current.process === window.process;
          return (
            <button
              key={window.hwnd}
              className={`win-row ${selected ? "on" : ""}`}
              onClick={() => attach(window)}
            >
              <span className="win-title">{window.title}</span>
              <span className="win-proc">
                {window.process || "unknown process"} · pid {window.pid} ·{" "}
                {window.client.w}×{window.client.h}
                {window.minimized ? " · minimized" : ""}
              </span>
            </button>
          );
        })}
      </div>

      {error && <p className="error small">{error}</p>}
    </div>
  );
}
