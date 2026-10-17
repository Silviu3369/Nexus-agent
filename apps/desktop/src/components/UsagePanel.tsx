// Usage today: the tokens each model was charged for (as the providers
// reported them), the daily budget the Host enforces, and a cost estimate
// from prices you enter — shown as an estimate, never as a bill.

import { useState } from "react";
import type { ModelUsageView, UsageView } from "@nexus/contracts";
import { api, describeError } from "../api";
import { useNexus, useNexusActions } from "../state/store";
import { Empty, ErrorLine, Facts, Panel } from "../ui";
import { PulseIcon } from "../icons";

const tokens = (n: number) => n.toLocaleString("en-US");

export function usd(n: number): string {
  if (n === 0) return "$0";
  return n < 0.01 ? `$${n.toFixed(6)}` : `$${n.toFixed(2)}`;
}

export function UsagePanel() {
  const d = useNexus();
  const usage = d.usage;
  return (
    <Panel title="Usage today" icon={<PulseIcon size={15} />} id="usage">
      {usage === null ? (
        <p className="muted">Loading…</p>
      ) : (
        <>
          <Facts
            rows={[
              [
                "Tokens today",
                usage.daily_token_budget
                  ? `${tokens(usage.tokens_today)} of ${tokens(usage.daily_token_budget)} (${Math.min(
                      100,
                      Math.round((usage.tokens_today / usage.daily_token_budget) * 100),
                    )}%)`
                  : `${tokens(usage.tokens_today)} · no daily limit`,
              ],
              ["Estimated cost today", costLine(usage)],
              ["Day started", new Date(usage.day_start_ms).toLocaleString("en-US")],
            ]}
          />
          <BudgetForm usage={usage} />
          {usage.models.length === 0 ? (
            <Empty>No model is chosen or used yet.</Empty>
          ) : (
            <table className="table usage-table">
              <thead>
                <tr>
                  <th>Model</th>
                  <th>Calls</th>
                  <th>Tokens sent</th>
                  <th>Tokens received</th>
                  <th>Price, $ per 1M tokens (in / out)</th>
                  <th>Estimated today</th>
                </tr>
              </thead>
              <tbody>
                {usage.models.map((m) => (
                  <ModelRow key={`${m.provider}/${m.model}`} model={m} />
                ))}
              </tbody>
            </table>
          )}
          <p className="muted small">
            Tokens are what each provider reported for each call. A cost is only an estimate from the prices you enter here;
            check them on the provider's pricing page — your bill is the provider's own count. A free tier is not charged.
          </p>
        </>
      )}
    </Panel>
  );
}

/** The day's estimate, saying what it rests on. */
function costLine(usage: UsageView): string {
  if (usage.estimated_cost_today === null) return "Enter a model's price to estimate it";
  const priced = usage.models.some((m) => !m.free_tier && m.price !== null);
  if (!priced) return "$0 · free tier, not charged";
  return `${usd(usage.estimated_cost_today)} · from the prices below`;
}

function BudgetForm({ usage }: { usage: UsageView }) {
  const { refresh } = useNexusActions();
  const [value, setValue] = useState(usage.daily_token_budget ? String(usage.daily_token_budget) : "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const parsed = Number(value.replace(/[\s,]/g, ""));
  const ok = value.trim() !== "" && Number.isInteger(parsed) && parsed > 0;

  async function save(tokens: number | null) {
    setBusy(true);
    setError(null);
    try {
      await api.setDailyTokenBudget(tokens);
      if (tokens === null) setValue("");
      await refresh(["usage"]);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form
      className="usage-budget"
      onSubmit={(e) => {
        e.preventDefault();
        if (ok) void save(parsed);
      }}
    >
      <label>
        Daily token budget
        <input
          inputMode="numeric"
          value={value}
          placeholder="No limit"
          onChange={(e) => setValue(e.currentTarget.value)}
        />
      </label>
      <button type="submit" className="secondary" disabled={!ok || busy}>
        Set budget
      </button>
      <button type="button" className="ghost" disabled={busy || usage.daily_token_budget === null} onClick={() => void save(null)}>
        Remove limit
      </button>
      <span className="muted small">Once today's tokens reach it, Nexus sends nothing more until tomorrow (this PC's time).</span>
      <ErrorLine error={error} />
    </form>
  );
}

function ModelRow({ model }: { model: ModelUsageView }) {
  const { refresh } = useNexusActions();
  const [input, setInput] = useState(model.price ? String(model.price.input_per_million) : "");
  const [output, setOutput] = useState(model.price ? String(model.price.output_per_million) : "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inNum = Number(input);
  const outNum = Number(output);
  const ok = input.trim() !== "" && output.trim() !== "" && Number.isFinite(inNum) && Number.isFinite(outNum) && inNum >= 0 && outNum >= 0;

  async function save(price: { input_per_million: number; output_per_million: number } | null) {
    setBusy(true);
    setError(null);
    try {
      await api.setModelPrice(model.provider, model.model, price);
      if (price === null) {
        setInput("");
        setOutput("");
      }
      await refresh(["usage"]);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <tr>
      <td>
        <span className="mono">{model.model}</span>
        <div className="muted small">
          {model.provider || "provider not recorded"}
          {model.free_tier && <span className="badge badge-attention"> Free tier</span>}
        </div>
      </td>
      <td>{model.calls_today}</td>
      <td>{tokens(model.prompt_tokens_today)}</td>
      <td>{tokens(model.output_tokens_today)}</td>
      <td>
        <form
          className="usage-price"
          aria-label={`Price of ${model.model}`}
          onSubmit={(e) => {
            e.preventDefault();
            if (ok) void save({ input_per_million: inNum, output_per_million: outNum });
          }}
        >
          <input
            inputMode="decimal"
            value={input}
            placeholder="in"
            aria-label={`Input price of ${model.model}`}
            disabled={!model.provider}
            onChange={(e) => setInput(e.currentTarget.value)}
          />
          <input
            inputMode="decimal"
            value={output}
            placeholder="out"
            aria-label={`Output price of ${model.model}`}
            disabled={!model.provider}
            onChange={(e) => setOutput(e.currentTarget.value)}
          />
          <button
            type="submit"
            className="ghost"
            aria-label={`Save price of ${model.model}`}
            disabled={!ok || busy || !model.provider}
          >
            Save
          </button>
          {model.price && (
            <button type="button" className="ghost" disabled={busy} onClick={() => void save(null)}>
              Clear
            </button>
          )}
        </form>
        <ErrorLine error={error} />
      </td>
      <td>
        {model.estimated_cost_today !== null
          ? model.free_tier
            ? "$0 (free tier)"
            : usd(model.estimated_cost_today)
          : "—"}
      </td>
    </tr>
  );
}
