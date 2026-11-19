use super::*;
use artisan_api::{PlanStep, StepStatus, TextContent};
fn database() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    crate::schema::migrate(&mut conn).unwrap();
    conn
}
fn steps(count: usize) -> Vec<PlanStep> {
    (0..count)
        .map(|index| PlanStep {
            text: format!("Step {index}: αβ🙂").into(),
            status: StepStatus::Pending,
        })
        .collect()
}
#[test]
fn five_thousand_records_use_indexed_bounded_ranges_and_keep_order() {
    let conn = database();
    let original = steps(5000);
    let reference = store(&conn, &original, &BTreeSet::new(), 0, 0).unwrap();
    assert_eq!(reference.count, 5000);
    assert!(serde_json::to_string(&reference).unwrap().len() < 256);
    let duplicate = store(&conn, &original, &BTreeSet::new(), 0, 0).unwrap();
    assert_eq!(reference, duplicate);
    assert_eq!(
        conn.query_row(include_str!("content_rows.sql"), [], |row| row
            .get::<_, u64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row(include_str!("record_rows.sql"), [], |row| row
            .get::<_, u64>(0))
            .unwrap(),
        5000
    );
    let mut actual = Vec::new();
    let mut offset = 0;
    while offset < reference.count {
        let page = range::<PlanStep>(
            &conn,
            &reference.source.id,
            CollectionQuery {
                offset,
                limit: None,
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(page.start, offset);
        assert_eq!(page.end - offset, page.records.len() as u64);
        assert!(page.records.len() <= 64);
        offset = page.end;
        actual.extend(page.records);
    }
    assert_eq!(actual, original);
    let last = range::<PlanStep>(
        &conn,
        &reference.source.id,
        CollectionQuery {
            offset: 4999,
            limit: Some(1),
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(last.records, original[4999..]);
    range::<PlanStep>(
        &conn,
        &reference.source.id,
        CollectionQuery {
            offset: 5001,
            limit: None,
        },
    )
    .expect_err("an offset past the end is refused");
    range::<PlanStep>(
        &conn,
        &reference.source.id,
        CollectionQuery {
            offset: 0,
            limit: Some(65),
        },
    )
    .expect_err("a limit over the range cap is refused");
}
#[test]
fn byte_budget_bounds_a_page_even_when_count_limit_is_not_reached() {
    let conn = database();
    let original: Vec<_> = (0..4000)
        .map(|index| PlanStep {
            text: format!("{index}{}", "α".repeat(6000)).into(),
            status: StepStatus::Pending,
        })
        .collect();
    let reference = store(&conn, &original, &BTreeSet::new(), 0, 0).unwrap();
    let page = range::<PlanStep>(&conn, &reference.source.id, CollectionQuery::default())
        .unwrap()
        .unwrap();
    assert!(page.records.len() < 64);
    assert_eq!(page.records, original[..page.records.len()]);
    let encoded: usize = page
        .records
        .iter()
        .map(|record| serde_json::to_string(record).unwrap().len())
        .sum();
    assert!(encoded <= artisan_api::COLLECTION_RANGE_BYTES);
}
#[test]
fn nested_sources_survive_item_updates_then_collect_after_parent_grace() {
    let conn = database();
    let mut text = TextContent::Inline("a large step\n".repeat(3000));
    crate::content::normalize_text(&conn, &mut text).unwrap();
    let TextContent::Stored(child_source) = &text else {
        panic!("large text must be stored")
    };
    let child_bytes = child_source.bytes;
    let child = child_source.id.clone();
    let dependencies = BTreeSet::from([child]);
    let parent = store(
        &conn,
        &[PlanStep {
            text,
            status: StepStatus::Pending,
        }],
        &dependencies,
        child_bytes,
        0,
    )
    .unwrap();
    conn.execute(
        include_str!("own_by_owner.sql"),
        [parent.source.id.as_str()],
    )
    .unwrap();
    assert_eq!(crate::content::collect(&conn, i64::MAX).unwrap(), 0);
    conn.execute(include_str!("disown_owner.sql"), []).unwrap();
    assert_eq!(
        crate::content::collect(&conn, artisan_api::now()).unwrap(),
        0
    );
    assert_eq!(crate::content::collect(&conn, i64::MAX).unwrap(), 1);
    assert_eq!(
        crate::content::collect(&conn, artisan_api::now()).unwrap(),
        0
    );
    assert_eq!(crate::content::collect(&conn, i64::MAX).unwrap(), 1);
    assert_eq!(
        conn.query_row(include_str!("record_rows.sql"), [], |row| row
            .get::<_, u64>(0))
            .unwrap(),
        0
    );
}
#[test]
fn rollback_and_missing_index_cannot_publish_partial_collections() {
    let mut conn = database();
    {
        let tx = conn.transaction().unwrap();
        store(&tx, &steps(5000), &BTreeSet::new(), 0, 0).unwrap();
    }
    assert_eq!(
        conn.query_row(include_str!("content_rows.sql"), [], |row| row
            .get::<_, u64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(include_str!("record_rows.sql"), [], |row| row
            .get::<_, u64>(0))
            .unwrap(),
        0
    );
    let reference = store(&conn, &steps(5000), &BTreeSet::new(), 0, 0).unwrap();
    conn.execute(
        include_str!("drop_record_4000.sql"),
        [reference.source.id.as_str()],
    )
    .unwrap();
    range::<PlanStep>(
        &conn,
        &reference.source.id,
        CollectionQuery {
            offset: 3999,
            limit: Some(4),
        },
    )
    .expect_err("a range over a missing record is refused");
}
#[test]
fn selected_option_lookup_uses_opaque_keys_and_preserves_duplicate_labels() {
    let conn = database();
    let options = (0..5000)
        .map(|index| artisan_api::QuestionOption {
            id: format!("opaque-{index}"),
            label: "Same".into(),
            description: None,
        })
        .collect::<Vec<_>>();
    let reference = store(&conn, &options, &BTreeSet::new(), 0, 0).unwrap();
    let question = artisan_api::Question {
        id: "question".into(),
        header: None,
        text: "Pick".into(),
        multi_select: false,
        options: artisan_api::Collection::Stored(reference),
    };
    let answer = artisan_api::QuestionAnswer::Choices {
        ids: vec!["opaque-4999".into()].into(),
    };
    validate_answer(&conn, &question, Some(&answer)).unwrap();
    assert_eq!(
        answer_text(&conn, &question, &answer).unwrap(),
        "Same (option 5000 of 5000)"
    );
    validate_answer(
        &conn,
        &question,
        Some(&artisan_api::QuestionAnswer::Choices {
            ids: vec!["Same".into()].into(),
        }),
    )
    .expect_err("a choice label is not a choice id");
    validate_answer(
        &conn,
        &question,
        Some(&artisan_api::QuestionAnswer::Choices {
            ids: vec!["opaque-1".into(), "opaque-1".into()].into(),
        }),
    )
    .expect_err("the same choice twice is refused");
}
#[test]
fn five_thousand_saved_answer_records_and_choice_ids_keep_nested_ownership() {
    let conn = database();
    let mut kind = artisan_api::ItemKind::Question {
        blocking: false,
        answer_text: None,
        questions: vec![artisan_api::Question {
            id: "q".into(),
            header: None,
            text: "Choose".into(),
            multi_select: true,
            options: vec![].into(),
        }]
        .into(),
        answers: Some(
            vec![Some(artisan_api::QuestionAnswer::Choices {
                ids: (0..5000)
                    .map(|index| format!("id-{index}"))
                    .collect::<Vec<_>>()
                    .into(),
            })]
            .into(),
        ),
    };
    normalize_item(&conn, &mut kind).unwrap();
    let artisan_api::ItemKind::Question {
        answers: Some(artisan_api::Collection::Inline(answers)),
        ..
    } = &kind
    else {
        panic!("small answer collection should stay inline")
    };
    let Some(artisan_api::QuestionAnswer::Choices {
        ids: artisan_api::Collection::Stored(ids),
    }) = &answers[0]
    else {
        panic!("many choices must use an indexed source")
    };
    assert_eq!(ids.count, 5000);
    let ordinal = conn
        .query_row(
            include_str!("ordinal_by_key.sql"),
            params![ids.source.id.as_str(), "id-4999"],
            |row| row.get::<_, u64>(0),
        )
        .unwrap();
    assert_eq!(ordinal, 4999);
    let page = range::<String>(
        &conn,
        &ids.source.id,
        CollectionQuery {
            offset: ordinal,
            limit: Some(1),
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(page.records, ["id-4999"]);
    let mut kind = artisan_api::ItemKind::Question {
        blocking: false,
        answer_text: None,
        questions: vec![].into(),
        answers: Some(vec![None; 5000].into()),
    };
    normalize_item(&conn, &mut kind).unwrap();
    let artisan_api::ItemKind::Question {
        answers: Some(artisan_api::Collection::Stored(answers)),
        ..
    } = kind
    else {
        panic!("large saved answer arrays must use a source")
    };
    assert_eq!(answers.count, 5000);
    let page = range::<Option<artisan_api::QuestionAnswer>>(
        &conn,
        &answers.source.id,
        CollectionQuery {
            offset: 4999,
            limit: Some(1),
        },
    )
    .unwrap()
    .unwrap();
    assert_eq!(page.records, vec![None]);
}
#[test]
fn missing_final_index_record_cannot_return_a_short_successful_page() {
    let conn = database();
    let reference = store(&conn, &steps(5000), &BTreeSet::new(), 0, 0).unwrap();
    conn.execute(
        include_str!("drop_record_4999.sql"),
        [reference.source.id.as_str()],
    )
    .unwrap();
    range::<PlanStep>(
        &conn,
        &reference.source.id,
        CollectionQuery {
            offset: 4990,
            limit: Some(64),
        },
    )
    .expect_err("a range ending on a missing record is refused");
}
#[test]
fn canonical_answer_display_is_derived_once_and_free_text_stays_literal() {
    let conn = database();
    let options = (0..5000)
        .map(|index| artisan_api::QuestionOption {
            id: format!("opaque-{index}"),
            label: "Same".into(),
            description: None,
        })
        .collect::<Vec<_>>();
    let reference = store(&conn, &options, &BTreeSet::new(), 0, 0).unwrap();
    let mut kind = artisan_api::ItemKind::Question {
        blocking: false,
        questions: vec![artisan_api::Question {
            id: "q".into(),
            header: None,
            text: "Pick".into(),
            multi_select: false,
            options: artisan_api::Collection::Stored(reference.clone()),
        }]
        .into(),
        answers: Some(
            vec![Some(artisan_api::QuestionAnswer::Choices {
                ids: vec!["opaque-4999".into()].into(),
            })]
            .into(),
        ),
        answer_text: None,
    };
    derive_answer_text(&conn, &mut kind, None).unwrap();
    let expected = TextContent::from("Same (option 5000 of 5000)");
    assert!(
        matches!(&kind,artisan_api::ItemKind::Question {answer_text:Some(text),..} if text==&expected)
    );
    let old = kind.clone();
    let artisan_api::ItemKind::Question { answer_text, .. } = &mut kind else {
        unreachable!()
    };
    *answer_text = None;
    conn.execute(
        include_str!("relabel_records.sql"),
        [reference.source.id.as_str()],
    )
    .unwrap();
    derive_answer_text(&conn, &mut kind, Some(&old)).unwrap();
    assert!(
        matches!(&kind,artisan_api::ItemKind::Question {answer_text:Some(text),..} if text==&expected)
    );
    let literal = " \n**still literal** \n";
    let mut kind = artisan_api::ItemKind::Question {
        blocking: false,
        questions: vec![artisan_api::Question {
            id: "free".into(),
            header: None,
            text: "Explain".into(),
            multi_select: false,
            options: vec![].into(),
        }]
        .into(),
        answers: Some(
            vec![Some(artisan_api::QuestionAnswer::Text {
                text: literal.into(),
            })]
            .into(),
        ),
        answer_text: None,
    };
    derive_answer_text(&conn, &mut kind, None).unwrap();
    assert!(
        matches!(&kind,artisan_api::ItemKind::Question {answer_text:Some(text),..} if text.inline()==Some(literal))
    );
}
#[test]
fn duplicate_opaque_record_keys_roll_back_with_the_item_transaction() {
    let mut conn = database();
    {
        let tx = conn.transaction().unwrap();
        let duplicate = artisan_api::QuestionOption {
            id: "same".into(),
            label: "Choice".into(),
            description: None,
        };
        store(&tx, &[duplicate.clone(), duplicate], &BTreeSet::new(), 0, 0)
            .expect_err("duplicate option ids are refused");
    }
    assert_eq!(
        conn.query_row(include_str!("content_rows.sql"), [], |row| row
            .get::<_, u64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(include_str!("record_rows.sql"), [], |row| row
            .get::<_, u64>(0))
            .unwrap(),
        0
    );
}
