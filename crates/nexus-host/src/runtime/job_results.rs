//! Jobs results (`PLAN_NEXUS_RO.md` §3a, the niche chosen on 2026-10-19):
//! what the applications led to, counted from them and their history in
//! this PC's local time — applications sent per week, answers, interviews,
//! offers, interviews by match score and by source, and what Nexus did and
//! how much of it the user undid. Nothing is estimated: a stage counts once
//! a change reached it (unless the user undid that change); an application
//! is dated by its "applied on" field, else by the change that marked it
//! applied, else it is counted as sent but placed in no week.

use std::collections::{BTreeMap, HashMap};

use nexus_core::{RecordActor, RecordChangeKind};
use nexus_protocol::{
    AdsHandlingView, CauseCountView, ErrorCode, GroupCountView, JobResultsView, NexusError,
    RecordId, RegisterId, ScoreBand, ScoreBandView, SourceCountView, WeekCountView,
};
use nexus_storage::{IntakeItemRow, PriceRow, RecordChangeRow, RecordRow};

use super::records::{JOB_APPLICATIONS, today, valid_date};
use super::{Runtime, to_nexus_error};
use crate::schedule;

const SENT: [&str; 4] = ["Applied", "Interviewing", "Offer", "Rejected"];
const ANSWERED: [&str; 3] = ["Interviewing", "Offer", "Rejected"];
const INTERVIEW: [&str; 2] = ["Interviewing", "Offer"];
const OFFER: [&str; 1] = ["Offer"];
const TO_APPLY: &str = "To apply";
const WITHDRAWN: &str = "Withdrawn";
const WEEKS: i64 = 8;
const LONG_WAIT_DAYS: i64 = 14;
const HIGH_SCORE: i64 = 70;
/// Sources shown by name; the rest are added up as "Other".
const MAX_SOURCES: usize = 5;
const MAX_RECORDS: u32 = 20_000;

/// Days since 1970-01-01 of a `YYYY-MM-DD` calendar day.
fn day_number(date: &str) -> Option<i64> {
    if !valid_date(date) {
        return None;
    }
    let y: i64 = date[0..4].parse().ok()?;
    let m: i64 = date[5..7].parse().ok()?;
    let d: i64 = date[8..10].parse().ok()?;
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// The `YYYY-MM-DD` of a day number.
fn day_date(n: i64) -> String {
    let z = n + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// The Monday of day `n`'s week (1970-01-01 was a Thursday).
fn monday(n: i64) -> i64 {
    n - (n + 3).rem_euclid(7)
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// A status a change set and still holds (not undone).
struct Reached<'a> {
    status: &'a str,
    at_ms: i64,
    /// Set when the application was added: its date is when it was written
    /// down, not when the answer came.
    on_creation: bool,
}

/// The results of the job applications in `records` with their `changes`
/// (oldest first), on `today` (`YYYY-MM-DD`); `date_of` turns a moment into
/// its local day.
pub(super) fn results(
    records: &[RecordRow],
    changes: &[RecordChangeRow],
    today: &str,
    ads_read: u32,
    partial: bool,
    date_of: impl Fn(i64) -> String,
) -> JobResultsView {
    let mut history: HashMap<RecordId, Vec<Reached<'_>>> = HashMap::new();
    for c in changes.iter().filter(|c| c.undone_at_ms.is_none()) {
        if let Some(status) = c.after.get("status") {
            history.entry(c.record_id).or_default().push(Reached {
                status,
                at_ms: c.at_ms,
                on_creation: c.kind == RecordChangeKind::Created,
            });
        }
    }
    let today_n = day_number(today);
    let mut weeks: BTreeMap<i64, u32> = today_n
        .map(monday)
        .map(|m| (0..WEEKS).map(|i| (m - 7 * (WEEKS - 1 - i), 0)).collect())
        .unwrap_or_default();
    let day_of = |at_ms: i64| day_number(&date_of(at_ms));

    let mut out = JobResultsView {
        sent: 0,
        to_apply: 0,
        answered: 0,
        interviews: 0,
        offers: 0,
        waiting: 0,
        waiting_long: 0,
        median_days_to_answer: None,
        weeks: Vec::new(),
        undated: 0,
        by_score: Vec::new(),
        by_source: Vec::new(),
        added_by_nexus: count(
            records
                .iter()
                .filter(|r| r.created_by == RecordActor::Agent)
                .count(),
        ),
        ads_read,
        nexus_changes: count(
            changes
                .iter()
                .filter(|c| c.actor == RecordActor::Agent)
                .count(),
        ),
        undone_changes: count(
            changes
                .iter()
                .filter(|c| c.actor == RecordActor::Agent && c.undone_at_ms.is_some())
                .count(),
        ),
        partial,
        ads: ads_handling(&[], &[], 0),
    };
    let mut days_to_answer = Vec::new();
    let mut bands = [
        (ScoreBand::High, 0, 0),
        (ScoreBand::Low, 0, 0),
        (ScoreBand::Unscored, 0, 0),
    ];
    // (grouping key, name as first written, sent, interviews)
    let mut sources: Vec<(String, String, u32, u32)> = Vec::new();

    for r in records {
        let status = r.status.as_deref().unwrap_or("");
        let reached = history.get(&r.id).map(Vec::as_slice).unwrap_or(&[]);
        let ever =
            |set: &[&str]| set.contains(&status) || reached.iter().any(|h| set.contains(&h.status));
        let applied_on = r.values.get("applied_on").filter(|d| valid_date(d));
        if status == TO_APPLY {
            out.to_apply += 1;
        }
        let sent = SENT.contains(&status)
            || (status == WITHDRAWN
                && (applied_on.is_some() || reached.iter().any(|h| SENT.contains(&h.status))));
        if !sent {
            continue;
        }
        out.sent += 1;
        let interview = ever(&INTERVIEW);
        out.answered += u32::from(ever(&ANSWERED));
        out.interviews += u32::from(interview);
        out.offers += u32::from(ever(&OFFER));

        let sent_day = applied_on.and_then(|d| day_number(d)).or_else(|| {
            reached
                .iter()
                .find(|h| SENT.contains(&h.status))
                .and_then(|h| day_of(h.at_ms))
        });
        if status == "Applied" {
            out.waiting += 1;
            if let (Some(sent), Some(today)) = (sent_day, today_n)
                && today - sent > LONG_WAIT_DAYS
            {
                out.waiting_long += 1;
            }
        }
        match sent_day {
            Some(day) => {
                if let Some(week) = weeks.get_mut(&monday(day)) {
                    *week += 1;
                }
            }
            None => out.undated += 1,
        }
        let answered_day = reached
            .iter()
            .find(|h| ANSWERED.contains(&h.status) && !h.on_creation)
            .and_then(|h| day_of(h.at_ms));
        if let (Some(sent), Some(answered)) = (sent_day, answered_day)
            && answered >= sent
        {
            days_to_answer.push(answered - sent);
        }

        let band = match r
            .values
            .get("match_score")
            .and_then(|s| s.trim().parse::<i64>().ok())
        {
            Some(score) if score >= HIGH_SCORE => 0,
            Some(_) => 1,
            None => 2,
        };
        bands[band].1 += 1;
        bands[band].2 += u32::from(interview);

        let shown = r
            .values
            .get("source")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or("Not set");
        let key = shown.to_lowercase();
        match sources.iter_mut().find(|s| s.0 == key) {
            Some(s) => {
                s.2 += 1;
                s.3 += u32::from(interview);
            }
            None => sources.push((key, shown.to_string(), 1, u32::from(interview))),
        }
    }

    days_to_answer.sort_unstable();
    out.median_days_to_answer = days_to_answer
        .get(days_to_answer.len().saturating_sub(1) / 2)
        .map(|d| u32::try_from(*d).unwrap_or(u32::MAX));
    out.weeks = weeks
        .into_iter()
        .map(|(start, sent)| WeekCountView {
            week_start: day_date(start),
            sent,
        })
        .collect();
    out.by_score = bands
        .into_iter()
        .map(|(band, sent, interviews)| ScoreBandView {
            band,
            counts: GroupCountView { sent, interviews },
        })
        .collect();
    sources.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
    if sources.len() > MAX_SOURCES + 1 {
        let rest = sources.split_off(MAX_SOURCES);
        let (sent, interviews) = rest
            .iter()
            .fold((0, 0), |acc, s| (acc.0 + s.2, acc.1 + s.3));
        sources.push((String::new(), "Other".to_string(), sent, interviews));
    }
    out.by_source = sources
        .into_iter()
        .map(|(_, source, sent, interviews)| SourceCountView {
            source,
            counts: GroupCountView { sent, interviews },
        })
        .collect();
    out
}

/// How the agent handled the inbox's job ads: finished (with an
/// application kept, or with nothing recorded), failed (by cause), still
/// waiting; tokens, an estimated cost from the user's prices, and the
/// median time per ad.
pub(super) fn ads_handling(
    items: &[IntakeItemRow],
    prices: &[PriceRow],
    now: i64,
) -> AdsHandlingView {
    let mut out = AdsHandlingView {
        seen: count(items.len()),
        finished: 0,
        recorded: 0,
        nothing_recorded: 0,
        failed: 0,
        waiting: 0,
        corrected: 0,
        accepted: 0,
        in_review: 0,
        failure_causes: Vec::new(),
        prompt_tokens: 0,
        output_tokens: 0,
        estimated_cost_usd: Some(0.0),
        median_seconds: None,
    };
    let mut seconds = Vec::new();
    for item in items {
        match item.status.as_str() {
            "Done" => {
                out.finished += 1;
                if item.agent_changes > item.undone {
                    out.recorded += 1;
                } else {
                    out.nothing_recorded += 1;
                }
                let state = item
                    .run_state
                    .as_deref()
                    .and_then(|s| s.parse::<nexus_core::RunState>().ok())
                    .unwrap_or(nexus_core::RunState::Succeeded);
                let ended = item.run_updated_ms.unwrap_or(0);
                match super::acceptance::acceptance(state, ended, &item.interventions, now).0 {
                    nexus_protocol::Acceptance::NotAccepted => out.corrected += 1,
                    nexus_protocol::Acceptance::Accepted => out.accepted += 1,
                    nexus_protocol::Acceptance::InReview => out.in_review += 1,
                    nexus_protocol::Acceptance::NotApplicable => {}
                }
                if let (Some(start), Some(end)) = (item.run_started_ms, item.run_updated_ms) {
                    seconds.push(u32::try_from((end - start).max(0) / 1000).unwrap_or(u32::MAX));
                }
                for (provider, model, free_tier, prompt, output) in &item.usage {
                    let (prompt, output) = (
                        u64::try_from(*prompt).unwrap_or(0),
                        u64::try_from(*output).unwrap_or(0),
                    );
                    out.prompt_tokens += prompt;
                    out.output_tokens += output;
                    if *free_tier {
                        continue;
                    }
                    let price = prices
                        .iter()
                        .find(|p| &p.provider == provider && &p.model == model);
                    out.estimated_cost_usd = match (out.estimated_cost_usd, price) {
                        (Some(sum), Some(p)) => Some(
                            sum + (prompt as f64 * p.input_per_million
                                + output as f64 * p.output_per_million)
                                / 1_000_000.0,
                        ),
                        _ => None,
                    };
                }
            }
            "Failed" => {
                out.failed += 1;
                let cause = item
                    .failure_cause
                    .clone()
                    .unwrap_or_else(|| "NotStarted".to_string());
                match out.failure_causes.iter_mut().find(|c| c.cause == cause) {
                    Some(c) => c.count += 1,
                    None => out.failure_causes.push(CauseCountView { cause, count: 1 }),
                }
            }
            _ => out.waiting += 1,
        }
    }
    out.failure_causes
        .sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.cause.cmp(&b.cause)));
    seconds.sort_unstable();
    out.median_seconds = seconds.get(seconds.len().saturating_sub(1) / 2).copied();
    out
}

impl Runtime {
    pub async fn job_results(&self, register_id: RegisterId) -> Result<JobResultsView, NexusError> {
        let register = self.register_by_id(register_id).await?;
        if register.row.key != JOB_APPLICATIONS {
            return Err(NexusError::new(
                ErrorCode::InvalidRequest,
                "results are counted for job applications only",
            ));
        }
        let (records, changes, partial) = self
            .store
            .register_activity(register_id, MAX_RECORDS)
            .await
            .map_err(to_nexus_error)?;
        let ads_read = self
            .store
            .inbox_done_count(register_id)
            .await
            .map_err(to_nexus_error)?;
        let items = self
            .store
            .intake_handling(register_id)
            .await
            .map_err(to_nexus_error)?;
        let prices = self.store.model_prices().await.map_err(to_nexus_error)?;
        let mut view = results(&records, &changes, &today(), ads_read, partial, |ms| {
            schedule::local_date(ms, None)
        });
        view.ads = ads_handling(&items, &prices, nexus_storage::now_ms());
        Ok(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_protocol::RegisterId;
    use nexus_storage::Values;

    const DAY_MS: i64 = 86_400_000;

    /// Noon UTC of a day, read back as that day.
    fn at(day: &str) -> i64 {
        day_number(day).unwrap() * DAY_MS + DAY_MS / 2
    }

    fn date_of(ms: i64) -> String {
        day_date(ms.div_euclid(DAY_MS))
    }

    fn record(status: &str, values: &[(&str, &str)], by: RecordActor) -> RecordRow {
        let mut v: Values = values
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        v.insert("status".into(), status.into());
        RecordRow {
            id: RecordId::new(),
            register_id: RegisterId::new(),
            values: v,
            status: Some(status.to_string()),
            follow_up_on: None,
            revision: 1,
            created_by: by,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    fn change(
        r: &RecordRow,
        kind: RecordChangeKind,
        actor: RecordActor,
        status: &str,
        day: &str,
        undone: bool,
    ) -> RecordChangeRow {
        let after: Values = [("status".to_string(), status.to_string())].into();
        RecordChangeRow {
            id: 0,
            record_id: r.id,
            kind,
            actor,
            before: (kind != RecordChangeKind::Created).then(Values::new),
            after,
            revision_after: 1,
            conversation_id: None,
            message_id: None,
            read_hosts: Vec::new(),
            undone_at_ms: undone.then_some(at(day)),
            at_ms: at(day),
            agent: None,
        }
    }

    #[test]
    fn calendar_days_round_trip_and_weeks_start_on_monday() {
        for day in [
            "1970-01-01",
            "2000-02-29",
            "2026-10-19",
            "2026-12-31",
            "2027-03-01",
        ] {
            assert_eq!(day_date(day_number(day).unwrap()), day);
        }
        assert_eq!(day_number("1970-01-01"), Some(0));
        assert_eq!(
            day_date(monday(day_number("2026-10-22").unwrap())),
            "2026-10-19"
        );
        assert_eq!(
            day_date(monday(day_number("2026-10-19").unwrap())),
            "2026-10-19"
        );
        assert_eq!(
            day_date(monday(day_number("2026-10-18").unwrap())),
            "2026-10-12"
        );
        assert_eq!(day_number("2026-02-30"), None);
    }

    #[test]
    fn results_count_what_happened_and_nothing_else() {
        use RecordActor::{Agent, User};
        use RecordChangeKind::{Created, Undo, Updated};
        // Applied 18 days ago by Nexus, still waiting.
        let a = record(
            "Applied",
            &[
                ("applied_on", "2026-10-01"),
                ("match_score", "82"),
                ("source", "TopJobs"),
            ],
            Agent,
        );
        // Applied, interviewed, then rejected.
        let b = record(
            "Rejected",
            &[
                ("applied_on", "2026-10-06"),
                ("match_score", "75"),
                ("source", "topjobs "),
            ],
            User,
        );
        // Nexus marked an interview the user then undid.
        let c = record(
            "Applied",
            &[("match_score", "40"), ("source", "LinkedIn")],
            User,
        );
        // Still to apply; withdrawn before applying.
        let d = record("To apply", &[("match_score", "90")], Agent);
        let e = record("Withdrawn", &[], User);
        // An offer, dated by the change that marked it applied.
        let f = record("Offer", &[], User);
        // Applied, with no date anywhere.
        let g = record("Applied", &[], User);
        let changes = vec![
            change(&f, Created, User, "Applied", "2026-09-01", false),
            change(&f, Updated, User, "Offer", "2026-09-20", false),
            change(&a, Created, Agent, "Applied", "2026-10-01", false),
            change(&b, Created, User, "To apply", "2026-10-05", false),
            change(&b, Updated, User, "Applied", "2026-10-06", false),
            change(&b, Updated, User, "Interviewing", "2026-10-10", false),
            change(&b, Updated, User, "Rejected", "2026-10-15", false),
            change(&c, Created, User, "Applied", "2026-10-13", false),
            change(&c, Updated, Agent, "Interviewing", "2026-10-14", true),
            change(&c, Undo, User, "Applied", "2026-10-14", false),
            change(&e, Created, User, "To apply", "2026-10-16", false),
            change(&e, Updated, User, "Withdrawn", "2026-10-17", false),
            change(&d, Created, Agent, "To apply", "2026-10-18", false),
        ];
        let records = [a, b, c, d, e, f, g];
        let v = results(&records, &changes, "2026-10-19", 7, false, date_of);

        assert_eq!((v.sent, v.to_apply), (5, 1));
        assert_eq!((v.answered, v.interviews, v.offers), (2, 2, 1));
        assert_eq!((v.waiting, v.waiting_long), (3, 1));
        // B: 4 days (10-06 → 10-10); F: 19 days; lower median.
        assert_eq!(v.median_days_to_answer, Some(4));
        let weeks: Vec<(&str, u32)> = v
            .weeks
            .iter()
            .map(|w| (w.week_start.as_str(), w.sent))
            .collect();
        assert_eq!(
            weeks,
            [
                ("2026-08-31", 1),
                ("2026-09-07", 0),
                ("2026-09-14", 0),
                ("2026-09-21", 0),
                ("2026-09-28", 1),
                ("2026-10-05", 1),
                ("2026-10-12", 1),
                ("2026-10-19", 0),
            ]
        );
        assert_eq!(v.undated, 1);
        let bands: Vec<(ScoreBand, u32, u32)> = v
            .by_score
            .iter()
            .map(|b| (b.band, b.counts.sent, b.counts.interviews))
            .collect();
        assert_eq!(
            bands,
            [
                (ScoreBand::High, 2, 1),
                (ScoreBand::Low, 1, 0),
                (ScoreBand::Unscored, 2, 1),
            ]
        );
        let sources: Vec<(&str, u32, u32)> = v
            .by_source
            .iter()
            .map(|s| (s.source.as_str(), s.counts.sent, s.counts.interviews))
            .collect();
        assert_eq!(
            sources,
            [("Not set", 2, 1), ("TopJobs", 2, 1), ("LinkedIn", 1, 0)]
        );
        assert_eq!((v.added_by_nexus, v.ads_read), (2, 7));
        assert_eq!((v.nexus_changes, v.undone_changes), (3, 1));
        assert!(!v.partial);
    }

    fn item(
        status: &str,
        run: Option<(&str, Option<&str>)>,
        changes: (i64, i64, i64),
    ) -> IntakeItemRow {
        IntakeItemRow {
            status: status.into(),
            reason: None,
            run_state: run.map(|r| r.0.to_string()),
            failure_cause: run.and_then(|r| r.1.map(str::to_string)),
            run_started_ms: run.map(|_| 1_000),
            run_updated_ms: run.map(|_| 31_000),
            usage: vec![("Gemini".into(), "flash".into(), false, 1_000_000, 100_000)],
            agent_changes: changes.0,
            undone: changes.1,
            // A correction (third number) on the turn, one day after it.
            interventions: (0..changes.2)
                .map(|_| (nexus_core::InterventionKind::Correction, 31_000 + DAY_MS))
                .chain(
                    (0..changes.1).map(|_| (nexus_core::InterventionKind::Undo, 31_000 + DAY_MS)),
                )
                .collect(),
        }
    }

    #[test]
    fn ads_are_counted_by_how_their_task_ended_with_tokens_cost_and_time() {
        let items = [
            item("Done", Some(("Succeeded", None)), (2, 0, 0)),
            item("Done", Some(("Succeeded", None)), (1, 0, 1)),
            item("Done", Some(("Succeeded", None)), (0, 0, 0)),
            item("Done", Some(("Succeeded", None)), (1, 1, 0)),
            item("Failed", Some(("Failed", Some("QuotaExceeded"))), (0, 0, 0)),
            item("Failed", None, (0, 0, 0)),
            item("Waiting", None, (0, 0, 0)),
            item("Started", Some(("Running", None)), (0, 0, 0)),
        ];
        let price = PriceRow {
            provider: "Gemini".into(),
            model: "flash".into(),
            input_per_million: 0.5,
            output_per_million: 2.0,
        };
        let v = ads_handling(&items, &[price], 31_000 + 2 * DAY_MS);
        assert_eq!((v.seen, v.finished, v.failed, v.waiting), (8, 4, 2, 2));
        // The fourth: its only change was undone, so nothing was kept.
        assert_eq!((v.recorded, v.nothing_recorded, v.corrected), (2, 2, 2));
        // The other two finished ads are still in their review window.
        assert_eq!((v.accepted, v.in_review), (0, 2));
        let later = ads_handling(&items, &[], 31_000 + 8 * DAY_MS);
        assert_eq!(
            (later.corrected, later.accepted, later.in_review),
            (2, 2, 0)
        );
        let causes: Vec<(&str, u32)> = v
            .failure_causes
            .iter()
            .map(|c| (c.cause.as_str(), c.count))
            .collect();
        assert_eq!(causes, [("NotStarted", 1), ("QuotaExceeded", 1)]);
        assert_eq!((v.prompt_tokens, v.output_tokens), (4_000_000, 400_000));
        // 4 × (1M × 0.5 + 0.1M × 2.0) / 1M = 4 × 0.7.
        assert!((v.estimated_cost_usd.unwrap() - 2.8).abs() < 1e-9);
        assert_eq!(v.median_seconds, Some(30));

        let unpriced = ads_handling(&items, &[], 0);
        assert_eq!(unpriced.estimated_cost_usd, None, "no price, no estimate");
        let mut free = items[0].clone();
        free.usage[0].2 = true;
        assert_eq!(ads_handling(&[free], &[], 0).estimated_cost_usd, Some(0.0));
    }

    #[test]
    fn many_sources_end_in_other_and_an_empty_register_is_all_zero() {
        let records: Vec<RecordRow> = (0..8)
            .map(|i| {
                let source = format!("Site {i}");
                record("Applied", &[("source", source.as_str())], RecordActor::User)
            })
            .collect();
        let v = results(&records, &[], "2026-10-19", 0, true, date_of);
        assert_eq!(v.by_source.len(), MAX_SOURCES + 1);
        let other = v.by_source.last().unwrap();
        assert_eq!((other.source.as_str(), other.counts.sent), ("Other", 3));
        assert!(v.partial);

        let empty = results(&[], &[], "2026-10-19", 0, false, date_of);
        assert_eq!(
            (empty.sent, empty.undated, empty.median_days_to_answer),
            (0, 0, None)
        );
        assert_eq!(empty.weeks.len(), 8);
        assert!(empty.by_source.is_empty());
    }
}
