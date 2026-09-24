//! Unit coverage for `daemon::question_guard`: which Codex frames count as
//! a question, when the guard trips, and what lands in the failure detail.

use pidash::daemon::question_guard::{
    MAX_UNANSWERED_QUESTIONS, UnansweredQuestionGuard, question_from_frame,
};
use serde_json::{Value, json};

fn question_item(text: Option<&str>) -> Value {
    let mut item = json!({
        "id": "call_1",
        "type": "agentMessage",
        "phase": "final_answer",
        "delivery": "async",
        "questions": [{"title": "确认", "options": ["按 Backlog 规则结束", "开始实现门户网站"]}],
    });
    if let Some(t) = text {
        item["text"] = json!(t);
    }
    json!({ "item": item })
}

#[test]
fn completed_agent_message_with_questions_is_a_question() {
    let q = question_from_frame(
        "item/completed",
        &question_item(Some("确认\n- 结束\n- 实现")),
    );
    assert_eq!(q.as_deref(), Some("确认\n- 结束\n- 实现"));
}

#[test]
fn item_started_is_ignored_so_questions_are_not_double_counted() {
    assert_eq!(
        question_from_frame("item/started", &question_item(Some("x"))),
        None
    );
}

#[test]
fn plain_agent_message_is_not_a_question() {
    let params = json!({"item": {"type": "agentMessage", "text": "working on it"}});
    assert_eq!(question_from_frame("item/completed", &params), None);
}

#[test]
fn empty_questions_array_is_not_a_question() {
    let params = json!({"item": {"type": "agentMessage", "text": "hi", "questions": []}});
    assert_eq!(question_from_frame("item/completed", &params), None);
}

#[test]
fn non_agent_message_items_are_ignored() {
    let params = json!({"item": {"type": "commandExecution", "questions": [{"title": "x"}]}});
    assert_eq!(question_from_frame("item/completed", &params), None);
}

#[test]
fn missing_text_falls_back_to_rendered_questions() {
    let q = question_from_frame("item/completed", &question_item(None)).unwrap();
    assert_eq!(q, "确认\n- 按 Backlog 规则结束\n- 开始实现门户网站");
}

#[test]
fn long_question_text_is_truncated() {
    let long = "問".repeat(2_000);
    let q = question_from_frame("item/completed", &question_item(Some(&long))).unwrap();
    assert!(q.chars().count() <= 501, "got {} chars", q.chars().count());
    assert!(q.ends_with('…'));
}

#[test]
fn guard_trips_exactly_once_at_the_limit() {
    let mut guard = UnansweredQuestionGuard::default();
    let frame = question_item(Some("confirm?"));
    for n in 1..MAX_UNANSWERED_QUESTIONS {
        assert_eq!(
            guard.observe("item/completed", &frame),
            None,
            "tripped early at {n}"
        );
    }
    let trip = guard
        .observe("item/completed", &frame)
        .expect("guard must trip at the limit");
    assert_eq!(trip.asked, MAX_UNANSWERED_QUESTIONS);
    assert_eq!(trip.last_question, "confirm?");
    // Already reported; later questions must not re-fire the failure path.
    assert_eq!(guard.observe("item/completed", &frame), None);
    assert_eq!(guard.asked(), MAX_UNANSWERED_QUESTIONS + 1);
}

#[test]
fn unrelated_frames_do_not_advance_the_guard() {
    let mut guard = UnansweredQuestionGuard::new(1);
    guard.observe("thread/tokenUsage/updated", &json!({}));
    guard.observe("item/started", &question_item(Some("x")));
    assert_eq!(guard.asked(), 0);
    assert!(
        guard
            .observe("item/completed", &question_item(Some("x")))
            .is_some()
    );
}

#[test]
fn zero_limit_is_clamped_to_one() {
    let mut guard = UnansweredQuestionGuard::new(0);
    assert!(
        guard
            .observe("item/completed", &question_item(Some("x")))
            .is_some()
    );
}

#[test]
fn failure_detail_names_the_count_and_the_question() {
    let mut guard = UnansweredQuestionGuard::new(1);
    let trip = guard
        .observe(
            "item/completed",
            &question_item(Some("Proceed with PIDAS-10?")),
        )
        .unwrap();
    let detail = trip.failure_detail();
    assert!(detail.contains("1 times"), "{detail}");
    assert!(detail.contains("Proceed with PIDAS-10?"), "{detail}");
}
