use super::*;

fn target(name: &str) -> MentionTarget {
    MentionTarget {
        name: name.to_owned(),
        description: String::new(),
    }
}

fn catalog() -> MentionCatalog {
    MentionCatalog::new(
        vec![target("apple-design"), target("animation-vocabulary")],
        vec![
            target("claude.ai Notion"),
            target("claude"),
            target("sentry"),
        ],
        vec![GOAL_COMMAND],
    )
}

fn ranges(text: &str) -> Vec<(&str, MentionKind)> {
    recognize_mentions(text, &catalog())
        .into_iter()
        .map(|span| (&text[span.range], span.kind))
        .collect()
}

#[test]
fn known_names_after_a_sigil_are_mentions() {
    assert_eq!(
        ranges("$apple-design then ask @sentry, please"),
        vec![
            ("$apple-design", MentionKind::Skill),
            ("@sentry", MentionKind::Connector),
        ]
    );
}

#[test]
fn unknown_names_and_mid_word_sigils_stay_text() {
    assert!(ranges("costs $5 or a@sentry or $apple-designer").is_empty());
    assert!(ranges("src/goal and /usr/bin").is_empty());
}

#[test]
fn the_longest_known_name_wins() {
    assert_eq!(
        ranges("use @claude.ai Notion now"),
        vec![("@claude.ai Notion", MentionKind::Connector)]
    );
    assert_eq!(
        ranges("use @claude now"),
        vec![("@claude", MentionKind::Connector)]
    );
}

#[test]
fn goal_counts_only_at_the_start() {
    assert_eq!(
        ranges("/goal ship it"),
        vec![("/goal", MentionKind::Command)]
    );
    assert!(ranges("please /goal ship it").is_empty());
    assert!(ranges("/goals").is_empty());
}

#[test]
fn a_trigger_follows_the_word_the_caret_types() {
    let text = "hello $ani world";
    let caret = "hello $ani".len();
    assert_eq!(
        mention_trigger(text, caret),
        Some(MentionTrigger {
            sigil: '$',
            range: 6..10,
            query: "ani".to_owned(),
        })
    );
    assert_eq!(
        mention_trigger("/", 1).map(|trigger| trigger.query),
        Some(String::new())
    );
    assert_eq!(mention_trigger("hello", 5), None);
    assert_eq!(mention_trigger("a/b", 3), None);
    assert_eq!(mention_trigger("$x y", 4), None);
}

#[test]
fn a_trigger_needs_the_caret_after_its_sigil() {
    assert_eq!(mention_trigger("@sentry", 0), None);
    let trigger = mention_trigger("@sentry", 3).expect("trigger");
    assert_eq!(trigger.range, 0..7);
    assert_eq!(trigger.query, "se");
}

#[test]
fn deleting_at_a_mention_removes_it_whole() {
    let text = "a $apple-design b";
    let spans = recognize_mentions(text, &catalog());
    assert_eq!(mention_deletion(&spans, 15, false), Some(2..15));
    assert_eq!(mention_deletion(&spans, 2, true), Some(2..15));
    assert_eq!(mention_deletion(&spans, 7, false), Some(2..15));
    assert_eq!(mention_deletion(&spans, 16, false), None);
}

#[test]
fn arrows_step_over_a_mention() {
    let text = "a $apple-design b";
    let spans = recognize_mentions(text, &catalog());
    assert_eq!(step_over_mention(&spans, 14, false), 2);
    assert_eq!(step_over_mention(&spans, 3, true), 15);
    assert_eq!(step_over_mention(&spans, 16, true), 16);
}

#[test]
fn catalogs_drop_blank_names_and_repeats() {
    let catalog = MentionCatalog::new(
        vec![
            target("b"),
            target(""),
            target("a"),
            target("b"),
            target(" x"),
        ],
        Vec::new(),
        Vec::new(),
    );
    let names: Vec<&str> = catalog
        .skills()
        .iter()
        .map(|target| target.name.as_str())
        .collect();
    assert_eq!(names, ["a", "b"]);
}

#[test]
fn the_engine_catalog_joins_placed_skills_with_what_the_engine_reported() {
    use artisan_domain::{
        ConnectorStatus, EngineInventory, Extension, ExtensionCatalog, ExtensionEngineState,
        ExtensionEngineStatus, ExtensionKind, ExtensionLinkSupport, ExtensionOrigin,
        InventoryConnector, InventorySkill, InventorySkillSource,
    };

    let placed = |name: &str, claude: ExtensionEngineStatus| Extension {
        id: format!("artisan.{name}"),
        kind: ExtensionKind::Skill,
        origin: ExtensionOrigin::FirstParty,
        name: name.to_owned(),
        description: format!("About {name}"),
        digest: "a".repeat(64),
        file_count: 1,
        byte_count: 1,
        enabled: true,
        engines: vec![ExtensionEngineState {
            engine_id: "claude".to_owned(),
            status: claude,
            reason: None,
        }],
    };
    let extensions = ExtensionCatalog::new(
        vec![
            placed("artisan-editor", ExtensionEngineStatus::Synced),
            placed("turned-off", ExtensionEngineStatus::Off),
        ],
        ExtensionLinkSupport::Available,
    )
    .expect("catalog")
    .with_inventories(vec![EngineInventory {
        engine_id: "claude".to_owned(),
        project_id: Some("p1".to_owned()),
        skills: vec![
            InventorySkill {
                name: "artisan-editor".to_owned(),
                description: String::new(),
                source: InventorySkillSource::User,
                plugin: None,
            },
            InventorySkill {
                name: "cloudflare:wrangler".to_owned(),
                description: "Run Wrangler.".to_owned(),
                source: InventorySkillSource::Plugin,
                plugin: Some("cloudflare-tools@market".to_owned()),
            },
        ],
        connectors: vec![InventoryConnector {
            name: "claude.ai Notion".to_owned(),
            status: ConnectorStatus::NeedsAuth,
            plugin: None,
        }],
    }])
    .expect("inventories");

    let catalog = catalog_for_engine(Some(&extensions), Some("claude"), Some("p1"));
    let skills: Vec<(&str, &str)> = catalog
        .skills()
        .iter()
        .map(|skill| (skill.name.as_str(), skill.description.as_str()))
        .collect();
    assert_eq!(
        skills,
        [
            ("artisan-editor", "About artisan-editor"),
            ("cloudflare:wrangler", "Run Wrangler."),
        ],
        "Artisan's own entry wins over the engine's copy, and an off skill is absent"
    );
    assert_eq!(catalog.connectors()[0].name, "claude.ai Notion");
    assert_eq!(catalog.connectors()[0].description, "Needs sign-in");

    let elsewhere = catalog_for_engine(Some(&extensions), Some("opencode2"), Some("p1"));
    assert!(elsewhere.connectors().is_empty());
}
