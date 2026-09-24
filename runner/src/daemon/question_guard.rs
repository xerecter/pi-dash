//! Stop unattended issue runs that keep asking a human for input.
//!
//! Codex can finish an assistant message with a `questions` block (a
//! multiple-choice prompt addressed to the user). In an interactive session
//! someone picks an option; in an issue run nobody is attached, so the model
//! re-asks the same question turn after turn. The turn never completes, the
//! stall watchdog never fires (token-usage frames keep arriving), and the
//! run burns tokens while holding the runner busy — in one observed case for
//! four hours and ~70M input tokens.
//!
//! [`UnansweredQuestionGuard`] watches the issue-run event stream and trips
//! once the agent has asked [`MAX_UNANSWERED_QUESTIONS`] questions, so the
//! supervisor can interrupt the agent and fail the run with the question in
//! the failure detail — which lands on the issue, where a human can answer.
//!
//! Only the issue-run lane consults this guard. Chat lanes have a human on
//! the other end and must keep passing questions through untouched.

use serde_json::Value;

/// How many questions an unattended run may ask before it is stopped. More
/// than one, so a single "should I proceed?" that the agent then answers
/// itself doesn't kill an otherwise healthy run.
pub const MAX_UNANSWERED_QUESTIONS: u32 = 3;

/// Upper bound on the question text carried into the failure detail.
const QUESTION_TEXT_MAX_CHARS: usize = 500;

/// Emitted when the guard trips.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionTrip {
    /// Number of questions asked, including the one that tripped the guard.
    pub asked: u32,
    /// Human-readable rendering of the most recent question.
    pub last_question: String,
}

impl QuestionTrip {
    /// Failure detail sent to the cloud, which posts it on the issue.
    pub fn failure_detail(&self) -> String {
        format!(
            "agent asked for human input {} times during an unattended run; \
             stopping instead of waiting for an answer that cannot arrive. \
             Answer on the issue (or refine it) and re-run.\n\nLast question:\n{}",
            self.asked, self.last_question
        )
    }
}

#[derive(Debug)]
pub struct UnansweredQuestionGuard {
    limit: u32,
    asked: u32,
}

impl Default for UnansweredQuestionGuard {
    fn default() -> Self {
        Self::new(MAX_UNANSWERED_QUESTIONS)
    }
}

impl UnansweredQuestionGuard {
    /// A guard that trips on the `limit`-th question. `limit` is clamped to
    /// at least 1.
    pub fn new(limit: u32) -> Self {
        Self {
            limit: limit.max(1),
            asked: 0,
        }
    }

    pub fn asked(&self) -> u32 {
        self.asked
    }

    /// Feed one raw agent frame. Returns `Some` exactly once: on the frame
    /// that brings the question count to the limit.
    pub fn observe(&mut self, method: &str, params: &Value) -> Option<QuestionTrip> {
        let question = question_from_frame(method, params)?;
        self.asked = self.asked.saturating_add(1);
        (self.asked == self.limit).then_some(QuestionTrip {
            asked: self.asked,
            last_question: question,
        })
    }
}

/// Extract the question an agent asked from a Codex `item/completed` frame
/// whose `agentMessage` item carries a non-empty `questions` array.
///
/// Only `item/completed` counts: Codex sends the same item on
/// `item/started` too, and counting both would double every question.
pub fn question_from_frame(method: &str, params: &Value) -> Option<String> {
    if method != "item/completed" {
        return None;
    }
    let item = params.get("item")?;
    if item.get("type").and_then(Value::as_str) != Some("agentMessage") {
        return None;
    }
    let questions = item.get("questions")?.as_array()?;
    if questions.is_empty() {
        return None;
    }
    let text = item
        .get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| render_questions(questions));
    Some(truncate_chars(&text, QUESTION_TEXT_MAX_CHARS))
}

/// Fallback rendering when the message has no `text`: each question's title
/// followed by its options as a bullet list.
fn render_questions(questions: &[Value]) -> String {
    let mut out = Vec::new();
    for q in questions {
        if let Some(title) = q.get("title").and_then(Value::as_str) {
            out.push(title.to_string());
        }
        for opt in q
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            out.push(format!("- {opt}"));
        }
    }
    if out.is_empty() {
        "(question text unavailable)".to_string()
    } else {
        out.join("\n")
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut t: String = s.chars().take(max).collect();
    t.push('…');
    t
}
