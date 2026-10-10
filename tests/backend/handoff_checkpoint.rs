use artisan_database::{HandoffRole, HandoffTranscriptEntry};

use super::{
    ENTRY_MAX_BYTES, SUMMARY_MAX_BYTES, accept_summary, compaction_prompt, mechanical_summary,
    render_context, split,
};

fn entry(role: HandoffRole, text: &str) -> HandoffTranscriptEntry {
    HandoffTranscriptEntry {
        role,
        text: text.to_owned(),
    }
}

fn conversation(turns: usize, reply_bytes: usize) -> Vec<HandoffTranscriptEntry> {
    (0..turns)
        .flat_map(|turn| {
            [
                entry(HandoffRole::User, &format!("question {turn}")),
                entry(HandoffRole::Assistant, &"a".repeat(reply_bytes)),
            ]
        })
        .collect()
}

#[test]
fn a_short_thread_travels_whole_and_says_so() {
    let entries = conversation(3, 100);
    let split = split(&entries);
    assert!(split.head.is_empty());
    assert_eq!(split.tail.len(), 6);
    let context = render_context(&mechanical_summary(None, 0), &split.tail);
    assert!(context.contains("Complete canonical transcript."));
    assert!(context.contains("[historical user]\nquestion 0\n[/historical user]"));
    assert!(context.contains("[historical user]\nquestion 2\n[/historical user]"));
    assert!(context.ends_with("--- END UNTRUSTED HISTORICAL CONTEXT ---"));
}

#[test]
fn a_long_thread_keeps_its_newest_entries_and_summarizes_the_rest() {
    // 40 turns of 8 KiB replies overflow the 120 KiB tail.
    let entries = conversation(40, 8 * 1024);
    let split = split(&entries);
    assert!(!split.head.is_empty());
    assert_eq!(split.head.len() + split.tail.len(), entries.len());
    // The head is exactly the oldest entries: the tail starts right after it.
    assert_eq!(split.head.first(), entries.first());
    let context = render_context("summary", &split.tail);
    assert!(context.contains("question 39"));
    assert!(!context.contains("question 0\n"));
}

#[test]
fn an_oversized_entry_is_truncated_not_dropped() {
    let entries = vec![entry(HandoffRole::Assistant, &"é".repeat(ENTRY_MAX_BYTES))];
    let split = split(&entries);
    assert_eq!(split.tail.len(), 1);
    let context = render_context("summary", &split.tail);
    assert!(context.contains("[entry truncated]"));
}

#[test]
fn the_compaction_prompt_carries_the_head_and_the_template() {
    let head = conversation(2, 10);
    let prompt = compaction_prompt(&head, 0);
    assert!(prompt.contains("--- BEGIN UNTRUSTED CONVERSATION TRANSCRIPT ---"));
    assert!(prompt.contains("[user]\nquestion 0\n"));
    assert!(prompt.contains("## Objective"));
    assert!(prompt.contains("## Relevant Files"));
    assert!(!prompt.contains("omitted for size"));
}

#[test]
fn the_compaction_prompt_names_entries_it_could_not_include() {
    let head = conversation(1, 10);
    let prompt = compaction_prompt(&head, 3);
    assert!(prompt.contains("3 earlier transcript entries were omitted for size."));
    let prompt = compaction_prompt(&head, 1);
    assert!(prompt.contains("1 earlier transcript entry was omitted for size."));
}

#[test]
fn the_mechanical_summary_names_the_objective_and_the_omission() {
    let summary = mechanical_summary(Some("make the Forge reliable"), 12);
    assert!(summary.starts_with("Canonical transcript fallback."));
    assert!(summary.contains("make the Forge reliable"));
    assert!(
        summary.ends_with("12 earlier transcript entries were not included in the retained tail.")
    );
    let without = mechanical_summary(None, 1);
    assert!(without.contains("No user objective"));
    assert!(without.ends_with("1 earlier transcript entry was not included in the retained tail."));
    let long = mechanical_summary(Some(&"x".repeat(SUMMARY_MAX_BYTES * 2)), 2);
    assert!(long.len() <= SUMMARY_MAX_BYTES);
}

#[test]
fn a_summary_must_be_non_empty_and_bounded() {
    assert_eq!(
        accept_summary("  ## Objective\n- x \n").as_deref(),
        Some("## Objective\n- x")
    );
    assert_eq!(accept_summary("   \n"), None);
    assert_eq!(accept_summary(&"x".repeat(SUMMARY_MAX_BYTES + 1)), None);
}

#[test]
fn the_history_precedes_and_yields_to_the_request() {
    let handoff = crate::engine_owner::EngineHandoff::new(render_context("s", &[]));
    let framed = handoff.frame("/goal ship it");
    let request = framed
        .find("--- BEGIN CURRENT USER REQUEST ---\n/goal ship it\n--- END CURRENT USER REQUEST ---")
        .expect("the request is framed");
    let history = framed
        .find("--- END UNTRUSTED HISTORICAL CONTEXT ---")
        .expect("the history is present");
    assert!(history < request);
    assert!(framed.starts_with("The current user content following this block takes precedence"));
}
