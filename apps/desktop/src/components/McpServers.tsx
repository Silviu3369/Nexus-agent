// Remote MCP servers: add one, see what its tools claim, enable tools one by
// one. A tool the server declares read-only runs when the model calls it;
// any other asks you before each call. Everything a server wrote (names,
// descriptions, hints, schemas) is shown as the server's claim. Enabling
// names the exact definition shown, so the Host refuses one the server
// changed meanwhile.

import { useState } from "react";
import type { McpServerView, McpToolView } from "@nexus/contracts";
import { api, describeError } from "../api";
import { ago } from "../model/format.ts";
import { useNexus, useNexusActions, useNow } from "../state/store";
import { Empty, ErrorLine, Facts, Panel } from "../ui";
import { AlertIcon, EyeIcon, PlugIcon, PlusIcon } from "../icons";

const MAX_NAME = 100;

export function McpServers() {
  const d = useNexus();
  return (
    <>
      <AddServer />
      {d.mcp === null ? (
        <p className="muted">Loading…</p>
      ) : d.mcp.length === 0 ? (
        <Panel title="Servers" icon={<PlugIcon size={15} />} id="mcp-servers">
          <Empty>No MCP servers yet. Add one above; none of its tools is offered to the model until you enable it.</Empty>
        </Panel>
      ) : (
        d.mcp.map((s) => <ServerCard key={s.server_id} server={s} />)
      )}
    </>
  );
}

function AddServer() {
  const { refresh } = useNexusActions();
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ok = name.trim().length > 0 && url.trim().length > 0;

  async function add() {
    setBusy(true);
    setError(null);
    try {
      await api.saveMcpServer(null, name.trim(), url.trim(), key.trim() || null, false);
      setName("");
      setUrl("");
      setKey("");
      await refresh(["mcp"]);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Panel title="Add an MCP server" icon={<PlusIcon size={15} />} id="mcp-add">
      <form
        className="stack"
        aria-label="Add an MCP server"
        onSubmit={(e) => {
          e.preventDefault();
          void add();
        }}
      >
        <label>
          Name
          <input value={name} maxLength={MAX_NAME} placeholder="e.g. Microsoft Learn" onChange={(e) => setName(e.currentTarget.value)} />
        </label>
        <label>
          Address
          <input
            value={url}
            maxLength={2000}
            spellCheck={false}
            placeholder="https://learn.microsoft.com/api/mcp"
            onChange={(e) => setUrl(e.currentTarget.value)}
          />
        </label>
        <label>
          Key (optional)
          <input
            type="password"
            value={key}
            autoComplete="off"
            spellCheck={false}
            maxLength={4096}
            placeholder="Only if the server asks for one"
            onChange={(e) => setKey(e.currentTarget.value)}
          />
        </label>
        <p className="muted">
          Remote servers over https only. A key is kept in the Windows vault and sent only to this address. Nexus asks the
          server for its tools now; nothing is offered to the model until you enable a tool.
        </p>
        <div className="actions">
          <button type="submit" className="primary" disabled={!ok || busy}>
            {busy ? "Connecting…" : "Add server"}
          </button>
        </div>
        <ErrorLine error={error} />
      </form>
    </Panel>
  );
}

function ServerCard({ server }: { server: McpServerView }) {
  const { refresh } = useNexusActions();
  const now = useNow(10_000);
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const enabled = server.tools.filter((t) => t.enabled).length;

  async function act(work: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await work();
      await refresh(["mcp"]);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Panel
      title={server.name}
      icon={<PlugIcon size={15} />}
      id={`mcp-${server.server_id}`}
      className="mcp-server"
      actions={
        <>
          <button type="button" className="ghost" disabled={busy} onClick={() => void act(() => api.refreshMcpTools(server.server_id))}>
            {busy ? "Asking…" : "Refresh tools"}
          </button>
          <button type="button" className="ghost" disabled={busy} onClick={() => setConfirm(true)}>
            Remove
          </button>
        </>
      }
    >
      {confirm && (
        <div className="confirm-bar" role="alert">
          <p>{`Remove “${server.name}” with its tools${server.has_key ? " and its key" : ""}? Past steps that used it stay in their tasks.`}</p>
          <div className="actions">
            <button type="button" className="danger" disabled={busy} onClick={() => void act(() => api.removeMcpServer(server.server_id))}>
              Remove for good
            </button>
            <button type="button" className="ghost" disabled={busy} onClick={() => setConfirm(false)}>
              Keep
            </button>
          </div>
        </div>
      )}
      <Facts
        rows={[
          ["Address", <span className="mono">{server.url}</span>],
          ["Key", server.has_key ? "in the Windows vault, sent only to this address" : "none"],
          ["Tools", `${server.tools.length} listed · ${enabled} enabled`],
          ["Last asked", server.checked_at_ms ? ago(server.checked_at_ms, now) : "never"],
        ]}
      />
      {server.last_error && (
        <p className="error" role="alert">
          Could not list its tools: {server.last_error}
        </p>
      )}
      {server.tools.length > 0 && (
        <ul className="mcp-tools" aria-label={`Tools of ${server.name}`}>
          {server.tools.map((t) => (
            <ToolRow key={t.name} server={server} tool={t} />
          ))}
        </ul>
      )}
      <ErrorLine error={error} />
    </Panel>
  );
}

function ToolRow({ server, tool }: { server: McpServerView; tool: McpToolView }) {
  const { refresh } = useNexusActions();
  const [inspect, setInspect] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const canEnable = tool.available && tool.blocked_reason === null;

  async function toggle(on: boolean) {
    setBusy(true);
    setError(null);
    try {
      await api.setMcpToolEnabled(server.server_id, tool.name, on ? tool.definition_sha256 : null);
      await refresh(["mcp"]);
    } catch (err) {
      setError(describeError(err));
      // A refused enable usually means the definition changed: show the new one.
      await refresh(["mcp"]);
    } finally {
      setBusy(false);
    }
  }

  return (
    <li className={`mcp-tool${tool.enabled ? " on" : ""}${tool.available ? "" : " dim"}`} data-tool={tool.name}>
      <div className="mcp-tool-head">
        <span className="mono mcp-tool-name">{tool.name}</span>
        {tool.title && <span className="mcp-tool-title">{tool.title}</span>}
        {tool.read_only_hint === true ? (
          <span className="badge badge-neutral" title="The server's own claim; Nexus cannot verify it">
            read-only (server's claim)
          </span>
        ) : (
          <span className="badge badge-attention" title="Each call waits for your approval of the exact call">
            asks you before each call
          </span>
        )}
        {tool.destructive_hint === true && <span className="badge badge-bad">destructive</span>}
        {!tool.available && <span className="badge badge-neutral">no longer listed</span>}
        <span className="spacer" />
        <label className="check">
          <input
            type="checkbox"
            checked={tool.enabled}
            disabled={busy || (!tool.enabled && !canEnable)}
            aria-label={`Enable ${tool.name}`}
            onChange={(e) => void toggle(e.currentTarget.checked)}
          />
          {tool.enabled ? "Enabled" : "Off"}
        </label>
      </div>
      {tool.changed && (
        <p className="warning mcp-changed">
          <AlertIcon size={14} /> The server changed this tool since you enabled it, so it is off. Review it below, then enable
          it again if you still trust it.
        </p>
      )}
      <p className="mcp-tool-description">
        <span className="muted">The server says: </span>
        {tool.description || <em>no description</em>}
      </p>
      <p className="muted mcp-tool-params">
        Parameters: {tool.parameters.length > 0 ? <span className="mono">{tool.parameters.join(", ")}</span> : "none"}
        <button type="button" className="ghost mcp-inspect" aria-expanded={inspect} onClick={() => setInspect(!inspect)}>
          <EyeIcon size={14} /> {inspect ? "Hide schema" : "Inspect schema"}
        </button>
      </p>
      {inspect && <pre className="mcp-schema">{tool.schema_json}</pre>}
      {tool.read_only_hint !== true && tool.enabled && (
        <p className="muted mcp-blocked">
          It can change things on the server: each call waits for your approval of the exact call, and is made once.
        </p>
      )}
      {tool.blocked_reason && <p className="muted mcp-blocked">{tool.blocked_reason}</p>}
      <ErrorLine error={error} />
    </li>
  );
}
