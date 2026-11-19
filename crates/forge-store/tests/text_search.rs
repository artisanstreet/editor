use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, Phase, ProjectId,
    Status, TextSearchQuery, ThreadId, UpdateThread,
};
use forge_store::{NewItem, Store};

#[expect(
    clippy::unwrap_used,
    reason = "test helper; a failure fails the calling test"
)]
async fn thread(store: &Store, title: &str, parent: Option<ThreadId>) -> ThreadId {
    let project = store
        .create_project(CreateProject {
            id: ProjectId::new(),
            path: "/work/app".into(),
            name: None,
        })
        .await
        .unwrap();
    store
        .create_thread(CreateThread {
            id: ThreadId::new(),
            project_id: project.id,
            config: EngineConfig::new(EngineId::Claude),
            title: Some(title.into()),
            parent_thread_id: parent,
            spawned_by: None,
        })
        .await
        .unwrap()
        .id
}

fn said(thread: &ThreadId, text: &str, phase: Phase) -> NewItem {
    NewItem {
        id: ItemId::new(),
        thread_id: thread.clone(),
        run_id: None,
        parent_id: None,
        native_id: None,
        status: Status::Done,
        kind: ItemKind::AssistantMessage {
            text: text.into(),
            phase,
        },
    }
}

#[tokio::test]
async fn a_search_finds_the_message_the_line_and_the_exact_match() {
    let dir = tempfile::tempdir().expect("create the test's temporary directory");
    let store = Store::open(dir.path()).unwrap();
    let fixed = thread(&store, "Scene bound fix", None).await;
    let child = thread(&store, "Worker", Some(fixed.clone())).await;
    let reply =
        "The thread froze.\nRaising the Scene Bound to 2048 items fixed the freeze for good.";
    let answer = store
        .upsert_item(said(&fixed, reply, Phase::Final))
        .await
        .unwrap();
    store
        .upsert_item(said(
            &fixed,
            "Checking the scene bound first.",
            Phase::Commentary,
        ))
        .await
        .unwrap();
    store
        .upsert_item(said(&child, "The scene bound is 512.", Phase::Final))
        .await
        .unwrap();

    let hits = store
        .text_search(TextSearchQuery {
            query: " scene bound ".into(),
            limit: None,
        })
        .await
        .unwrap();

    assert_eq!(hits.len(), 1);
    let hit = &hits[0];
    assert_eq!((&hit.thread_id, &hit.item_id), (&fixed, &answer.id));
    assert_eq!(hit.thread_title.as_deref(), Some("Scene bound fix"));
    assert_eq!(hit.position, answer.position);
    let (start, end) = (hit.offset.0 as usize, hit.offset.1 as usize);
    assert_eq!(reply.get(start..end), Some("Scene Bound"));
    let (from, to) = (hit.matched.0 as usize, hit.matched.1 as usize);
    assert_eq!(hit.snippet.get(from..to), Some("Scene Bound"));
    assert_eq!(
        hit.snippet,
        "Raising the Scene Bound to 2048 items fixed the freeze for good."
    );
    assert!(
        store
            .text_search(TextSearchQuery {
                query: "no such words".into(),
                limit: None,
            })
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_long_line_is_cut_around_the_match() {
    let dir = tempfile::tempdir().expect("create the test's temporary directory");
    let store = Store::open(dir.path()).unwrap();
    let home = thread(&store, "Long lines", None).await;
    let line = format!(
        "{} the needle sits here {}",
        "before ".repeat(30),
        "after ".repeat(40)
    );
    store
        .upsert_item(said(&home, &line, Phase::Final))
        .await
        .unwrap();

    let hits = store
        .text_search(TextSearchQuery {
            query: "needle".into(),
            limit: None,
        })
        .await
        .unwrap();

    let hit = &hits[0];
    assert!(hit.snippet.starts_with('…') && hit.snippet.ends_with('…'));
    assert!(hit.snippet.chars().count() <= 122);
    assert_eq!(
        hit.snippet
            .get(hit.matched.0 as usize..hit.matched.1 as usize),
        Some("needle")
    );
}

#[tokio::test]
async fn archived_threads_never_crowd_out_listed_ones() {
    let dir = tempfile::tempdir().expect("create the test's temporary directory");
    let store = Store::open(dir.path()).unwrap();
    let listed = thread(&store, "Listed", None).await;
    store
        .upsert_item(said(&listed, "the pelican flew north", Phase::Final))
        .await
        .unwrap();
    for _ in 0..6 {
        let archived = thread(&store, "Archived", None).await;
        store
            .upsert_item(said(&archived, "another pelican sighting", Phase::Final))
            .await
            .unwrap();
        store
            .update_thread(
                &archived,
                UpdateThread {
                    title: None,
                    archived: Some(true),
                    config: None,
                    read_seq: None,
                },
            )
            .await
            .unwrap();
    }
    let search = |query: &str| {
        store.text_search(TextSearchQuery {
            query: query.into(),
            limit: Some(3),
        })
    };
    let hits = search("PELICAN").await.unwrap();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].thread_id, listed);
    let short = search("fl").await.unwrap();
    assert_eq!(short.len(), 1, "{short:?}");
    assert_eq!(short[0].thread_id, listed);
}

#[expect(
    clippy::unwrap_used,
    reason = "test helper; a failure fails the calling test"
)]
async fn matches(messages: &[&str], query: &str) -> Vec<(usize, String)> {
    let dir = tempfile::tempdir().expect("create the test's temporary directory");
    let store = Store::open(dir.path()).unwrap();
    for (index, text) in messages.iter().enumerate() {
        let home = thread(&store, &index.to_string(), None).await;
        store
            .upsert_item(said(&home, text, Phase::Final))
            .await
            .unwrap();
    }
    let mut found: Vec<(usize, String)> = store
        .text_search(TextSearchQuery {
            query: query.into(),
            limit: None,
        })
        .await
        .unwrap()
        .into_iter()
        .map(|hit| {
            let index: usize = hit.thread_title.unwrap().parse().unwrap();
            let text = messages[index];
            (
                index,
                text.get(hit.offset.0 as usize..hit.offset.1 as usize)
                    .expect("a hit's offsets lie on character boundaries")
                    .to_owned(),
            )
        })
        .collect();
    found.sort();
    found
}

fn hits<const N: usize>(expected: [(usize, &str); N]) -> Vec<(usize, String)> {
    expected
        .into_iter()
        .map(|(index, text)| (index, text.to_owned()))
        .collect()
}

#[tokio::test]
async fn non_ascii_letters_match_in_any_case() {
    let messages = ["Brød og SMØR", "smør på brødet", "Ψωμί και ΒΟΎΤΥΡΟ", "smor"];
    assert_eq!(
        matches(&messages, "Smør").await,
        hits([(0, "SMØR"), (1, "smør")])
    );
    assert_eq!(matches(&messages, "βούτυρο").await, hits([(2, "ΒΟΎΤΥΡΟ")]));
    assert_eq!(matches(&messages, "øR").await, hits([(0, "ØR"), (1, "ør")]));
}

#[tokio::test]
async fn sharp_s_matches_double_s_both_ways() {
    let messages = ["Die Straße ist lang", "DIE STRASSE IST LANG", "Maß"];
    assert_eq!(
        matches(&messages, "strasse").await,
        hits([(0, "Straße"), (1, "STRASSE")])
    );
    assert_eq!(
        matches(&messages, "STRAẞE").await,
        hits([(0, "Straße"), (1, "STRASSE")])
    );
    assert_eq!(
        matches(&messages, "ss").await,
        hits([(0, "ß"), (1, "SS"), (2, "ß")])
    );
    assert_eq!(
        matches(&messages, "s").await,
        hits([(0, "S"), (1, "S")]),
        "half of ß is not a hit"
    );
}

#[tokio::test]
async fn composed_and_decomposed_accents_match_each_other() {
    let messages = [
        "Et glass på caf\u{e9}en",
        "Et glass på cafe\u{301}en",
        "Kaffe på cafeteria",
    ];
    assert_eq!(
        matches(&messages, "CAFÉ").await,
        hits([(0, "caf\u{e9}"), (1, "cafe\u{301}")])
    );
    assert_eq!(
        matches(&messages, "cafe\u{301}en").await,
        hits([(0, "caf\u{e9}en"), (1, "cafe\u{301}en")])
    );
    assert_eq!(
        matches(&messages, "cafe").await,
        hits([(2, "cafe")]),
        "a bare e never matches an accented one"
    );
    assert_eq!(
        matches(&messages, "\u{e9}").await,
        hits([(0, "\u{e9}"), (1, "e\u{301}")])
    );
}

#[tokio::test]
async fn a_failed_search_keeps_the_query_out_of_its_error() {
    let dir = tempfile::tempdir().expect("create the test's temporary directory");
    let store = Store::open(dir.path()).expect("open the store");
    rusqlite::Connection::open(dir.path().join("forge.sqlite3"))
        .expect("open the database beside the store")
        .execute_batch(include_str!("text_search/drop_message_search.sql"))
        .expect("drop the message index");

    let error = store
        .text_search(TextSearchQuery {
            query: "my private plans".into(),
            limit: None,
        })
        .await
        .expect_err("searching without the message index fails");

    let error: &(dyn std::error::Error + 'static) = &error;
    let logged = std::iter::successors(Some(error), |error| error.source())
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(": ");
    assert!(
        !logged.contains("private plans"),
        "the logged error carries the user's words: {logged}"
    );
    assert!(logged.contains("16-character"), "{logged}");
}
