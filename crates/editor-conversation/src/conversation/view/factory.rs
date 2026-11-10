use super::{row_frame, status, transcript::BatchedRowFactory, work};
use crate::conversation::adapter::RowModel;
use crate::conversation::disclosure::RowDisclosure;
use crate::conversation::fold::{self, Row, RowKey, Slot};
use crate::conversation::presentation::{
    BodyKey, MarkdownOwners, MarkdownRender, PresentationScope, ScopedCodeAction,
};
use editor_ui::markdown::presenter::HeatmapServices;
use editor_ui::markdown::visual::{BodyTone, LinkAction};
use editor_ui::rich::actions::{ResourceScope, ShowCopyMenu};
use editor_ui::theme::ArtisanTheme;
use aqua_text::document::SelectionSourceId;
use aqua_ui::collection::BatchedCollectionRow;
use aqua_ui::collection_runtime::CollectionError;
use artisan_api::{ItemKind, Status, TextContent};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

fn truncate(
    text: String,
    color: aqua_core::GpuiTextColor,
    mono: bool,
    theme: ArtisanTheme,
) -> aqua_ui::element::Element {
    use aqua_text::{FontFamily, TextStyle, overflow::OverflowMode};
    use aqua_ui::style::{Length, Style};
    let style = TextStyle {
        families: vec![FontFamily::Named(
            if mono {
                theme.typography.mono.family
            } else {
                theme.typography.sans.family
            }
            .to_owned(),
        )],
        size_px: if mono {
            theme.typography.control_text
        } else {
            editor_ui::theme::ProseTypography::BODY_SIZE_PX
        },
        line_height_px: 24.0,
        weight: editor_ui::theme::ProseTypography::BODY_WEIGHT,
        letter_spacing_em: Some(if mono {
            0.0
        } else {
            theme.typography.tracking_em
        }),
        ..TextStyle::default()
    };
    aqua_ui::element::Text::new(text)
        .text_style(style)
        .gpui_source_color(color)
        .overflow(OverflowMode::EndEllipsis)
        .style(
            Style::new()
                .min_width(Length::Px(0.0))
                .flex_grow(1.0)
                .max_height(Length::Px(24.0)),
        )
        .build()
}

fn user_bubble(
    content: aqua_ui::element::Element,
    index: usize,
    count: usize,
    empty: bool,
    attachments: Option<aqua_ui::element::Element>,
    sender: Option<artisan_api::EngineId>,
    theme: ArtisanTheme,
) -> aqua_ui::element::Element {
    use editor_ui::chat_bubble::{ChatBubbleSegment, SegmentPosition};
    use aqua_ui::element::Box as UiBox;
    use aqua_ui::style::{Length, Style};
    let mut root = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .gap(8.0)
        .style(Style::new().align(aqua_ui::AlignItems::END));
    if let Some(attachments) = attachments {
        root = root.child(attachments);
    }
    if empty {
        return root.build();
    }
    let bubble = ChatBubbleSegment::new(theme, SegmentPosition::for_index(index, count), content);
    let bubble = match sender {
        Some(engine) => bubble.tinted(match engine {
            artisan_api::EngineId::Claude => theme.colors.claude,
            artisan_api::EngineId::Codex => theme.colors.text,
            artisan_api::EngineId::OpenCode => theme.colors.gray.step(9),
        }),
        None => bubble,
    };
    root.child(bubble.build()).build()
}

struct WorkResources {
    motion: Rc<RefCell<super::motion::RowMotion>>,
    names: HashMap<artisan_api::ItemId, String>,
    now: Instant,
    seconds: f64,
    timestamp: artisan_api::Timestamp,
}

struct RevealResources {
    owners: Rc<RefCell<super::reveal::MountedReveals>>,
    arrivals: Rc<
        RefCell<HashMap<artisan_api::ItemId, crate::conversation::admission::LiveBlockArrivals>>,
    >,
    live: HashMap<artisan_api::ItemId, crate::conversation::live::LiveBinding>,
    now: Instant,
    swift: bool,
}

struct LazyRows {
    snapshots: crate::conversation::adapter::RowSnapshots,
    disclosure: Vec<RowDisclosure>,
    held: Vec<bool>,
}

#[derive(Clone)]
pub enum BlobAction {
    Load {
        hash: artisan_api::BlobHash,
        retry: bool,
    },
    Open {
        hash: artisan_api::BlobHash,
        image: aqua::ImageHandle,
        opener_key: String,
    },
}

struct BlobResources {
    images: editor_core::host::BlobImages,
    emit: Rc<dyn Fn(artisan_api::ItemId, u64, BlobAction)>,
}

struct SourceResources {
    provider: editor_core::host::ScopedSourceProvider,
    document: aqua_text::document::SelectionDocumentId,
    emit: Rc<dyn Fn(super::row_sources::RowSourceAction)>,
}

struct StoredResources {
    owner: Rc<RefCell<crate::conversation::stored::StoredConversation>>,
    provider: editor_core::host::ScopedSourceProvider,
}

pub struct RealizedRow {
    pub model: RowModel,
    pub disclosure: RowDisclosure,
    pub held: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AfterLayout {
    pub repaint: bool,
    pub rebuild: bool,
}

pub struct RowFactory {
    pub rows: Vec<RealizedRow>,
    lazy_rows: Option<LazyRows>,
    pub scope: PresentationScope,
    pub markdown: Rc<RefCell<MarkdownOwners>>,
    pub statuses: Rc<RefCell<status::StatusShimmers>>,
    pub theme: ArtisanTheme,
    pub reduced_motion: bool,
    pub allocate: Rc<dyn Fn() -> SelectionSourceId>,
    pub invalidate: Rc<dyn Fn()>,
    pub on_link: Rc<dyn Fn(LinkAction)>,
    pub on_color: Rc<dyn Fn(ShowCopyMenu)>,
    pub on_code: Rc<dyn Fn(ScopedCodeAction)>,
    dispatch: Option<super::Dispatch>,
    copy: Option<Rc<dyn Fn(super::source_actions::CopySource)>>,
    work: Option<WorkResources>,
    engine: Rc<RefCell<Option<editor_ui::markdown::MarkdownEngine>>>,
    projection_versions: HashMap<BodyKey, u64>,
    shape_counts: HashMap<BodyKey, usize>,
    pending_projections: RefCell<Vec<(BodyKey, crate::conversation::projection::BodyProjection)>>,
    pending_live: RefCell<Vec<(crate::conversation::live::LiveBinding, usize)>>,
    observations: Option<super::transcript::TranscriptObservations>,
    admission: Option<Rc<RefCell<crate::conversation::admission::RevealQueue>>>,
    reveals: Option<RevealResources>,
    media:
        Option<Rc<dyn Fn(&BodyKey, f32) -> Option<editor_ui::markdown::presenter::MediaServices>>>,
    blob_images: Option<BlobResources>,
    source_resources: Option<SourceResources>,
    stored: Option<StoredResources>,
    row_cache: Rc<RefCell<super::row_cache::RowCache>>,
    realized_owners: RefCell<HashMap<RowKey, std::rc::Weak<super::row_cache::RowOwner>>>,
    compactions: RefCell<
        HashMap<
            RowKey,
            (
                crate::conversation::compaction::CompactionPhase,
                bool,
                crate::conversation::compaction::CompactionFrame,
            ),
        >,
    >,
    active: Rc<Cell<bool>>,
    live: Cell<bool>,
    settled: Rc<RefCell<HashMap<RowKey, u64>>>,
    style_epoch: u64,
    unsettled: Cell<bool>,
    content_keyed: Cell<bool>,
}

pub struct RowFingerprint<'a> {
    pub model: &'a RowModel,
    pub disclosure: &'a RowDisclosure,
    pub held: bool,
    pub body_version: Option<u64>,
    pub style_epoch: u64,
}

impl RowFingerprint<'_> {
    pub fn value(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.model.row.hash(&mut hasher);
        self.model.version.hash(&mut hasher);
        self.model.gap.to_bits().hash(&mut hasher);
        self.model.end.to_bits().hash(&mut hasher);
        let mut ui: Vec<_> = self.model.ui.iter().collect();
        ui.sort_by(|a, b| a.0.to_string().cmp(&b.0.to_string()));
        for (id, state) in ui {
            id.hash(&mut hasher);
            state.hash(&mut hasher);
        }
        format!("{:?}", self.disclosure).hash(&mut hasher);
        self.held.hash(&mut hasher);
        self.body_version.hash(&mut hasher);
        self.style_epoch.hash(&mut hasher);
        hasher.finish()
    }
}

pub fn row_settles_statically(model: &RowModel) -> bool {
    let status = matches!(model.row.semantic(), Row::Status(_));
    (status
        || matches!(
            model.row.semantic(),
            Row::Text { .. } | Row::Chain { .. } | Row::WorkEntry { .. } | Row::Work { .. }
        ))
        && model.items.values().all(|item| item.status != Status::Running)
        && model.ui.values().all(|ui| !ui.fresh())
        && (status || model.live.is_empty())
}

impl RowFactory {
    fn stored_row(
        &self,
        row: &RealizedRow,
        row_owner: &Rc<super::row_cache::RowOwner>,
        item: &artisan_api::Item,
        slot: Slot,
        block: usize,
        reference: &artisan_api::ContentRef,
        reveal: &Option<(
            Rc<editor_ui::markdown::reveal::PreparedReveal>,
            bool,
            Rc<Cell<bool>>,
        )>,
        content_width: Option<f32>,
        batches: &mut Vec<Arc<aqua_ui::collection::SelectionSourceBatch>>,
    ) -> anyhow::Result<(
        aqua_ui::element::Element,
        Option<crate::conversation::stored::StoredProof>,
    )> {
        use editor_ui::markdown::stored::{LeafPreparation, StoredPreparation};
        use aqua_ui::element::Text;
        let services = self
            .stored
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("stored conversation owner is unavailable"))?;
        let key = BodyKey {
            item: item.id.clone(),
            slot,
        };
        let version = self
            .projection_versions
            .get(&key)
            .copied()
            .unwrap_or(row.model.version);
        let pending = |text: &str| {
            self.unsettled.set(true);
            Text::new(text.to_owned()).build()
        };
        let state = services.owner.borrow_mut().ensure(
            &self.scope,
            &services.provider,
            key.clone(),
            reference.clone(),
            version,
        )?;
        let source = match state {
            StoredPreparation::Pending => return Ok((pending("Loading formatted content…"), None)),
            StoredPreparation::Failed(error) => {
                let collected = RefCell::new(Vec::new());
                let used = RefCell::new(std::collections::HashSet::new());
                let full = self.source_field(
                    row_owner,
                    item,
                    crate::conversation::content::SourceField::of_slot(slot),
                    &collected,
                    &used,
                )?;
                row_owner.retain_batches(&used.into_inner());
                batches.extend(collected.into_inner());
                return Ok((
                    aqua_ui::element::Box::new()
                        .layout("column")
                        .child(pending(&error))
                        .child(full)
                        .build(),
                    None,
                ));
            }
            StoredPreparation::Ready(source) => source,
        };
        if block >= source.block_count() || source.descriptor(block).is_none() {
            return Ok((pending("Content layout is updating…"), None));
        }
        let leaf = match services.owner.borrow_mut().leaf(&key, &source, block) {
            LeafPreparation::Pending => return Ok((pending("Loading content…"), None)),
            LeafPreparation::Failed(error) => {
                let collected = RefCell::new(Vec::new());
                let used = RefCell::new(std::collections::HashSet::new());
                let full = self.source_field(
                    row_owner,
                    item,
                    crate::conversation::content::SourceField::of_slot(slot),
                    &collected,
                    &used,
                )?;
                row_owner.retain_batches(&used.into_inner());
                batches.extend(collected.into_inner());
                return Ok((
                    aqua_ui::element::Box::new()
                        .layout("column")
                        .child(pending(&error))
                        .child(full)
                        .build(),
                    None,
                ));
            }
            LeafPreparation::Ready(leaf) => leaf,
        };
        let document = self
            .source_resources
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("conversation source services are unavailable"))?
            .document;
        if let (Some((owner, fresh, ready)), Some(resources)) = (reveal, &self.reveals) {
            ready.set(false);
            owner.begin_frame(
                resources.now,
                false,
                *fresh,
                self.reduced_motion,
                resources.swift,
            );
            owner.set_paused(false);
            owner.set_rushed(
                self.admission
                    .as_ref()
                    .is_some_and(|queue| queue.borrow().rushed(item.position)),
            );
        }
        let mut order = fold::selection_order(self.scope.agent.is_some(), item.position, slot);
        order.push(block as u64);
        let media = if let (Some(width), Some(factory)) = (content_width, &self.media) {
            let mut width = width.min(768.0) - 48.0;
            if slot == Slot::Text(fold::TextSection::User) {
                width = width.min(576.0) - 32.0;
            }
            width -= leaf.context.lists.len() as f32
                * editor_ui::theme::ProseTypography::LIST_INDENT_PX;
            anyhow::ensure!(width > 0.0, "measured media width is unavailable");
            factory(&key, width)
        } else {
            None
        };
        let proof_leaf = leaf.proof().clone();
        let mut stored = services.owner.borrow_mut();
        let proof = stored
            .proof(&self.scope, &key, &source, block, item.seq, &proof_leaf)
            .ok_or_else(|| anyhow::anyhow!("stored Markdown proof is stale"))?;
        let emit = self.on_code.clone();
        let code_scope = self.scope.clone();
        let code_key = key.clone();
        let code_proof = proof.clone();
        let code_owner = Rc::downgrade(&services.owner);
        let code_emit = Rc::new(move |action| {
            emit(ScopedCodeAction::stored(
                code_scope.clone(),
                code_key.clone(),
                code_owner.clone(),
                code_proof.clone(),
                action,
            ));
        });
        let (rendered, lease) = stored.render_leaf(
            &key,
            &source,
            block,
            leaf,
            document,
            &order,
            &self.theme,
            BodyTone::Foreground,
            self.allocate.as_ref(),
            self.invalidate.clone(),
            self.on_link.clone(),
            self.on_color.clone(),
            ResourceScope {
                host: self.scope.host.to_string(),
                thread: self.scope.thread.clone(),
            },
            reveal.as_ref().map(|(owner, _, _)| owner.clone()),
            media,
            code_emit,
            self.scope.instance,
            self.reduced_motion,
        )?;
        *row_owner.stored_leaf.borrow_mut() = Some(lease);
        let prepared = if let (Some(resources), Some(_)) = (&self.reveals, reveal) {
            resources.owners.borrow_mut().record_stored(
                row_owner,
                row.model.version,
                block,
                proof.clone(),
                rendered,
            )
        } else {
            BatchedCollectionRow {
                element: rendered.element,
                batches: vec![rendered.selection_batch],
            }
        };
        batches.extend(prepared.batches);
        Ok((prepared.element, Some(proof)))
    }
    fn literal_row(
        &self,
        row_owner: &Rc<super::row_cache::RowOwner>,
        item: &artisan_api::Item,
        slot: Slot,
        section: fold::PlainSection,
        block: usize,
        reference: &artisan_api::ContentRef,
        version: u64,
        batches: &mut Vec<std::sync::Arc<aqua_ui::collection::SelectionSourceBatch>>,
    ) -> anyhow::Result<aqua_ui::element::Element> {
        use crate::conversation::view::resources::ResourceKey;
        use editor_ui::markdown::literal::LiteralPreparation;
        use editor_ui::markdown::stored::StoredPreparation;
        use aqua_ui::element::Text;
        let services = self
            .stored
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("stored conversation owner is unavailable"))?;
        let key = BodyKey {
            item: item.id.clone(),
            slot,
        };
        let version = self
            .projection_versions
            .get(&key)
            .copied()
            .unwrap_or(version);
        let pending = |text: &str| {
            self.unsettled.set(true);
            Text::new(text.to_owned()).build()
        };
        let state = services.owner.borrow_mut().ensure(
            &self.scope,
            &services.provider,
            key.clone(),
            reference.clone(),
            version,
        )?;
        let source = match state {
            StoredPreparation::Pending => return Ok(pending("Loading content…")),
            StoredPreparation::Failed(error) => return Ok(pending(&error)),
            StoredPreparation::Ready(source) => source,
        };
        if block >= source.block_count() || source.descriptor(block).is_none() {
            return Ok(pending("Content layout is updating…"));
        }
        let leaf = match services
            .owner
            .borrow_mut()
            .literal_leaf(&key, &source, block)
        {
            LiteralPreparation::Pending => return Ok(pending("Loading content…")),
            LiteralPreparation::Failed(error) => {
                let collected = RefCell::new(Vec::new());
                let used = RefCell::new(std::collections::HashSet::new());
                let full = self.source_field(
                    row_owner,
                    item,
                    crate::conversation::content::SourceField::of_slot(slot),
                    &collected,
                    &used,
                )?;
                row_owner.retain_batches(&used.into_inner());
                batches.extend(collected.into_inner());
                return Ok(aqua_ui::element::Box::new()
                    .layout("column")
                    .child(pending(&error))
                    .child(full)
                    .build());
            }
            LiteralPreparation::Ready(leaf) => leaf,
        };
        if leaf.needs_native_window() {
            let collected = RefCell::new(Vec::new());
            let used = RefCell::new(std::collections::HashSet::new());
            let full = self.source_field(
                row_owner,
                item,
                crate::conversation::content::SourceField::of_slot(slot),
                &collected,
                &used,
            )?;
            row_owner.retain_batches(&used.into_inner());
            batches.extend(collected.into_inner());
            return Ok(full);
        }
        let document = self
            .source_resources
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("conversation source services are unavailable"))?
            .document;
        let field = crate::conversation::content::SourceField::of_slot(slot);
        let resource = ResourceKey::Source(item.id.clone(), field);
        let id = *row_owner
            .source_ids
            .borrow_mut()
            .entry(resource.clone())
            .or_insert_with(|| (self.allocate)());
        let mut order = fold::selection_order(self.scope.agent.is_some(), item.position, slot);
        order.push(section.order());
        order.push(block as u64);
        let separator = if block == 0 { "\n\n" } else { "" };
        let selection = leaf.selection(id, order, separator);
        let visible = leaf.visible_text().to_owned();
        let (size_px, line_height_px, color) = match section {
            fold::PlainSection::ImageCaption => (
                self.theme.typography.control_text,
                self.theme.typography.control_text * (4.0 / 3.0),
                self.theme.colors.text_muted.to_reference_text_color(),
            ),
            _ => (
                self.theme.typography.editor_text_desktop,
                self.theme.spacing.steps(6.0),
                self.theme.colors.text.to_reference_text_color(),
            ),
        };
        let element = Text::new(visible.clone())
            .text_style(aqua_text::TextStyle {
                families: vec![aqua_text::FontFamily::Named(
                    self.theme.typography.sans.family.to_owned(),
                )],
                size_px,
                line_height_px,
                letter_spacing_em: Some(self.theme.typography.tracking_em),
                ..Default::default()
            })
            .gpui_source_color(color)
            .selection_source_in(document, editor_sources::source_slice(id, 0..visible.len()))
            .build();
        let view = editor_sources::ContentView {
            element,
            selection: Some(selection.source().clone()),
        };
        let mut retained = row_owner.batches.borrow_mut();
        anyhow::ensure!(retained.set_scope(&self.scope), "stale literal batch scope");
        let prepared = retained.row(&self.scope, resource, document, view)?;
        batches.extend(prepared.batches);
        Ok(prepared.element)
    }
    fn approval_card(
        &self,
        owner: &Rc<super::row_cache::RowOwner>,
        item: &artisan_api::Item,
        ui: Option<&crate::conversation::thread::ItemUi>,
        dispatch: super::Dispatch,
        batches: &mut Vec<std::sync::Arc<aqua_ui::collection::SelectionSourceBatch>>,
    ) -> anyhow::Result<Option<aqua_ui::element::Element>> {
        let collected = RefCell::new(Vec::new());
        let used = RefCell::new(std::collections::HashSet::new());
        let result = super::request::approval(
            item,
            ui.is_some_and(|ui| ui.sending),
            ui.and_then(|ui| ui.error.as_deref()),
            self.theme,
            |field| self.source_field(owner, item, field, &collected, &used),
            dispatch,
        )?;
        owner.retain_batches(&used.into_inner());
        batches.extend(collected.into_inner());
        Ok(result)
    }

    fn notice_row(
        &self,
        owner: &Rc<super::row_cache::RowOwner>,
        item: &artisan_api::Item,
        batches: &mut Vec<std::sync::Arc<aqua_ui::collection::SelectionSourceBatch>>,
    ) -> anyhow::Result<Option<aqua_ui::element::Element>> {
        let collected = RefCell::new(Vec::new());
        let used = RefCell::new(std::collections::HashSet::new());
        let result = super::notice::render(
            item,
            self.theme,
            |text, color| truncate(text, color, false, self.theme),
            |field| self.source_field(owner, item, field, &collected, &used),
        )?;
        owner.retain_batches(&used.into_inner());
        batches.extend(collected.into_inner());
        Ok(result)
    }

    fn question_card(
        &self,
        owner: &Rc<super::row_cache::RowOwner>,
        item: &artisan_api::Item,
        batches: &mut Vec<std::sync::Arc<aqua_ui::collection::SelectionSourceBatch>>,
    ) -> anyhow::Result<Option<aqua_ui::element::Element>> {
        use crate::conversation::content::SourceField;
        let collected = RefCell::new(Vec::new());
        let used = RefCell::new(std::collections::HashSet::new());
        let result = super::request::question(
            item,
            self.theme,
            |field| self.source_field(owner, item, field, &collected, &used),
            |source| self.question_collection(owner, item, source, &collected, &used),
            |item, questions| {
                super::request::answered(
                    questions,
                    self.theme,
                    |field| self.source_field(owner, item, field, &collected, &used),
                    |source| self.question_collection(owner, item, source, &collected, &used),
                    || self.source_field(owner, item, SourceField::AnswerText, &collected, &used),
                )
            },
        )?;
        owner.retain_batches(&used.into_inner());
        batches.extend(collected.into_inner());
        Ok(result)
    }
    fn source_field(
        &self,
        owner: &Rc<super::row_cache::RowOwner>,
        item: &artisan_api::Item,
        field: crate::conversation::content::SourceField,
        batches: &RefCell<Vec<std::sync::Arc<aqua_ui::collection::SelectionSourceBatch>>>,
        used: &RefCell<std::collections::HashSet<super::resources::ResourceKey>>,
    ) -> anyhow::Result<aqua_ui::element::Element> {
        use crate::conversation::view::resources::ResourceKey;
        use crate::conversation::view::row_sources::{RowSourceAction, RowSourceActionKind};
        use aqua_ui::element::Text;
        let theme = match &field {
            crate::conversation::content::SourceField::Question(_) => {
                super::request::question_theme(item, self.theme)
            }
            _ => self.theme,
        };
        let source = field
            .text(item)
            .ok_or_else(|| anyhow::anyhow!("conversation source field is unavailable"))?;
        if let TextContent::Inline(text) = source {
            if field == crate::conversation::content::SourceField::AnswerText {
                let services = self.source_resources.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("conversation source services are unavailable")
                })?;
                let key = ResourceKey::Source(item.id.clone(), field);
                used.borrow_mut().insert(key.clone());
                let id = *owner
                    .source_ids
                    .borrow_mut()
                    .entry(key.clone())
                    .or_insert_with(|| (self.allocate)());
                let text = text.replace("You answered: ", "");
                let source = aqua_text::document::SelectionSource {
                    id,
                    order: fold::selection_order(
                        self.scope.agent.is_some(),
                        item.position,
                        Slot::Answer,
                    ),
                    text: std::sync::Arc::from(text.as_str()),
                    separator: std::sync::Arc::from("\n\n"),
                    copies: Vec::new(),
                };
                let element = Text::new(text.clone())
                    .wrap(true)
                    .style(
                        aqua_ui::style::Style::new()
                            .width(aqua_ui::style::Length::Pct(100.0))
                            .min_width(aqua_ui::style::Length::Px(0.0)),
                    )
                    .gpui_source_color(theme.colors.text.to_reference_text_color())
                    .selection_source_in(
                        services.document,
                        editor_sources::source_slice(id, 0..text.len()),
                    )
                    .build();
                let view = editor_sources::ContentView {
                    element,
                    selection: Some(source),
                };
                let mut retained = owner.batches.borrow_mut();
                anyhow::ensure!(
                    retained.set_scope(&self.scope),
                    "stale conversation answer batch scope"
                );
                let row = retained.row(&self.scope, key, services.document, view)?;
                batches.borrow_mut().extend(row.batches);
                return Ok(row.element);
            }
            let mut element = Text::new(text.clone())
                .text_style(aqua_text::TextStyle {
                    families: vec![aqua_text::FontFamily::Named(
                        if field == crate::conversation::content::SourceField::ApprovalCommand {
                            theme.typography.mono.family
                        } else {
                            theme.typography.sans.family
                        }
                        .to_owned(),
                    )],
                    size_px: theme.typography.editor_text_desktop,
                    line_height_px: theme.spacing.steps(6.0),
                    letter_spacing_em: Some(
                        if field == crate::conversation::content::SourceField::ApprovalCommand {
                            0.0
                        } else {
                            theme.typography.tracking_em
                        },
                    ),
                    ..Default::default()
                })
                .gpui_source_color(theme.colors.text.to_reference_text_color());
            if matches!(field, crate::conversation::content::SourceField::Question(_)) {
                element = element.wrap(true).style(
                    aqua_ui::style::Style::new()
                        .width(aqua_ui::style::Length::Pct(100.0))
                        .min_width(aqua_ui::style::Length::Px(0.0)),
                );
            }
            return Ok(element.build());
        }
        let services = self
            .source_resources
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("conversation source services are unavailable"))?;
        anyhow::ensure!(
            services.provider.is_available(),
            "conversation source provider is unavailable"
        );
        let key = ResourceKey::Source(item.id.clone(), field.clone());
        used.borrow_mut().insert(key.clone());
        let id = *owner
            .source_ids
            .borrow_mut()
            .entry(key.clone())
            .or_insert_with(|| (self.allocate)());
        anyhow::ensure!(
            owner
                .sources
                .borrow_mut()
                .open_provider(&services.provider, item, field.clone()),
            "conversation source is unavailable"
        );
        let scope = self.scope.clone();
        let provider = services.provider.clone();
        let emit = services.emit.clone();
        let weak = Rc::downgrade(owner);
        let item_id = item.id.clone();
        let seq = item.seq;
        let callback = Rc::new(move |action| {
            emit(RowSourceAction {
                item: item_id.clone(),
                seq,
                scope: scope.clone(),
                provider: provider.clone(),
                owner: weak.clone(),
                action: RowSourceActionKind::Content(action),
            })
        });
        let view = owner
            .sources
            .borrow()
            .view(&item.id, &field, theme, services.document, id, callback)
            .ok_or_else(|| anyhow::anyhow!("conversation source view is unavailable"))?;
        let mut retained = owner.batches.borrow_mut();
        anyhow::ensure!(
            retained.set_scope(&self.scope),
            "stale conversation source batch scope"
        );
        let row = retained.row(&self.scope, key, services.document, view)?;
        batches.borrow_mut().extend(row.batches);
        Ok(row.element)
    }

    fn question_collection(
        &self,
        owner: &Rc<super::row_cache::RowOwner>,
        item: &artisan_api::Item,
        source: &artisan_api::CollectionRef,
        batches: &RefCell<Vec<std::sync::Arc<aqua_ui::collection::SelectionSourceBatch>>>,
        used: &RefCell<std::collections::HashSet<super::resources::ResourceKey>>,
    ) -> anyhow::Result<aqua_ui::element::Element> {
        use crate::conversation::view::resources::ResourceKey;
        use crate::conversation::view::row_sources::{RowSourceAction, RowSourceActionKind};
        let theme = super::request::question_theme(item, self.theme);
        let services = self
            .source_resources
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("conversation source services are unavailable"))?;
        anyhow::ensure!(
            services.provider.is_available(),
            "conversation source provider is unavailable"
        );
        let key = ResourceKey::Questions(item.id.clone());
        used.borrow_mut().insert(key.clone());
        let id = *owner
            .source_ids
            .borrow_mut()
            .entry(key.clone())
            .or_insert_with(|| (self.allocate)());
        anyhow::ensure!(
            owner
                .questions
                .borrow_mut()
                .open_provider(&services.provider, &item.id, source),
            "question collection is unavailable"
        );
        let stored_detail = if let Some(browser) = owner.questions.borrow().browser() {
            let browser = browser.borrow();
            if let Some(detail) = &browser.detail {
                let detail_key = ResourceKey::QuestionDetail(item.id.clone());
                used.borrow_mut().insert(detail_key.clone());
                let detail_id = *owner
                    .source_ids
                    .borrow_mut()
                    .entry(detail_key.clone())
                    .or_insert_with(|| (self.allocate)());
                let detail_source = detail.window().source().clone();
                let identity = detail.identity();
                let collection = source.clone();
                let scope = self.scope.clone();
                let provider = services.provider.clone();
                let emit = services.emit.clone();
                let weak = Rc::downgrade(owner);
                let item_id = item.id.clone();
                let seq = item.seq;
                let callback = Rc::new(move |action| {
                    emit(RowSourceAction {
                        item: item_id.clone(),
                        seq,
                        scope: scope.clone(),
                        provider: provider.clone(),
                        owner: weak.clone(),
                        action: RowSourceActionKind::QuestionDetail {
                            collection: collection.clone(),
                            source: detail_source.clone(),
                            identity,
                            action,
                        },
                    })
                });
                let view = editor_sources::content_view(
                    detail,
                    theme,
                    services.document,
                    detail_id,
                    callback,
                );
                let mut retained = owner.batches.borrow_mut();
                anyhow::ensure!(
                    retained.set_scope(&self.scope),
                    "stale conversation detail batch scope"
                );
                let row = retained.row(&self.scope, detail_key, services.document, view)?;
                batches.borrow_mut().extend(row.batches);
                Some(row.element)
            } else {
                None
            }
        } else {
            None
        };
        let scope = self.scope.clone();
        let provider = services.provider.clone();
        let emit = services.emit.clone();
        let weak = Rc::downgrade(owner);
        let item_id = item.id.clone();
        let seq = item.seq;
        let callback = Rc::new(move |action| {
            emit(RowSourceAction {
                item: item_id.clone(),
                seq,
                scope: scope.clone(),
                provider: provider.clone(),
                owner: weak.clone(),
                action: RowSourceActionKind::Questions(action),
            })
        });
        let view = owner
            .questions
            .borrow()
            .view(theme, services.document, id, stored_detail, callback)
            .map_err(anyhow::Error::msg)?
            .ok_or_else(|| anyhow::anyhow!("question collection view is unavailable"))?;
        let mut retained = owner.batches.borrow_mut();
        anyhow::ensure!(
            retained.set_scope(&self.scope),
            "stale conversation question batch scope"
        );
        let row = retained.row(&self.scope, key, services.document, view)?;
        batches.borrow_mut().extend(row.batches);
        Ok(row.element)
    }
    fn user_attachments(
        &self,
        item: &artisan_api::Item,
    ) -> anyhow::Result<Option<aqua_ui::element::Element>> {
        use aqua_ui::element::Box as UiBox;
        use aqua_ui::style::{Length, Style};
        let ItemKind::UserMessage { images, .. } = &item.kind else {
            return Ok(None);
        };
        if images.is_empty() {
            return Ok(None);
        }
        let mut row = UiBox::new().layout("row").gap(8.0).style(
            Style::new()
                .wrap(true)
                .max_width(Length::Px(576.0))
                .justify(aqua_ui::JustifyContent::END),
        );
        for (index, hash) in images.iter().enumerate() {
            row = row.child(self.blob_frame(item, hash, Some(index), 96.0)?);
        }
        Ok(Some(row.build()))
    }

    fn blob_frame(
        &self,
        item: &artisan_api::Item,
        hash: &artisan_api::BlobHash,
        attachment: Option<usize>,
        width: f32,
    ) -> anyhow::Result<aqua_ui::element::Element> {
        use editor_core::host::BlobImage;
        use editor_ui::button::{Button, ButtonVariant};
        use editor_ui::media_frame::MediaFrame;
        use aqua_core::{GpuiBackground, Insets, image::ImageFit};
        use aqua_ui::element::{Box as UiBox, Media, Text};
        use aqua_ui::style::{Length, Style};
        let resources = self
            .blob_images
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("conversation image services are unavailable"))?;
        let state = resources.images.lookup(hash);
        if !matches!(&state, Some(BlobImage::Ready(_))) {
            self.unsettled.set(true);
        }
        let image = match &state {
            Some(BlobImage::Ready(image)) => Some((**image).clone()),
            _ => None,
        };
        let content = match (&state, &image) {
            (_, Some(image)) if attachment.is_some() => Media::new(image.clone())
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0))
                .fit(ImageFit::Cover)
                .style(
                    Style::new().radius(Length::Px(MediaFrame::sent_attachment_content_radius())),
                )
                .build(),
            (_, Some(image)) => MediaFrame::image_content(image.clone()),
            (Some(BlobImage::Failed(error)), _) => {
                let emit = Rc::clone(&resources.emit);
                let active = Rc::clone(&self.active);
                let id = item.id.clone();
                let seq = item.seq;
                let retry_hash = hash.clone();
                let retry_id = attachment.map_or_else(
                    || format!("retry-image-{hash}"),
                    |index| format!("retry-sent-{}-{index}", item.id),
                );
                let retry = Button::text(retry_id, self.theme, "Retry")
                    .variant(ButtonVariant::Ghost)
                    .on_activate(move |_| {
                        if active.get() {
                            emit(
                                id.clone(),
                                seq,
                                BlobAction::Load {
                                    hash: retry_hash.clone(),
                                    retry: true,
                                },
                            );
                        }
                    })
                    .build();
                let message = if attachment.is_some() {
                    "Couldn't load".to_owned()
                } else {
                    format!("Couldn't load the image: {error}")
                };
                let text = Text::new(message)
                    .text_style(aqua_text::TextStyle {
                        size_px: if attachment.is_some() {
                            self.theme.typography.label_text
                        } else {
                            self.theme.typography.control_text
                        },
                        ..Default::default()
                    })
                    .gpui_source_color(self.theme.colors.text_muted.to_reference_text_color())
                    .build();
                UiBox::new()
                    .layout("column")
                    .width(Length::Pct(100.0))
                    .height(Length::Pct(100.0))
                    .style(
                        Style::new()
                            .align(aqua_ui::AlignItems::CENTER)
                            .justify(aqua_ui::JustifyContent::CENTER)
                            .padding_insets(Insets {
                                left: if attachment.is_some() { 8.0 } else { 0.0 },
                                right: if attachment.is_some() { 8.0 } else { 0.0 },
                                ..Default::default()
                            }),
                    )
                    .child(text)
                    .child(retry)
                    .build()
            }
            _ => {
                let mut style = Style::new()
                    .width(Length::Pct(100.0))
                    .height(Length::Pct(100.0));
                if attachment.is_none() {
                    style = style.gpui_background(GpuiBackground::Solid(
                        self.theme.colors.raised.to_reference_paint(),
                    ));
                }
                UiBox::new().style(style).build()
            }
        };
        let aspect = image
            .as_ref()
            .and_then(|image| match image.status() {
                aqua::images::ImageStatus::Ready { width, height } => {
                    Some(width as f32 / height.max(1) as f32)
                }
                _ => None,
            })
            .unwrap_or(16.0 / 9.0);
        let mut frame = if attachment.is_some() {
            MediaFrame::sent_attachment(self.theme, content)
        } else {
            MediaFrame::transcript_image(self.theme, aspect, width, content)
        };
        let emit = Rc::clone(&resources.emit);
        let active = Rc::clone(&self.active);
        let id = item.id.clone();
        let seq = item.seq;
        let visible_hash = hash.clone();
        frame = frame.on_visible(move || {
            if active.get() {
                emit(
                    id.clone(),
                    seq,
                    BlobAction::Load {
                        hash: visible_hash.clone(),
                        retry: false,
                    },
                );
            }
        });
        if let Some(index) = attachment {
            frame = frame.debug_selector(format!("sent-image-{}-{index}", item.id));
        }
        if let Some(image) = image {
            let opener_key = attachment.map_or_else(
                || format!("image-{hash}"),
                |index| format!("sent-image-{}-{index}", item.id),
            );
            let emit = Rc::clone(&resources.emit);
            let active = Rc::clone(&self.active);
            let id = item.id.clone();
            let open_hash = hash.clone();
            let key = opener_key.clone();
            frame = frame.on_activate(
                opener_key,
                if attachment.is_some() {
                    "View attached image"
                } else {
                    "View image"
                },
                move |_| {
                    if active.get() {
                        emit(
                            id.clone(),
                            seq,
                            BlobAction::Open {
                                hash: open_hash.clone(),
                                image: image.clone(),
                                opener_key: key.clone(),
                            },
                        );
                    }
                },
            );
        }
        let frame = frame.build();
        if attachment.is_some() {
            Ok(frame)
        } else {
            Ok(UiBox::new()
                .width(Length::Pct(100.0))
                .style(Style::new().max_width(Length::Px(576.0)))
                .child(frame)
                .build())
        }
    }

    pub fn prepare_prose(
        conversation: &mut crate::conversation::controller::Conversation,
        core: &editor_core::Core,
        app: &mut aqua::UiApp,
        markdown: &mut MarkdownOwners,
        engine: &editor_ui::markdown::MarkdownEngine,
        scope: &PresentationScope,
        id: &artisan_api::ItemId,
    ) -> anyhow::Result<bool> {
        anyhow::ensure!(
            scope.current(conversation, core),
            "stale conversation presentation scope"
        );
        let key = BodyKey {
            item: id.clone(),
            slot: Slot::Prose,
        };
        let version = conversation
            .body_version(&key)
            .ok_or_else(|| anyhow::anyhow!("conversation body is unavailable"))?;
        let item = conversation
            .view
            .as_ref()
            .and_then(|view| view.shared_item(id))
            .ok_or_else(|| anyhow::anyhow!("conversation item is unavailable"))?;
        anyhow::ensure!(
            item.status != Status::Running,
            "live prose reveal is unavailable"
        );
        let ItemKind::AssistantMessage {
            text: TextContent::Inline(source),
            ..
        } = &item.kind
        else {
            anyhow::bail!("stored or non-assistant prose is unavailable");
        };
        markdown.rebind(app, Some(scope.clone()));
        let (count, revision) =
            markdown.prepare_projected(app, scope, key.clone(), &id.to_string(), engine, source)?;
        Ok(conversation.publish_projection(
            core,
            scope,
            key,
            crate::conversation::projection::BodyProjection {
                version,
                revision,
                leaves: 0..count,
            },
        ))
    }

    pub fn new(
        rows: Vec<RealizedRow>,
        scope: PresentationScope,
        markdown: Rc<RefCell<MarkdownOwners>>,
        statuses: Rc<RefCell<status::StatusShimmers>>,
        theme: ArtisanTheme,
        reduced_motion: bool,
        allocate: Rc<dyn Fn() -> SelectionSourceId>,
        invalidate: Rc<dyn Fn()>,
        on_link: Rc<dyn Fn(LinkAction)>,
        on_color: Rc<dyn Fn(ShowCopyMenu)>,
        on_code: Rc<dyn Fn(ScopedCodeAction)>,
    ) -> Self {
        statuses.borrow_mut().set_scope(&scope);
        let mut row_cache = super::row_cache::RowCache::default();
        row_cache.rebind(Some(scope.clone()));
        Self {
            rows,
            lazy_rows: None,
            scope,
            markdown,
            statuses,
            theme,
            reduced_motion,
            allocate,
            invalidate,
            on_link,
            on_color,
            on_code,
            dispatch: None,
            copy: None,
            work: None,
            engine: Rc::new(RefCell::new(None)),
            projection_versions: HashMap::new(),
            shape_counts: HashMap::new(),
            pending_projections: RefCell::new(Vec::new()),
            pending_live: RefCell::new(Vec::new()),
            observations: None,
            admission: None,
            reveals: None,
            media: None,
            blob_images: None,
            source_resources: None,
            stored: None,
            row_cache: Rc::new(RefCell::new(row_cache)),
            realized_owners: RefCell::new(HashMap::new()),
            compactions: RefCell::new(HashMap::new()),
            active: Rc::new(Cell::new(true)),
            live: Cell::new(true),
            settled: Rc::default(),
            style_epoch: 0,
            unsettled: Cell::new(false),
            content_keyed: Cell::new(false),
        }
    }

    pub fn with_active(mut self, active: Rc<Cell<bool>>) -> Self {
        self.active = active;
        self
    }

    pub fn with_settled(
        mut self,
        settled: Rc<RefCell<HashMap<RowKey, u64>>>,
        style_epoch: u64,
    ) -> Self {
        self.settled = settled;
        self.style_epoch = style_epoch;
        self
    }

    fn fingerprint(&self, index: usize, key: &RowKey) -> Option<u64> {
        let lazy = self.lazy_rows.as_ref()?;
        let model = lazy.snapshots.model(index)?;
        if &model.row.key() != key {
            return None;
        }
        let body_version = match model.row.semantic() {
            Row::Text { id, slot, .. } => {
                let key = BodyKey {
                    item: id.clone(),
                    slot: *slot,
                };
                if self.shape_counts.contains_key(&key) {
                    None
                } else {
                    self.projection_versions.get(&key).copied()
                }
            }
            Row::Work { .. } if model.run.as_ref().is_some_and(|run| run.live()) => self
                .work
                .as_ref()
                .and_then(|work| u64::try_from(work.timestamp / 1_000).ok()),
            _ => None,
        };
        Some(
            RowFingerprint {
                model: &model,
                disclosure: lazy.disclosure.get(index)?,
                held: *lazy.held.get(index)?,
                body_version,
                style_epoch: self.style_epoch,
            }
            .value(),
        )
    }

    pub fn revision(&self, index: usize, key: &RowKey) -> Option<u64> {
        let fingerprint = self.fingerprint(index, key)?;
        (self.settled.borrow().get(key) == Some(&fingerprint)).then_some(fingerprint)
    }

    pub fn with_row_cache(mut self, cache: Rc<RefCell<super::row_cache::RowCache>>) -> Self {
        cache.borrow_mut().rebind(Some(self.scope.clone()));
        self.row_cache = cache;
        self
    }

    pub fn with_sources(
        mut self,
        provider: editor_core::host::ScopedSourceProvider,
        document: aqua_text::document::SelectionDocumentId,
        emit: Rc<dyn Fn(super::row_sources::RowSourceAction)>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            provider.is_available()
                && provider.host() == &self.scope.host
                && provider.instance() == self.scope.instance,
            "stale conversation source provider"
        );
        self.source_resources = Some(SourceResources {
            provider,
            document,
            emit,
        });
        Ok(self)
    }

    pub fn with_stored(
        mut self,
        owner: Rc<RefCell<crate::conversation::stored::StoredConversation>>,
        provider: editor_core::host::ScopedSourceProvider,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            provider.is_available()
                && provider.host() == &self.scope.host
                && provider.instance() == self.scope.instance,
            "stale stored conversation scope"
        );
        owner.borrow_mut().rebind(Some(self.scope.clone()));
        self.stored = Some(StoredResources { owner, provider });
        Ok(self)
    }

    pub fn with_dispatch(mut self, dispatch: super::Dispatch) -> Self {
        self.dispatch = Some(dispatch);
        self
    }

    pub fn with_copy(mut self, copy: Rc<dyn Fn(super::source_actions::CopySource)>) -> Self {
        self.copy = Some(copy);
        self
    }

    pub fn with_blob_images(
        mut self,
        images: editor_core::host::BlobImages,
        emit: Rc<dyn Fn(artisan_api::ItemId, u64, BlobAction)>,
    ) -> Self {
        self.blob_images = Some(BlobResources { images, emit });
        self
    }

    pub fn with_admission(
        mut self,
        admission: Rc<RefCell<crate::conversation::admission::RevealQueue>>,
    ) -> Self {
        self.admission = Some(admission);
        self
    }

    pub fn with_reveals(
        mut self,
        conversation: &crate::conversation::controller::Conversation,
        core: &editor_core::Core,
        owners: Rc<RefCell<super::reveal::MountedReveals>>,
        now: Instant,
        swift: bool,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            self.scope.current(conversation, core),
            "stale conversation presentation scope"
        );
        owners.borrow_mut().rebind(Some(self.scope.clone()));
        let mut live = HashMap::new();
        for target in conversation.view.iter().flat_map(|view| view.live.keys()) {
            if let artisan_api::LiveTarget::Text { item_id: id } = target
                && let Some(binding) =
                    crate::conversation::live::LiveBinding::capture(conversation, core, id)
            {
                live.entry(id.clone()).or_insert(binding);
            }
        }
        self.admission = Some(conversation.reveal.clone());
        self.reveals = Some(RevealResources {
            owners,
            arrivals: conversation.arrivals.clone(),
            live,
            now,
            swift,
        });
        Ok(self)
    }

    pub fn with_lazy_rows(
        mut self,
        conversation: &crate::conversation::controller::Conversation,
        core: &editor_core::Core,
        composer_height: Option<f32>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            self.scope.current(conversation, core),
            "stale conversation presentation scope"
        );
        let snapshots =
            crate::conversation::adapter::RowSnapshots::capture(conversation, composer_height)
                .ok_or_else(|| anyhow::anyhow!("conversation snapshot is unavailable"))?;
        let frontier = conversation.reveal_frontier.as_ref().and_then(|frontier| {
            conversation.row_position(frontier)
        });
        let mut disclosure = Vec::with_capacity(conversation.rows.len());
        let mut held = Vec::with_capacity(conversation.rows.len());
        for (index, (row, _)) in conversation.rows.iter().enumerate() {
            disclosure.push(conversation.disclosures.row_disclosure(row));
            held.push(crate::conversation::admission::behind_reveal(
                index, row, frontier,
            ));
            if let Row::Text { id, slot, .. } = row.semantic() {
                let key = BodyKey {
                    item: id.clone(),
                    slot: *slot,
                };
                if let Some(count) = conversation.body_shape_count(&key) {
                    self.shape_counts.insert(key.clone(), count);
                }
                if let Some(version) = conversation.body_version(&key) {
                    self.projection_versions.insert(key, version);
                }
            }
        }
        self.rows.clear();
        self.lazy_rows = Some(LazyRows {
            snapshots,
            disclosure,
            held,
        });
        Ok(self)
    }

    pub fn with_media(
        mut self,
        media: Rc<dyn Fn(&BodyKey, f32) -> Option<editor_ui::markdown::presenter::MediaServices>>,
    ) -> Self {
        self.media = Some(media);
        self
    }

    pub fn with_observations(
        mut self,
        observations: super::transcript::TranscriptObservations,
    ) -> Self {
        self.observations = Some(observations);
        self
    }

    pub fn set_media_services(
        &self,
        conversation: &crate::conversation::controller::Conversation,
        core: &editor_core::Core,
        key: BodyKey,
        services: editor_ui::markdown::presenter::MediaServices,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(self.live.get(), "disposed conversation row factory");
        let id = key.item.to_string();
        self.markdown.borrow_mut().set_media_services(
            conversation,
            core,
            &self.scope,
            key,
            &id,
            services,
        )?;
        (self.invalidate)();
        Ok(())
    }

    pub fn viewport_callback(
        &self,
    ) -> anyhow::Result<Rc<dyn Fn(aqua_ui::collection_runtime::CollectionViewport)>> {
        let observations = self
            .observations
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("transcript observations are unavailable"))?;
        Ok(observations.viewport_callback(self.active.clone()))
    }

    pub fn with_work(
        mut self,
        motion: Rc<RefCell<super::motion::RowMotion>>,
        names: HashMap<artisan_api::ItemId, String>,
        now: Instant,
        seconds: f64,
        timestamp: artisan_api::Timestamp,
    ) -> Self {
        {
            let mut motion = motion.borrow_mut();
            motion.rebind_scope(&self.scope);
            motion.begin_frame();
        }
        self.work = Some(WorkResources {
            motion,
            names,
            now,
            seconds,
            timestamp,
        });
        self
    }

    pub fn with_engine(
        mut self,
        engine: Rc<RefCell<Option<editor_ui::markdown::MarkdownEngine>>>,
    ) -> Self {
        self.engine = engine;
        self
    }

    pub fn with_body_versions(mut self, versions: HashMap<BodyKey, u64>) -> Self {
        self.projection_versions = versions;
        self
    }

    pub fn publish_projections(
        &self,
        conversation: &mut crate::conversation::controller::Conversation,
        core: &editor_core::Core,
    ) -> bool {
        if !self.live.get() {
            self.pending_projections.borrow_mut().clear();
            self.pending_live.borrow_mut().clear();
            return false;
        }
        for (binding, count) in std::mem::take(&mut *self.pending_live.borrow_mut()) {
            conversation.observe_live_preparation(core, &binding, count);
        }
        let pending = std::mem::take(&mut *self.pending_projections.borrow_mut());
        conversation.publish_projections(core, &self.scope, pending)
    }

    pub fn retire(&self) {
        self.active.set(false);
    }

    pub fn dispose(&self) {
        self.live.set(false);
        self.realized_owners.borrow_mut().clear();
        self.compactions.borrow_mut().clear();
        self.pending_projections.borrow_mut().clear();
        self.pending_live.borrow_mut().clear();
    }

    pub fn sync_after_layout(&self, app: &mut aqua::UiApp) -> bool {
        self.after_layout(app).repaint
    }

    pub fn after_layout(&self, app: &mut aqua::UiApp) -> AfterLayout {
        if !self.live.get() || !self.row_cache.borrow().scope_is(&self.scope) {
            return AfterLayout::default();
        }
        if self
            .work
            .as_ref()
            .is_some_and(|work| !work.motion.borrow().scope_is(&self.scope))
        {
            return AfterLayout::default();
        }
        let shimmer = self.statuses.borrow().sync(app, self.reduced_motion);
        let mut compaction = false;
        let frames = self.compactions.borrow();
        for node in app.tree().traverse() {
            let Some(key) = app.tree().get(node).and_then(|record| record.key) else {
                continue;
            };
            for (row, (phase, has_switch, frame)) in frames.iter() {
                if key == aqua_ui::tree::hash_key(aqua_ui::element::Key::from(format!("{row:?}"))) {
                    compaction |= super::compaction::sync_label(
                        app,
                        node,
                        *phase,
                        *has_switch,
                        *frame,
                        &self.theme,
                        self.reduced_motion,
                    );
                }
            }
        }
        let reveal = self.markdown.borrow().reveal_heatmaps(app);
        let stored_reveal = self
            .stored
            .as_ref()
            .is_some_and(|stored| stored.owner.borrow().sync_after_layout(app));
        let mounted: std::collections::HashSet<_> = app
            .tree()
            .traverse()
            .into_iter()
            .filter_map(|node| app.tree().get(node)?.key)
            .collect();
        let note = editor_core::diagnostics::rebuild_stats::note;
        let rebuild = note("al_compaction", compaction)
            | note("al_reveal", reveal)
            | note("al_stored", stored_reveal)
            | note(
                "al_motion",
                self.realized_owners.borrow().iter().any(|(key, owner)| {
                    mounted.contains(&aqua_ui::tree::hash_key(aqua_ui::element::Key::from(
                        format!("{key:?}"),
                    ))) && owner
                        .upgrade()
                        .is_some_and(|owner| owner.motion.borrow().animating())
                }),
            );
        AfterLayout {
            repaint: rebuild || shimmer,
            rebuild,
        }
    }

    pub fn sync_reveals(
        &self,
        app: &mut aqua::UiApp,
        conversation: &mut crate::conversation::controller::Conversation,
        core: &editor_core::Core,
    ) -> super::reveal::RevealSync {
        let Some(resources) = &self.reveals else {
            return super::reveal::RevealSync::default();
        };
        if !self.live.get()
            || !self.row_cache.borrow().scope_is(&self.scope)
            || !self.scope.current(conversation, core)
        {
            return super::reveal::RevealSync {
                reason: "factory_scope",
                pending: true,
                ..Default::default()
            };
        }
        let stored = self.stored.as_ref().map(|stored| stored.owner.borrow());
        let mut result = resources.owners.borrow_mut().sync(
            app,
            conversation,
            core,
            &mut self.markdown.borrow_mut(),
            stored.as_deref(),
        );
        if let Some(queue) = &self.admission {
            let frontier = queue.borrow().frontier_key();
            if conversation.reveal_frontier != frontier {
                conversation.reveal_frontier = frontier;
                result.changed = true;
                result.frontier_changed = true;
            }
        }
        result
    }

    pub fn sync_reveal_geometry(
        &self,
        app: &mut aqua::UiApp,
        conversation: &crate::conversation::controller::Conversation,
        core: &editor_core::Core,
        frame: super::reveal::GeometryFrame<'_>,
    ) -> Option<bool> {
        if !self.live.get() || frame.scope != &self.scope {
            return None;
        }
        super::reveal::sync_geometry(app, conversation, core, &self.markdown.borrow(), frame)
    }

    pub fn callback(self: &Rc<Self>) -> BatchedRowFactory {
        let owner = Rc::clone(self);
        Rc::new(move |index, key| {
            owner.realize(index, key).map_err(|error| {
                tracing::error!(?key, "conversation row failed: {error:#}");
                CollectionError::Validation(format!("{error:#}"))
            })
        })
    }

    pub fn layout_callback(self: &Rc<Self>) -> super::transcript::LayoutBatchedRowFactory {
        let owner = Rc::clone(self);
        Rc::new(move |index, key, layout| {
            owner
                .realize_with_layout(index, key, layout)
                .map_err(|error| {
                    tracing::error!(?key, "conversation row failed: {error:#}");
                    CollectionError::Validation(format!("{error:#}"))
                })
        })
    }

    pub fn realize(&self, index: usize, key: &RowKey) -> anyhow::Result<BatchedCollectionRow> {
        let started = Instant::now();
        let realized = self.realize_inner(index, key, None);
        editor_core::diagnostics::open_timing::realized(started.elapsed());
        realized
    }

    pub fn realize_with_layout(
        &self,
        index: usize,
        key: &RowKey,
        layout: &aqua_ui::collection_runtime::CollectionLayout,
    ) -> anyhow::Result<BatchedCollectionRow> {
        anyhow::ensure!(
            layout.content_width.is_finite() && layout.content_width > 0.0,
            "committed transcript width is unavailable"
        );
        let started = Instant::now();
        let realized = self.realize_inner(index, key, Some(layout.content_width));
        editor_core::diagnostics::open_timing::realized(started.elapsed());
        realized
    }

    fn realize_inner(
        &self,
        index: usize,
        key: &RowKey,
        content_width: Option<f32>,
    ) -> anyhow::Result<BatchedCollectionRow> {
        let _realize_span = editor_core::diagnostics::rebuild_stats::span("f_realize");
        anyhow::ensure!(self.live.get(), "stale conversation row factory");
        anyhow::ensure!(
            self.row_cache.borrow().scope_is(&self.scope),
            "stale conversation row cache"
        );
        anyhow::ensure!(
            self.work
                .as_ref()
                .is_none_or(|work| work.motion.borrow().scope_is(&self.scope)),
            "stale conversation motion scope"
        );
        let generated;
        let row = if let Some(lazy) = &self.lazy_rows {
            generated = RealizedRow {
                model: lazy
                    .snapshots
                    .model(index)
                    .ok_or_else(|| anyhow::anyhow!("conversation row is unavailable"))?,
                disclosure: *lazy
                    .disclosure
                    .get(index)
                    .ok_or_else(|| anyhow::anyhow!("conversation disclosure is unavailable"))?,
                held: *lazy
                    .held
                    .get(index)
                    .ok_or_else(|| anyhow::anyhow!("conversation admission is unavailable"))?,
            };
            &generated
        } else {
            self.rows
                .get(index)
                .ok_or_else(|| anyhow::anyhow!("conversation row is unavailable"))?
        };
        anyhow::ensure!(
            &row.model.row.key() == key,
            "stale conversation row identity"
        );
        editor_core::diagnostics::rebuild_stats::record_event(if self.row_cache.borrow().get(key).is_some() {
            "row_rerealized"
        } else {
            "row_realized"
        });
        self.unsettled.set(false);
        self.content_keyed.set(false);
        self.settled.borrow_mut().remove(key);
        let row_owner = self.row_cache.borrow().candidate(key);
        *row_owner.stored_leaf.borrow_mut() = None;
        row_owner.motion.borrow_mut().begin_frame();
        let reveal = match (&self.reveals, row.model.row.semantic()) {
            (
                Some(resources),
                Row::Text {
                    id,
                    slot: Slot::Prose,
                    block,
                    ..
                },
            ) => {
                let initial = row.model.ui.get(id).is_some_and(|ui| ui.fresh());
                Some(resources.owners.borrow_mut().acquire(&row_owner, || {
                    resources
                        .arrivals
                        .borrow_mut()
                        .get_mut(id)
                        .map_or(initial, |arrival| arrival.claim(*block))
                }))
            }
            _ => None,
        };
        let queue_held = self.admission.as_ref().is_some_and(|queue| {
            let semantic = row.model.row.semantic();
            let position = row
                .model
                .row
                .anchor()
                .and_then(|id| row.model.items.get(id))
                .map_or(u64::MAX, |item| item.position);
            let (part, ordinal) = match semantic {
                Row::Chain { .. } => (0, 0),
                Row::Text {
                    slot: Slot::Prose | Slot::Text(_),
                    block,
                    ..
                } => (2, *block),
                _ => (1, 0),
            };
            let mut queue = queue.borrow_mut();
            let enabled = !crate::conversation::admission::immediate(semantic)
                && !queue.rushed(position)
                && !self.reduced_motion;
            queue.begin(key.clone(), (position, part, ordinal), enabled)
        });
        let deferred = row.disclosure.clip.is_none()
            && row.disclosure.fraction.is_none()
            && row.disclosure.hidden;
        if row.held || queue_held || deferred {
            if let Some((reveal, _, ready)) = &reveal {
                reveal.set_paused(true);
                ready.set(false);
            }
            if let Some(queue) = &self.admission {
                queue.borrow_mut().finish(key, row.held || queue_held);
            }
            return Ok(BatchedCollectionRow {
                element: row_frame(
                    &row.model,
                    row.disclosure,
                    row.held || queue_held,
                    None,
                    self.theme,
                ),
                batches: Vec::new(),
            });
        }
        let mut batches = Vec::new();
        let mut stored_proof = None;
        let content = match &row.model.row {
            Row::Gap | Row::EarlierWork { .. } => {
                let dispatch = self
                    .dispatch
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("conversation actions are unavailable"))?;
                super::load_row(&row.model.row, self.theme, Rc::clone(dispatch))
            }
            Row::Work {
                run,
                segment,
                entries,
                unloaded,
            } => {
                let work = self
                    .work
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("conversation work motion is unavailable"))?;
                let dispatch = self
                    .dispatch
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("conversation actions are unavailable"))?;
                if *segment != 0 || row.model.run.is_none() {
                    None
                } else {
                    let summary = row
                        .model
                        .run
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("conversation run is unavailable"))?;
                    let key = format!(
                        "work-{}",
                        run.as_ref().map_or_else(String::new, ToString::to_string)
                    );
                    let label = row_owner.motion.borrow_mut().phrase(
                        &format!("{key}-label"),
                        work::section_title(summary, work.timestamp),
                        self.theme.colors.text_muted.to_reference_text_color(),
                        self.theme,
                        work.seconds,
                        self.reduced_motion,
                    );
                    work::header(
                        run.as_ref(),
                        *segment,
                        !entries.is_empty() || !unloaded.is_empty(),
                        Some(summary),
                        self.theme,
                        label,
                        |open| {
                            row_owner.motion.borrow_mut().chevron(
                                &key,
                                open,
                                row.disclosure.fraction,
                                16.0,
                                self.theme.colors.text_muted.to_reference_paint(),
                                work.now,
                                self.reduced_motion,
                            )
                        },
                        Rc::clone(dispatch),
                    )
                }
            }
            Row::Chain { ids, .. } => {
                let work = self
                    .work
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("conversation work motion is unavailable"))?;
                let dispatch = self
                    .dispatch
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("conversation actions are unavailable"))?;
                let open = ids
                    .first()
                    .and_then(|id| row.model.ui.get(id))
                    .and_then(|ui| ui.chain_open)
                    .unwrap_or(false);
                let key = ids
                    .first()
                    .map_or_else(String::new, |id| format!("chain-{id}"));
                work::chain(
                    ids,
                    &row.model.items,
                    open,
                    self.theme,
                    |text, color| {
                        row_owner.motion.borrow_mut().phrase(
                            &format!("{key}-clause"),
                            text,
                            color,
                            self.theme,
                            work.seconds,
                            self.reduced_motion,
                        )
                    },
                    |open, color| {
                        row_owner.motion.borrow_mut().chevron(
                            &key,
                            open,
                            row.disclosure.fraction,
                            14.0,
                            color,
                            work.now,
                            self.reduced_motion,
                        )
                    },
                    Rc::clone(dispatch),
                )
            }
            Row::WorkEntry { id, .. } => {
                let work = self
                    .work
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("conversation work motion is unavailable"))?;
                let dispatch = self
                    .dispatch
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("conversation actions are unavailable"))?;
                let item = row
                    .model
                    .items
                    .get(id)
                    .ok_or_else(|| anyhow::anyhow!("conversation item is unavailable"))?;
                let expanded = row
                    .model
                    .ui
                    .get(id)
                    .and_then(|ui| ui.expanded)
                    .unwrap_or(false);
                match &item.kind {
                    ItemKind::Agent { .. } => work::agent(
                        item,
                        self.theme,
                        |text, color| truncate(text, color, false, self.theme),
                        Rc::clone(dispatch),
                    ),
                    ItemKind::Tool { .. }
                    | ItemKind::UserMessage {
                        from_agent: Some(_),
                        ..
                    } => work::activity_row(
                        item,
                        expanded,
                        &work.names,
                        self.theme,
                        |text, color, mono| truncate(text, color, mono, self.theme),
                        Rc::clone(dispatch),
                    ),
                    ItemKind::Plan { .. } => work::plan_entry(item, self.theme),
                    ItemKind::Notice { .. } => self.notice_row(&row_owner, item, &mut batches)?,
                    ItemKind::Question { .. } => self.question_card(&row_owner, item, &mut batches)?,
                    _ => {
                        anyhow::bail!("conversation work entry renderer is unavailable for {key:?}")
                    }
                }
            }
            Row::Text { id, slot: slot @ Slot::Plain(section), block, .. }
                if row.model.items.get(id).is_some_and(|item| matches!(slot.text(item), Some(TextContent::Stored(_)))) => {
                let item = row.model.items.get(id).ok_or_else(|| anyhow::anyhow!("conversation item is unavailable"))?;
                let Some(TextContent::Stored(reference)) = slot.text(item) else { anyhow::bail!("stored source changed") };
                Some(self.literal_row(&row_owner, item, *slot, *section, *block, reference, row.model.version, &mut batches)?)
            }
            Row::Text { id, slot, block, .. }
                if matches!(slot, Slot::Prose | Slot::Text(fold::TextSection::User))
                    && row.model.items.get(id).is_some_and(|item| {
                        item.status != Status::Running && matches!(slot.text(item), Some(TextContent::Stored(_)))
                    }) => {
                let item = row.model.items.get(id).ok_or_else(|| anyhow::anyhow!("conversation item is unavailable"))?;
                let Some(TextContent::Stored(reference)) = slot.text(item) else { anyhow::bail!("stored source changed") };
                let (element, proof) = self.stored_row(row, &row_owner, item, *slot, *block, reference, &reveal, content_width, &mut batches)?;
                stored_proof = proof;
                Some(if *slot == Slot::Text(fold::TextSection::User) {
                    let count = self.stored.as_ref().and_then(|services| services.owner.borrow().current_range(&BodyKey { item: id.clone(), slot: *slot })).map_or(1, |range| range.end);
                    user_bubble(element, *block, count, false, if *block == 0 { self.user_attachments(item)? } else { None }, match &item.kind { ItemKind::UserMessage { sender, .. } => sender.as_ref().map(|sender| sender.engine), _ => None }, self.theme)
                } else { element })
            }
            Row::Text {
                id,
                slot: Slot::Text(fold::TextSection::User),
                block: 0,
                ..
            } if row.model.items.get(id).is_some_and(|item| {
                matches!(&item.kind, ItemKind::UserMessage { text: TextContent::Inline(text), .. } if text.is_empty())
            }) => {
                use aqua_ui::element::Box as UiBox;
                use aqua_ui::style::{Length, Style};
                let item = row.model.items.get(id).ok_or_else(|| anyhow::anyhow!("conversation item is unavailable"))?;
                let mut root = UiBox::new().layout("column").width(Length::Pct(100.0)).gap(8.0).style(Style::new().align(aqua_ui::AlignItems::END));
                if let Some(attachments) = self.user_attachments(item)? {
                    root = root.child(attachments);
                }
                Some(root.build())
            }
            Row::Text {
                id,
                slot: slot @ (Slot::Prose | Slot::Text(fold::TextSection::User)),
                block,
                ..
            } => 'prose: {
                let item = row
                    .model
                    .items
                    .get(id)
                    .ok_or_else(|| anyhow::anyhow!("conversation item is unavailable"))?;
                let running = item.status == Status::Running && *slot == Slot::Prose;
                let shape_count = self
                    .shape_counts
                    .get(&BodyKey {
                        item: id.clone(),
                        slot: *slot,
                    })
                    .copied();
                let sealed = shape_count.is_some_and(|count| *block + 1 < count);
                let fresh = *slot == Slot::Prose && row.model.ui.get(id).is_some_and(|ui| ui.fresh());
                anyhow::ensure!(
                    reveal.is_some() || (!running && !fresh),
                    "live or fresh prose reveal is unavailable"
                );
                let saved = match (&item.kind, slot) {
                    (
                        ItemKind::AssistantMessage {
                            text: TextContent::Inline(source),
                            ..
                        },
                        Slot::Prose,
                    ) => Some(source.as_str()),
                    (
                        ItemKind::AssistantMessage {
                            text: TextContent::Stored(_),
                            ..
                        },
                        Slot::Prose,
                    ) if running => None,
                    (
                        ItemKind::UserMessage {
                            text: TextContent::Inline(source),
                            ..
                        },
                        Slot::Text(fold::TextSection::User),
                    ) => Some(source.as_str()),
                    _ => anyhow::bail!("stored or mismatched conversation text is unavailable"),
                };
                let live = if running {
                    let bound = self
                        .reveals
                        .as_ref()
                        .and_then(|resources| resources.live.get(id));
                    match (bound, saved) {
                        (Some(binding), _) => {
                            let target = artisan_api::LiveTarget::Text {
                                item_id: id.clone(),
                            };
                            anyhow::ensure!(
                                row.model.live.get(&target).is_some_and(|source| {
                                    std::sync::Arc::ptr_eq(source, &binding.text)
                                }),
                                "stale live source snapshot"
                            );
                            Some(binding.clone())
                        }
                        (None, Some(_)) => None,
                        (None, None) => {
                            anyhow::bail!("current live source is unavailable")
                        }
                    }
                } else {
                    None
                };
                self.content_keyed
                    .set(shape_count.is_some() && live.is_none());
                let live_source = live.as_ref().map(|binding| binding.text.to_string());
                let source = live_source
                    .as_deref()
                    .or(saved)
                    .ok_or_else(|| anyhow::anyhow!("current live source is unavailable"))?;
                if let (Some((owner, is_fresh, ready)), Some(resources)) = (&reveal, &self.reveals)
                {
                    ready.set(false);
                    owner.begin_frame(
                        resources.now,
                        running && !sealed,
                        *is_fresh,
                        self.reduced_motion,
                        resources.swift,
                    );
                    owner.set_paused(false);
                    owner.set_rushed(
                        self.admission
                            .as_ref()
                            .is_some_and(|queue| queue.borrow().rushed(item.position)),
                    );
                }
                let body_key = BodyKey {
                    item: id.clone(),
                    slot: *slot,
                };
                let mut owners = self.markdown.borrow_mut();
                let mut engine = self.engine.borrow_mut();
                if engine.is_none() {
                    *engine = Some(editor_ui::markdown::MarkdownEngine::new()?);
                }
                let engine = engine
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("Markdown engine is unavailable"))?;
                let prepare_span = editor_core::diagnostics::rebuild_stats::span("f_md_prepare");
                let (count, revision) = match owners.prepare_on_realization(
                    &self.scope,
                    body_key.clone(),
                    &id.to_string(),
                    engine,
                    source,
                    item.seq,
                ) {
                    Ok(prepared) => prepared,
                    Err(error) => match self.copy.clone() {
                        None => return Err(error),
                        Some(copy) => {
                            if let Some((owner, _, _)) = &reveal {
                                owner.mark_presented();
                            }
                            let copy_source = super::source_actions::CopySource::from_parts(
                                live.as_ref().map(|binding| binding.text.clone()),
                                std::sync::Arc::from(source),
                            );
                            break 'prose Some(super::source_actions::failed_body(
                                self.theme,
                                error.to_string(),
                                format!("copy-full-{id}"),
                                copy_source,
                                copy,
                            ));
                        }
                    },
                };
                drop(prepare_span);
                let preview_notice = (*block + 1 == count)
                    .then(|| live.as_ref().and_then(|binding| binding.preview_notice()))
                    .flatten();
                let shaped = self.shape_counts.contains_key(&body_key);
                if (if shaped { *block + 1 == count } else { *block == 0 })
                    && let Some(binding) = &live
                {
                    self.pending_live
                        .borrow_mut()
                        .push((binding.clone(), count));
                }
                if let (Some(width), Some(media)) = (content_width, &self.media) {
                    let info = owners
                        .projected_leaf_info(&self.scope, &body_key, *block)
                        .ok_or_else(|| anyhow::anyhow!("Markdown leaf geometry is unavailable"))?;
                    anyhow::ensure!(
                        owners.projected_leaf_is_current(&self.scope, &body_key, *block, &info),
                        "stale Markdown leaf geometry"
                    );
                    let mut width = width.min(768.0) - 48.0;
                    if *slot != Slot::Prose {
                        width = width.min(576.0) - 32.0;
                    }
                    let width = width
                        - info.list_depth as f32
                            * editor_ui::theme::ProseTypography::LIST_INDENT_PX;
                    anyhow::ensure!(width > 0.0, "measured media width is unavailable");
                    if let Some(services) = media(&body_key, width) {
                        anyhow::ensure!(
                            services.width == width,
                            "media services width does not match the measured body"
                        );
                        owners.set_realization_media(
                            &self.scope,
                            &body_key,
                            &id.to_string(),
                            services,
                        )?;
                    }
                }
                let code = owners.code_services(
                    &self.scope,
                    &body_key,
                    self.theme,
                    self.reduced_motion,
                    true,
                    self.on_code.clone(),
                )?;
                let order = fold::selection_order(self.scope.agent.is_some(), item.position, *slot);
                let id = id.to_string();
                let services = HeatmapServices::new(self.theme);
                let projected_span = editor_core::diagnostics::rebuild_stats::span("f_md_project");
                let prepared = owners.prepare_projected_row_with_geometry(
                    &self.scope,
                    MarkdownRender {
                        key: body_key,
                        id: &id,
                        source,
                        theme: &self.theme,
                        tone: BodyTone::Foreground,
                        order: &order,
                        glyph: Rc::new(editor_ui::asset::source_asset_glyph),
                        invalidate: self.invalidate.clone(),
                        on_link: self.on_link.clone(),
                        on_color: self.on_color.clone(),
                        reference_scope: Some(ResourceScope {
                            host: self.scope.host.to_string(),
                            thread: self.scope.thread.clone(),
                        }),
                        heatmap: &services,
                    },
                    *block,
                    Some(&code),
                    reveal.as_ref().map(|(owner, _, _)| owner.clone()),
                    || (self.allocate)(),
                )?;
                drop(projected_span);
                let entrance = if prepared.geometry.block_entrance {
                    reveal.as_ref().and_then(|(owner, _, _)| {
                        owner
                            .block_entrance(*block)
                            .map(|(alpha, rise)| (alpha, rise, owner.block_panel_height(*block)))
                    })
                } else {
                    None
                };
                if *block == 0 && !shaped {
                    let version = self
                        .projection_versions
                        .get(&BodyKey {
                            item: item.id.clone(),
                            slot: *slot,
                        })
                        .copied()
                        .unwrap_or(row.model.version);
                    self.pending_projections.borrow_mut().push((
                        BodyKey {
                            item: item.id.clone(),
                            slot: *slot,
                        },
                        crate::conversation::projection::BodyProjection {
                            version,
                            revision,
                            leaves: 0..count,
                        },
                    ));
                }
                let prepared = if let Some(resources) = &self.reveals
                    && reveal.is_some()
                {
                    resources.owners.borrow_mut().record(
                        &row_owner,
                        row.model.version,
                        *block,
                        prepared,
                        live.filter(|_| !sealed),
                    )
                } else {
                    prepared.row
                };
                batches = prepared.batches;
                let element = if let Some((alpha, rise, height)) = entrance {
                    use aqua_ui::element::Box as UiBox;
                    use aqua_ui::style::{Length, Style};
                    let inner = UiBox::new()
                        .width(Length::Pct(100.0))
                        .style(
                            Style::new()
                                .flex_shrink(0.0)
                                .opacity(alpha)
                                .transform(aqua_core::Transform2D::translate(0.0, rise)),
                        )
                        .child(prepared.element)
                        .build();
                    UiBox::new()
                        .layout("column")
                        .width(Length::Pct(100.0))
                        .style(Style::new().height(Length::Px(height)).clip(true))
                        .child(inner)
                        .build()
                } else {
                    prepared.element
                };
                let element = if let Some((owner, _, _)) = &reveal {
                    owner.mark_presented();
                    if !owner.fully_visible() && !self.reduced_motion {
                        use aqua_ui::element::Box as UiBox;
                        use aqua_ui::style::{Length, Style};
                        UiBox::new()
                            .layout("column")
                            .width(Length::Pct(100.0))
                            .style(
                                Style::new()
                                    .height(Length::Px(owner.visible_height()))
                                    .clip(true),
                            )
                            .child(
                                UiBox::new()
                                    .width(Length::Pct(100.0))
                                    .style(Style::new().flex_shrink(0.0))
                                    .child(element)
                                    .build(),
                            )
                            .build()
                    } else {
                        element
                    }
                } else {
                    element
                };
                Some(if *slot == Slot::Prose {
                    if let Some(notice) = preview_notice {
                        aqua_ui::element::Box::new()
                            .width(aqua_ui::style::Length::Pct(100.0))
                            .layout("column")
                            .child(element)
                            .child(editor_ui::page::notice(&self.theme, notice).build())
                            .build()
                    } else {
                        element
                    }
                } else {
                    user_bubble(
                        element,
                        *block,
                        count,
                        source.is_empty(),
                        if *block == 0 {
                            self.user_attachments(item)?
                        } else {
                            None
                        },
                        match &item.kind {
                            ItemKind::UserMessage { sender, .. } => {
                                sender.as_ref().map(|sender| sender.engine)
                            }
                            _ => None,
                        },
                        self.theme,
                    )
                })
            }
            Row::Prompt(id) => {
                let item = row.model.items.get(id).ok_or_else(|| anyhow::anyhow!("conversation prompt item is unavailable"))?;
                if matches!(&item.kind, ItemKind::Question { .. }) {
                    self.question_card(&row_owner, item, &mut batches)?
                } else {
                    let dispatch = self.dispatch.as_ref().ok_or_else(|| anyhow::anyhow!("conversation actions are unavailable"))?;
                    let approval = self.approval_card(&row_owner, item, row.model.ui.get(id), Rc::clone(dispatch), &mut batches)?;
                    if approval.is_some() {
                        approval
                    } else {
                        let work = self.work.as_ref().ok_or_else(|| anyhow::anyhow!("conversation work motion is unavailable"))?;
                        match &item.kind {
                            ItemKind::Tool { .. } | ItemKind::UserMessage { from_agent: Some(_), .. } => work::activity_row(item, row.model.ui.get(id).and_then(|ui| ui.expanded).unwrap_or(false), &work.names, self.theme, |text, color, mono| truncate(text, color, mono, self.theme), Rc::clone(dispatch)),
                            ItemKind::Agent { .. } => work::agent(item, self.theme, |text, color| truncate(text, color, false, self.theme), Rc::clone(dispatch)),
                            ItemKind::Plan { .. } => work::plan_entry(item, self.theme),
                            ItemKind::Notice { .. } => self.notice_row(&row_owner, item, &mut batches)?,
                            _ => None,
                        }
                    }
                }
            }
            Row::Image(id) => {
                let item = row
                    .model
                    .items
                    .get(id)
                    .ok_or_else(|| anyhow::anyhow!("conversation image item is unavailable"))?;
                let ItemKind::Image { blob, .. } = &item.kind else {
                    anyhow::bail!("conversation image item has changed");
                };
                let width = content_width
                    .ok_or_else(|| anyhow::anyhow!("committed transcript width is unavailable"))?
                    .min(768.0)
                    - 48.0;
                anyhow::ensure!(width > 0.0, "measured image width is unavailable");
                let image = self.blob_frame(item, blob, None, width)?;
                Some(
                    aqua_ui::element::Box::new()
                        .layout("column")
                        .gap(6.0)
                        .child(image)
                        .build(),
                )
            }
            Row::Compaction(id) => {
                use crate::conversation::compaction::CompactionPhase;
                let item = row.model.items.get(id).ok_or_else(|| anyhow::anyhow!("conversation compaction item is unavailable"))?;
                let ItemKind::Compaction { switch, .. } = &item.kind else {
                    anyhow::bail!("conversation compaction item has changed");
                };
                let work = self.work.as_ref().ok_or_else(|| anyhow::anyhow!("conversation work motion is unavailable"))?;
                let phase = match item.status {
                    Status::Running => CompactionPhase::Running,
                    Status::Done => CompactionPhase::Done,
                    Status::AwaitingUser | Status::Failed | Status::Declined | Status::Cancelled => CompactionPhase::Stopped,
                };
                let fresh = row.model.ui.get(id).is_some_and(|ui| ui.fresh());
                let frame = row_owner.motion.borrow_mut().compaction(id, phase, fresh, work.now, self.reduced_motion);
                self.compactions.borrow_mut().insert(key.clone(), (phase, switch.is_some(), frame));
                super::compaction::row(item, frame, self.theme, super::compaction::label)
            }
            Row::AgentResult(_) => None,
            Row::Status(run) => status::run(&row.model, run, self.theme, |text, live, theme| {
                self.statuses
                    .borrow_mut()
                    .render(key.clone(), text, live, theme)
            })
            .transpose()?,
            Row::AgentStatus(_, working) => status::agent(*working, self.theme, |text, theme| {
                self.statuses.borrow_mut().agent(key.clone(), text, theme)
            })
            .transpose()?,
            Row::Text { id, slot: Slot::Plain(section), .. } => {
                let item = row
                    .model
                    .items
                    .get(id)
                    .ok_or_else(|| anyhow::anyhow!("conversation item is unavailable"))?;
                let Some(text) = section.inline_text(item) else {
                    anyhow::bail!("conversation plain text source changed");
                };
                Some(match section {
                    fold::PlainSection::ImageCaption => image_caption(text, self.theme),
                    _ => aqua_ui::element::Text::new(text.to_owned()).build(),
                })
            }
            _ => anyhow::bail!("conversation row renderer is unavailable for {key:?}"),
        };
        let mut work_settling = false;
        let content = match (content, &self.work) {
            (Some(element), Some(work)) => {
                let id = match row.model.row.semantic() {
                    Row::WorkEntry { id, .. } => Some(id),
                    Row::Chain { ids, .. } => ids.first(),
                    _ => None,
                };
                if let Some(id) = id {
                    let fresh = row.disclosure.clip.is_none()
                        && row.model.ui.get(id).is_some_and(|ui| ui.fresh());
                    let live = row.model.run.as_ref().is_some_and(|run| run.live());
                    let entrance = row_owner.motion.borrow_mut().entrance(
                        key.clone(),
                        fresh,
                        live,
                        self.reduced_motion,
                        work.now,
                    );
                    if row.disclosure.clip.is_some() {
                        Some(element)
                    } else if let Some(entrance) = entrance {
                        work_settling = entrance.settling;
                        Some(entrance.wrap(
                            element,
                            row.disclosure.gap.unwrap_or(row.model.gap),
                            self.invalidate.clone(),
                        ))
                    } else {
                        Some(element)
                    }
                } else {
                    Some(element)
                }
            }
            (other, _) => other,
        };
        if let Some(queue) = &self.admission {
            let position = row
                .model
                .row
                .anchor()
                .and_then(|id| row.model.items.get(id))
                .map_or(u64::MAX, |item| item.position);
            let settling = !queue.borrow().rushed(position)
                && (work_settling
                    || reveal
                        .as_ref()
                        .is_some_and(|(owner, _, _)| owner.is_settling()));
            queue.borrow_mut().finish(key, settling);
        }
        let reveal_done = reveal
            .as_ref()
            .is_none_or(|(owner, _, _)| {
                owner.fully_visible() && !owner.is_settling() && !owner.is_animating()
            });
        let quiescent = self.content_keyed.get() || row_settles_statically(&row.model);
        let motionless = !row_owner.motion.borrow().animating();
        let disclosure_rest = row.disclosure == RowDisclosure::default();
        let settled = !self.unsettled.get()
            && !work_settling
            && !row.held
            && disclosure_rest
            && quiescent
            && reveal_done
            && motionless;
        editor_core::diagnostics::rebuild_stats::record_event(match row.model.row.semantic() {
            Row::Text { .. } => "rz_kind_text",
            Row::Chain { .. } | Row::WorkEntry { .. } => "rz_kind_chain",
            Row::Work { .. } => "rz_kind_work",
            Row::Status(_) => "rz_kind_status",
            _ => "rz_kind_other",
        });
        editor_core::diagnostics::rebuild_stats::record_event(match () {
            () if settled => "rz_settled",
            () if self.unsettled.get() => "rz_why_pending",
            () if work_settling => "rz_why_entrance",
            () if row.held => "rz_why_held",
            () if !disclosure_rest => "rz_why_disclosure",
            () if !quiescent => "rz_why_live",
            () if !reveal_done => "rz_why_reveal",
            () => "rz_why_motion",
        });
        if settled && let Some(fingerprint) = self.fingerprint(index, key) {
            self.settled.borrow_mut().insert(key.clone(), fingerprint);
        }
        let element = if let Some(observations) = &self.observations {
            let item = row
                .model
                .row
                .anchor()
                .and_then(|id| row.model.items.get(id));
            let snapshot_live = row
                .model
                .row
                .anchor()
                .and_then(|id| self.reveals.as_ref()?.live.get(id).cloned());
            let snapshot = super::transcript::RowSnapshot {
                row: row.model.row.clone(),
                version: row.model.version,
                item_seq: item.map(|item| item.seq),
                source: snapshot_live
                    .as_ref()
                    .map(|live| Rc::from(live.text.to_string()))
                    .or_else(|| {
                        item.and_then(|item| match &item.kind {
                            ItemKind::AssistantMessage {
                                text: TextContent::Inline(source),
                                ..
                            } => Some(Rc::from(source.as_str())),
                            _ => None,
                        })
                    }),
                active: self.active.clone(),
                revealed: !row.held && !row.disclosure.hidden,
                reveal_ready: reveal.as_ref().map(|(_, _, ready)| ready.clone()),
                stored: stored_proof.map(|proof| {
                    (
                        Rc::downgrade(
                            &self
                                .stored
                                .as_ref()
                                .expect("stored proof has an owner")
                                .owner,
                        ),
                        proof,
                    )
                }),
                live: snapshot_live,
            };
            let (height, painted) = observations.callbacks(snapshot, self.markdown.clone());
            let height_active = self.active.clone();
            let paint_active = self.active.clone();
            let height = Rc::new(move |bounds| {
                if height_active.get() {
                    height(bounds);
                }
            });
            let painted = Rc::new(move |bounds| {
                if paint_active.get() {
                    painted(bounds);
                }
            });
            super::measured_row_frame(
                &row.model,
                row.disclosure,
                row.held,
                content,
                self.theme,
                height,
                painted,
            )
        } else {
            row_frame(&row.model, row.disclosure, row.held, content, self.theme)
        };
        if let Some(resources) = &self.reveals {
            resources
                .owners
                .borrow_mut()
                .activate(key.clone(), &row_owner);
        }
        self.row_cache
            .borrow_mut()
            .commit(key.clone(), row_owner.clone());
        self.realized_owners
            .borrow_mut()
            .insert(key.clone(), Rc::downgrade(&row_owner));
        let stored_lease = row_owner.stored_leaf.borrow().clone();
        let lease = aqua_ui::element::Canvas::new()
            .style(aqua_ui::style::Style::new().absolute_at(
                Some(0.0),
                Some(0.0),
                Some(0.0),
                Some(0.0),
            ))
            .on_prepaint(move |_, _| {
                let _ = &row_owner;
                let _ = &stored_lease;
            })
            .build()
            .exclude_from_hit_testing();
        let element = aqua_ui::element::Box::new()
            .width(aqua_ui::style::Length::Pct(100.0))
            .child(element)
            .child(lease)
            .build();
        Ok(BatchedCollectionRow { element, batches })
    }
}

pub(crate) fn image_caption(
    text: &str,
    theme: editor_ui::theme::ArtisanTheme,
) -> aqua_ui::element::Element {
    use aqua_ui::style::Length;
    aqua_ui::element::Box::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .child(
            aqua_ui::element::Text::new(text.to_owned())
                .font_size(theme.typography.control_text)
                .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                .wrap(true)
                .build(),
        )
        .build()
}
