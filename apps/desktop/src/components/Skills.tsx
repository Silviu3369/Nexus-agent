// The user's skills (ADR-PACK-001): instructions in the open Agent Skills
// format. A skill comes in off; the user reads its whole text and turns it
// on. It grants nothing: its scripts never run and its allowed-tools is
// ignored. New text turns it off until the user reads it again. The packs
// built into Nexus are shown too, read-only: their skills and the rules of
// the tasks their intake starts, which only take away from the user's.

import { useCallback, useEffect, useState } from "react";
import type { PackSkillView, PackView, SkillContentView, SkillView } from "@nexus/contracts";
import { api, describeError } from "../api";
import { ago } from "../model/format.ts";
import { useNow } from "../state/store";
import { Empty, ErrorLine, Facts, Panel } from "../ui";
import { PlusIcon, ToolIcon } from "../icons";

export function Skills() {
  const [skills, setSkills] = useState<SkillView[] | null>(null);
  const [packs, setPacks] = useState<PackView[]>([]);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(async () => {
    try {
      const [mine, built] = await Promise.all([api.listSkills(), api.listPacks()]);
      setSkills(mine);
      setPacks(built);
      setError(null);
    } catch (err) {
      setError(describeError(err));
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  return (
    <>
      <ImportSkill onDone={load} />
      <ErrorLine error={error} />
      {skills === null ? (
        <p className="muted">Loading…</p>
      ) : skills.length === 0 ? (
        <Panel title="Your skills" icon={<ToolIcon size={15} />} id="skills">
          <Empty>No skill yet. Import one above, for example the folder of a skill you use in Claude.</Empty>
        </Panel>
      ) : (
        skills.map((s) => <SkillCard key={s.skill_id} skill={s} onChange={load} />)
      )}
      {packs.map((p) => (
        <PackCard key={p.pack_id} pack={p} />
      ))}
    </>
  );
}

function PackCard({ pack }: { pack: PackView }) {
  const intake = pack.intake;
  const writes = intake?.record_writes_without_asking;
  return (
    <Panel title={`${pack.name} pack`} icon={<ToolIcon size={15} />} id={`pack-${pack.pack_id}`}>
      <p>{pack.description}</p>
      <Facts
        rows={(
          [
            ["Version", String(pack.version)],
            ["Registers", pack.registers.join(", ")],
            ["Tasks it starts", intake ? `${intake.label}: one task per file, following ${intake.skill}` : null],
            ["Turned off in them", intake && intake.turned_off.length > 0 ? intake.turned_off.join("; ") : null],
            [
              "Register changes without asking",
              writes !== null && writes !== undefined
                ? `at most ${writes} per task (fewer if you set fewer in Permissions); the rest wait for you`
                : null,
            ],
          ] as [string, string | null][]
        ).filter((row): row is [string, string] => row[1] !== null)}
      />
      <p className="muted">
        Part of Nexus: it cannot be changed. Its rules only take away from yours; what you write in one of its tasks
        runs under your rules alone.
      </p>
      {pack.skills.map((s) => (
        <PackSkill key={s.name} skill={s} />
      ))}
    </Panel>
  );
}

function PackSkill({ skill }: { skill: PackSkillView }) {
  const [open, setOpen] = useState(false);
  return (
    <section className="stack">
      <h3 className="mono">{skill.name}</h3>
      <p>{skill.description}</p>
      <div className="actions">
        <button type="button" className="ghost" aria-expanded={open} onClick={() => setOpen(!open)}>
          {open ? "Hide text" : "Read skill"}
        </button>
      </div>
      {open && (
        <div className="skill-text" aria-label={`Text of ${skill.name}`}>
          <pre className="skill-file">{skill.text}</pre>
        </div>
      )}
    </section>
  );
}

function ImportSkill({ onDone }: { onDone: () => Promise<void> }) {
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function add() {
    setBusy(true);
    setError(null);
    try {
      await api.importSkill(path.trim());
      setPath("");
      await onDone();
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Panel title="Import a skill" icon={<PlusIcon size={15} />} id="skill-import">
      <form
        className="stack"
        aria-label="Import a skill"
        onSubmit={(e) => {
          e.preventDefault();
          if (path.trim() !== "") void add();
        }}
      >
        <label>
          Skill folder or .zip
          <input
            value={path}
            spellCheck={false}
            maxLength={1024}
            placeholder="C:\…\my-skill  ·  or …\my-skill.zip"
            onChange={(e) => setPath(e.currentTarget.value)}
          />
        </label>
        <p className="muted">
          The folder that holds SKILL.md, or a .zip of it. A skill comes in off: read it, then turn it on. Its scripts
          never run, and nothing it declares gives it permissions.
        </p>
        <div className="actions">
          <button type="submit" className="primary" disabled={path.trim() === "" || busy}>
            Import skill
          </button>
        </div>
        <ErrorLine error={error} />
      </form>
    </Panel>
  );
}

function SkillCard({ skill, onChange }: { skill: SkillView; onChange: () => Promise<void> }) {
  const now = useNow(60_000);
  const [content, setContent] = useState<SkillContentView | null>(null);
  const [open, setOpen] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Turning on is for the text the user has read: this revision.
  const read = content !== null && content.skill.revision === skill.revision;

  async function act(work: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await work();
      await onChange();
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  async function toggleText() {
    if (open) {
      setOpen(false);
      return;
    }
    setError(null);
    try {
      setContent(await api.getSkill(skill.skill_id));
      setOpen(true);
    } catch (err) {
      setError(describeError(err));
    }
  }

  const state = skill.enabled
    ? "On"
    : skill.changed_since_enabled
      ? "Off: its text changed since you turned it on; read it again"
      : "Off";
  return (
    <Panel
      title={skill.name}
      icon={<ToolIcon size={15} />}
      id={`skill-${skill.name}`}
      actions={
        <>
          <button type="button" className="ghost" aria-expanded={open} onClick={() => void toggleText()}>
            {open ? "Hide text" : "Read skill"}
          </button>
          {skill.enabled ? (
            <button
              type="button"
              className="ghost"
              disabled={busy}
              onClick={() =>
                void act(() => api.setSkillEnabled({ skill_id: skill.skill_id, revision: skill.revision, enabled: false }))
              }
            >
              Turn off
            </button>
          ) : (
            <button
              type="button"
              className="primary"
              disabled={busy || !read}
              title={read ? undefined : "Read the skill first"}
              onClick={() =>
                void act(() => api.setSkillEnabled({ skill_id: skill.skill_id, revision: skill.revision, enabled: true }))
              }
            >
              Turn on
            </button>
          )}
          <button type="button" className="ghost" disabled={busy} onClick={() => setConfirm(true)}>
            Remove
          </button>
        </>
      }
    >
      {confirm && (
        <div className="confirm-bar" role="alert">
          <p>{`Remove the skill “${skill.name}”? Its text is deleted from Nexus; your own copy is not touched.`}</p>
          <div className="actions">
            <button type="button" className="danger" disabled={busy} onClick={() => void act(() => api.removeSkill(skill.skill_id))}>
              Remove now
            </button>
            <button type="button" className="ghost" disabled={busy} onClick={() => setConfirm(false)}>
              Keep
            </button>
          </div>
        </div>
      )}
      <p>{skill.description}</p>
      <Facts
        rows={(
          [
            ["State", state],
          ["Files", skill.files.map((f) => `${f.path} (${f.chars.toLocaleString("en-US")} characters)`).join(", ")],
          ["Scripts", skill.has_scripts ? "present, never run in Nexus" : null],
          ["allowed-tools", skill.allowed_tools ? `${skill.allowed_tools} (ignored: a skill approves nothing)` : null],
          ["Not imported", skill.skipped.length > 0 ? skill.skipped.join("; ") : null],
          ["License", skill.license],
          ["Compatibility", skill.compatibility],
            ["Imported", ago(skill.updated_at_ms, now)],
          ] as [string, string | null][]
        ).filter((row): row is [string, string] => row[1] !== null)}
      />
      {!skill.enabled && !read && <p className="muted">Read the skill before you turn it on.</p>}
      {open && content && (
        <div className="skill-text" aria-label={`Text of ${skill.name}`}>
          {content.files.map((f) => (
            <section key={f.path}>
              <h3 className="mono">{f.path}</h3>
              <pre className="skill-file">{f.text}</pre>
            </section>
          ))}
        </div>
      )}
      <ErrorLine error={error} />
    </Panel>
  );
}
