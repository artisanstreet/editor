use super::*;
#[test]
fn a_message_starting_with_a_command_names_it_and_a_path_does_not() {
    for (text, command) in [
        ("/goal ship it", Some("/goal")),
        ("  /ultracode build", Some("/ultracode")),
        ("/compact", Some("/compact")),
        ("/cloudflare:wrangler deploy", Some("/cloudflare:wrangler")),
        ("/home/sander is full", None),
        ("/ alone", None),
        ("see /goal later", None),
        ("/goal, now", None),
    ] {
        assert_eq!(
            leading_command(text).and_then(|range| text.get(range)),
            command,
            "{text}"
        );
    }
}
#[test]
fn thread_titles_with_spaces_take_the_longest_match_at_a_word_end() {
    let text = "see @Fix sidebar lag, then @Fix and email@Fix or @Fixed";
    let mentions = ThreadTitles::new(&["Fix", "Fix sidebar lag", "Fix sidebar"]).mentions(text);
    assert_eq!(
        mentions
            .iter()
            .map(|mention| text.get(mention.range.clone()))
            .collect::<Vec<_>>(),
        [Some("@Fix sidebar lag"), Some("@Fix")]
    );
}
#[test]
fn titles_sharing_prefixes_match_the_longest_title_that_ends_at_a_word_end() {
    let text = "@abc d, @ab. @abd @abx @a";
    let titles = ThreadTitles::new(&["a", "ab", "abc d", "abd", " padded", "two\nlines"]);
    assert_eq!(
        titles
            .mentions(text)
            .iter()
            .map(|mention| text.get(mention.range.clone()))
            .collect::<Vec<_>>(),
        [Some("@abc d"), Some("@ab"), Some("@abd"), Some("@a")]
    );
}
#[test]
fn qualified_names_and_punctuation_share_one_longest_match() {
    let text = "$docs.web, then $docs. Again $docs.web!";
    let mentions = skill_mentions(text, &["docs", "docs.web"]);
    assert_eq!(
        mentions
            .iter()
            .map(|mention| mention.name.as_str())
            .collect::<Vec<_>>(),
        ["docs.web", "docs", "docs.web"]
    );
    assert!(
        mentions
            .iter()
            .all(|mention| text.get(mention.range.clone())
                == Some(format!("${}", mention.name).as_str()))
    );
}
#[test]
fn unicode_byte_ranges_and_word_boundaries_are_stable() {
    let text = "é $review; email$review $reviewer $🦀";
    assert_eq!(
        skill_mentions(text, &["review", "🦀"])
            .iter()
            .map(|mention| mention.name.as_str())
            .collect::<Vec<_>>(),
        ["review", "🦀"]
    );
}
#[test]
fn long_unmatched_tokens_and_duplicate_names_remain_bounded() {
    let text = format!("${} $short", "a.".repeat(100_000));
    assert_eq!(skill_mentions(&text, &["short", "short"]).len(), 1);
}

#[test]
fn a_message_names_known_skills_and_threads_and_leaves_the_rest_plain() {
    let vocabulary = MessageVocabulary::new(
        &["review", "workflow"],
        &["Orchestartor and GPT OSS (Medium)", "Fix $review flow"],
    );
    let text =
        "Ask @Orchestartor and GPT OSS (Medium), then $review and $foo; see @Fix $review flow";
    let mentions: Vec<_> = vocabulary
        .mentions(text)
        .into_iter()
        .map(|mention| (mention.kind, text.get(mention.range)))
        .collect();
    assert_eq!(
        mentions,
        [
            (
                MentionKind::Thread,
                Some("@Orchestartor and GPT OSS (Medium)")
            ),
            (MentionKind::Skill, Some("$review")),
            (MentionKind::Thread, Some("@Fix $review flow")),
        ]
    );
}
