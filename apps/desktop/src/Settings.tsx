import { useCallback, useEffect, useState } from "react";
import type { BackupSummary, ConnectionView, ModelView, PersonaView } from "@nexus/contracts";
import { api, describeError } from "./api";
import { PlugIcon, SparkleIcon } from "./icons";

export function ConnectionsCard() {
  const [connections, setConnections] = useState<ConnectionView[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setConnections(await api.listConnections());
      setError(null);
    } catch (err) {
      setError(describeError(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <section className="card" aria-labelledby="connections-heading">
      <h2 id="connections-heading">
        <PlugIcon /> AI connections
      </h2>
      <p className="muted">
        The API key is kept only in your Windows vault (Credential Manager). Nexus never displays
        it and never puts it in the database, logs, or backups.
      </p>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      {connections?.map((c) => (
        <ConnectionItem key={c.connection_id} connection={c} onChanged={() => void load()} />
      ))}
      {connections !== null && connections.length === 0 && <p className="muted">No connections yet.</p>}
      <NewGeminiConnection onSaved={() => void load()} />
    </section>
  );
}

function FreeTierNotice() {
  return (
    <p className="warning">
      Google AI Studio free tier: outside the EEA, Switzerland and the UK, Google may use what you
      send to improve its products, and human reviewers may read your conversations. Don't send
      personal data, passwords, or sensitive information.
    </p>
  );
}

function NewGeminiConnection({ onSaved }: { onSaved: () => void }) {
  const [name, setName] = useState("Google AI Studio");
  const [key, setKey] = useState("");
  const [freeTier, setFreeTier] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function save(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api.saveGeminiConnection(null, name, freeTier, key);
      setKey("");
      onSaved();
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="note-form" onSubmit={save} aria-label="New Gemini connection">
      <h3>New Gemini connection</h3>
      <label>
        Connection name
        <input value={name} maxLength={100} onChange={(e) => setName(e.currentTarget.value)} required />
      </label>
      <label>
        Gemini API key
        <input
          type="password"
          value={key}
          autoComplete="off"
          spellCheck={false}
          maxLength={512}
          onChange={(e) => setKey(e.currentTarget.value)}
          required
        />
      </label>
      <label className="check">
        <input type="checkbox" checked={freeTier} onChange={(e) => setFreeTier(e.currentTarget.checked)} />
        I'm using the free tier
      </label>
      {freeTier && <FreeTierNotice />}
      <div className="actions">
        <button type="submit" disabled={busy || key.trim() === "" || name.trim() === ""}>
          {busy ? "Verifying key…" : "Verify and save"}
        </button>
      </div>
      <p className="muted">
        The key is verified with a real request to Google (the model list); a rejected key is not
        saved.
      </p>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </form>
  );
}

function ConnectionItem({ connection, onChanged }: { connection: ConnectionView; onChanged: () => void }) {
  const [models, setModels] = useState<ModelView[] | null>(null);
  const [choice, setChoice] = useState(connection.model ?? "");
  const [newKey, setNewKey] = useState("");
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function act(action: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await action();
      onChanged();
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="connection" aria-label={`Connection ${connection.display_name}`}>
      <h3>
        {connection.display_name}{" "}
        {connection.active && <span className="badge badge-good">Active</span>}{" "}
        {connection.free_tier && <span className="badge badge-attention">Free tier</span>}
      </h3>
      <dl className="facts">
        <dt>Provider</dt>
        <dd>{connection.provider}</dd>
        <dt>Model</dt>
        <dd>{connection.model ?? "not chosen"}</dd>
        <dt>Key</dt>
        <dd>{connection.has_key ? "in the Windows vault" : "missing — enter a new one"}</dd>
      </dl>

      <div className="actions">
        <button
          type="button"
          className="secondary"
          disabled={busy || !connection.has_key}
          onClick={() =>
            act(async () => {
              const list = await api.listModels(connection.connection_id);
              setModels(list);
              if (!choice && list.length > 0) setChoice(list[0].id);
            })
          }
        >
          Load models
        </button>
      </div>
      {models !== null &&
        (models.length === 0 ? (
          <p className="muted">No text model is available with this key.</p>
        ) : (
          <div className="model-pick">
            <label>
              Model
              <select value={choice} onChange={(e) => setChoice(e.currentTarget.value)}>
                {models.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.display_name ? `${m.display_name} (${m.id})` : m.id}
                  </option>
                ))}
              </select>
            </label>
            <div className="actions">
              <button
                type="button"
                disabled={busy || choice === ""}
                onClick={() => act(() => api.selectModel(connection.connection_id, choice))}
              >
                Use this model
              </button>
            </div>
          </div>
        ))}

      <form
        className="note-form"
        onSubmit={(e) => {
          e.preventDefault();
          void act(async () => {
            await api.saveGeminiConnection(
              connection.connection_id,
              connection.display_name,
              connection.free_tier,
              newKey,
            );
            setNewKey("");
          });
        }}
      >
        <label>
          New API key
          <input
            type="password"
            value={newKey}
            autoComplete="off"
            spellCheck={false}
            maxLength={512}
            onChange={(e) => setNewKey(e.currentTarget.value)}
          />
        </label>
        <div className="actions">
          <button type="submit" className="secondary" disabled={busy || newKey.trim() === ""}>
            Replace key
          </button>
          {!confirmRemove ? (
            <button type="button" className="secondary" disabled={busy} onClick={() => setConfirmRemove(true)}>
              Remove connection
            </button>
          ) : (
            <>
              <button
                type="button"
                className="danger"
                disabled={busy}
                onClick={() => act(() => api.removeConnection(connection.connection_id))}
              >
                Confirm removal
              </button>
              <button type="button" className="secondary" disabled={busy} onClick={() => setConfirmRemove(false)}>
                Cancel
              </button>
            </>
          )}
        </div>
        {confirmRemove && (
          <p className="muted">This removes the connection and its key from the vault. Conversations are kept.</p>
        )}
      </form>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

export function PersonaCard() {
  const [persona, setPersona] = useState<PersonaView | null>(null);
  const [name, setName] = useState("");
  const [tone, setTone] = useState("");
  const [style, setStyle] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  function show(p: PersonaView) {
    setPersona(p);
    setName(p.name);
    setTone(p.tone);
    setStyle(p.style);
  }

  useEffect(() => {
    api
      .getPersona()
      .then(show)
      .catch((err) => setError(describeError(err)));
  }, []);

  async function save(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    setSaved(false);
    try {
      show(await api.savePersona(name, tone, style));
      setSaved(true);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  const changed = persona !== null && (name !== persona.name || tone !== persona.tone || style !== persona.style);

  return (
    <section className="card" aria-labelledby="persona-heading">
      <h2 id="persona-heading">
        <SparkleIcon /> Persona
      </h2>
      <p className="muted">
        Persona changes only how the assistant speaks, not what it's allowed to do. Each save
        creates a new version; older messages keep the version they were written with.
      </p>
      {persona && (
        <form className="note-form" onSubmit={save}>
          <label>
            Assistant name
            <input value={name} maxLength={60} onChange={(e) => setName(e.currentTarget.value)} required />
          </label>
          <label>
            Tone
            <textarea value={tone} rows={2} maxLength={500} onChange={(e) => setTone(e.currentTarget.value)} />
          </label>
          <label>
            Style
            <textarea value={style} rows={3} maxLength={1000} onChange={(e) => setStyle(e.currentTarget.value)} />
          </label>
          <div className="actions">
            <button type="submit" disabled={busy || !changed || name.trim() === ""}>
              {busy ? "Saving…" : "Save persona"}
            </button>
            <span className="muted">{`Version ${persona.version}`}</span>
          </div>
          {saved && (
            <p className="muted" role="status">
              {`Saved as version ${persona.version}.`}
            </p>
          )}
        </form>
      )}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}

export function BackupCard() {
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<BackupSummary | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function backup() {
    setBusy(true);
    setError(null);
    try {
      setResult(await api.createBackup());
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="card" aria-labelledby="backup-heading">
      <h2 id="backup-heading">Data</h2>
      <p className="muted">
        Backup copies the database and the verified notes, with a manifest of hashes. It can only
        be restored into a new profile, where nothing runs automatically.
      </p>
      <div className="actions">
        <button type="button" onClick={backup} disabled={busy}>
          {busy ? "Backing up…" : "Back up now"}
        </button>
      </div>
      {result && (
        <div role="status">
          <p>{`Backup created: ${result.artifacts} ${result.artifacts === 1 ? "note" : "notes"}.`}</p>
          <p className="path">{result.path}</p>
          {result.unmatched_artifacts > 0 && (
            <p className="error">
              {result.unmatched_artifacts} note(s) had been modified outside Nexus and were copied
              as they were.
            </p>
          )}
        </div>
      )}
      {error && (
        <p className="error" role="alert">
          Backup failed: {error}
        </p>
      )}
    </section>
  );
}
