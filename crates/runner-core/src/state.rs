// Copyright 2026 Kotelnikovekb
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://apache.org
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Received,
    Preparing,
    Running,
    Collecting,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
    Destroying,
    Destroyed,
}
impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Received => "received",
            Self::Preparing => "preparing",
            Self::Running => "running",
            Self::Collecting => "collecting",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::Destroying => "destroying",
            Self::Destroyed => "destroyed",
        }
    }
}
impl State {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "completed" => Self::Completed,
            "cancelled" => Self::Cancelled,
            "timed_out" => Self::TimedOut,
            _ => Self::Failed,
        }
    }
}
#[allow(dead_code)]
pub fn valid_transition(from: State, to: State) -> bool {
    matches!(
        (from, to),
        (State::Received, State::Preparing)
            | (State::Preparing, State::Running)
            | (State::Preparing, State::Failed)
            | (State::Running, State::Collecting)
            | (State::Running, State::Cancelled)
            | (State::Running, State::TimedOut)
            | (State::Running, State::Failed)
            | (State::Collecting, State::Completed)
            | (State::Collecting, State::Failed)
            | (State::Completed, State::Destroying)
            | (State::Failed, State::Destroying)
            | (State::Cancelled, State::Destroying)
            | (State::TimedOut, State::Destroying)
            | (State::Destroying, State::Destroyed)
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle_is_explicit() {
        assert!(valid_transition(State::Received, State::Preparing));
        assert!(!valid_transition(State::Received, State::Running));
    }
}
