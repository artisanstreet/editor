use super::{Kind, Mentions, Metrics, Reveal, Row, offered_threads, scroll_offset};

fn row(kind: Kind, name: &str) -> Row {
    Row::new(kind, name.to_owned(), name.to_owned(), String::new())
}

fn mentions() -> Mentions {
    Mentions {
        catalog: vec![
            row(Kind::Thread, "release notes"),
            row(Kind::Skill, "review"),
            row(Kind::Connector, "github"),
            row(Kind::Command, "goal"),
            row(Kind::Skill, "pr"),
            row(Kind::Thread, "scroll bug"),
        ],
        ..Mentions::default()
    }
}

fn shown(mentions: &Mentions) -> Vec<(Kind, &str)> {
    mentions
        .rows
        .iter()
        .map(|index| {
            let row = &mentions.catalog[*index];
            (row.kind, row.name.as_str())
        })
        .collect()
}

fn highlighted(mentions: &Mentions) -> &str {
    &mentions.catalog[mentions.rows[mentions.highlighted]].name
}

#[test]
fn every_sigil_opens_the_same_menu_at_its_own_section() {
    let mut mentions = mentions();
    mentions.sync("see $", 5);
    assert_eq!(
        shown(&mentions),
        [
            (Kind::Skill, "review"),
            (Kind::Skill, "pr"),
            (Kind::Connector, "github"),
            (Kind::Thread, "release notes"),
            (Kind::Thread, "scroll bug"),
        ]
    );
    assert_eq!(highlighted(&mentions), "review");
    assert_eq!(mentions.reveal, Some(Reveal::Section));

    mentions.sync("see @", 5);
    assert_eq!(mentions.rows.len(), 5);
    assert_eq!(highlighted(&mentions), "github");

    mentions.sync("/", 1);
    assert_eq!(shown(&mentions)[0], (Kind::Command, "goal"));
    assert_eq!(highlighted(&mentions), "goal");
}

#[test]
fn a_query_filters_every_section_and_falls_back_to_the_first_match() {
    let mut mentions = mentions();
    mentions.sync("$re", 3);
    assert_eq!(
        shown(&mentions),
        [(Kind::Skill, "review"), (Kind::Thread, "release notes")]
    );
    assert_eq!(highlighted(&mentions), "review");

    mentions.sync("$scroll", 7);
    assert_eq!(shown(&mentions), [(Kind::Thread, "scroll bug")]);
    assert_eq!(highlighted(&mentions), "scroll bug");
    assert_eq!(
        mentions.replacement("$scroll", 0),
        Some((0..7, "@scroll bug ".to_owned()))
    );
}

#[test]
fn hovering_moves_the_highlight_without_asking_to_scroll() {
    let mut mentions = mentions();
    mentions.sync("$", 1);
    mentions.mark_revealed();
    mentions.hover(3, true);
    assert_eq!(mentions.highlighted, 3);
    assert_eq!(mentions.scroll(42.0, METRICS), 42.0);
    mentions.down();
    assert_eq!(mentions.highlighted, 4);
    assert_eq!(mentions.reveal, Some(Reveal::Highlighted));
}

#[test]
fn a_reveal_holds_until_the_menu_shows_it() {
    let mut mentions = mentions();
    mentions.sync("see @", 5);
    let section = mentions.scroll(0.0, METRICS);
    assert!(section > 0.0);
    assert_eq!(mentions.scroll(0.0, METRICS), section);
    mentions.mark_revealed();
    assert_eq!(mentions.scroll(0.0, METRICS), 0.0);
    mentions = Mentions {
        catalog: std::mem::take(&mut mentions.catalog),
        ..Mentions::default()
    };
    mentions.sync("see @", 5);
    assert_eq!(mentions.scroll(0.0, METRICS), section);
}

const METRICS: Metrics = Metrics {
    divider: 10.0,
    heading: 20.0,
    row: 30.0,
    viewport: 100.0,
};

#[test]
fn a_section_reveal_puts_its_heading_at_the_top() {
    let kinds = [
        Kind::Skill,
        Kind::Skill,
        Kind::Skill,
        Kind::Connector,
        Kind::Thread,
        Kind::Thread,
        Kind::Thread,
    ];
    assert_eq!(
        scroll_offset(&kinds, 0, Reveal::Section, 55.0, METRICS),
        0.0
    );
    assert_eq!(
        scroll_offset(&kinds, 3, Reveal::Section, 0.0, METRICS),
        120.0
    );
    assert_eq!(
        scroll_offset(&kinds, 4, Reveal::Section, 0.0, METRICS),
        180.0
    );
}

#[test]
fn a_section_reveal_stops_at_the_end_of_the_list() {
    let kinds = [Kind::Skill, Kind::Skill, Kind::Skill, Kind::Thread];
    assert_eq!(
        scroll_offset(&kinds, 3, Reveal::Section, 0.0, METRICS),
        70.0
    );
}

#[test]
fn a_highlight_reveal_scrolls_only_as_far_as_needed() {
    let kinds = [Kind::Skill; 8];
    assert_eq!(
        scroll_offset(&kinds, 1, Reveal::Highlighted, 0.0, METRICS),
        0.0
    );
    assert_eq!(
        scroll_offset(&kinds, 4, Reveal::Highlighted, 0.0, METRICS),
        70.0
    );
    assert_eq!(
        scroll_offset(&kinds, 2, Reveal::Highlighted, 100.0, METRICS),
        80.0
    );
    assert_eq!(
        scroll_offset(&kinds, 0, Reveal::Highlighted, 100.0, METRICS),
        0.0
    );
}

#[test]
fn keys_keep_the_highlight_until_the_pointer_moves() {
    let mut mentions = mentions();
    mentions.sync("see @", 5);
    let resting = aqua_core::Point::new(10.0, 10.0);
    mentions.pointer_at(Some(resting));
    mentions.hover(3, true);
    assert_eq!(mentions.highlighted, 3);
    mentions.down();
    mentions.hover(2, false);
    mentions.hover(1, true);
    mentions.pointer_at(Some(resting));
    assert_eq!(mentions.highlighted, 4);
    mentions.pointer_at(Some(aqua_core::Point::new(10.0, 40.0)));
    assert_eq!(mentions.highlighted, 1);
    mentions.hover(2, true);
    assert_eq!(mentions.highlighted, 2);
}

#[test]
fn a_catalog_refresh_keeps_the_highlighted_row() {
    let mut mentions = mentions();
    let catalog = std::mem::take(&mut mentions.catalog);
    let threads_only: Vec<Row> = catalog
        .iter()
        .filter(|row| row.kind == Kind::Thread)
        .cloned()
        .collect();
    mentions.set_catalog(threads_only, Vec::new());
    mentions.sync("see @", 5);
    mentions.down();
    assert_eq!(highlighted(&mentions), "scroll bug");
    mentions.mark_revealed();

    mentions.set_catalog(catalog, Vec::new());

    assert_eq!(highlighted(&mentions), "scroll bug");
    assert_eq!(mentions.reveal, Some(Reveal::Highlighted));
}

#[test]
fn an_archived_newer_duplicate_still_offers_the_live_thread_with_that_title() {
    let (offered, titles) = offered_threads([
        ("archived", "release notes", true),
        ("live", "release notes", false),
        ("older live", "release notes", false),
        ("other", "scroll bug", false),
    ]);
    assert_eq!(
        offered,
        [("live", "release notes"), ("other", "scroll bug")]
    );
    assert_eq!(titles, ["release notes", "scroll bug"]);
}
