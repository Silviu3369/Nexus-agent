// Shared building blocks for the command-center views.

import type { ReactNode } from "react";
import { STATUS_LABEL } from "./model/execution.ts";
import type { ExecStatus } from "./model/execution.ts";

export function StatusPill({ status, label }: { status: ExecStatus | null; label?: string }) {
  if (status === null) return <span className="pill pill-unknown">{label ?? "—"}</span>;
  return (
    <span className={`pill pill-${status}`}>
      <span className="pill-dot" aria-hidden="true" />
      {label ?? STATUS_LABEL[status]}
    </span>
  );
}

export function Panel({
  title,
  icon,
  actions,
  children,
  className,
  id,
}: {
  title: ReactNode;
  icon?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
  id?: string;
}) {
  const headingId = id ? `${id}-heading` : undefined;
  return (
    <section className={`panel${className ? ` ${className}` : ""}`} aria-labelledby={headingId}>
      <header className="panel-head">
        <h2 id={headingId}>
          {icon}
          {title}
        </h2>
        {actions && <div className="panel-actions">{actions}</div>}
      </header>
      <div className="panel-body">{children}</div>
    </section>
  );
}

/** A capability this build does not have. Says so plainly, with when it is
 *  planned; never offers a control that would do nothing. */
export function Unavailable({ title, reason, planned }: { title?: string; reason: string; planned?: string }) {
  return (
    <div className="unavailable">
      {title && <p className="unavailable-title">{title}</p>}
      <p>{reason}</p>
      {planned && <p className="unavailable-plan">{planned}</p>}
    </div>
  );
}

export function Empty({ children }: { children: ReactNode }) {
  return <p className="empty">{children}</p>;
}

export function Facts({ rows }: { rows: [string, ReactNode][] }) {
  return (
    <dl className="kv">
      {rows.map(([k, v]) => (
        <div className="kv-row" key={k}>
          <dt>{k}</dt>
          <dd>{v}</dd>
        </div>
      ))}
    </dl>
  );
}

export function ErrorLine({ error }: { error: string | null }) {
  if (!error) return null;
  return (
    <p className="error" role="alert">
      {error}
    </p>
  );
}

function hash(text: string): number {
  let h = 2166136261;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

/** An abstract, deterministic identity: a hexagon with a pattern of lit
 *  cells derived from the agent's id. No faces, no avatars. */
export function AgentGlyph({ id, size = 36, active = false }: { id: string; size?: number; active?: boolean }) {
  const h = hash(id);
  const cells: [number, number][] = [
    [0, -10],
    [8.7, -5],
    [8.7, 5],
    [0, 10],
    [-8.7, 5],
    [-8.7, -5],
    [0, 0],
  ];
  return (
    <svg className={`glyph${active ? " glyph-active" : ""}`} width={size} height={size} viewBox="-20 -20 40 40" aria-hidden="true">
      <path d="M0 -18 15.6 -9 15.6 9 0 18 -15.6 9 -15.6 -9z" className="glyph-shell" />
      {cells.map(([x, y], i) => (
        <circle key={i} cx={x} cy={y} r={i === 6 ? 3.2 : 2.4} className={(h >> i) & 1 || i === 6 ? "glyph-on" : "glyph-off"} />
      ))}
      {cells.slice(0, 6).map(([x, y], i) =>
        (h >> (i + 8)) & 1 ? <line key={`l${i}`} x1="0" y1="0" x2={x} y2={y} className="glyph-link" /> : null,
      )}
    </svg>
  );
}
