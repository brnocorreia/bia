//! What one claim of a Schedule fires: the Catch-up Policy picks among the occurrences that
//! fell due, then the Overlap Policy decides which of them start now (ADR-0039).

use bia_core::cron::{Cron, jitter};
use bia_core::dsl::{CatchUpPolicy, OverlapPolicy};
use bia_store::schedule::Schedule;
use chrono::{DateTime, TimeDelta, Utc};

/// The most missed occurrences `catch_up: all` fires; older ones are dropped.
pub const CATCH_UP_LIMIT: usize = 100;
/// The most occurrences `overlap: queue` holds back; later ones are skipped.
pub const QUEUE_LIMIT: usize = 100;
/// How late after its due time an occurrence counts as missed rather than on time.
pub const MISSED_AFTER: TimeDelta = TimeDelta::minutes(1);

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct Plan {
    /// Occurrences to start Runs for now, oldest first.
    pub(super) fire: Vec<DateTime<Utc>>,
    pub(super) queued: Vec<DateTime<Utc>>,
    /// The next occurrence that has not fallen due; `None` when the expression never fires
    /// again.
    pub(super) next_at: Option<DateTime<Utc>>,
    /// Occurrences the Overlap Policy skipped.
    pub(super) skipped: Vec<DateTime<Utc>>,
    /// How many missed occurrences the Catch-up Policy left out, counting only the newest
    /// [`CATCH_UP_LIMIT`] and whether older ones existed.
    pub(super) missed: usize,
    pub(super) missed_more: bool,
}

/// When an occurrence of `schedule` fires: the occurrence plus its jitter.
pub(super) fn due_at(schedule: &Schedule, at: DateTime<Utc>) -> DateTime<Utc> {
    let Some(window) = schedule.trigger.jitter else {
        return at;
    };
    let seed = format!("{}/{}", schedule.workflow_id, schedule.trigger_id);
    at + jitter(&seed, at, window)
}

pub(super) fn plan(
    schedule: &Schedule,
    cron: &Cron,
    previous_active: bool,
    now: DateTime<Utc>,
) -> Plan {
    let mut plan = Plan {
        next_at: Some(schedule.next_at),
        queued: schedule.queued.clone(),
        ..Plan::default()
    };

    let mut due = Vec::new();
    if schedule.due_at <= now {
        let (mut occurrences, truncated) = cron.occurrences(schedule.next_at, now, CATCH_UP_LIMIT);
        if occurrences.is_empty() {
            occurrences.push(schedule.next_at);
        }
        // Jitter can hold the newest occurrences back; they and those after them wait.
        let ready = occurrences
            .iter()
            .take_while(|at| due_at(schedule, **at) <= now)
            .count()
            .max(1);
        plan.next_at = match occurrences.get(ready) {
            Some(waiting) => Some(*waiting),
            None => cron.next_after(occurrences[ready - 1].max(now)),
        };
        occurrences.truncate(ready);
        plan.missed_more = truncated;
        due = occurrences;
    }

    let (on_time, missed): (Vec<_>, Vec<_>) = due
        .iter()
        .partition(|at| now - due_at(schedule, **at) <= MISSED_AFTER);
    let chosen = match schedule.trigger.catch_up_policy() {
        CatchUpPolicy::None => on_time,
        CatchUpPolicy::Latest if on_time.is_empty() => missed.last().copied().into_iter().collect(),
        CatchUpPolicy::Latest => on_time,
        CatchUpPolicy::All => due.clone(),
    };
    plan.missed = due.len() - chosen.len();

    let mut active = previous_active;
    if !active && !plan.queued.is_empty() {
        plan.fire.push(plan.queued.remove(0));
        active = true;
    }
    for at in chosen {
        match schedule.trigger.overlap_policy() {
            OverlapPolicy::Allow => plan.fire.push(at),
            _ if !active => {
                plan.fire.push(at);
                active = true;
            }
            OverlapPolicy::Queue if plan.queued.len() < QUEUE_LIMIT => plan.queued.push(at),
            OverlapPolicy::Skip | OverlapPolicy::Queue => plan.skipped.push(at),
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use bia_core::cron::Cron;
    use bia_core::dsl::{CatchUpPolicy, CronTrigger, OverlapPolicy, TriggerId, WorkflowId};
    use bia_store::schedule::Schedule;
    use bia_store::version::VersionNumber;
    use chrono::{DateTime, TimeDelta, Utc};

    use super::{CATCH_UP_LIMIT, MISSED_AFTER, QUEUE_LIMIT, plan};

    fn at(rfc3339: &str) -> DateTime<Utc> {
        rfc3339.parse().unwrap()
    }

    fn hourly(overlap: OverlapPolicy, catch_up: CatchUpPolicy, next_at: &str) -> Schedule {
        Schedule {
            workflow_id: WorkflowId::new("wf").unwrap(),
            trigger_id: TriggerId::new("hourly").unwrap(),
            version: VersionNumber::new(1).unwrap(),
            trigger: CronTrigger {
                expr: "0 * * * *".into(),
                tz: None,
                jitter: None,
                overlap: Some(overlap),
                catch_up: Some(catch_up),
            },
            next_at: at(next_at),
            due_at: at(next_at),
            queued: Vec::new(),
            last_run: None,
        }
    }

    fn cron() -> Cron {
        Cron::new("0 * * * *", None).unwrap()
    }

    #[test]
    fn an_on_time_occurrence_fires_and_the_next_one_waits() {
        let schedule = hourly(
            OverlapPolicy::Skip,
            CatchUpPolicy::None,
            "2026-01-01T09:00:00Z",
        );

        let early = plan(&schedule, &cron(), false, at("2026-01-01T08:59:59Z"));
        assert!(early.fire.is_empty());
        assert_eq!(early.next_at, Some(at("2026-01-01T09:00:00Z")));

        let on_time = plan(&schedule, &cron(), false, at("2026-01-01T09:00:30Z"));
        assert_eq!(on_time.fire, [at("2026-01-01T09:00:00Z")]);
        assert_eq!(on_time.next_at, Some(at("2026-01-01T10:00:00Z")));
    }

    #[test]
    fn catch_up_picks_none_the_latest_or_all_missed_occurrences() {
        let now = at("2026-01-01T12:30:00Z");
        let fired = |catch_up| {
            let schedule = hourly(OverlapPolicy::Allow, catch_up, "2026-01-01T09:00:00Z");
            let plan = plan(&schedule, &cron(), false, now);
            assert_eq!(plan.next_at, Some(at("2026-01-01T13:00:00Z")));
            plan.fire
        };

        assert!(fired(CatchUpPolicy::None).is_empty());
        assert_eq!(fired(CatchUpPolicy::Latest), [at("2026-01-01T12:00:00Z")]);
        assert_eq!(fired(CatchUpPolicy::All).len(), 4);
    }

    #[test]
    fn an_occurrence_late_by_more_than_the_threshold_counts_as_missed() {
        let schedule = hourly(
            OverlapPolicy::Skip,
            CatchUpPolicy::None,
            "2026-01-01T09:00:00Z",
        );
        let late = at("2026-01-01T09:00:00Z") + MISSED_AFTER + TimeDelta::seconds(1);

        let plan = plan(&schedule, &cron(), false, late);

        assert!(plan.fire.is_empty());
        assert_eq!(plan.missed, 1);
    }

    #[test]
    fn catch_up_all_keeps_only_the_newest_occurrences_within_the_limit() {
        let schedule = hourly(
            OverlapPolicy::Allow,
            CatchUpPolicy::All,
            "2026-01-01T00:00:00Z",
        );
        let now = at("2026-01-01T00:00:00Z") + TimeDelta::hours(150);

        let plan = plan(&schedule, &cron(), false, now);

        assert_eq!(plan.fire.len(), CATCH_UP_LIMIT);
        assert_eq!(plan.fire.last(), Some(&now));
        assert!(plan.missed_more);
    }

    #[test]
    fn overlap_skips_queues_or_allows_while_the_previous_run_is_active() {
        let now = at("2026-01-01T09:00:00Z");
        let decide = |overlap| {
            let schedule = hourly(overlap, CatchUpPolicy::Latest, "2026-01-01T09:00:00Z");
            plan(&schedule, &cron(), true, now)
        };

        let skip = decide(OverlapPolicy::Skip);
        assert!(skip.fire.is_empty());
        assert_eq!(skip.skipped, [now]);

        let queue = decide(OverlapPolicy::Queue);
        assert!(queue.fire.is_empty());
        assert_eq!(queue.queued, [now]);

        assert_eq!(decide(OverlapPolicy::Allow).fire, [now]);
    }

    #[test]
    fn a_queued_occurrence_fires_once_the_previous_run_finished_and_the_rest_wait() {
        let mut schedule = hourly(
            OverlapPolicy::Queue,
            CatchUpPolicy::All,
            "2026-01-01T09:00:00Z",
        );
        schedule.queued = vec![at("2026-01-01T07:00:00Z"), at("2026-01-01T08:00:00Z")];

        let plan = plan(&schedule, &cron(), false, at("2026-01-01T09:00:00Z"));

        assert_eq!(plan.fire, [at("2026-01-01T07:00:00Z")]);
        assert_eq!(
            plan.queued,
            [at("2026-01-01T08:00:00Z"), at("2026-01-01T09:00:00Z")]
        );
    }

    #[test]
    fn a_full_queue_skips_further_occurrences() {
        let mut schedule = hourly(
            OverlapPolicy::Queue,
            CatchUpPolicy::Latest,
            "2026-01-01T09:00:00Z",
        );
        schedule.queued = vec![at("2026-01-01T08:00:00Z"); QUEUE_LIMIT];

        let plan = plan(&schedule, &cron(), true, at("2026-01-01T09:00:00Z"));

        assert_eq!(plan.queued.len(), QUEUE_LIMIT);
        assert_eq!(plan.skipped, [at("2026-01-01T09:00:00Z")]);
    }
}
