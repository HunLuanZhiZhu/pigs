//! Pure phase orchestration state shared by transport-specific runtimes.

use crate::phased_markers::{detect_marker, strip_markers, Marker};
use crate::phased_phase::Phase;

/// Iteration limits for one phased turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrchestrationLimits {
    /// Maximum markerless Post outputs retained before failing the turn.
    pub max_post_iterations: u32,
    /// Maximum Post failures that may return to Pre.
    pub max_pre_replans: u32,
}

impl Default for OrchestrationLimits {
    fn default() -> Self {
        Self {
            max_post_iterations: 3,
            max_pre_replans: 2,
        }
    }
}

/// Result of applying one completed model output to the state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Advance {
    /// The turn has completed through a valid `PIGEND` marker.
    Complete,
    /// Continue by executing the given phase.
    Continue(Phase),
}

/// Explicit terminal failures. Budget exhaustion is never a successful result.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OrchestrationError {
    /// Markerless Post iterations exceeded their configured budget.
    #[error("Post phase budget exhausted after {limit} markerless iteration(s)")]
    PostBudgetExceeded { limit: u32 },
    /// Failed replans exceeded their configured budget.
    #[error("Pre replan budget exhausted after {limit} failure(s)")]
    ReplanBudgetExceeded { limit: u32 },
}

/// Pure Pre -> Executor -> Post orchestration state.
#[derive(Debug, Clone)]
pub struct OrchestrationState {
    phase: Phase,
    limits: OrchestrationLimits,
    pre_output: String,
    executor_outputs: Vec<String>,
    post_outputs: Vec<String>,
    failure_outputs: Vec<String>,
    post_iterations: u32,
    pre_replans: u32,
}

impl OrchestrationState {
    /// Creates a turn beginning in Pre.
    pub fn new(limits: OrchestrationLimits) -> Self {
        Self {
            phase: Phase::Pre,
            limits,
            pre_output: String::new(),
            executor_outputs: Vec::new(),
            post_outputs: Vec::new(),
            failure_outputs: Vec::new(),
            post_iterations: 0,
            pre_replans: 0,
        }
    }

    /// Returns the phase that must run next.
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Applies the completed output of the current phase.
    pub fn advance(&mut self, output: &str) -> Result<Advance, OrchestrationError> {
        let visible = strip_markers(output);
        match self.phase {
            Phase::Pre => match detect_marker(output) {
                Some(Marker::End) => Ok(Advance::Complete),
                Some(Marker::Failed) => self.replan(visible),
                None => {
                    self.pre_output = visible;
                    self.phase = Phase::Executor;
                    Ok(Advance::Continue(self.phase))
                }
            },
            Phase::Executor => {
                self.executor_outputs.push(visible);
                self.phase = Phase::Post;
                Ok(Advance::Continue(self.phase))
            }
            Phase::Post => {
                self.post_outputs.push(visible.clone());
                match detect_marker(output) {
                    Some(Marker::End) => Ok(Advance::Complete),
                    Some(Marker::Failed) => self.replan(visible),
                    None => {
                        if self.post_iterations >= self.limits.max_post_iterations {
                            return Err(OrchestrationError::PostBudgetExceeded {
                                limit: self.limits.max_post_iterations,
                            });
                        }
                        self.post_iterations += 1;
                        Ok(Advance::Continue(Phase::Post))
                    }
                }
            }
        }
    }

    /// Returns the latest Pre analysis.
    pub fn pre_output(&self) -> &str {
        &self.pre_output
    }

    /// Returns all Executor outputs retained for Post review.
    pub fn executor_outputs(&self) -> &[String] {
        &self.executor_outputs
    }

    /// Returns all Post outputs, including the output that caused a replan.
    pub fn post_outputs(&self) -> &[String] {
        &self.post_outputs
    }

    /// Returns complete failed Post or Pre outputs.
    pub fn failure_outputs(&self) -> &[String] {
        &self.failure_outputs
    }

    /// Returns the current Pre replan count.
    pub fn pre_replan_count(&self) -> u32 {
        self.pre_replans
    }

    /// Returns the current Post iteration count.
    pub fn post_iteration_count(&self) -> u32 {
        self.post_iterations
    }

    /// Formats failures for the next Pre prompt.
    pub fn numbered_failures(&self) -> String {
        self.failure_outputs
            .iter()
            .enumerate()
            .map(|(index, output)| format!("第 {} 次失败：\n{output}", index + 1))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn replan(&mut self, failure: String) -> Result<Advance, OrchestrationError> {
        if self.pre_replans >= self.limits.max_pre_replans {
            return Err(OrchestrationError::ReplanBudgetExceeded {
                limit: self.limits.max_pre_replans,
            });
        }
        self.failure_outputs.push(failure);
        self.pre_replans += 1;
        self.pre_output.clear();
        self.post_iterations = 0;
        self.phase = Phase::Pre;
        Ok(Advance::Continue(self.phase))
    }
}
