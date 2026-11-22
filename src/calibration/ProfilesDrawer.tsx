// The Profiles drawer: the UI Size choice and the saved box sets per client
// size, with clearing. Which size is shown and which row is being confirmed
// are this drawer's own state.

import { useState } from "react";
import type { ProfileSummary, UiModeOption } from "../types";

type Props = {
  profiles: ProfileSummary[];
  activeProfile: ProfileSummary | null;
  uiModes: UiModeOption[];
  /** The live client size, when a frame has been frozen. */
  client: { w: number; h: number } | null;
  canReset: boolean;
  onChooseUiMode: (option: UiModeOption) => void;
  /** Delete a profile; resolves to whether it was removed. */
  onClear: (profile: ProfileSummary) => Promise<boolean>;
  onResetPreset: () => void;
};

export default function ProfilesDrawer({
  profiles,
  activeProfile,
  uiModes,
  client,
  canReset,
  onChooseUiMode,
  onClear,
  onResetPreset,
}: Props) {
  const [sizeChoice, setSizeChoice] = useState<string | null>(null);
  const [confirmKey, setConfirmKey] = useState<string | null>(null);
  const [busyKey, setBusyKey] = useState<string | null>(null);

  const liveMode = uiModes.find((option) => option.active)?.id ?? "default";
  const liveSize = client ? `${client.w}x${client.h}` : null;
  const sizeOptions = Array.from(
    new Set([
      ...(liveSize ? [liveSize] : []),
      ...profiles.map((profile) => `${profile.clientW}x${profile.clientH}`),
    ]),
  ).sort();
  const chosenSize = sizeChoice ?? liveSize ?? sizeOptions[0] ?? null;
  const shownProfiles = profiles.filter(
    (profile) => `${profile.clientW}x${profile.clientH}` === chosenSize,
  );
  const sameSet = (profile: ProfileSummary) =>
    client !== null &&
    profile.clientW === client.w &&
    profile.clientH === client.h &&
    profile.uiMode === liveMode;

  const clear = async (profile: ProfileSummary) => {
    setBusyKey(profile.key);
    try {
      // A failed delete keeps the row's confirm open, as the error is shown.
      if (await onClear(profile)) setConfirmKey(null);
    } finally {
      setBusyKey(null);
    }
  };

  return (
    <section className="cal-drawer">
      <div className="drawer-head">
        <strong>Profiles</strong>
        <span className="muted small">
          Boxes, digits and labels are kept per picture size and UI size, and
          the engine uses the pair matching the live window. Above 768 lines the
          UI size is detected from the game, so the choice below only applies at
          768 lines and below, where every UI size looks the same.
        </span>
      </div>

      <div className="cal-row ui-mode-choices">
        <span className="muted small">UI size (768 lines and below)</span>
        {uiModes.map((option) => (
          <button
            key={option.id}
            className={`ghost ${option.active ? "on" : ""}`}
            onClick={() => onChooseUiMode(option)}
          >
            {option.label}
          </button>
        ))}
      </div>

      <div className="cal-row ui-mode-choices">
        <span className="muted small">Show size</span>
        {sizeOptions.map((size) => (
          <button
            key={size}
            className={`ghost ${size === chosenSize ? "on" : ""}`}
            onClick={() => setSizeChoice(size)}
          >
            {size.replace("x", "×")}
            {size === liveSize ? " (live)" : ""}
          </button>
        ))}
      </div>

      <div className="profile-list">
        {shownProfiles.length === 0 && (
          <span className="muted small">
            No profile for{" "}
            {chosenSize ? chosenSize.replace("x", "×") : "this client size"} yet —
            freeze a frame to create one.
          </span>
        )}
        {shownProfiles.map((profile) => (
          <div
            key={profile.key}
            className={`profile-row ${
              activeProfile?.key === profile.key ? "on" : ""
            }`}
          >
            <div className="profile-meta">
              <strong>{profile.key}</strong>
              <span className="muted small">
                {uiModes.find((option) => option.id === profile.uiMode)?.label ??
                  profile.uiMode}
                {" · "}
                {profile.calibrated ? "can read" : "needs EXP box"}
                {` · ${profile.glyphs} digits · ${profile.maps} maps`}
                {profile.source
                  ? ` · ${profile.source} ${profile.glyphSize}`
                  : ""}
                {profile.name ? ` · ${profile.name}` : ""}
                {profile.job ? ` · ${profile.job}` : ""}
              </span>
            </div>
            <div className="profile-tags">
              {sameSet(profile) && <span className="tag current">live</span>}
              {activeProfile?.key === profile.key && (
                <span className="tag active">active</span>
              )}
            </div>
            {confirmKey === profile.key ? (
              <span className="profile-actions">
                <button
                  className="ghost danger"
                  disabled={busyKey === profile.key}
                  onClick={() => void clear(profile)}
                >
                  {busyKey === profile.key ? "removing…" : "delete"}
                </button>
                <button className="ghost" onClick={() => setConfirmKey(null)}>
                  cancel
                </button>
              </span>
            ) : (
              <button
                className="ghost"
                onClick={() => setConfirmKey(profile.key)}
              >
                clear
              </button>
            )}
          </div>
        ))}
      </div>

      <div className="cal-row">
        <button
          className="ghost"
          onClick={onResetPreset}
          disabled={!canReset}
        >
          Reset to preset
        </button>
      </div>
    </section>
  );
}
