use super::super::Span;
use artisan_mentions::MentionKind;

#[test]
fn a_rendered_mention_hides_its_sigil_under_a_glyph_but_keeps_it_in_the_text() {
    let theme = crate::ui::theme::ArtisanTheme::resolved();
    let spans = vec![
        Span::Text("use ".to_owned()),
        Span::MentionLead {
            kind: MentionKind::Skill,
            name: "re".to_owned(),
        },
        Span::Strong(vec![Span::MentionTail {
            kind: MentionKind::Skill,
            text: "view".to_owned(),
        }]),
    ];
    let prepared = super::super::visual::prepare_inline(
        &spans,
        &theme,
        aqua_text::document::SelectionSourceId(1),
        vec![0],
        "\n",
    );
    let text = &prepared.source.text;
    assert_eq!(&**text, "use $review");
    assert_eq!(
        prepared
            .mentions
            .iter()
            .map(|mention| text.get(mention.glyph.clone()))
            .collect::<Vec<_>>(),
        [Some("$")]
    );
    let cells: Vec<_> = prepared
        .runs
        .iter()
        .filter(|run| run.style.cell == super::super::visual::InlineCell::MentionGlyph)
        .map(|run| text.get(run.range.clone()))
        .collect();
    assert_eq!(cells, [Some("$")]);
    let mention_color = prepared
        .runs
        .iter()
        .find(|run| run.style.cell == super::super::visual::InlineCell::MentionGlyph)
        .and_then(|run| run.style.color);
    assert!(
        prepared
            .runs
            .iter()
            .filter(|run| text.get(run.range.clone()).is_some_and(|run| run != "$"))
            .all(|run| run.style.color == mention_color),
        "every piece of the mention shares its colour"
    );
}

#[test]
fn an_empty_mention_lead_still_shows_its_sigil_glyph() {
    let theme = crate::ui::theme::ArtisanTheme::resolved();
    let spans = vec![Span::MentionLead {
        kind: MentionKind::Thread,
        name: String::new(),
    }];
    let prepared = super::super::visual::prepare_inline(
        &spans,
        &theme,
        aqua_text::document::SelectionSourceId(1),
        vec![0],
        "\n",
    );
    assert_eq!(&*prepared.source.text, "@");
    assert_eq!(prepared.mentions.len(), 1);
}
