import { useState, useEffect } from "react";
import type { CustomAppConfig, DiscordAppDetails, RunningProcessView } from "../types";
import { getRunningApplications, inspectDiscordApp } from "../api";

interface CustomAppModalProps {
  isOpen: boolean;
  initialApp: CustomAppConfig | null;
  onSave: (app: CustomAppConfig) => Promise<void>;
  onClose: () => void;
}

export default function CustomAppModal({
  isOpen,
  initialApp,
  onSave,
  onClose,
}: CustomAppModalProps) {
  const [name, setName] = useState("");
  const [processName, setProcessName] = useState("");
  const [stateText, setStateText] = useState("Active");
  const [detailsText, setDetailsText] = useState("");
  const [discordAppId, setDiscordAppId] = useState("");
  const [logoAsset, setLogoAsset] = useState("");
  const [verifiedDiscordApp, setVerifiedDiscordApp] = useState<DiscordAppDetails | null>(null);
  const [verifyingDiscord, setVerifyingDiscord] = useState(false);
  const [discordAppError, setDiscordAppError] = useState<string | null>(null);
  const [runningApps, setRunningApps] = useState<RunningProcessView[]>([]);
  const [loadingApps, setLoadingApps] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!isOpen) return;

    if (initialApp) {
      setName(initialApp.name);
      setProcessName(initialApp.process_name);
      setStateText(initialApp.state || "Active");
      setDetailsText(initialApp.details || "");
      setDiscordAppId(initialApp.discord_app_id ? String(initialApp.discord_app_id) : "");
      setLogoAsset(initialApp.logo_asset || "");
    } else {
      setName("");
      setProcessName("");
      setStateText("Active");
      setDetailsText("");
      setDiscordAppId("");
      setLogoAsset("");
    }
    setVerifiedDiscordApp(null);
    setDiscordAppError(null);
    setVerifyingDiscord(false);
    setError(null);
    setSaving(false);

    setLoadingApps(true);
    getRunningApplications()
      .then((apps) => setRunningApps(apps))
      .catch(() => setRunningApps([]))
      .finally(() => setLoadingApps(false));
  }, [isOpen, initialApp]);

  // Debounced Discord App ID inspection (passed as String to avoid JS IEEE-754 precision loss on 19-digit snowflakes)
  useEffect(() => {
    const trimmedId = discordAppId.trim();
    if (!trimmedId) {
      setVerifiedDiscordApp(null);
      setDiscordAppError(null);
      setVerifyingDiscord(false);
      return;
    }

    if (!/^\d+$/.test(trimmedId)) {
      setVerifiedDiscordApp(null);
      setDiscordAppError("Application ID must contain only digits (e.g. 1533559059125637311).");
      setVerifyingDiscord(false);
      return;
    }

    setVerifyingDiscord(true);
    setDiscordAppError(null);

    const timer = setTimeout(() => {
      inspectDiscordApp(trimmedId)
        .then((details) => {
          setVerifiedDiscordApp(details);
          setDiscordAppError(null);
          if (details.logo_asset) {
            setLogoAsset(details.logo_asset);
          }
          // Pre-populate application name if user hasn't typed one yet
          setName((prev) => (prev.trim() ? prev : details.name));
        })
        .catch((err) => {
          setVerifiedDiscordApp(null);
          setDiscordAppError(typeof err === "string" ? err : String(err));
        })
        .finally(() => {
          setVerifyingDiscord(false);
        });
    }, 450);

    return () => clearTimeout(timer);
  }, [discordAppId]);

  if (!isOpen) return null;

  const handleSelectRunningApp = (e: React.ChangeEvent<HTMLSelectElement>) => {
    const selectedProc = e.target.value;
    if (!selectedProc) return;

    const matched = runningApps.find((a) => a.process_name === selectedProc);
    if (matched) {
      setName(matched.name);
      setProcessName(matched.process_name);
      if (stateText === "Active" || !stateText) {
        setStateText("Active");
      }
    }
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const trimmedName = name.trim();
    let trimmedProc = processName.trim().toLowerCase();

    if (!trimmedName) {
      setError("Please provide an application name.");
      return;
    }
    if (trimmedProc && !trimmedProc.endsWith(".exe")) {
      trimmedProc += ".exe";
    }

    const trimmedDiscordId = discordAppId.trim();
    if (trimmedDiscordId && !/^\d+$/.test(trimmedDiscordId)) {
      setError("Discord App ID must contain only digits (e.g. 1533559059125637311).");
      return;
    }

    const resolvedLogoAsset = logoAsset.trim()
      ? logoAsset.trim()
      : verifiedDiscordApp?.logo_asset || null;

    const appConfig: CustomAppConfig = {
      id: initialApp ? initialApp.id : `custom_${Date.now()}`,
      name: trimmedName,
      process_name: trimmedProc,
      state: stateText.trim() || "Active",
      details: detailsText.trim() ? detailsText.trim() : null,
      discord_app_id: trimmedDiscordId,
      logo_asset: resolvedLogoAsset,
      enabled: initialApp ? initialApp.enabled : true,
    };

    setSaving(true);
    setError(null);
    try {
      await onSave(appConfig);
      onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal-content"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-labelledby="custom-app-modal-title"
      >
        <div className="modal-header">
          <div className="modal-title-wrap">
            <div className="modal-icon-badge">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2">
                <rect x="2" y="3" width="20" height="14" rx="2" ry="2" />
                <line x1="8" y1="21" x2="16" y2="21" />
                <line x1="12" y1="17" x2="12" y2="21" />
              </svg>
            </div>
            <h3 id="custom-app-modal-title" className="modal-title">
              {initialApp ? "Edit Custom Application" : "Add Custom Application"}
            </h3>
          </div>
          <button
            type="button"
            className="modal-close-btn"
            onClick={onClose}
            aria-label="Close modal"
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <line x1="18" y1="6" x2="6" y2="18" />
              <line x1="6" y1="6" x2="18" y2="18" />
            </svg>
          </button>
        </div>

        <form onSubmit={handleSubmit} className="modal-form">
          {error && <div className="modal-error-banner">{error}</div>}

          <div className="form-group">
            <label htmlFor="running-app-select" className="form-label">
              Pick from Running Applications
            </label>
            <div className="select-wrapper">
              <select
                id="running-app-select"
                className="form-select"
                onChange={handleSelectRunningApp}
                defaultValue=""
                disabled={loadingApps}
              >
                <option value="">
                  {loadingApps ? "Scanning running apps..." : "Choose a detected window / process..."}
                </option>
                {runningApps.map((app) => (
                  <option key={app.process_name} value={app.process_name}>
                    {app.name} ({app.process_name}) — {app.window_title}
                  </option>
                ))}
              </select>
            </div>
            <span className="form-help">
              Selecting an app automatically fills its name and executable.
            </span>
          </div>

          <div className="form-row">
            <div className="form-group flex-1">
              <label htmlFor="app-name-input" className="form-label">
                Display Name <span className="required-star">*</span>
              </label>
              <input
                id="app-name-input"
                type="text"
                className="form-input"
                placeholder="e.g. Blender, Photoshop, Ableton"
                value={name}
                onChange={(e) => setName(e.target.value)}
                required
              />
            </div>

            <div className="form-group flex-1">
              <label htmlFor="process-name-input" className="form-label">
                Process Name (Optional)
              </label>
              <input
                id="process-name-input"
                type="text"
                className="form-input font-mono"
                placeholder="e.g. blender.exe (optional)"
                value={processName}
                onChange={(e) => setProcessName(e.target.value)}
              />
              <span className="form-help">Leave blank to broadcast presence always (standalone mode).</span>
            </div>
          </div>

          <div className="form-row">
            <div className="form-group flex-1">
              <label htmlFor="state-text-input" className="form-label">
                Primary Activity / Line 1
              </label>
              <input
                id="state-text-input"
                type="text"
                className="form-input"
                placeholder="e.g. Active, 3D Modeling, Designing"
                value={stateText}
                onChange={(e) => setStateText(e.target.value)}
              />
              <span className="form-help">
                Primary line in Discord (overridden by active window title when focused).
              </span>
            </div>

            <div className="form-group flex-1">
              <label htmlFor="details-text-input" className="form-label">
                Secondary Details / Line 2 (Optional)
              </label>
              <input
                id="details-text-input"
                type="text"
                className="form-input"
                placeholder="e.g. Project Work, Sculpting"
                value={detailsText}
                onChange={(e) => setDetailsText(e.target.value)}
              />
              <span className="form-help">Secondary descriptive line shown below Line 1 in Discord.</span>
            </div>
          </div>

          <div className="form-group">
            <label htmlFor="discord-id-input" className="form-label">
              Custom Discord Application ID (Optional)
            </label>
            <input
              id="discord-id-input"
              type="text"
              className="form-input font-mono"
              placeholder="Leave empty to use Presence default ID"
              value={discordAppId}
              onChange={(e) => setDiscordAppId(e.target.value)}
            />
            {verifyingDiscord && (
              <div className="discord-verify-status is-loading">
                <span className="verify-spin">⌛</span> Verifying Discord application ID...
              </div>
            )}
            {discordAppError && (
              <div className="discord-verify-status is-error">
                <span>⚠️</span> {discordAppError}
              </div>
            )}
            {verifiedDiscordApp && (
              <div className="discord-verify-status is-success">
                <div className="verified-app-name">
                  ✓ Discord Application: <strong>{verifiedDiscordApp.name}</strong>
                </div>
                <div className="verified-asset-info">
                  {verifiedDiscordApp.logo_asset ? (
                    <span>🖼️ Rich Presence Asset detected: <code>{verifiedDiscordApp.logo_asset}</code></span>
                  ) : (
                    <span>ℹ️ No asset named <code>logo</code> found. Status will show without a broken image.</span>
                  )}
                </div>
              </div>
            )}
            <span className="form-help">
              Leave blank to broadcast under Presence with your app name as Line 1. If you enter your own Discord Application ID, Presence will verify the app and automatically use your <code>logo</code> asset.
            </span>
          </div>

          <div className="modal-actions">
            <button
              type="button"
              className="modal-btn-cancel"
              onClick={onClose}
              disabled={saving}
            >
              Cancel
            </button>
            <button
              type="submit"
              className="modal-btn-save"
              disabled={saving}
            >
              {saving ? "Saving..." : initialApp ? "Save Changes" : "Add Application"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
