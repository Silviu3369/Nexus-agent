//! Whether the user kept what the agent did in a turn (ADR-TASK-001): a
//! turn that succeeded is in review for 7 days after it ended; a rejection,
//! an undo, a correction or a stop until then (or during the turn) means it
//! was not accepted; with none of those, it is accepted once the window
//! passes. Approvals and retries say nothing against it.

use nexus_core::{InterventionKind, RunState};
use nexus_protocol::{Acceptance, TurnView};
use nexus_storage::TurnReviewRow;

/// The review window after a turn ends.
pub(crate) const REVIEW_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// How a turn that ended at `ended_at_ms` stands at `now`.
pub(crate) fn acceptance(
    state: RunState,
    ended_at_ms: i64,
    interventions: &[(InterventionKind, i64)],
    now: i64,
) -> (Acceptance, Option<i64>) {
    if state != RunState::Succeeded {
        return (Acceptance::NotApplicable, None);
    }
    let until = ended_at_ms.saturating_add(REVIEW_MS);
    if interventions
        .iter()
        .any(|(kind, at)| kind.is_negative() && *at <= until)
    {
        return (Acceptance::NotAccepted, None);
    }
    if now < until {
        (Acceptance::InReview, Some(until))
    } else {
        (Acceptance::Accepted, None)
    }
}

pub(crate) fn turn_view(row: &TurnReviewRow, now: i64) -> TurnView {
    let (acceptance, review_until_ms) =
        acceptance(row.state, row.ended_at_ms, &row.interventions, now);
    TurnView {
        run_id: row.run_id,
        status: super::status(row.state),
        failure_cause: row.failure_cause.clone(),
        acceptance,
        review_until_ms,
        interventions: row
            .interventions
            .iter()
            .map(|(kind, _)| kind.as_str().to_string())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use InterventionKind::*;

    const DAY: i64 = 24 * 60 * 60 * 1000;

    #[test]
    fn a_succeeded_turn_is_accepted_only_after_seven_days_without_a_correction() {
        let end = 10 * DAY;
        assert_eq!(
            acceptance(RunState::Succeeded, end, &[], end + DAY),
            (Acceptance::InReview, Some(end + 7 * DAY))
        );
        assert_eq!(
            acceptance(
                RunState::Succeeded,
                end,
                &[(Approved, end - 1)],
                end + 7 * DAY
            ),
            (Acceptance::Accepted, None),
            "an approval says nothing against it"
        );
        for kind in [Rejected, Undo, Correction, Stopped] {
            assert_eq!(
                acceptance(
                    RunState::Succeeded,
                    end,
                    &[(kind, end + 3 * DAY)],
                    end + DAY
                )
                .0,
                Acceptance::NotAccepted,
                "{kind}"
            );
        }
        assert_eq!(
            acceptance(
                RunState::Succeeded,
                end,
                &[(Correction, end + 8 * DAY)],
                end + 9 * DAY
            )
            .0,
            Acceptance::Accepted,
            "a correction after the window does not count against it"
        );
        assert_eq!(
            acceptance(
                RunState::Succeeded,
                end,
                &[(Rejected, end - DAY)],
                end + 8 * DAY
            )
            .0,
            Acceptance::NotAccepted,
            "a proposal rejected during the turn counts"
        );
        assert_eq!(
            acceptance(RunState::Failed, end, &[], end + 8 * DAY).0,
            Acceptance::NotApplicable
        );
        assert_eq!(
            acceptance(RunState::AwaitingApproval, end, &[], end + DAY).0,
            Acceptance::NotApplicable
        );
    }
}
