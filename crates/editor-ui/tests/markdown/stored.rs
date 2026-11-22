use super::*;
use crate::ui::markdown::presenter::BodyServices;
use crate::ui::markdown::presenter::RevealSlot;
use crate::ui::markdown::presenter::{MarkdownPresenter, PresenterError, StoredProjectedMarkdown};
use crate::ui::markdown::visual::BodyContext;
use crate::ui::markdown::visual::BodyTone;
use crate::ui::text::slice::slice;
use artisan_api::{SourceIndexHeader, SourceLeafDescriptor};
use std::sync::Arc;

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

type PreparedLeaf = (
    artisan_markdown_index::LeafContext,
    bool,
    IndexedLeafContent,
    Arc<str>,
    usize,
);

fn parts(
    (leaf_context, table_continues, content, source, bytes): PreparedLeaf,
) -> Box<PreparedParts> {
    Box::new(PreparedParts {
        context: leaf_context,
        table_continues,
        content,
        mentions: LeafMentions::Plain,
        source,
        bytes,
    })
}

fn cached_source(provider: ScopedSourceProvider) -> (StoredOwner, StoredSource) {
    let reference = ContentRef::from_text("abc");
    let metadata = ContentRef::from_text("{}");
    let mut owner = StoredOwner::default();
    owner.remember_page(provider.clone(), Arc::new(page(&reference, &metadata)));
    let StoredPreparation::Ready(source) =
        owner.ensure("row", provider, reference, SourceIndexFormat::Markdown)
    else {
        panic!("expected cached page")
    };
    (owner, source)
}

fn prepared(
    owner: &mut StoredOwner,
    source: &StoredSource,
    key: &str,
    ordinal: usize,
    generation: u64,
    result: PreparedLeaf,
) -> LeafPreparation {
    let descriptor = owner.descriptor(source, ordinal).unwrap().clone();
    owner.leaves.push_back(LeafEntry {
        key: key.into(),
        kind: LeafKind::Plain,
        source: source.clone(),
        descriptor,
        generation,
        capacity: None,
        phase: LeafPhase::Working,
        task: None,
    });
    runtime::runtime()
        .block_on(owner.delivery.sender().send(Completion::Prepared {
            key: key.into(),
            generation,
            _worker: None,
            _capacity: None,
            vocabulary: None,
            result: Ok(parts(result)),
        }))
        .unwrap();
    owner.drain();
    owner.leaf(key, source, ordinal)
}

fn render_leaf(
    presenter: &MarkdownPresenter,
    owner: &StoredOwner,
    ordinal: usize,
    slot_start: usize,
) -> Result<StoredProjectedMarkdown, PresenterError> {
    let glyph: crate::ui::rich::narrative::callout::GlyphFactory =
        Rc::new(|_, _, _| aqua_ui::element::ui::Box::new().build());
    let invalidate: Rc<dyn Fn()> = Rc::new(|| {});
    presenter.render_stored_leaf(
        owner,
        "row",
        ordinal,
        RevealSlot {
            start: slot_start,
            last: true,
        },
        BodyContext {
            theme: crate::ui::theme::ArtisanTheme::for_mode(crate::ui::theme::ThemeMode::Dark),
            tone: BodyTone::Foreground,
            on_link: None,
            on_color: None,
            scope: None,
        },
        BodyServices {
            glyph: &glyph,
            invalidate: &invalidate,
            charts: None,
            code: None,
        },
    )
}

#[test]
fn descriptor_page_rejects_identity_offsets_and_metadata_corruption() {
    let source = ContentRef::from_text("αx");
    let metadata = ContentRef::from_text("{}");
    let valid = page(&source, &metadata);
    validate_page(&source, SourceIndexFormat::Markdown, None, &valid).unwrap();
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
        .push(&ContentRange {
            id: reference.id.clone(),
            total: reference.bytes,
            start: 2,
            end: 5,
            text: slice(full, 2..5).to_owned(),
        })
        .unwrap();
    collector
        .push(&ContentRange {
            id: reference.id.clone(),
            total: reference.bytes,
            start: 5,
            end: 9,
            text: slice(full, 5..9).to_owned(),
        })
        .unwrap();
    assert_eq!(collector.finish().unwrap(), "β🙂");
    let mut incomplete = RangeCollector::new(reference.clone(), 3..9).unwrap();
    assert!(
        incomplete
            .push(&ContentRange {
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
            .push(&ContentRange {
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
    (tempfile::TempDir, test_support::net::SilentListener),
    editor_core::Core,
    editor_core::HostConfig,
    ScopedSourceProvider,
) {
    use editor_core::{ForgeClient, HostConfig, HostId, HostKind};
    let directory = tempfile::tempdir().unwrap();
    let forge = test_support::net::SilentListener::bind();
    let mut core = test_support::editor::core::test_core(directory.path());
    let config = HostConfig {
        id: HostId("stored".into()),
        name: "stored".into(),
        address: forge.address().to_string(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
    };
    let client = ForgeClient::new(
        &config.address,
        &config.fingerprint,
        core.identity().unwrap(),
    )
    .unwrap();
    test_support::editor::core::connect_client(&mut core, &config, client);
    let provider = core
        .source_provider(&config.id, core.host(&config.id).unwrap().instance())
        .unwrap();
    ((directory, forge), core, config, provider)
}

fn long_page(
    source: &ContentRef,
    metadata: &ContentRef,
    leaves: u64,
    range: Range<u64>,
) -> SourceIndexRange {
    SourceIndexRange {
        index: SourceIndexHeader {
            format: SourceIndexFormat::Markdown,
            source: source.clone(),
            metadata: metadata.clone(),
            version: artisan_api::SOURCE_INDEX_VERSION,
            leaves,
        },
        start: range.start,
        end: range.end,
        leaves: range
            .map(|ordinal| SourceLeafDescriptor {
                ordinal,
                source_start: 0,
                source_end: source.bytes,
                metadata_start: 0,
                metadata_end: metadata.bytes,
            })
            .collect(),
    }
}

fn hold_fetch(owner: &mut StoredOwner, start: u64) {
    owner.indices[0].fetches = vec![PageFetch {
        start,
        state: FetchState::Loading {
            _task: Task(
                runtime::runtime()
                    .spawn(std::future::pending::<()>())
                    .abort_handle(),
            ),
        },
    }];
}

fn deliver_page(
    owner: &mut StoredOwner,
    revision: u64,
    offset: u64,
    result: Result<SourceIndexRange, String>,
) -> Vec<StoredUpdate> {
    runtime::runtime()
        .block_on(owner.delivery.sender().send(Completion::Index {
            owner: "row".into(),
            revision,
            offset: Some(offset),
            result,
        }))
        .unwrap();
    owner.drain()
}

#[test]
fn cached_pages_obey_global_and_per_source_capacity() {
    let (_directory, _core, _config, provider) = owner_fixture();
    let mut owner = StoredOwner::default();
    let metadata = ContentRef::from_text("{}");
    let source = ContentRef::from_text("abc");
    for start in 0..=PAGES_PER_SOURCE as u64 {
        let mut item = page(&source, &metadata);
        item.start = start;
        owner.remember_page(provider.clone(), Arc::new(item));
    }
    assert_eq!(owner.pages.len(), PAGES_PER_SOURCE);
    assert_eq!(owner.pages.front().unwrap().1.start, 1);
    for index in 0..MAX_CACHED_PAGES {
        let item = page(
            &ContentRef::from_text(&format!("source {index}")),
            &metadata,
        );
        owner.remember_page(provider.clone(), Arc::new(item));
    }
    assert_eq!(owner.pages.len(), MAX_CACHED_PAGES);
    assert!(
        owner
            .pages
            .iter()
            .all(|(_, page)| page.index.source != source)
    );
}

#[test]
fn a_failed_page_reports_its_offset_and_cause_until_retried() {
    let (_directory, mut core, config, provider) = owner_fixture();
    let source = ContentRef::from_text("abc");
    let metadata = ContentRef::from_text("{}");
    let mut owner = StoredOwner::default();
    owner.remember_page(
        provider.clone(),
        Arc::new(long_page(&source, &metadata, 130, 66..130)),
    );
    let StoredPreparation::Ready(tail) =
        owner.ensure("row", provider, source.clone(), SourceIndexFormat::Markdown)
    else {
        panic!("expected the cached tail page")
    };
    assert_eq!(tail.block_count(), 130);
    assert!(matches!(
        owner.page(&tail, 129),
        PagePreparation::Ready(descriptor) if descriptor.ordinal == 129
    ));
    assert_eq!(owner.page(&tail, 10), PagePreparation::Pending);
    hold_fetch(&mut owner, 0);
    let failed = PagePreparation::Failed {
        offset: 0,
        cause: "connection refused".to_owned(),
    };
    assert!(
        !deliver_page(
            &mut owner,
            tail.revision(),
            0,
            Err("connection refused".into())
        )
        .is_empty()
    );
    assert_eq!(owner.page(&tail, 10), failed);
    assert_eq!(owner.page(&tail, 63), failed);
    assert!(matches!(
        owner.page(&tail, 70),
        PagePreparation::Ready(descriptor) if descriptor.ordinal == 70
    ));
    assert!(!owner.retry_page("row", 64));
    assert!(owner.retry_page("row", 0));
    assert_eq!(owner.page(&tail, 10), PagePreparation::Pending);
    assert!(!owner.retry_page("row", 0));
    hold_fetch(&mut owner, 0);
    let first = long_page(&source, &metadata, 130, 0..64);
    assert!(deliver_page(&mut owner, tail.revision() + 1, 0, Ok(first.clone())).is_empty());
    assert_eq!(owner.page(&tail, 10), PagePreparation::Pending);
    assert!(!deliver_page(&mut owner, tail.revision(), 0, Ok(first)).is_empty());
    assert!(matches!(
        owner.page(&tail, 10),
        PagePreparation::Ready(descriptor) if descriptor.ordinal == 10
    ));
    let client = editor_core::ForgeClient::new(
        &config.address,
        &config.fingerprint,
        core.identity().unwrap(),
    )
    .unwrap();
    core.remove_host(&config.id).unwrap();
    test_support::editor::core::connect_client(&mut core, &config, client);
    owner.drain();
    assert!(owner.indices.is_empty());
    assert!(!owner.source_is_current(&tail));
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
    let held = Arc::clone(&owner.capacity)
        .try_acquire_many_owned(u32::try_from(PREPARED_BYTES).expect("budget fits u32"))
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
    let (mut owner, source) = cached_source(provider);
    let descriptor = owner.descriptor(&source, 0).unwrap().clone();
    owner.leaves.push_back(LeafEntry {
        key: "leaf".into(),
        kind: LeafKind::Plain,
        source: source.clone(),
        descriptor,
        generation: 9,
        capacity: None,
        phase: LeafPhase::Working,
        task: None,
    });
    let make_result = || {
        Ok(parts((
            artisan_markdown_index::LeafContext::default(),
            false,
            IndexedLeafContent::Block(super::super::Block::Html {
                source: "abc".into(),
            }),
            Arc::<str>::from("abc"),
            123,
        )))
    };
    runtime::runtime()
        .block_on(owner.delivery.sender().send(Completion::Prepared {
            key: "leaf".into(),
            generation: 8,
            result: make_result(),
            vocabulary: None,
            _worker: None,
            _capacity: None,
        }))
        .unwrap();
    assert!(owner.drain().is_empty());
    assert_eq!(owner.prepared_capacity.available_permits(), PREPARED_BYTES);
    runtime::runtime()
        .block_on(owner.delivery.sender().send(Completion::Prepared {
            key: "leaf".into(),
            generation: 9,
            result: make_result(),
            vocabulary: None,
            _worker: None,
            _capacity: None,
        }))
        .unwrap();
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
        worker
            .try_send(Box::new(move |engine| {
                let engine = engine.as_ref().unwrap();
                send.send(std::ptr::from_ref(engine) as usize).unwrap();
            }))
            .unwrap();
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
        page.index.source,
        SourceIndexFormat::Markdown,
    ) else {
        panic!("expected cached page")
    };
    let permit = Arc::new(
        Arc::clone(&owner.capacity)
            .try_acquire_many_owned(
                u32::try_from(metadata.len() + RANGE_BYTES as usize).expect("budget fits u32"),
            )
            .unwrap(),
    );
    owner.leaves.push_back(LeafEntry {
        key: "empty".into(),
        kind: LeafKind::Plain,
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
    runtime::runtime()
        .block_on(owner.delivery.sender().send(completion))
        .unwrap();
    owner.drain();
    assert!(matches!(
        owner.leaves[0].phase,
        LeafPhase::Working | LeafPhase::WaitingWorker { .. }
    ));
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario walking a stored leaf from retention through revoked proof"
)]
fn stored_presenter_retains_leaf_batch_and_rejects_revoked_proof() {
    use crate::ui::markdown::{Block, Span};
    use crate::ui::theme::{ArtisanTheme, ThemeMode};
    use aqua_text::document::{SelectionDocumentId, SelectionSourceId};
    use std::rc::Rc;
    let (_directory, _core, _config, provider) = owner_fixture();
    let (mut owner, source) = cached_source(provider);
    let context = artisan_markdown_index::LeafContext {
        lists: vec![artisan_markdown_index::ListLevel {
            source_start: 0,
            number: None,
            task: None,
            first: true,
        }],
        ..Default::default()
    };
    let result = (
        context,
        false,
        IndexedLeafContent::Block(Block::Paragraph {
            spans: vec![Span::Text("abc".into())],
            range: 0..3,
        }),
        Arc::<str>::from("abc"),
        256,
    );
    let LeafPreparation::Ready(leaf) = prepared(&mut owner, &source, "leaf", 0, 4, result) else {
        panic!("expected leaf")
    };
    let mut presenter = MarkdownPresenter::default();
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    presenter
        .prepare_stored_leaf(
            &owner,
            "row",
            0,
            leaf,
            theme,
            SelectionDocumentId(9),
            &[7],
            |part| SelectionSourceId(100 + part as u64),
        )
        .unwrap();
    let render =
        |presenter: &MarkdownPresenter, owner: &StoredOwner| render_leaf(presenter, owner, 0, 0);
    let first = render(&presenter, &owner).unwrap();
    let second = render(&presenter, &owner).unwrap();
    assert!(Arc::ptr_eq(&first.selection_batch, &second.selection_batch));
    let selected = &first.selection_batch.sources()[0];
    assert_eq!(selected.id, SelectionSourceId(100));
    assert_eq!(selected.order, [7, 0]);
    assert_eq!(selected.separator.as_ref(), "\n");
    assert_eq!(first.geometry.text[0].text.as_ref(), "abc");
    let reveal = Rc::new(crate::ui::markdown::reveal::PreparedReveal::default());
    let now = std::time::Instant::now();
    reveal.begin_frame(now, true, true, false, false);
    presenter.set_projected_reveal("row", 0, Some(Rc::clone(&reveal)));
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
                        Some(Arc::clone(&text.gpui_text_opacity))
                    }
                    aqua_ui::tree::NodeKind::Box(_)
                    | aqua_ui::tree::NodeKind::Text(_)
                    | aqua_ui::tree::NodeKind::Media(_)
                    | aqua_ui::tree::NodeKind::Scroll(_)
                    | aqua_ui::tree::NodeKind::Canvas(_)
                    | aqua_ui::tree::NodeKind::Viewport3D(_) => None,
                }
            })
            .unwrap()
    };
    let initial_ranges = ranges(&app);
    assert_eq!(initial_ranges.len(), 1);
    assert_eq!(initial_ranges[0].bytes, 0..3);
    assert_eq!(
        initial_ranges[0].opacity.to_bits(),
        0.35_f32.to_bits(),
        "fresh stored text starts readable while its fade begins"
    );
    reveal.begin_frame(
        now + std::time::Duration::from_millis(60),
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
    let active = ranges(&app);
    assert_eq!(active[0].bytes, 0..3);
    assert!(
        active[0].opacity > initial_ranges[0].opacity && active[0].opacity < 1.0,
        "the retained stored text must advance through its fade"
    );
    reveal.begin_frame(
        now + std::time::Duration::from_millis(150),
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
    assert!(
        ranges(&app).is_empty(),
        "settled stored text must release its completed fade ranges"
    );
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
    let result = (
        artisan_markdown_index::LeafContext::default(),
        false,
        table,
        Arc::<str>::from("c"),
        256,
    );
    let LeafPreparation::Ready(table) = prepared(&mut owner, &source, "table", 1, 5, result) else {
        panic!("expected table")
    };
    presenter
        .prepare_stored_leaf(
            &owner,
            "row",
            1,
            table,
            theme,
            SelectionDocumentId(9),
            &[7],
            |part| SelectionSourceId(200 + part as u64),
        )
        .unwrap();
    let table_row = render_leaf(&presenter, &owner, 1, 10).unwrap();
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
    render_leaf(&presenter, &owner, 1, 10).expect_err("a released leaf no longer renders");
    let oversized = IndexedLeafContent::Block(Block::Paragraph {
        spans: vec![Span::Text("x".repeat(PREPARED_BYTES / 2 + 1))],
        range: 0..1,
    });
    let result = (
        artisan_markdown_index::LeafContext::default(),
        false,
        oversized,
        Arc::<str>::from("x"),
        1,
    );
    let LeafPreparation::Ready(oversized) =
        prepared(&mut owner, &source, "oversized", 0, 6, result)
    else {
        panic!("expected oversized fixture")
    };
    assert!(
        presenter
            .prepare_stored_leaf(
                &owner,
                "row",
                0,
                oversized,
                theme,
                SelectionDocumentId(9),
                &[7],
                |part| SelectionSourceId(300 + part as u64)
            )
            .is_err()
    );
    owner.release("row");
    render(&presenter, &owner).expect_err("a released row no longer renders");
}

fn tampered_record(token: &serde_json::Value) -> artisan_markdown_index::LeafRecord {
    let mut record =
        serde_json::to_value(artisan_markdown_index::LeafRecord::literal(0..1)).unwrap();
    record["leaf"]["events"] = serde_json::json!([
        { "token": token, "range": { "start": 0, "end": 2 } }
    ]);
    serde_json::from_value(record).unwrap()
}

#[test]
fn stored_metadata_with_an_event_beyond_its_leaf_is_rejected_instead_of_aborting() {
    let engine = MarkdownEngine::new().unwrap();
    for token in [
        serde_json::json!({ "Structural": { "Html": "<" } }),
        serde_json::json!({ "Html": { "Range": { "start": 0, "end": 1 } } }),
    ] {
        let result = normalize_record(
            &tampered_record(&token),
            0,
            "<",
            &engine,
            &mut crate::ui::markdown::NoFenceTokenMemo,
        );
        assert!(
            matches!(
                result,
                Err(crate::ui::markdown::MarkdownIndexError::Replay(_))
            ),
            "{result:?}"
        );
    }
}

#[test]
fn a_stored_message_leaf_marks_its_mentions_from_the_start_of_the_leaf() {
    let filler = "Plain words that pad the message past the inline limit.\n\n".repeat(400);
    let source = format!("{filler}$review the last part\n\n`$review` stays code");
    assert!(source.len() > 16 * 1024);
    let index = artisan_markdown_index::MarkdownIndex::build(&source).expect("index");
    let titles: [&str; 0] = [];
    let vocabulary = artisan_mentions::MessageVocabulary::new(&["review"], &titles);
    let engine = MarkdownEngine::new().expect("markdown engine");
    let last = index.leaves().len();
    let marked = |ordinal: usize| {
        let record = index.record(ordinal).expect("record");
        let range = record.leaf.source_range();
        let fetched = slice(&source, range.clone());
        super::super::source_index::normalize_message_record(
            &record,
            range.start,
            fetched,
            &engine,
            &mut super::super::NoFenceTokenMemo,
            &vocabulary,
        )
        .expect("stored leaf")
        .mentions
        .len()
    };
    assert_eq!(marked(last - 2), 1);
    assert_eq!(marked(last - 1), 0);
}

#[test]
fn stored_metadata_closing_a_tag_it_never_opened_is_a_typed_error() {
    let source = "[a](b) $review";
    let index = artisan_markdown_index::MarkdownIndex::build(source).expect("index");
    let mut record = serde_json::to_value(index.record(0).expect("record")).unwrap();
    let events = record["leaf"]["events"].as_array_mut().expect("events");
    let opened = events
        .iter()
        .position(|event| {
            let text = event.to_string();
            text.contains("Start") && text.contains("Link")
        })
        .expect("a link start");
    events.remove(opened);
    let record: artisan_markdown_index::LeafRecord = serde_json::from_value(record).unwrap();
    let vocabulary = artisan_mentions::MessageVocabulary::new(&["review"], &[""; 0]);
    let engine = MarkdownEngine::new().unwrap();
    let result = super::super::source_index::normalize_message_record(
        &record,
        0,
        source,
        &engine,
        &mut super::super::NoFenceTokenMemo,
        &vocabulary,
    );
    assert!(
        matches!(
            result,
            Err(crate::ui::markdown::MarkdownIndexError::UnbalancedEvents { .. })
        ),
        "{:?}",
        result.err()
    );
}

struct MessageSource {
    owner: StoredOwner,
    source: StoredSource,
    leaves: Vec<(String, String)>,
}

fn message_source(provider: ScopedSourceProvider, text: &str) -> MessageSource {
    budgeted_source(provider, text, PREPARED_BYTES, PREPARED_BYTES / 8)
}

fn budgeted_source(
    provider: ScopedSourceProvider,
    text: &str,
    budget: usize,
    syntax_budget: usize,
) -> MessageSource {
    let index = artisan_markdown_index::MarkdownIndex::build(text).expect("index");
    let mut metadata = String::new();
    let mut descriptors = Vec::new();
    let mut leaves = Vec::new();
    for ordinal in 0..index.leaves().len() {
        let record = index.record(ordinal).expect("record");
        let json = serde_json::to_string(&record).unwrap();
        let range = record.leaf.source_range();
        descriptors.push(SourceLeafDescriptor {
            ordinal: ordinal as u64,
            source_start: range.start as u64,
            source_end: range.end as u64,
            metadata_start: metadata.len() as u64,
            metadata_end: (metadata.len() + json.len()) as u64,
        });
        metadata.push_str(&json);
        leaves.push((json, slice(text, range).to_owned()));
    }
    let reference = ContentRef::from_text(text);
    let metadata = ContentRef::from_text(&metadata);
    let page = SourceIndexRange {
        index: SourceIndexHeader {
            format: SourceIndexFormat::Markdown,
            source: reference.clone(),
            metadata,
            version: artisan_api::SOURCE_INDEX_VERSION,
            leaves: descriptors.len() as u64,
        },
        start: 0,
        end: descriptors.len() as u64,
        leaves: descriptors,
    };
    let mut owner = StoredOwner::with_budgets(budget, syntax_budget);
    owner.remember_page(provider.clone(), Arc::new(page));
    let StoredPreparation::Ready(source) =
        owner.ensure("row", provider, reference, SourceIndexFormat::Markdown)
    else {
        panic!("expected cached page")
    };
    MessageSource {
        owner,
        source,
        leaves,
    }
}

impl MessageSource {
    fn waiting(&mut self, ordinal: usize, vocabulary: &Arc<artisan_mentions::MessageVocabulary>) {
        self.owner.revise_mentions(vocabulary);
        let (metadata, source) = self.leaves[ordinal].clone();
        let descriptor = self
            .owner
            .descriptor(&self.source, ordinal)
            .unwrap()
            .clone();
        let permit = Arc::new(
            Arc::clone(&self.owner.capacity)
                .try_acquire_many_owned(
                    u32::try_from(metadata.len() + source.len()).expect("budget fits u32"),
                )
                .unwrap(),
        );
        let generation = self.owner.next_generation();
        self.owner.leaves.push_back(LeafEntry {
            key: format!("leaf:{ordinal}"),
            kind: LeafKind::Message,
            source: self.source.clone(),
            descriptor,
            generation,
            capacity: Some(permit),
            phase: LeafPhase::WaitingWorker(PreparationInput::Fetched { metadata, source }),
            task: None,
        });
    }

    fn shown(
        &mut self,
        ordinal: usize,
        vocabulary: &Arc<artisan_mentions::MessageVocabulary>,
    ) -> Option<Rc<StoredLeaf>> {
        let source = self.source.clone();
        match self
            .owner
            .message_leaf(&format!("leaf:{ordinal}"), &source, ordinal, vocabulary)
        {
            LeafPreparation::Ready(leaf) => Some(leaf),
            LeafPreparation::Pending => None,
            LeafPreparation::Failed(error) => panic!("leaf {ordinal} failed: {error}"),
        }
    }

    fn drain_until(&mut self, what: &str, mut done: impl FnMut(&[StoredUpdate]) -> bool) {
        let mut seen = Vec::new();
        test_support::wait::wait_until(what, test_support::wait::LIVENESS, || {
            seen.extend(self.owner.drain());
            done(&seen)
        });
    }
}

fn chips(leaf: &StoredLeaf) -> usize {
    fn count(spans: &[super::super::Span]) -> usize {
        spans
            .iter()
            .map(|span| match span {
                super::super::Span::MentionLead { .. } => 1,
                super::super::Span::Emphasis(inner) | super::super::Span::Strong(inner) => {
                    count(inner)
                }
                super::super::Span::Link { label, .. } => count(label),
                super::super::Span::MentionTail { .. }
                | super::super::Span::Text(_)
                | super::super::Span::Html(_)
                | super::super::Span::Code(_)
                | super::super::Span::Command(_) => 0,
            })
            .sum()
    }
    match leaf.content() {
        IndexedLeafContent::Block(super::super::Block::Paragraph { spans, .. }) => count(spans),
        IndexedLeafContent::Block(_) => 0,
        IndexedLeafContent::TableRow { cells, .. } => {
            cells.iter().map(|cell| count(&cell.spans)).sum()
        }
    }
}

fn vocabulary(skills: &[&str]) -> Arc<artisan_mentions::MessageVocabulary> {
    Arc::new(artisan_mentions::MessageVocabulary::new(skills, &[""; 0]))
}

fn remarked(updates: &[StoredUpdate], ordinal: usize) -> bool {
    updates.iter().any(|update| {
        *update
            == StoredUpdate::Remarked {
                key: format!("leaf:{ordinal}"),
                ordinal,
            }
    })
}

#[test]
fn a_vocabulary_change_keeps_showing_stored_leaves_until_the_chip_is_ready() {
    let (_directory, _core, _config, provider) = owner_fixture();
    let mut stored = message_source(provider, "run $review now\n\nnothing here");
    let before = vocabulary(&[]);
    stored.waiting(0, &before);
    stored.waiting(1, &before);
    stored.drain_until("both leaves to be prepared", |updates| {
        updates
            .iter()
            .filter(|update| matches!(update, StoredUpdate::Leaf { .. }))
            .count()
            == 2
    });
    let old = stored.shown(0, &before).expect("ready leaf");
    let other = stored.shown(1, &before).expect("ready leaf");
    assert_eq!(chips(&old), 0);

    let after = vocabulary(&["review"]);
    stored.owner.revise_mentions(&after);
    let shown = stored
        .shown(0, &after)
        .expect("still shown while it is marked again");
    assert!(Rc::ptr_eq(&shown, &old));
    assert!(Rc::ptr_eq(
        &stored.shown(1, &after).expect("untouched leaf"),
        &other
    ));
    drop(shown);

    stored.drain_until("the mentioned leaf to be marked", |updates| {
        remarked(updates, 0)
    });
    let marked = stored.shown(0, &after).expect("marked leaf");
    assert_eq!(chips(&marked), 1);
    assert_eq!(marked.proof().generation, old.proof().generation);
    assert!(Rc::ptr_eq(
        &stored.shown(1, &after).expect("untouched leaf"),
        &other
    ));
}

#[test]
fn a_vocabulary_change_while_a_leaf_prepares_publishes_only_the_latest_marks() {
    let (_directory, _core, _config, provider) = owner_fixture();
    let mut stored = message_source(provider, "run $review now\n\nnothing here");
    let before = vocabulary(&[]);
    stored.waiting(0, &before);
    stored.waiting(1, &before);
    let generations: Vec<u64> = stored
        .owner
        .leaves
        .iter()
        .map(|entry| entry.generation)
        .collect();
    let after = vocabulary(&["review"]);
    stored.owner.revise_mentions(&after);
    stored.drain_until("both leaves to be prepared", |updates| {
        updates
            .iter()
            .filter(|update| matches!(update, StoredUpdate::Leaf { .. }))
            .count()
            == 2
    });
    assert_eq!(
        stored
            .owner
            .leaves
            .iter()
            .map(|entry| entry.generation)
            .collect::<Vec<_>>(),
        generations
    );
    assert_eq!(chips(&stored.shown(0, &after).expect("ready")), 1);

    let (_second_directory, _second_core, _second_config, second) = owner_fixture();
    let mut working = message_source(second, "run $review now");
    working.waiting(0, &before);
    working.owner.drain();
    assert!(matches!(working.owner.leaves[0].phase, LeafPhase::Working));
    working.owner.revise_mentions(&after);
    let first = test_support::wait::wait_for(
        "the leaf to be published",
        test_support::wait::LIVENESS,
        || {
            working.owner.drain();
            working.shown(0, &after)
        },
    );
    assert_eq!(
        chips(&first),
        1,
        "the obsolete unmarked leaf was never shown"
    );
}

#[test]
fn a_chip_added_then_removed_while_it_is_marked_never_appears() {
    let (_directory, _core, _config, provider) = owner_fixture();
    let mut stored = message_source(provider, "run $review now");
    let before = vocabulary(&[]);
    stored.waiting(0, &before);
    stored.drain_until("the leaf to be prepared", |updates| !updates.is_empty());
    let old = stored.shown(0, &before).expect("ready leaf");

    stored.owner.revise_mentions(&vocabulary(&["review"]));
    assert!(matches!(
        stored.owner.leaves[0].phase,
        LeafPhase::Remarking(_)
    ));
    let removed = vocabulary(&[]);
    stored.owner.revise_mentions(&removed);
    test_support::wait::wait_until(
        "the obsolete marking to settle",
        test_support::wait::LIVENESS,
        || {
            assert!(
                !remarked(&stored.owner.drain(), 0),
                "the obsolete chip was published"
            );
            matches!(stored.owner.leaves[0].phase, LeafPhase::Ready(_))
        },
    );
    let shown = stored.shown(0, &removed).expect("still shown");
    assert!(Rc::ptr_eq(&shown, &old));
    assert_eq!(chips(&shown), 0);
}

fn used(stored: &MessageSource, budget: usize) -> usize {
    budget - stored.owner.prepared_capacity.available_permits()
}

fn prepared_both(
    provider: ScopedSourceProvider,
    text: &str,
    budget: usize,
    before: &Arc<artisan_mentions::MessageVocabulary>,
) -> (MessageSource, Rc<StoredLeaf>, Rc<StoredLeaf>) {
    let mut stored = budgeted_source(provider, text, budget, PREPARED_BYTES / 8);
    stored.waiting(0, before);
    stored.waiting(1, before);
    stored.drain_until("both leaves to be prepared", |updates| updates.len() == 2);
    let first = stored.shown(0, before).expect("ready leaf");
    let second = stored.shown(1, before).expect("ready leaf");
    (stored, first, second)
}

fn phase_of<'a>(stored: &'a MessageSource, key: &str) -> &'a LeafPhase {
    &stored
        .owner
        .leaves
        .iter()
        .find(|entry| entry.key == key)
        .expect("the leaf is still owned")
        .phase
}

const CHIPS: usize = 20;

fn many_chips() -> String {
    vec!["$review"; CHIPS].join(" ")
}

fn two_leaves() -> String {
    format!("{}\n\n{}", many_chips(), "plain words ".repeat(1_000))
}

fn full_budget(before: &Arc<artisan_mentions::MessageVocabulary>) -> usize {
    let (_directory, _core, _config, provider) = owner_fixture();
    let (measured, _, _) = prepared_both(provider, &two_leaves(), PREPARED_BYTES, before);
    used(&measured, PREPARED_BYTES)
}

#[test]
fn a_chip_without_presentation_memory_keeps_the_shown_leaf_until_a_held_leaf_lets_go() {
    let before = vocabulary(&[]);
    let budget = full_budget(&before);
    let (_directory, _core, _config, provider) = owner_fixture();
    let (mut stored, old, held) = prepared_both(provider, &two_leaves(), budget, &before);
    assert_eq!(used(&stored, budget), budget);

    let after = vocabulary(&["review"]);
    stored.owner.revise_mentions(&after);
    test_support::wait::wait_until(
        "the marking to wait for memory",
        test_support::wait::LIVENESS,
        || {
            assert!(!remarked(&stored.owner.drain(), 0));
            matches!(phase_of(&stored, "leaf:0"), LeafPhase::RemarkBlocked { .. })
        },
    );
    let shown = stored
        .shown(0, &after)
        .expect("the shown leaf stays readable");
    assert!(Rc::ptr_eq(&shown, &old));
    drop(shown);

    drop(held);
    stored.drain_until(
        "the leaf to be marked once the held leaf lets go",
        |updates| remarked(updates, 0),
    );
    assert_eq!(chips(&stored.shown(0, &after).expect("marked leaf")), CHIPS);
}

#[test]
fn a_blocked_chip_the_vocabulary_no_longer_needs_returns_to_the_shown_leaf() {
    let before = vocabulary(&[]);
    let budget = full_budget(&before);
    let (_directory, _core, _config, provider) = owner_fixture();
    let (mut stored, old, _held) = prepared_both(provider, &two_leaves(), budget, &before);
    stored.owner.revise_mentions(&vocabulary(&["review"]));
    test_support::wait::wait_until(
        "the marking to wait for memory",
        test_support::wait::LIVENESS,
        || {
            stored.owner.drain();
            matches!(phase_of(&stored, "leaf:0"), LeafPhase::RemarkBlocked { .. })
        },
    );

    stored.owner.revise_mentions(&vocabulary(&[]));
    assert!(matches!(
        phase_of(&stored, "leaf:0"),
        LeafPhase::Ready(leaf) if Rc::ptr_eq(leaf, &old)
    ));
}

#[test]
fn a_failure_under_an_outdated_vocabulary_is_retried_with_the_current_one() {
    let text = many_chips();
    let text = text.as_str();
    let chipped = vocabulary(&["review"]);
    let plain = vocabulary(&[]);
    let index = artisan_markdown_index::MarkdownIndex::build(text).expect("index");
    let record = index.record(0).expect("record");
    let engine = MarkdownEngine::new().unwrap();
    let syntax = |vocabulary: &Arc<artisan_mentions::MessageVocabulary>| {
        super::super::source_index::normalize_message_record(
            &record,
            0,
            text,
            &engine,
            &mut super::super::NoFenceTokenMemo,
            vocabulary,
        )
        .expect("leaf")
        .content
        .resident_bytes()
    };
    assert!(syntax(&chipped) > syntax(&plain));
    let (_directory, _core, _config, provider) = owner_fixture();
    let mut stored = budgeted_source(provider, text, PREPARED_BYTES, syntax(&plain));
    stored.waiting(0, &chipped);
    stored.owner.drain();
    assert!(matches!(phase_of(&stored, "leaf:0"), LeafPhase::Working));
    stored.owner.revise_mentions(&plain);
    let leaf = test_support::wait::wait_for(
        "the leaf to be prepared with the current vocabulary",
        test_support::wait::LIVENESS,
        || {
            stored.owner.drain();
            stored.shown(0, &plain)
        },
    );
    assert_eq!(chips(&leaf), 0);
}
