import { useEffect, useState } from "react";
import type { AutostartView } from "@nexus/contracts";
import { api, describeError } from "../api";
import { uptime } from "../model/format.ts";
import { BackupCard, PersonaCard } from "../Settings";
import { useGoTo } from "../shell/nav";
import { usePref } from "../state/prefs";
import type { CoreMode, Theme } from "../state/prefs";
import { useNexus } from "../state/store";
import { ErrorLine, Facts, Panel } from "../ui";
import { CoreIcon, PlugIcon, SettingsIcon } from "../icons";

/** Off unless you turn it on: one entry in your own Windows sign-in list. */
function StartWithWindows() {
  const [state, setState] = useState<AutostartView | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .getAutostart()
      .then(setState)
      .catch((err) => setError(describeError(err)));
  }, []);

  async function toggle(enabled: boolean) {
    setBusy(true);
    setError(null);
    try {
      setState(await api.setAutostart(enabled));
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="startup">
      <label className="check">
        <input
          type="checkbox"
          checked={state?.enabled ?? false}
          disabled={busy || state === null}
          onChange={(e) => void toggle(e.currentTarget.checked)}
        />
        Start Nexus.Host when I sign in to Windows
      </label>
      <p className="muted small">
        Off unless you turn it on. When on, Windows starts the Host in the background after you sign in, so this window
        connects at once; the window itself does not open. Turning it off removes the entry.
      </p>
      {state?.enabled && !state.points_here && (
        <p className="warning small">It is set to start another copy of Nexus: {state.command}</p>
      )}
      <ErrorLine error={error} />
    </div>
  );
}

export function SettingsView() {
  const d = useNexus();
  const goTo = useGoTo();
  const [theme, setTheme] = usePref("theme");
  const [core, setCore] = usePref("core");
  const host = d.host?.state === "Connected" ? d.host : null;
  return (
    <div className="settings">
      <Panel title="Host" icon={<CoreIcon size={15} />} id="host">
        {host && (
          <Facts
            rows={[
              ["Status", host.paused ? "Paused (restored profile)" : "Connected"],
              ["Identity", host.host_identity],
              ["Version", d.system?.host_version ?? "—"],
              ["Protocol", `v${host.protocol_version}`],
              ["Running for", uptime(host.uptime_seconds)],
              ["Transport", "Local named pipe, this Windows account only"],
            ]}
          />
        )}
        <p className="muted small">Closing this window does not stop the Host or any accepted task.</p>
        <StartWithWindows />
      </Panel>

      <Panel title="Appearance" icon={<SettingsIcon size={15} />} id="appearance">
        <div className="form-grid">
          <label>
            Theme
            <select value={theme} onChange={(e) => setTheme(e.currentTarget.value as Theme)}>
              <option value="dark">Dark</option>
              <option value="light">Light</option>
              <option value="system">Follow Windows</option>
            </select>
          </label>
          <label>
            Nexus Core
            <select value={core} onChange={(e) => setCore(e.currentTarget.value as CoreMode)}>
              <option value="animated">Animated (WebGL)</option>
              <option value="still">Still frame (WebGL, no motion)</option>
              <option value="off">Off (static drawing, no WebGL)</option>
            </select>
          </label>
        </div>
        <p className="muted small">
          Windows “reduce animations” always gives a still core. The animated core renders only while Nexus is visible, and
          slower when idle or unfocused.
        </p>
      </Panel>

      <Panel title="AI connections" icon={<PlugIcon size={15} />} id="connections-link">
        <p className="muted small">Connections, keys, models and thinking level are managed in Models.</p>
        <button type="button" className="secondary" onClick={() => goTo({ section: "models" })}>
          Open Models
        </button>
      </Panel>

      <PersonaCard />
      <BackupCard />
    </div>
  );
}
