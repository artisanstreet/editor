use super::super::presenter::MarkdownPresenter;
use super::super::{
    Block, IndexedLeafContent, MarkdownEngine, MessageMarks, NoFenceTokenMemo, Span,
};
use super::ProjectedDocument;
use artisan_mentions::{MentionKind, MessageVocabulary};
use std::sync::Arc;

#[derive(Debug, PartialEq, Eq)]
enum Piece {
    Plain(String),
    Code(String),
    Command(String),
    Mention(MentionKind, String),
    Continued(MentionKind, String),
}

fn pieces(spans: &[Span], into: &mut Vec<Piece>) {
    for span in spans {
        match span {
            Span::Text(text) | Span::Html(text) => into.push(Piece::Plain(text.clone())),
            Span::Code(text) => into.push(Piece::Code(text.clone())),
            Span::Command(text) => into.push(Piece::Command(text.clone())),
            Span::MentionLead { kind, name } => {
                into.push(Piece::Mention(*kind, format!("{}{name}", kind.sigil())));
            }
            Span::MentionTail { kind, text } => into.push(Piece::Continued(*kind, text.clone())),
            Span::Emphasis(inner) | Span::Strong(inner) => pieces(inner, into),
            Span::Link { label, .. } => pieces(label, into),
        }
    }
}

fn vocabulary() -> Arc<MessageVocabulary> {
    Arc::new(MessageVocabulary::new(
        &["review", "workflow"],
        &[
            "Orchestartor and GPT OSS (Medium)",
            "Sidebar lag",
            "Fix **sidebar**",
            "Fix | sidebar",
            "Read https://example.com",
        ],
    ))
}

fn leaves(source: &str, vocabulary: &Arc<MessageVocabulary>) -> (Vec<Vec<Piece>>, Vec<u64>) {
    let engine = MarkdownEngine::new().expect("markdown engine");
    let marks = MessageMarks::new(source, vocabulary);
    let projected = ProjectedDocument::new(&engine, source, Some(&marks), &mut NoFenceTokenMemo)
        .expect("projected message");
    let contents = projected
        .leaves
        .iter()
        .map(|leaf| {
            let mut found = Vec::new();
            match &leaf.content {
                IndexedLeafContent::Block(
                    Block::Paragraph { spans, .. } | Block::Heading { spans, .. },
                ) => pieces(spans, &mut found),
                IndexedLeafContent::Block(Block::Code(fence)) => {
                    found.push(Piece::Code(fence.source.clone()));
                }
                IndexedLeafContent::TableRow { cells, .. } => {
                    for cell in cells {
                        pieces(&cell.spans, &mut found);
                    }
                }
                IndexedLeafContent::Block(_) => {}
            }
            found
        })
        .collect();
    let signatures = projected.leaves.iter().map(|leaf| leaf.signature).collect();
    (contents, signatures)
}

fn projected(source: &str) -> Vec<Vec<Piece>> {
    leaves(source, &vocabulary()).0
}

fn plain(text: &str) -> Piece {
    Piece::Plain(text.to_owned())
}

fn mention(kind: MentionKind, text: &str) -> Piece {
    Piece::Mention(kind, text.to_owned())
}

fn marked(source: &str) -> bool {
    projected(source)
        .iter()
        .flatten()
        .any(|piece| matches!(piece, Piece::Mention(..) | Piece::Continued(..)))
}

#[test]
fn a_thread_title_with_spaces_and_parentheses_is_one_mention() {
    assert_eq!(
        projected("Ask @Orchestartor and GPT OSS (Medium) about it"),
        [vec![
            plain("Ask "),
            mention(MentionKind::Thread, "@Orchestartor and GPT OSS (Medium)"),
            plain(" about it"),
        ]]
    );
}

#[test]
fn an_unknown_skill_stays_plain_text() {
    assert_eq!(
        projected("run $foo then $reviewer"),
        [vec![plain("run $foo then $reviewer")]]
    );
}

#[test]
fn mentions_inside_code_spans_fences_and_links_stay_literal() {
    assert_eq!(
        projected(
            "try ` $review` and [see $review](https://example.com)\n\n```\n$workflow @Sidebar lag\n```"
        ),
        [
            vec![
                plain("try "),
                Piece::Code(" $review".to_owned()),
                plain(" and "),
                plain("see $review"),
            ],
            vec![Piece::Code("$workflow @Sidebar lag\n".to_owned())],
        ]
    );
}

#[test]
fn two_mentions_after_a_command_are_both_marked() {
    assert_eq!(
        projected("/goal use $review with @Sidebar lag, then rest"),
        [vec![
            Piece::Command("/goal".to_owned()),
            plain(" use "),
            mention(MentionKind::Skill, "$review"),
            plain(" with "),
            mention(MentionKind::Thread, "@Sidebar lag"),
            plain(", then rest"),
        ]]
    );
}

#[test]
fn any_leading_slash_command_is_marked_but_paths_stay_plain() {
    assert_eq!(
        projected("/compact\n\nAnd use the real color token values"),
        [
            vec![Piece::Command("/compact".to_owned())],
            vec![plain("And use the real color token values")],
        ]
    );
    assert_eq!(
        projected("/home/sander is full"),
        [vec![plain("/home/sander is full")]]
    );
}

#[test]
fn a_sigil_counts_only_after_whitespace_in_the_source_as_in_the_composer() {
    for source in [
        "foo**$review**",
        "`x`$review",
        "**a**@Sidebar lag",
        "\\$review",
        "**$review**",
    ] {
        assert!(!marked(source), "{source}");
    }
}

#[test]
fn a_title_with_markdown_punctuation_stays_one_mention_across_its_nodes() {
    assert_eq!(
        projected("see @Fix **sidebar**"),
        [vec![
            plain("see "),
            mention(MentionKind::Thread, "@Fix "),
            Piece::Continued(MentionKind::Thread, "sidebar".to_owned()),
        ]]
    );
}

#[test]
fn table_cells_mark_plain_mentions_but_not_code() {
    assert_eq!(
        projected("| skill | code |\n| --- | --- |\n| use $review | `$review` |"),
        [
            vec![plain("skill"), plain("code")],
            vec![
                plain("use "),
                mention(MentionKind::Skill, "$review"),
                Piece::Code("$review".to_owned()),
            ],
        ]
    );
}

#[test]
fn a_prepared_message_gains_and_loses_its_chip_with_the_vocabulary() {
    let source = "ask @Sidebar lag";
    let known = vocabulary();
    let unknown = Arc::new(MessageVocabulary::default());
    let (with, signed) = leaves(source, &known);
    let (without, unsigned) = leaves(source, &unknown);
    assert_eq!(
        with,
        [vec![
            plain("ask "),
            mention(MentionKind::Thread, "@Sidebar lag")
        ]]
    );
    assert_eq!(without, [vec![plain("ask @Sidebar lag")]]);
    assert_ne!(signed, unsigned);

    let engine = MarkdownEngine::new().expect("markdown engine");
    let mut presenter = MarkdownPresenter::default();
    presenter
        .prepare_message(&engine, source, &known)
        .expect("prepared with the chip");
    let chipped = presenter.projected_leaf_info(0).expect("leaf");
    presenter
        .prepare_message(&engine, source, &unknown)
        .expect("prepared without the chip");
    assert!(!presenter.projected_leaf_is_current(0, &chipped));
    let plain_leaf = presenter.projected_leaf_info(0).expect("leaf");
    presenter
        .prepare_message(&engine, source, &known)
        .expect("prepared with the chip again");
    assert!(!presenter.projected_leaf_is_current(0, &plain_leaf));
}

#[test]
fn a_title_never_spans_table_cells() {
    assert!(!marked("| a | b |\n| --- | --- |\n| @Fix | sidebar |"));
}

#[test]
fn a_mention_inside_inline_html_stays_literal_but_one_after_it_is_marked() {
    assert!(!marked("see <span> $review </span> here"));
    assert_eq!(
        projected("see <span>x</span> $review"),
        [vec![
            plain("see "),
            Piece::Plain("<span>".to_owned()),
            plain("x"),
            Piece::Plain("</span>".to_owned()),
            plain(" "),
            mention(MentionKind::Skill, "$review"),
        ]]
    );
}

#[test]
fn a_bare_url_inside_a_title_stays_a_link() {
    assert!(!marked("open @Read https://example.com now"));
    let engine = MarkdownEngine::new().expect("markdown engine");
    let source = "open @Read https://example.com now";
    let marks = MessageMarks::new(source, &vocabulary());
    let projected = ProjectedDocument::new(&engine, source, Some(&marks), &mut NoFenceTokenMemo)
        .expect("projected message");
    let IndexedLeafContent::Block(Block::Paragraph { spans, .. }) = &projected.leaves[0].content
    else {
        panic!("a paragraph");
    };
    let flat = super::super::inline::flatten_inline_with_swatches(spans);
    assert_eq!(
        flat.links
            .iter()
            .map(|link| link.destination.as_str())
            .collect::<Vec<_>>(),
        ["https://example.com"]
    );
}

#[test]
fn compact_and_padded_table_cells_mark_their_own_mentions() {
    for row in ["|$review|other|", "| $review | other |", "|other|$review|"] {
        let source = format!("| a | b |\n| --- | --- |\n{row}");
        assert!(
            projected(&source)[1]
                .iter()
                .any(|piece| *piece == mention(MentionKind::Skill, "$review")),
            "{row}"
        );
    }
}

#[test]
fn an_unclosed_generic_does_not_hide_the_mentions_after_it() {
    for source in [
        "takes a Vec<String> and $review",
        "an Option<T> then $review",
        "<b> $review </i>",
    ] {
        assert!(marked(source), "{source}");
    }
}

#[test]
fn a_custom_inline_element_hides_the_mentions_inside_it() {
    assert!(!marked("see <img-caption> $review </img-caption> there"));
    assert!(!marked("see <span><b> $review </b></span> there"));
    assert!(marked("see <img-caption>x</img-caption> $review"));
}

#[test]
fn a_quoted_mention_gains_and_loses_its_chip_with_the_vocabulary() {
    use super::super::presenter::MentionRevision;
    let source = ">$review";
    let known = vocabulary();
    assert!(marked(source));
    let engine = MarkdownEngine::new().expect("markdown engine");
    let mut presenter = MarkdownPresenter::default();
    presenter
        .prepare_message(&engine, source, &known)
        .expect("prepared with the chip");
    let unknown = Arc::new(MessageVocabulary::new(&["workflow"], &["Sidebar lag"]));
    assert_eq!(
        presenter.revise_mentions(&unknown).expect("revised"),
        MentionRevision::Remarked
    );
    let unrelated = Arc::new(MessageVocabulary::new(
        &["review", "other"],
        &["Sidebar lag"],
    ));
    assert_eq!(
        presenter.revise_mentions(&unrelated).expect("revised"),
        MentionRevision::Unchanged
    );
    let chipped = presenter.projected_leaf_info(0).expect("leaf");
    presenter
        .prepare_message(&engine, source, &unrelated)
        .expect("kept");
    assert!(presenter.projected_leaf_is_current(0, &chipped));
}

#[test]
fn links_and_html_stay_inside_their_own_table_cell() {
    for row in [
        "|https://example.com/path|$review|",
        "|<span>| $review |</span>|",
    ] {
        let source = format!("| a | b | c |\n| --- | --- | --- |\n{row}");
        assert!(
            projected(&source)[1]
                .iter()
                .any(|piece| *piece == mention(MentionKind::Skill, "$review")),
            "{row}"
        );
    }
}

#[test]
fn a_title_that_crosses_code_gives_way_to_the_mentions_inside_it() {
    let source = "ask @Read `log` then $review";
    let titles = Arc::new(MessageVocabulary::new(
        &["review"],
        &["Read `log` then $review", "Read"],
    ));
    let pieces = leaves(source, &titles).0;
    assert_eq!(
        pieces,
        [vec![
            plain("ask "),
            mention(MentionKind::Thread, "@Read"),
            plain(" "),
            Piece::Code("log".to_owned()),
            plain(" then "),
            mention(MentionKind::Skill, "$review"),
        ]]
    );
}

#[test]
fn many_schemes_without_a_host_still_leave_the_mention_marked() {
    let source = format!("{} $review", "http:///".repeat(20_000));
    assert!(marked(&source));
}

#[test]
fn an_underscore_host_still_links_and_keeps_a_mention_beside_it() {
    let engine = MarkdownEngine::new().expect("markdown engine");
    let source = "open http://_internal/path then $review";
    let marks = MessageMarks::new(source, &vocabulary());
    let projected = ProjectedDocument::new(&engine, source, Some(&marks), &mut NoFenceTokenMemo)
        .expect("projected message");
    let IndexedLeafContent::Block(Block::Paragraph { spans, .. }) = &projected.leaves[0].content
    else {
        panic!("a paragraph");
    };
    let flat = super::super::inline::flatten_inline_with_swatches(spans);
    assert_eq!(
        flat.links
            .iter()
            .map(|link| link.destination.as_str())
            .collect::<Vec<_>>(),
        ["http://_internal/path"]
    );
    assert!(marked(source));
    assert!(!marked("open http://_internal/$review"));
}
