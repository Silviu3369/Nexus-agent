import { useCallback, useEffect, useState } from "react";
import type {
  BackupModelView,
  BackupSummary,
  ConnectionView,
  ModelView,
  PersonaView,
  ProviderChoice,
  ThinkingLevel,
} from "@nexus/contracts";
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
      <NewConnection onSaved={() => void load()} />
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

const PROVIDER_NAME: Record<ProviderChoice, string> = {
  Gemini: "Google Gemini",
  OpenAi: "OpenAI",
  Anthropic: "Anthropic (Claude)",
  DeepSeek: "DeepSeek",
};

/** A preview of what the Host will read from the key's format, so the user
 *  sees it before saving. The Host decides; this only mirrors its rules
 *  (`crates/nexus-providers/src/kind.rs`). */
export function guessProvider(key: string): ProviderChoice | "ambiguous" | null {
  const k = key.trim();
  if (k === "") return null;
  if (k.startsWith("AIza")) return "Gemini";
  if (k.startsWith("sk-ant-")) return "Anthropic";
  if (["sk-proj-", "sk-svcacct-", "sk-admin-"].some((p) => k.startsWith(p)) || (k.startsWith("sk-") && k.includes("T3BlbkFJ")))
    return "OpenAi";
  if (k.startsWith("sk-")) return /^[0-9a-fA-F]{32}$/.test(k.slice(3)) ? "DeepSeek" : "ambiguous";
  return "ambiguous";
}

function NewConnection({ onSaved }: { onSaved: () => void }) {
  const [key, setKey] = useState("");
  const [choice, setChoice] = useState<ProviderChoice | "auto">("auto");
  const [name, setName] = useState("");
  const [freeTier, setFreeTier] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const guess = guessProvider(key);
  const provider: ProviderChoice | null = choice !== "auto" ? choice : guess === "ambiguous" ? null : guess;
  const needsChoice = choice === "auto" && guess === "ambiguous";

  async function save(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api.saveConnection(null, name.trim() || null, choice === "auto" ? null : choice, provider === "Gemini" && freeTier, key);
      setKey("");
      setName("");
      setChoice("auto");
      onSaved();
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="note-form" onSubmit={save} aria-label="New AI connection">
      <h3>Add an API key</h3>
      <label>
        API key
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
      {key.trim() !== "" && (
        <p className="muted small" role="status">
          {choice !== "auto"
            ? `Provider chosen: ${PROVIDER_NAME[choice]}`
            : guess === "ambiguous"
              ? "The key's format does not say which provider it belongs to — choose it below."
              : guess
                ? `Detected: ${PROVIDER_NAME[guess]}`
                : null}
        </p>
      )}
      <label>
        Provider
        <select value={choice} onChange={(e) => setChoice(e.currentTarget.value as ProviderChoice | "auto")}>
          <option value="auto">Detect from the key</option>
          {(Object.keys(PROVIDER_NAME) as ProviderChoice[]).map((p) => (
            <option key={p} value={p}>
              {PROVIDER_NAME[p]}
            </option>
          ))}
        </select>
      </label>
      <label>
        Connection name (optional)
        <input value={name} maxLength={100} placeholder={provider ? PROVIDER_NAME[provider] : ""} onChange={(e) => setName(e.currentTarget.value)} />
      </label>
      {provider === "Gemini" && (
        <>
          <label className="check">
            <input type="checkbox" checked={freeTier} onChange={(e) => setFreeTier(e.currentTarget.checked)} />
            I'm using the free tier
          </label>
          {freeTier && <FreeTierNotice />}
        </>
      )}
      <div className="actions">
        <button type="submit" disabled={busy || key.trim() === "" || needsChoice}>
          {busy ? "Verifying key…" : "Verify and save"}
        </button>
      </div>
      <p className="muted">
        The key is checked with one real request to its own provider (the model list) and is sent
        nowhere else; a rejected key is not saved. You then choose the model from what the key can
        use.
      </p>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </form>
  );
}

function providerName(stored: string): string {
  return stored === "OpenAI" ? "OpenAI" : (PROVIDER_NAME[stored as ProviderChoice] ?? stored);
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
        <dd>{providerName(connection.provider)}</dd>
        <dt>Model</dt>
        <dd>{connection.model ?? "not chosen"}</dd>
        <dt>Key</dt>
        <dd>{connection.has_key ? "in the Windows vault" : "missing — enter a new one"}</dd>
      </dl>

      <div className="model-pick">
        <label>
          Reasoning effort
          <select
            value={connection.thinking_level}
            disabled={busy}
            onChange={(e) =>
              act(() =>
                api.setThinkingLevel(connection.connection_id, e.currentTarget.value as ThinkingLevel),
              )
            }
          >
            <option value="Low">Low — fastest replies</option>
            <option value="Medium">Medium — balanced</option>
            <option value="High">High — reasons more, slower</option>
          </select>
        </label>
        <p className="muted">
          Sent as the provider's own reasoning control (Gemini 3.x thinking level, OpenAI reasoning
          effort, Claude effort). A model that does not support it is asked again without it.
        </p>
      </div>

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
                    {m.display_name && m.display_name !== m.id ? `${m.display_name} (${m.id})` : m.id}
                  </option>
                ))}
              </select>
            </label>
            <p className="muted small">The list comes from the provider for this key; nothing is preselected for you.</p>
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
            await api.saveConnection(connection.connection_id, null, null, connection.free_tier, newKey);
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

/** The model used only when the active one stays unavailable after its
 *  retries. Chosen explicitly; there is no other fallback. */
export function BackupModelCard({
  connections,
  onChange,
}: {
  connections: ConnectionView[];
  /** Told the current backup whenever it is read or changed. */
  onChange?: (backup: BackupModelView | null) => void;
}) {
  const [backup, setBackup] = useState<BackupModelView | null | undefined>(undefined);
  const [connectionId, setConnectionId] = useState<string>("");
  const [models, setModels] = useState<ModelView[] | null>(null);
  const [model, setModel] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .getBackupModel()
      .then((b) => {
        setBackup(b);
        onChange?.(b);
      })
      .catch((err) => setError(describeError(err)));
  }, [connections, onChange]);

  const usable = connections.filter((c) => c.has_key);
  const active = connections.find((c) => c.active) ?? null;
  const same = backup && active && backup.connection_id === active.connection_id && backup.model === active.model;

  async function act(action: () => Promise<BackupModelView | null>) {
    setBusy(true);
    setError(null);
    try {
      const b = await action();
      setBackup(b);
      onChange?.(b);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="card" aria-labelledby="backup-model-heading">
      <h2 id="backup-model-heading">
        <SparkleIcon /> Backup model
      </h2>
      <p className="muted">
        Used only when the active model is still unavailable after up to three tries (for example
        Google's “high demand” 503). Every switch shows up as a step. Without one, the task stops
        and says why.
      </p>
      {backup === undefined ? (
        <p className="muted">Loading…</p>
      ) : backup === null ? (
        <p>No backup model.</p>
      ) : (
        <p>
          Backup: <span className="mono">{backup.model}</span> on {backup.display_name} ({providerName(backup.provider)})
        </p>
      )}
      {same && <p className="warning">The backup is the same model as the active one, so it cannot help when that model is overloaded.</p>}
      {usable.length === 0 ? (
        <p className="muted">Add a connection first.</p>
      ) : (
        <div className="model-pick">
          <label>
            Connection
            <select
              value={connectionId}
              onChange={(e) => {
                setConnectionId(e.currentTarget.value);
                setModels(null);
                setModel("");
              }}
            >
              <option value="">Choose…</option>
              {usable.map((c) => (
                <option key={c.connection_id} value={c.connection_id}>
                  {c.display_name} ({providerName(c.provider)})
                </option>
              ))}
            </select>
          </label>
          <div className="actions">
            <button
              type="button"
              className="secondary"
              disabled={busy || connectionId === ""}
              onClick={async () => {
                setBusy(true);
                setError(null);
                try {
                  const list = await api.listModels(connectionId);
                  setModels(list);
                  setModel(list[0]?.id ?? "");
                } catch (err) {
                  setError(describeError(err));
                } finally {
                  setBusy(false);
                }
              }}
            >
              Load backup models
            </button>
            {backup && (
              <button type="button" className="secondary" disabled={busy} onClick={() => act(() => api.setBackupModel(null, null))}>
                Clear backup
              </button>
            )}
          </div>
          {models !== null && models.length > 0 && (
            <>
              <label>
                Backup model
                <select value={model} onChange={(e) => setModel(e.currentTarget.value)}>
                  {models.map((m) => (
                    <option key={m.id} value={m.id}>
                      {m.id}
                    </option>
                  ))}
                </select>
              </label>
              <div className="actions">
                <button type="button" disabled={busy || model === ""} onClick={() => act(() => api.setBackupModel(connectionId, model))}>
                  Use as backup
                </button>
              </div>
            </>
          )}
        </div>
      )}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </section>
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
