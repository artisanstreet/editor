use super::{AppMedia, ConversationAction, ConversationMedia, RootViews};
use crate::shell::{application::BuildError, host::Shell, view};
use aqua::UiApp;
use aqua::prelude::ObservedWindowState;
use aqua_ui::element::Element;
use std::{cell::Cell, collections::HashMap, rc::Rc};

pub(super) type MediaProviders =
    Rc<HashMap<artisan_api::ItemId, (crate::ui::media::source::MediaFetcher, artisan_api::Seq)>>;

type ConversationCallbacks = (
    Rc<dyn Fn(crate::ui::markdown::visual::LinkAction)>,
    Rc<dyn Fn(crate::ui::rich::actions::ShowCopyMenu)>,
);

#[derive(Clone, Copy, Debug)]
pub(super) struct ContentPass {
    pub(super) width: f32,
    pub(super) reduced_motion: bool,
    pub(super) swift: bool,
    pub(super) now: std::time::Instant,
    pub(super) transcript_only: bool,
}

impl RootViews {
    #[expect(
        clippy::too_many_lines,
        reason = "one thread content pass that shares scope, caches and row factory across its regions"
    )]
    pub(super) fn thread_content(
        &mut self,
        workspace: &mut crate::shell::workspace::Workspace,
        core: &crate::core::Core,
        ui: &mut UiApp,
        theme: &crate::ui::theme::ArtisanTheme,
        media_resources: AppMedia,
        pass: ContentPass,
    ) -> Result<Element, BuildError> {
        use crate::conversation::presentation::PresentationScope;
        use crate::conversation::view::factory::RowFactory;
        let ContentPass {
            width,
            reduced_motion,
            swift,
            now,
            transcript_only,
        } = pass;
        let scope = PresentationScope::capture(&workspace.conversation, core)
            .ok_or(BuildError::ConversationGone)?;
        if self
            .transcript
            .rows
            .as_ref()
            .is_none_or(|rows| rows.scope() != &scope)
        {
            if let Some(token) = self.transcript.row_token.take() {
                token.set(false);
            }
            self.transcript.row_settled.borrow_mut().clear();
            self.transcript.observations.clear();
            self.navigation_views.navigation.dispose();
            self.scroll.transcript_viewport = None;
            self.transcript.mounted_reveals.borrow_mut().rebind(None);
            self.transcript.row_cache.borrow_mut().rebind(None);
            self.retained_rows = None;
            self.scroll.smooth_scroll.cancel();
            self.scroll.scroll_request = None;
            self.scroll.wheel.cancel_transcript();
            self.transcript.row_motion = Rc::default();
        }
        workspace
            .markdown
            .borrow_mut()
            .rebind(ui, Some(scope.clone()));
        workspace
            .conversation_selection
            .rebind(ui, Some(scope.clone()));
        workspace
            .conversation
            .advance_disclosure_frame(core, now, reduced_motion);
        if self.media_views.media.as_ref().is_none_or(|media| {
            media.host != scope.host
                || media.instance != scope.instance
                || media.thread != scope.thread
        }) {
            drop(self.media_views.media.take());
            self.transcript.observations.clear();
            let client = core
                .host_if_instance(&scope.host, scope.instance)
                .ok_or_else(|| BuildError::HostGone {
                    host: scope.host.clone(),
                })?
                .client
                .clone();
            let thread = scope.thread.clone();
            let fetcher = crate::ui::media::source::MediaFetcher::new(
                format!("{}/{}", scope.host, scope.thread),
                move |request| {
                    let client = client.clone();
                    let thread = thread.clone();
                    Box::pin(async move {
                        client
                            .resource(
                                &thread,
                                &request.path,
                                request.from_home,
                                request.item.as_deref(),
                            )
                            .await
                            .map(std::sync::Arc::<[u8]>::from)
                            .map_err(media_failure)
                    })
                },
            );
            self.media_views.media = Some(ConversationMedia {
                host: scope.host.clone(),
                instance: scope.instance,
                thread: scope.thread.clone(),
                fetcher,
                owners: HashMap::new(),
                models: media_resources.model_assets.clone(),
                audio: media_resources.audio_assets.clone(),
                index: None,
            });
        }
        let Some(media) = self.media_views.media.as_mut() else {
            unreachable!(
                "invariant: media for thread {} on {} was bound above",
                scope.thread, scope.host
            )
        };
        let rows_epoch = workspace.conversation.rows_epoch();
        if self.retained_rows != Some(rows_epoch) {
            let row_keys: std::collections::HashSet<_> = workspace
                .conversation
                .rows
                .iter()
                .map(|(row, _)| row.key())
                .collect();
            self.transcript
                .row_cache
                .borrow_mut()
                .retain_rows(&row_keys);
            self.retained_rows = Some(rows_epoch);
        }
        let items_revision = workspace
            .conversation
            .view
            .as_ref()
            .map(|view| (view.items.revision(), view.context.revision()));
        let preview = self
            .media_views
            .image_preview
            .as_ref()
            .and_then(|preview| preview.owner.as_ref())
            .map(|owner| owner.to_vec());
        let providers = if let Some(index) = media
            .index
            .as_ref()
            .filter(|index| index.items == items_revision && index.preview == preview)
        {
            Rc::clone(&index.providers)
        } else {
            let providers = Rc::new(index_media(workspace, ui, media, preview.as_deref()));
            media.index = Some(super::MediaIndex {
                items: items_revision,
                preview,
                providers: Rc::clone(&providers),
            });
            providers
        };
        let images = workspace.images.as_ref().clone();
        let media_actions = Rc::clone(&self.transcript.conversation_actions);
        let media_scope = scope.clone();
        let sender = core.wake_mailbox().sender();
        let wake: aqua::mailbox::Wake = std::sync::Arc::new(move || {
            let _queued = sender.send(());
        });
        let actions = Rc::clone(&self.transcript.row_actions);
        let dispatch = crate::conversation::view::scoped_dispatch(
            &workspace.conversation,
            Rc::new(move |action| actions.borrow_mut().push_back(action)),
        )
        .ok_or_else(|| BuildError::NoActionScope {
            host: scope.host.clone(),
            thread: scope.thread.clone(),
        })?;
        let (on_link, on_color) = self.conversation_callbacks(scope.clone());
        let invalidated = Rc::clone(&self.rows_invalidated);
        let row_token = Rc::clone(
            self.transcript
                .row_token
                .get_or_insert_with(|| Rc::new(Cell::new(true))),
        );
        let style = (
            *theme,
            reduced_motion,
            swift,
            media_resources.scale.to_bits(),
        );
        if self.transcript.row_style != Some(style) {
            self.transcript.row_style = Some(style);
            self.transcript.row_style_epoch += 1;
            self.transcript.row_settled.borrow_mut().clear();
        }
        let settled = Rc::clone(&self.transcript.row_settled);
        let invalidate_settled = Rc::clone(&self.transcript.row_settled);
        let seconds = now
            .saturating_duration_since(*self.transcript.motion_epoch.get_or_insert(now))
            .as_secs_f64();
        let host_gone = || BuildError::HostGone {
            host: scope.host.clone(),
        };
        let blob_images = core
            .host_if_instance(&scope.host, scope.instance)
            .ok_or_else(host_gone)?
            .blob_images();
        let provider = core
            .source_provider(&scope.host, scope.instance)
            .ok_or_else(host_gone)?;
        let blob_actions = Rc::clone(&self.transcript.conversation_actions);
        let blob_scope = scope.clone();
        let source_actions = Rc::clone(&self.transcript.conversation_actions);
        let services = crate::conversation::view::factory::RowServices::new(
            &scope,
            crate::conversation::view::factory::RowServiceParts {
                provider,
                document: workspace.selections.conversation,
                on_source: Rc::new(move |action| {
                    source_actions
                        .borrow_mut()
                        .push_back(ConversationAction::Source(action));
                }),
                stored: Rc::clone(&self.transcript.stored),
                images: blob_images,
                on_blob: Rc::new(move |item, revision, action| {
                    blob_actions
                        .borrow_mut()
                        .push_back(ConversationAction::Blob(
                            blob_scope.clone(),
                            item,
                            revision,
                            action,
                        ));
                }),
            },
        )
        .map_err(BuildError::Rows)?;
        let factory = Rc::new(
            RowFactory::new(
                Vec::new(),
                scope.clone(),
                Rc::clone(&workspace.markdown),
                Rc::clone(&workspace.status_shimmers),
                theme,
                reduced_motion,
                now,
                workspace.selections.source_allocator(),
                Rc::new(move || {
                    crate::core::diagnostics::rebuild_stats::record_event("inv_rows");
                    invalidated.set(true);
                    invalidate_settled.borrow_mut().clear();
                }),
                on_link,
                on_color,
                workspace.code_action_emit(),
                Rc::clone(&dispatch),
                crate::conversation::view::factory::WorkResources {
                    motion: Rc::clone(&self.transcript.row_motion),
                    names: crate::conversation::adapter::agent_names(&workspace.conversation),
                    seconds,
                    timestamp: core.clock().timestamp(),
                },
                services,
            )
            .with_active(row_token)
            .with_settled(settled, self.transcript.row_style_epoch)
            .with_row_cache(Rc::clone(&self.transcript.row_cache))
            .with_copy({
                let copy_actions = Rc::clone(&self.transcript.conversation_actions);
                let copy_scope = scope.clone();
                Rc::new(move |source| {
                    copy_actions
                        .borrow_mut()
                        .push_back(ConversationAction::Copy(copy_scope.clone(), source));
                })
            })
            .with_lazy_rows(&workspace.conversation, core, self.composer_height)
            .map_err(BuildError::Rows)?
            .with_reveals(
                &workspace.conversation,
                core,
                Rc::clone(&self.transcript.mounted_reveals),
                swift,
            )
            .map_err(BuildError::Rows)?
            .with_observations(self.transcript.observations.clone())
            .with_engine(Rc::clone(&self.transcript.markdown_engine))
            .with_media(Rc::new(move |key, width| {
                let (fetcher, revision) = providers.get(&key.item)?;
                let revision = *revision;
                let actions = Rc::clone(&media_actions);
                let scope = media_scope.clone();
                let key = key.clone();
                Some(crate::ui::markdown::presenter::MediaServices {
                    width,
                    fetcher: fetcher.clone(),
                    effect: Rc::new(move |effect| {
                        actions.borrow_mut().push_back(ConversationAction::Media(
                            scope.clone(),
                            key.clone(),
                            revision,
                            effect,
                        ));
                    }),
                    images: images.clone(),
                    player: media_resources.player.clone(),
                    audio_assets: media_resources.audio_assets.clone(),
                    model_assets: media_resources.model_assets.clone(),
                    image_assets: media_resources.image_assets.clone(),
                    model_capability: Rc::clone(&media_resources.model_capability),
                    scale: media_resources.scale,
                    wake: std::sync::Arc::clone(&wake),
                })
            })),
        );
        let viewport = factory.viewport_callback().map_err(BuildError::Rows)?;
        self.navigation_views.transcript_landing = Some(workspace.conversation.is_landing());
        if transcript_only {
            let heights = workspace.conversation.row_heights.clone();
            let revisions = Rc::clone(&factory);
            let list = crate::conversation::view::transcript::layout_batched_list(
                &workspace.conversation,
                24.0,
                factory.layout_callback(),
                Rc::new(move |index, key| revisions.revision(index, key)),
                viewport,
                Rc::new(move |_, key| heights.get(key).copied().unwrap_or(24.0)),
            );
            self.transcript.replace_rows(Some(factory));
            return Ok(list);
        }
        self.navigation_views.latest.show(
            crate::shell::thread_navigation::latest_visible(
                &workspace.conversation,
                core,
                workspace.composer.is_some(),
            ),
            now,
            reduced_motion,
        );
        let presence = self.navigation_views.latest.value(now);
        let jump = (self.navigation_views.latest.shown() || presence > 0.0).then(|| {
            crate::shell::thread_navigation::jump_to_latest(theme, presence, Rc::clone(&dispatch))
        });
        let unavailable = workspace
            .conversation
            .agent
            .is_none()
            .then(|| {
                core.host_if_instance(&scope.host, scope.instance)
                    .and_then(|host| host.thread_unavailable(&scope.thread))
            })
            .flatten();
        let mut controls = Vec::new();
        controls.extend(jump);
        if let Some(unavailable) = &unavailable {
            let mut notice = crate::ui::kit::ProblemNotice::new(
                "thread-problem",
                *theme,
                &unavailable.problem(),
            )
            .over(theme.colors.background);
            if matches!(unavailable, crate::core::ThreadUnavailable::Failed(_)) {
                let actions = Rc::clone(&self.transcript.conversation_actions);
                let scope = scope.clone();
                notice = notice.action(
                    crate::ui::kit::Button::text(
                        "retry-thread",
                        *theme,
                        crate::ui::kit::ButtonTone::Neutral,
                        "Try again",
                    )
                    .on_activate(move |_| {
                        actions
                            .borrow_mut()
                            .push_back(ConversationAction::RetryThread(scope.clone()));
                    })
                    .build(),
                );
            }
            controls.push(notice.build());
        }
        if workspace.conversation.agent.is_none()
            && let Some(error) = core
                .host_if_instance(&scope.host, scope.instance)
                .filter(|host| host.is_live())
                .and_then(|host| host.thread_summary(&scope.thread))
                .and_then(|summary| summary.error.as_ref())
        {
            let actions = Rc::clone(&self.transcript.conversation_actions);
            let scope = scope.clone();
            let retry = crate::ui::kit::Button::text(
                "retry-thread-controls",
                *theme,
                crate::ui::kit::ButtonTone::Neutral,
                "Try again",
            )
            .on_activate(move |_| {
                actions
                    .borrow_mut()
                    .push_back(ConversationAction::RetryControls(scope.clone()));
            })
            .build();
            controls.push(
                crate::ui::kit::ProblemNotice::new("thread-controls-problem", *theme, error)
                    .over(theme.colors.background)
                    .action(retry)
                    .build(),
            );
        }
        if let Some(failure) = workspace
            .conversation
            .load_error
            .as_ref()
            .filter(|_| unavailable.is_none())
        {
            let mut notice =
                crate::ui::kit::ProblemNotice::new("transcript-problem", *theme, failure.problem())
                    .over(theme.colors.background);
            if failure.retryable() {
                let actions = Rc::clone(&self.transcript.conversation_actions);
                let scope = scope.clone();
                notice = notice.action(
                    crate::ui::kit::Button::text(
                        "retry-transcript",
                        *theme,
                        crate::ui::kit::ButtonTone::Neutral,
                        "Try again",
                    )
                    .on_activate(move |_| {
                        actions
                            .borrow_mut()
                            .push_back(ConversationAction::RetryLoad(scope.clone()));
                    })
                    .build(),
                );
            }
            controls.push(notice.build());
        }
        let above_composer = if controls.is_empty() {
            None
        } else {
            let mut column = crate::ui::kit::Spaced::spaced(
                aqua_ui::element::Box::new()
                    .layout("column")
                    .width(aqua_ui::style::Length::Pct(100.0)),
                crate::ui::kit::Space::S8,
            );
            for control in controls {
                column = column.child(control);
            }
            Some(column.build())
        };
        let heights = workspace.conversation.row_heights.clone();
        let (questionnaires, attachments) =
            workspace.composer_content(ui, theme, reduced_motion)?;
        let parts = view::ComposerParts {
            questionnaires,
            attachments,
            mention_overlay: None,
            thread_error: None,
            shift: self.navigation_views.composer_glide.shift(now),
            measurement: Rc::clone(&self.composer_measurement),
            above: above_composer,
        };
        let center = if workspace.conversation.rows.is_empty() {
            self.thread_landing.reset();
            let loading = workspace.conversation.view.as_ref().is_some_and(|view| {
                let window = view.window();
                window.in_flight || window.oldest.is_none() && !window.exhausted
            });
            let body = match &workspace.conversation.load_error {
                Some(_) => {
                    self.thread_loading.reset();
                    view::centered_body(aqua_ui::element::Box::new().build())
                }
                None if loading => view::centered_body(self.thread_loading.build(
                    theme,
                    core.clock(),
                    reduced_motion,
                    "conversation-loading",
                )),
                None if unavailable.is_some() => {
                    self.thread_loading.reset();
                    view::centered_body(aqua_ui::element::Box::new().build())
                }
                None => {
                    self.thread_loading.reset();
                    view::conversation_notice(theme, "No messages yet.".to_owned())
                }
            };
            view::conversation_body(workspace, core, theme, width, body, Vec::new(), parts)?
        } else {
            self.thread_loading.reset();
            let mut overlays = vec![
                self.scroll
                    .transcript_edges
                    .band(
                        "conversation-edge-top",
                        crate::ui::scroll_edge::Edge::Top,
                        theme.colors.background,
                    )
                    .style(view::conversation_edge_style(
                        crate::ui::scroll_edge::Edge::Top,
                    )),
                self.scroll
                    .transcript_edges
                    .band(
                        "conversation-edge-bottom",
                        crate::ui::scroll_edge::Edge::Bottom,
                        theme.colors.background,
                    )
                    .style(view::conversation_edge_style(
                        crate::ui::scroll_edge::Edge::Bottom,
                    )),
            ];
            let drawer = self.scroll.transcript_viewport.and_then(|viewport| {
                self.navigation_views.navigation.build(
                    &workspace.conversation,
                    core,
                    viewport,
                    theme,
                    now,
                    Rc::clone(&dispatch),
                )
            });
            if workspace.conversation.is_landing() {
                overlays.push(
                    aqua_ui::element::Box::new()
                        .width(aqua_ui::style::Length::Pct(100.0))
                        .height(aqua_ui::style::Length::Pct(100.0))
                        .style(aqua_ui::style::Style::new().absolute_at(
                            Some(0.0),
                            Some(0.0),
                            Some(0.0),
                            Some(0.0),
                        ))
                        .child(view::centered_body(self.thread_landing.build(
                            theme,
                            core.clock(),
                            reduced_motion,
                            "conversation-landing",
                        )))
                        .build(),
                );
            } else {
                self.thread_landing.reset();
            }
            view::conversation_virtual(
                workspace,
                core,
                theme,
                width,
                view::Transcript {
                    estimated_height: 24.0,
                    row: factory.layout_callback(),
                    revision: {
                        let revisions = Rc::clone(&factory);
                        Rc::new(move |index, key| revisions.revision(index, key))
                    },
                    viewport,
                    initial_height: Rc::new(move |_, key| {
                        heights.get(key).copied().unwrap_or(24.0)
                    }),
                    overlays,
                    drawer,
                },
                parts,
            )?
        };
        self.transcript.replace_rows(Some(factory));
        Ok(center)
    }

    fn conversation_callbacks(
        &self,
        scope: crate::conversation::presentation::PresentationScope,
    ) -> ConversationCallbacks {
        let links = Rc::clone(&self.transcript.conversation_actions);
        let colors = Rc::clone(&self.transcript.conversation_actions);
        let link_scope = scope.clone();
        (
            Rc::new(move |action| {
                links
                    .borrow_mut()
                    .push_back(ConversationAction::Link(link_scope.clone(), action));
            }),
            Rc::new(move |action| {
                colors
                    .borrow_mut()
                    .push_back(ConversationAction::Color(scope.clone(), action));
            }),
        )
    }

    pub(super) fn app_media(shell: &Shell, window: &ObservedWindowState) -> AppMedia {
        let capability = Rc::clone(&shell.meshes_supported);
        AppMedia {
            player: shell.audio.clone(),
            audio_assets: shell.audio_assets.clone(),
            model_assets: shell.models.clone(),
            image_assets: shell.images().clone(),
            model_capability: Rc::new(move || {
                capability.get().map(|supported| {
                    if supported {
                        crate::ui::media::model::load::RendererCapability::Mesh
                    } else {
                        crate::ui::media::model::load::RendererCapability::Software
                    }
                })
            }),
            scale: crate::measure::narrow(window.scale_factor),
        }
    }
}

fn media_failure(error: crate::core::client::CallError) -> crate::ui::media::source::MediaFailure {
    use crate::ui::media::source::MediaFailure;
    use artisan_api::{ErrorCode, Failure, ResourceFailure, ThreadFailure};
    match error.refused() {
        Some(
            Failure::Resource(ResourceFailure::FileNotFound { .. })
            | Failure::Thread(ThreadFailure::NotFound { .. })
            | Failure::Legacy {
                code: ErrorCode::NotFound,
            },
        ) => MediaFailure::Missing,
        Some(_) => MediaFailure::Unreadable(std::sync::Arc::new(error)),
        None => MediaFailure::ForgeUnreachable(std::sync::Arc::new(error)),
    }
}

fn index_media(
    workspace: &mut crate::shell::workspace::Workspace,
    ui: &mut UiApp,
    media: &mut ConversationMedia,
    preview: Option<&[u64]>,
) -> HashMap<artisan_api::ItemId, (crate::ui::media::source::MediaFetcher, artisan_api::Seq)> {
    let items: std::collections::BTreeMap<_, _> = workspace
        .conversation
        .view
        .iter()
        .flat_map(|view| view.items.values().chain(view.context.values()))
        .map(|item| (item.id.clone(), item.seq))
        .collect();
    workspace
        .markdown
        .borrow_mut()
        .retain(ui, |key| items.contains_key(&key.item));
    workspace
        .conversation_selection
        .retain(ui, |key| items.contains_key(&key.item));
    media.owners.retain(|item, _| items.contains_key(item));
    for item in items.keys() {
        media
            .owners
            .entry(item.clone())
            .or_insert_with(|| workspace.selections.next_source().0);
    }
    let mut retained: Vec<_> = media.owners.values().map(|owner| vec![*owner]).collect();
    retained.extend(preview.map(<[u64]>::to_vec));
    media.fetcher.retain_resources(&retained);
    items
        .into_iter()
        .filter_map(|(item, revision)| {
            Some((
                item.clone(),
                (
                    media
                        .fetcher
                        .clone()
                        .owned_by(&[*media.owners.get(&item)?], revision)
                        .showing(item.as_str()),
                    revision,
                ),
            ))
        })
        .collect()
}
