//! The global limits a Run is held to while it executes (ADR-0012, ADR-0038).
//!
//! Validation rejects Definitions that exceed a limit statically; these checks cover what only
//! a running Run knows: how many Steps it has taken, how many iterations a loop gets, how deep
//! Subworkflows nest and how large an Output is. Every breach is a `fatal` error, so it never
//! consumes a Step's `retry` policy.

use std::fmt;
use std::io;

use bia_core::action::ActionError;
use bia_core::dsl::Definition;
use serde_json::{Value, json};

use super::failure::{Failure, FailureCode};

/// The configured global limits. A Definition's own `limits` can only lower them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunLimits {
    /// Steps one Run may take up, counted once per Step reached, whatever it ends as;
    /// retries and waking from a sleep do not count again, and each loop iteration does.
    pub max_steps_per_run: u64,
    /// Iterations one loop may run, `flow.foreach` items included.
    pub max_iterations: u32,
    /// How many levels of Subworkflow may run below a Run started by any other Trigger.
    pub max_subworkflow_depth: u32,
    /// The largest Output a Step may produce, measured as its compact JSON serialisation.
    pub max_output_bytes: u64,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self {
            max_steps_per_run: 10_000,
            max_iterations: 10_000,
            max_subworkflow_depth: 8,
            max_output_bytes: 256 * 1024,
        }
    }
}

/// A Run went past one of its limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitExceeded {
    pub limit: Limit,
    /// The limit in force for the Run, after its Definition's own `limits`.
    pub max: u64,
    /// The value that went past it; at least `max + 1`.
    pub actual: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    MaxStepsPerRun,
    MaxIterations,
    MaxSubworkflowDepth,
    MaxOutputBytes,
}

impl Limit {
    /// The configuration key, as the error on the Timeline names it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MaxStepsPerRun => "max_steps_per_run",
            Self::MaxIterations => "max_iterations",
            Self::MaxSubworkflowDepth => "max_subworkflow_depth",
            Self::MaxOutputBytes => "max_output_bytes",
        }
    }

    fn code(self) -> FailureCode {
        match self {
            Self::MaxStepsPerRun => FailureCode::RunStepLimitExceeded,
            Self::MaxIterations => FailureCode::IterationLimitExceeded,
            Self::MaxSubworkflowDepth => FailureCode::SubworkflowDepthExceeded,
            Self::MaxOutputBytes => FailureCode::OutputLimitExceeded,
        }
    }
}

impl fmt::Display for LimitExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { max, actual, .. } = self;
        match self.limit {
            Limit::MaxStepsPerRun => {
                write!(f, "the Run reached Step {actual}, more than {max}")
            }
            Limit::MaxIterations => {
                write!(f, "the loop needs {actual} iterations, more than {max}")
            }
            Limit::MaxSubworkflowDepth => {
                write!(
                    f,
                    "the Subworkflow would run {actual} levels deep, more than {max}"
                )
            }
            Limit::MaxOutputBytes => write!(f, "the Output is {actual} bytes, more than {max}"),
        }?;
        write!(f, " allowed by {}", self.limit.as_str())
    }
}

impl LimitExceeded {
    pub(crate) fn failure(self) -> Failure {
        let details = json!({
            "limit": self.limit.as_str(),
            "max": self.max,
            "actual": self.actual,
        });
        Failure::raised(
            self.limit.code(),
            ActionError::fatal(self.to_string()).with_details(details),
        )
    }
}

impl RunLimits {
    /// The limits for Runs of `definition`: its own `limits`, where lower, else these.
    pub fn for_definition(self, definition: &Definition) -> Self {
        let Some(own) = &definition.limits else {
            return self;
        };
        Self {
            max_steps_per_run: own.max_steps_per_run.map_or(self.max_steps_per_run, |own| {
                own.min(self.max_steps_per_run)
            }),
            max_output_bytes: own
                .max_output_bytes
                .map_or(self.max_output_bytes, |own| own.min(self.max_output_bytes)),
            ..self
        }
    }

    /// Checks that the Run may take up its `step`th Step, counting from 1.
    pub fn check_steps(&self, step: u64) -> Result<(), LimitExceeded> {
        check(Limit::MaxStepsPerRun, self.max_steps_per_run, step)
    }

    /// Checks that a loop may run `iterations` iterations: a `flow.foreach` passes its item
    /// count before the first one, a `flow.loop_until` the number of the iteration it is about
    /// to start, counting from 1. `declared` is the Block's own `max_iterations`, which can
    /// only lower the global limit.
    pub fn check_iterations(
        &self,
        declared: Option<u32>,
        iterations: u64,
    ) -> Result<(), LimitExceeded> {
        let max = declared.map_or(self.max_iterations, |own| own.min(self.max_iterations));
        check(Limit::MaxIterations, u64::from(max), iterations)
    }

    /// Checks that a Subworkflow may start at `depth`: 1 for a child of a Run that no
    /// `flow.subworkflow` started, its parent's depth plus one otherwise.
    pub fn check_subworkflow_depth(&self, depth: u32) -> Result<(), LimitExceeded> {
        check(
            Limit::MaxSubworkflowDepth,
            u64::from(self.max_subworkflow_depth),
            u64::from(depth),
        )
    }

    /// Checks that `output` fits in `max_output_bytes` once serialised. The size is counted as
    /// it serialises, so checking allocates nothing.
    pub fn check_output(&self, output: &Value) -> Result<(), LimitExceeded> {
        let mut counter = Counter(0);
        serde_json::to_writer(&mut counter, output)
            .unwrap_or_else(|_| unreachable!("a `Value` always serialises"));
        check(Limit::MaxOutputBytes, self.max_output_bytes, counter.0)
    }
}

fn check(limit: Limit, max: u64, actual: u64) -> Result<(), LimitExceeded> {
    if actual <= max {
        Ok(())
    } else {
        Err(LimitExceeded { limit, max, actual })
    }
}

/// Counts the bytes written to it.
struct Counter(u64);

impl io::Write for Counter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .saturating_add(u64::try_from(buf.len()).unwrap_or(u64::MAX));
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use bia_core::dsl::Definition;
    use serde_json::json;

    use super::{Limit, LimitExceeded, RunLimits};

    fn limits() -> RunLimits {
        RunLimits {
            max_steps_per_run: 3,
            max_iterations: 100,
            max_subworkflow_depth: 2,
            max_output_bytes: 16,
        }
    }

    fn exceeded(limit: Limit, max: u64, actual: u64) -> Result<(), LimitExceeded> {
        Err(LimitExceeded { limit, max, actual })
    }

    #[test]
    fn steps_up_to_the_limit_pass_and_the_next_one_fails() {
        assert_eq!(limits().check_steps(3), Ok(()));
        assert_eq!(
            limits().check_steps(4),
            exceeded(Limit::MaxStepsPerRun, 3, 4)
        );
    }

    #[test]
    fn a_loop_gets_the_lower_of_its_own_max_iterations_and_the_global_limit() {
        let limits = limits();
        assert_eq!(limits.check_iterations(None, 100), Ok(()));
        assert_eq!(
            limits.check_iterations(None, 101),
            exceeded(Limit::MaxIterations, 100, 101)
        );
        assert_eq!(
            limits.check_iterations(Some(5), 6),
            exceeded(Limit::MaxIterations, 5, 6),
            "a Block lowers the limit"
        );
        assert_eq!(
            limits.check_iterations(Some(1_000), 101),
            exceeded(Limit::MaxIterations, 100, 101),
            "a Block cannot raise it"
        );
        assert_eq!(limits.check_iterations(Some(0), 0), Ok(()), "an empty list");
    }

    #[test]
    fn subworkflows_may_nest_down_to_the_depth_limit() {
        assert_eq!(limits().check_subworkflow_depth(1), Ok(()));
        assert_eq!(limits().check_subworkflow_depth(2), Ok(()));
        assert_eq!(
            limits().check_subworkflow_depth(3),
            exceeded(Limit::MaxSubworkflowDepth, 2, 3)
        );
    }

    #[test]
    fn output_size_is_its_compact_json_serialisation() {
        // `{"a":"0123456789"}` is 18 bytes.
        let output = json!({ "a": "0123456789" });
        let fits = RunLimits {
            max_output_bytes: 18,
            ..limits()
        };
        assert_eq!(fits.check_output(&output), Ok(()));
        assert_eq!(
            limits().check_output(&output),
            exceeded(Limit::MaxOutputBytes, 16, 18)
        );
    }

    #[test]
    fn a_definition_lowers_but_never_raises_the_global_limits() {
        let definition = |limits: serde_json::Value| {
            Definition::from_value(json!({
                "schema_version": 1, "id": "x", "steps": [], "limits": limits,
            }))
            .unwrap()
        };
        let lowered = limits().for_definition(&definition(json!({
            "max_steps_per_run": 2, "max_output_bytes": 8,
        })));
        assert_eq!(lowered.max_steps_per_run, 2);
        assert_eq!(lowered.max_output_bytes, 8);

        let raised = limits().for_definition(&definition(json!({
            "max_steps_per_run": 50, "max_output_bytes": 1_000,
        })));
        assert_eq!(raised, limits());

        assert_eq!(limits().for_definition(&definition(json!({}))), limits());
    }

    #[test]
    fn the_error_names_the_limit() {
        let error = limits().check_steps(4).unwrap_err().failure().to_value();
        assert_eq!(error["class"], "fatal");
        assert_eq!(error["code"], "run_step_limit_exceeded");
        assert_eq!(
            error["details"],
            json!({ "limit": "max_steps_per_run", "max": 3, "actual": 4 })
        );
        let message = error["message"].as_str().unwrap();
        assert!(message.contains("max_steps_per_run"), "{message}");
    }
}
