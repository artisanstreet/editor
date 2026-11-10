use super::*;
use artisan_api::{SourceIndexHeader, SourceLeafDescriptor};

fn page(source: &ContentRef, metadata: &ContentRef) -> SourceIndexRange {
    SourceIndexRange {
        index: SourceIndexHeader {
            format: SourceIndexFormat::Markdown,
            source: source.clone(),
            metadata: metadata.clone(),
            version: artisan_api::SOURCE_INDEX_VERSION,
            leaves: 2,
        },
        start: 0,
        end: 2,
        leaves: vec![
            SourceLeafDescriptor {
                ordinal: 0,
                source_start: 0,
                source_end: 2,
                metadata_start: 0,
                metadata_end: 1,
            },
            SourceLeafDescriptor {
                ordinal: 1,
                source_start: 2,
                source_end: source.bytes,
                metadata_start: 1,
                metadata_end: metadata.bytes,
            },
        ],
    }
}

#[test]
fn descriptor_page_rejects_identity_offsets_and_metadata_corruption() {
    let source = ContentRef::from_text("αx");
    let metadata = ContentRef::from_text("{}");
    let valid = page(&source, &metadata);
    assert!(validate_page(&source, SourceIndexFormat::Markdown, None, &valid).is_ok());
    let mut corrupted = valid.clone();
    corrupted.leaves[1].ordinal = 0;
    assert!(validate_page(&source, SourceIndexFormat::Markdown, None, &corrupted).is_err());
    let mut corrupted = valid.clone();
    corrupted.leaves[0].metadata_end = metadata.bytes + 1;
    assert!(validate_page(&source, SourceIndexFormat::Markdown, None, &corrupted).is_err());
    assert!(validate_page(&source, SourceIndexFormat::Literal, None, &valid).is_err());
    assert!(validate_page(&source, SourceIndexFormat::Markdown, Some(1), &valid).is_err());
}

#[test]
fn exact_range_accepts_utf8_overlap_and_rejects_nonprogress_or_split_boundary() {
    let full = "abcβ🙂end";
    let reference = ContentRef::from_text(full);
    let mut collector = RangeCollector::new(reference.clone(), 3..full.len() - 3).unwrap();
    assert_eq!(collector.next_request(), Some((3, 6)));
    collector
        .push(ContentRange {
            id: reference.id.clone(),
            total: reference.bytes,
            start: 2,
            end: 5,
            text: full[2..5].to_owned(),
        })
        .unwrap();
    collector
        .push(ContentRange {
            id: reference.id.clone(),
            total: reference.bytes,
            start: 5,
            end: 9,
            text: full[5..9].to_owned(),
        })
        .unwrap();
    assert_eq!(collector.finish().unwrap(), "β🙂");
    let mut incomplete = RangeCollector::new(reference.clone(), 3..9).unwrap();
    assert!(
        incomplete
            .push(ContentRange {
                id: reference.id.clone(),
                total: reference.bytes,
                start: 3,
                end: 3,
                text: String::new()
            })
            .is_err()
    );
    let mut split = RangeCollector::new(reference.clone(), 4..9).unwrap();
    assert!(
        split
            .push(ContentRange {
                id: reference.id.clone(),
                total: reference.bytes,
                start: 3,
                end: 5,
                text: "β".to_owned()
            })
            .is_err()
    );
}

fn owner_fixture() -> (
    tempfile::TempDir,
    editor_core::Core,
    editor_core::HostConfig,
    ScopedSourceProvider,
) {
    use editor_core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind};
    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    let config = HostConfig {
        id: HostId("stored".into()),
        name: "stored".into(),
        address: "127.0.0.1:1".into(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
    };
    let client = ForgeClient::new(
        &config.address,
        &config.fingerprint,
        core.identity.as_ref().unwrap(),
    )
    .unwrap();
    core.add_test_host(&config, client);
    core.host_mut(&config.id).unwrap().test_stop_connection();
    let provider = core
        .source_provider(&config.id, core.host(&config.id).unwrap().instance())
        .unwrap();
    (directory, core, config, provider)
}

#[test]
fn cached_pages_obey_global_and_per_source_capacity() {
    let (_directory, _core, _config, provider) = owner_fixture();
    let mut owner = StoredOwner::default();
    let metadata = ContentRef::from_text("{}");
    let source = ContentRef::from_text("abc");
    for start in 0..3 {
        let mut item = page(&source, &metadata);
        item.start = start;
        owner.remember_page(provider.clone(), Arc::new(item));
    }
    assert_eq!(owner.pages.len(), 2);
    assert_eq!(owner.pages.front().unwrap().1.start, 1);
    for index in 0..50 {
        let item = page(
            &ContentRef::from_text(&format!("source {index}")),
            &metadata,
        );
        owner.remember_page(provider.clone(), Arc::new(item));
    }
    assert_eq!(owner.pages.len(), MAX_INDEX_ENTRIES);
    assert!(
        owner
            .pages
            .iter()
            .all(|(_, page)| page.index.source != source)
    );
}

#[test]
fn moved_window_survives_ensure_and_stale_completion() {
    let (_directory, mut core, config, provider) = owner_fixture();
    let source = ContentRef::from_text("abc");
    let metadata = ContentRef::from_text("{}");
    let mut first = page(&source, &metadata);
    first.end = 1;
    first.leaves.truncate(1);
    let mut second = page(&source, &metadata);
    second.start = 1;
    second.leaves.remove(0);
    let mut owner = StoredOwner::default();
    owner.remember_page(provider.clone(), Arc::new(first.clone()));
    owner.remember_page(provider.clone(), Arc::new(second));
    let StoredPreparation::Ready(tail) = owner.ensure(
        "row",
        provider.clone(),
        source.clone(),
        SourceIndexFormat::Markdown,
    ) else {
        panic!("expected cached tail")
    };
    let StoredPreparation::Ready(moved) = owner.move_window("row", &tail, 0) else {
        panic!("expected cached window")
    };
    let StoredPreparation::Ready(current) =
        owner.ensure("row", provider.clone(), source, SourceIndexFormat::Markdown)
    else {
        panic!("expected retained window")
    };
    assert_eq!(current.loaded_range(), moved.loaded_range());
    assert!(!owner.source_is_current(&tail));
    let old_revision = current.revision();
    owner.indices[0].revision = old_revision + 1;
    owner.indices[0].pending_offset = Some(Some(0));
    assert!(
        runtime::runtime()
            .block_on(owner.delivery.sender().send(Completion::Index {
                owner: "row".into(),
                revision: old_revision,
                result: Ok(first),
            }))
            .is_ok()
    );
    assert!(owner.drain().is_empty());
    assert_eq!(owner.indices[0].pending_offset, Some(Some(0)));
    let client = editor_core::ForgeClient::new(
        &config.address,
        &config.fingerprint,
        core.identity.as_ref().unwrap(),
    )
    .unwrap();
    core.add_test_host(&config, client);
    owner.drain();
    assert!(owner.indices.is_empty());
    assert!(!owner.source_is_current(&current));
}

#[test]
fn leaf_admission_precedes_collector_allocation() {
    let (_directory, _core, _config, provider) = owner_fixture();
    let source = ContentRef::from_text("abc");
    let metadata = ContentRef::from_text("{}");
    let mut owner = StoredOwner::default();
    owner.remember_page(provider.clone(), Arc::new(page(&source, &metadata)));
    let StoredPreparation::Ready(current) =
        owner.ensure("row", provider, source, SourceIndexFormat::Markdown)
    else {
        panic!("expected cached page")
    };
    let held = owner
        .capacity
        .clone()
        .try_acquire_many_owned(PREPARED_BYTES as u32)
        .unwrap();
    assert!(matches!(
        owner.leaf("leaf", &current, 0),
        LeafPreparation::Pending
    ));
    assert!(owner.leaves.is_empty());
    drop(held);
}

#[test]
fn prepared_publication_charges_retained_bytes_and_ignores_cancelled_generation() {
    let (_directory, _core, _config, provider) = owner_fixture();
    let reference = ContentRef::from_text("abc");
    let metadata = ContentRef::from_text("{}");
    let mut owner = StoredOwner::default();
    owner.remember_page(provider.clone(), Arc::new(page(&reference, &metadata)));
    let StoredPreparation::Ready(source) =
        owner.ensure("row", provider, reference, SourceIndexFormat::Markdown)
    else {
        panic!("expected cached page")
    };
    let descriptor = source.descriptor(0).unwrap().clone();
    owner.leaves.push_back(LeafEntry {
        key: "leaf".into(),
        source: source.clone(),
        descriptor,
        generation: 9,
        capacity: None,
        phase: LeafPhase::Working,
        task: None,
    });
    let make_result = || {
        Ok((
            artisan_markdown_index::LeafContext::default(),
            false,
            IndexedLeafContent::Block(super::super::Block::Html {
                source: "abc".into(),
            }),
            Arc::<str>::from("abc"),
            123,
        ))
    };
    assert!(
        runtime::runtime()
            .block_on(owner.delivery.sender().send(Completion::Prepared {
                key: "leaf".into(),
                generation: 8,
                result: make_result(),
                _worker: None,
                _capacity: None,
            }))
            .is_ok()
    );
    assert!(owner.drain().is_empty());
    assert_eq!(owner.prepared_capacity.available_permits(), PREPARED_BYTES);
    assert!(
        runtime::runtime()
            .block_on(owner.delivery.sender().send(Completion::Prepared {
                key: "leaf".into(),
                generation: 9,
                result: make_result(),
                _worker: None,
                _capacity: None,
            }))
            .is_ok()
    );
    assert_eq!(
        owner.drain(),
        vec![StoredUpdate::Leaf {
            key: "leaf".into(),
            ordinal: 0
        }]
    );
    assert_eq!(
        owner.prepared_capacity.available_permits(),
        PREPARED_BYTES - 123
    );
    let LeafPreparation::Ready(held) = owner.leaf("leaf", &source, 0) else {
        panic!("expected published leaf")
    };
    owner.release("row");
    assert_eq!(
        owner.prepared_capacity.available_permits(),
        PREPARED_BYTES - 123
    );
    drop(held);
    assert_eq!(owner.prepared_capacity.available_permits(), PREPARED_BYTES);
}

#[test]
fn worker_reuses_engine_across_jobs() {
    let worker = preparation_worker().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    for _ in 0..2 {
        let send = send.clone();
        assert!(
            worker
                .try_send(Box::new(move |engine| {
                    let engine = engine.as_ref().unwrap();
                    send.send(std::ptr::from_ref(engine) as usize).unwrap();
                }))
                .is_ok()
        );
    }
    assert_eq!(receive.recv().unwrap(), receive.recv().unwrap());
}

#[test]
fn empty_source_range_advances_from_metadata_without_fetching_source() {
    let (_directory, _core, _config, provider) = owner_fixture();
    let source_ref = ContentRef::from_text("");
    let record = artisan_markdown_index::LeafRecord::literal(0..0);
    let metadata = serde_json::to_string(&record).unwrap();
    let metadata_ref = ContentRef::from_text(&metadata);
    let descriptor = SourceLeafDescriptor {
        ordinal: 0,
        source_start: 0,
        source_end: 0,
        metadata_start: 0,
        metadata_end: metadata_ref.bytes,
    };
    let page = SourceIndexRange {
        index: SourceIndexHeader {
            format: SourceIndexFormat::Markdown,
            source: source_ref,
            metadata: metadata_ref.clone(),
            version: artisan_api::SOURCE_INDEX_VERSION,
            leaves: 1,
        },
        start: 0,
        end: 1,
        leaves: vec![descriptor.clone()],
    };
    let mut owner = StoredOwner::default();
    owner.remember_page(provider.clone(), Arc::new(page.clone()));
    let StoredPreparation::Ready(source) = owner.ensure(
        "row",
        provider,
        page.index.source.clone(),
        SourceIndexFormat::Markdown,
    ) else {
        panic!("expected cached page")
    };
    let permit = Arc::new(
        owner
            .capacity
            .clone()
            .try_acquire_many_owned((metadata.len() + RANGE_BYTES as usize) as u32)
            .unwrap(),
    );
    owner.leaves.push_back(LeafEntry {
        key: "empty".into(),
        source,
        descriptor,
        generation: 7,
        capacity: Some(permit),
        phase: LeafPhase::Metadata(
            RangeCollector::new(metadata_ref.clone(), 0..metadata.len()).unwrap(),
        ),
        task: None,
    });
    let completion = Completion::Range {
        key: "empty".into(),
        generation: 7,
        result: Ok(ContentRange {
            id: metadata_ref.id,
            total: metadata_ref.bytes,
            start: 0,
            end: metadata.len() as u64,
            text: metadata,
        }),
    };
    assert!(
        runtime::runtime()
            .block_on(owner.delivery.sender().send(completion))
            .is_ok()
    );
    owner.drain();
    assert!(matches!(
        owner.leaves[0].phase,
        LeafPhase::Working | LeafPhase::WaitingWorker { .. }
    ));
}

#[test]
fn stored_presenter_retains_leaf_batch_and_rejects_revoked_proof() {
    use crate::ui::markdown::presenter::MarkdownPresenter;
    use crate::ui::markdown::visual::BodyTone;
    use crate::ui::markdown::{Block, Span};
    use crate::ui::theme::{ArtisanTheme, ThemeMode};
    use aqua_text::document::{SelectionDocumentId, SelectionSourceId};
    use std::rc::Rc;
    let (_directory, _core, _config, provider) = owner_fixture();
    let reference = ContentRef::from_text("abc");
    let metadata = ContentRef::from_text("{}");
    let mut owner = StoredOwner::default();
    owner.remember_page(provider.clone(), Arc::new(page(&reference, &metadata)));
    let StoredPreparation::Ready(source) =
        owner.ensure("row", provider, reference, SourceIndexFormat::Markdown)
    else {
        panic!("expected page")
    };
    owner.leaves.push_back(LeafEntry {
        key: "leaf".into(),
        source: source.clone(),
        descriptor: source.descriptor(0).unwrap().clone(),
        generation: 4,
        capacity: None,
        phase: LeafPhase::Working,
        task: None,
    });
    let context = artisan_markdown_index::LeafContext {
        lists: vec![artisan_markdown_index::ListLevel {
            source_start: 0,
            number: None,
            task: None,
            first: true,
        }],
        ..Default::default()
    };
    let result = Ok((
        context,
        false,
        IndexedLeafContent::Block(Block::Paragraph {
            spans: vec![Span::Text("abc".into())],
            range: 0..3,
        }),
        Arc::<str>::from("abc"),
        256,
    ));
    assert!(
        runtime::runtime()
            .block_on(owner.delivery.sender().send(Completion::Prepared {
                key: "leaf".into(),
                generation: 4,
                _worker: None,
                _capacity: None,
                result
            }))
            .is_ok()
    );
    owner.drain();
    let LeafPreparation::Ready(leaf) = owner.leaf("leaf", &source, 0) else {
        panic!("expected leaf")
    };
    let mut presenter = MarkdownPresenter::default();
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    presenter
        .prepare_stored_leaf(
            &owner,
            "row",
            0,
            leaf.clone(),
            &theme,
            SelectionDocumentId(9),
            &[7],
            |part| SelectionSourceId(100 + part as u64),
        )
        .unwrap();
    let render = |presenter: &MarkdownPresenter, owner: &StoredOwner| {
        presenter.render_stored_leaf(
            owner,
            "row",
            0,
            0,
            true,
            &theme,
            BodyTone::Foreground,
            Rc::new(|_, _, _| aqua_ui::element::ui::Box::new().build()),
            Rc::new(|| {}),
            None,
            None,
            None,
            None,
            None,
        )
    };
    let first = render(&presenter, &owner).unwrap();
    let second = render(&presenter, &owner).unwrap();
    assert!(Arc::ptr_eq(&first.selection_batch, &second.selection_batch));
    let selected = &first.selection_batch.sources()[0];
    assert_eq!(selected.id, SelectionSourceId(100));
    assert_eq!(selected.order, [7, 0]);
    assert_eq!(selected.separator.as_ref(), "\n");
    assert_eq!(first.list_depth, 1);
    assert_eq!(first.geometry.text[0].text.as_ref(), "abc");
    let reveal = Rc::new(crate::ui::markdown::reveal::PreparedReveal::default());
    let now = std::time::Instant::now();
    reveal.begin_frame(now, true, true, false, false);
    presenter.set_projected_reveal("row", 0, Some(reveal.clone()));
    let initial = render(&presenter, &owner).unwrap();
    assert!(Arc::ptr_eq(
        &first.selection_batch,
        &initial.selection_batch
    ));
    assert!(owner.leaf_is_current(&initial.proof));
    let mut app = aqua::UiApp::new(aqua_text::SharedFonts::new(), aqua_text::SharedRuns::new());
    app.mount(move |_| initial.element).unwrap();
    app.set_viewport(400.0, 200.0, 1.0);
    app.build_frame().unwrap();
    let ranges = |app: &aqua::UiApp| {
        app.tree()
            .traverse()
            .into_iter()
            .find_map(|id| {
                let node = app.tree().get(id)?;
                match &node.kind {
                    aqua_ui::tree::NodeKind::Text(text) if text.text() == "abc" => {
                        Some(text.gpui_text_opacity.clone())
                    }
                    _ => None,
                }
            })
            .unwrap()
    };
    let hidden = ranges(&app);
    assert_eq!(hidden.len(), 1);
    assert_eq!(hidden[0].bytes, 0..3);
    assert_eq!(hidden[0].opacity, 0.0);
    reveal.begin_frame(
        now + std::time::Duration::from_millis(150),
        false,
        true,
        false,
        false,
    );
    let started = render(&presenter, &owner).unwrap();
    assert!(Arc::ptr_eq(
        &first.selection_batch,
        &started.selection_batch
    ));
    assert!(owner.leaf_is_current(&started.proof));
    app.refresh(vec![started.element]).unwrap();
    app.build_frame().unwrap();
    assert_eq!(ranges(&app)[0].opacity, 0.0);
    reveal.begin_frame(
        now + std::time::Duration::from_millis(350),
        false,
        true,
        false,
        false,
    );
    let progressed = render(&presenter, &owner).unwrap();
    assert!(Arc::ptr_eq(
        &first.selection_batch,
        &progressed.selection_batch
    ));
    assert!(owner.leaf_is_current(&progressed.proof));
    app.refresh(vec![progressed.element]).unwrap();
    app.build_frame().unwrap();
    let active = ranges(&app);
    assert!(active[0].opacity > 0.0 && active[0].opacity < 1.0);
    owner.leaves.push_back(LeafEntry {
        key: "table".into(),
        source: source.clone(),
        descriptor: source.descriptor(1).unwrap().clone(),
        generation: 5,
        capacity: None,
        phase: LeafPhase::Working,
        task: None,
    });
    let table = IndexedLeafContent::TableRow {
        header: false,
        alignments: vec![
            super::super::TableAlignment::Left,
            super::super::TableAlignment::Right,
        ],
        cells: vec![
            super::super::TableCell {
                spans: vec![Span::Text("left".into())],
            },
            super::super::TableCell {
                spans: vec![Span::Text("right".into())],
            },
        ],
    };
    let result = Ok((Default::default(), false, table, Arc::<str>::from("c"), 256));
    assert!(
        runtime::runtime()
            .block_on(owner.delivery.sender().send(Completion::Prepared {
                key: "table".into(),
                generation: 5,
                _worker: None,
                _capacity: None,
                result
            }))
            .is_ok()
    );
    owner.drain();
    let LeafPreparation::Ready(table) = owner.leaf("table", &source, 1) else {
        panic!("expected table")
    };
    presenter
        .prepare_stored_leaf(
            &owner,
            "row",
            1,
            table,
            &theme,
            SelectionDocumentId(9),
            &[7],
            |part| SelectionSourceId(200 + part as u64),
        )
        .unwrap();
    let table_row = presenter
        .render_stored_leaf(
            &owner,
            "row",
            1,
            10,
            true,
            &theme,
            BodyTone::Foreground,
            Rc::new(|_, _, _| aqua_ui::element::ui::Box::new().build()),
            Rc::new(|| {}),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
    let table_sources = table_row.selection_batch.sources();
    assert_eq!(
        table_sources
            .iter()
            .map(|source| source.id)
            .collect::<Vec<_>>(),
        [SelectionSourceId(200), SelectionSourceId(201)]
    );
    assert_eq!(table_sources[0].order, [7, 1, 0]);
    assert_eq!(table_sources[1].order, [7, 1, 1]);
    assert_eq!(table_sources[0].separator.as_ref(), "\n");
    assert_eq!(table_sources[1].separator.as_ref(), "\t");
    assert_eq!(
        table_row
            .geometry
            .text
            .iter()
            .map(|text| text.slot)
            .collect::<Vec<_>>(),
        [10, 11]
    );
    presenter.release_stored_leaf("row", 1);
    assert!(
        presenter
            .render_stored_leaf(
                &owner,
                "row",
                1,
                10,
                true,
                &theme,
                BodyTone::Foreground,
                Rc::new(|_, _, _| aqua_ui::element::ui::Box::new().build()),
                Rc::new(|| {}),
                None,
                None,
                None,
                None,
                None
            )
            .is_err()
    );
    owner.leaves.push_back(LeafEntry {
        key: "oversized".into(),
        source: source.clone(),
        descriptor: source.descriptor(0).unwrap().clone(),
        generation: 6,
        capacity: None,
        phase: LeafPhase::Working,
        task: None,
    });
    let oversized = IndexedLeafContent::Block(Block::Paragraph {
        spans: vec![Span::Text("x".repeat(PREPARED_BYTES / 2 + 1))],
        range: 0..1,
    });
    let result = Ok((
        Default::default(),
        false,
        oversized,
        Arc::<str>::from("x"),
        1,
    ));
    assert!(
        runtime::runtime()
            .block_on(owner.delivery.sender().send(Completion::Prepared {
                key: "oversized".into(),
                generation: 6,
                _worker: None,
                _capacity: None,
                result
            }))
            .is_ok()
    );
    owner.drain();
    let LeafPreparation::Ready(oversized) = owner.leaf("oversized", &source, 0) else {
        panic!("expected oversized fixture")
    };
    assert!(
        presenter
            .prepare_stored_leaf(
                &owner,
                "row",
                0,
                oversized,
                &theme,
                SelectionDocumentId(9),
                &[7],
                |part| SelectionSourceId(300 + part as u64)
            )
            .is_err()
    );
    owner.release("row");
    assert!(render(&presenter, &owner).is_err());
}
