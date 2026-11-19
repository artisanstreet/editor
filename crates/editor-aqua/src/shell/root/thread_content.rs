use super::{AppMedia, ConversationAction, ConversationMedia, RootViews};
use crate::shell::{application::BuildError, host::Shell, view};
use aqua::UiApp;
use aqua::prelude::ObservedWindowState;
use aqua_ui::element::Element;
use std::{cell::Cell, collections::HashMap, rc::Rc};

impl RootViews {
    pub(super) fn thread_content(
        &mut self,
        workspace: &mut crate::shell::workspace::Workspace,
        core: &crate::core::Core,
        ui: &mut UiApp,
        theme: crate::ui::theme::ArtisanTheme,
        width: f32,
        reduced_motion: bool,
        swift: bool,
        media_resources: AppMedia,
        now: std::time::Instant,
        transcript_only: bool,
    ) -> Result<Element, BuildError> {
        use crate::conversation::presentation::PresentationScope;
        use crate::conversation::view::factory::RowFactory;
        let scope = PresentationScope::capture(&workspace.conversation, core)
            .ok_or(BuildError::ConversationGone)?;
        if self.rows.as_ref().is_none_or(|rows| rows.scope != scope) {
            if let Some(token) = self.row_token.take() {
                token.set(false);
            }
            self.row_settled.borrow_mut().clear();
            self.observations.clear();
            self.navigation.dispose();
            self.transcript_viewport = None;
            self.mounted_reveals.borrow_mut().rebind(None);
            self.row_cache.borrow_mut().rebind(None);
            self.smooth_scroll.cancel();
            self.scroll_request = None;
            self.wheel.cancel_transcript();
            self.row_motion = Rc::default();
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
        if self.media.as_ref().is_none_or(|media| {
            media.host != scope.host
                || media.instance != scope.instance
                || media.thread != scope.thread
        }) {
            drop(self.media.take());
            self.observations.clear();
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
                            .map_err(|error| media_failure(&error))
                    })
                },
            );
            self.media = Some(ConversationMedia {
                host: scope.host.clone(),
                instance: scope.instance,
                thread: scope.thread.clone(),
                fetcher,
                owners: HashMap::new(),
                models: media_resources.model_assets.clone(),
                audio: media_resources.audio_assets.clone(),
            });
        }
        let Some(media) = self.media.as_mut() else {
            unreachable!(
                "invariant: media for thread {} on {} was bound above",
                scope.thread, scope.host
            )
        };
        let items: HashMap<_, _> = workspace
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
        let row_keys: std::collections::HashSet<_> = workspace
            .conversation
            .rows
            .iter()
            .map(|(row, _)| row.key())
            .collect();
        self.row_cache.borrow_mut().retain_rows(&row_keys);
        media.owners.retain(|item, _| items.contains_key(item));
        for item in items.keys() {
            media
                .owners
                .entry(item.clone())
                .or_insert_with(|| workspace.selections.next_source().0);
        }
        let mut retained: Vec<_> = media.owners.values().map(|owner| vec![*owner]).collect();
        if let Some(owner) = self
            .image_preview
            .as_ref()
            .and_then(|preview| preview.owner.as_ref())
        {
            retained.push(owner.to_vec());
        }
        media.fetcher.retain_resources(&retained);
        let providers: HashMap<_, _> = items
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
            .collect();
        let images = workspace.images.as_ref().clone();
        let media_actions = Rc::clone(&self.conversation_actions);
        let media_scope = scope.clone();
        let sender = core.wake_mailbox().sender();
        let wake: aqua::mailbox::Wake = std::sync::Arc::new(move || {
            let _queued = sender.send(());
        });
        let actions = Rc::clone(&self.row_actions);
        let dispatch = crate::conversation::view::scoped_dispatch(
            &workspace.conversation,
            Rc::new(move |action| actions.borrow_mut().push_back(action)),
        )
        .ok_or_else(|| BuildError::NoActionScope {
            host: scope.host.clone(),
            thread: scope.thread.clone(),
        })?;
        let (on_link, on_color) = self.conversation_callbacks(scope.clone());
        let invalidated = Rc::clone(&self.conversation_invalidated);
        let row_token = Rc::clone(
            self.row_token
                .get_or_insert_with(|| Rc::new(Cell::new(true))),
        );
        let style = (
            theme,
            reduced_motion,
            swift,
            media_resources.scale.to_bits(),
        );
        if self.row_style != Some(style) {
            self.row_style = Some(style);
            self.row_style_epoch += 1;
            self.row_settled.borrow_mut().clear();
        }
        let settled = Rc::clone(&self.row_settled);
        let invalidate_settled = Rc::clone(&self.row_settled);
        let seconds = now
            .saturating_duration_since(*self.motion_epoch.get_or_insert(now))
            .as_secs_f64();
        let blob_images = core
            .host_if_instance(&scope.host, scope.instance)
            .ok_or_else(|| BuildError::HostGone {
                host: scope.host.clone(),
            })?
            .blob_images();
        let blob_actions = Rc::clone(&self.conversation_actions);
        let blob_scope = scope.clone();
        let source_provider = core
            .source_provider(&scope.host, scope.instance)
            .ok_or_else(|| BuildError::HostGone {
                host: scope.host.clone(),
            })?;
        let source_actions = Rc::clone(&self.conversation_actions);
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
                    motion: Rc::clone(&self.row_motion),
                    names: crate::conversation::adapter::agent_names(&workspace.conversation),
                    seconds,
                    timestamp: core.clock().timestamp(),
                },
            )
            .with_active(row_token)
            .with_settled(settled, self.row_style_epoch)
            .with_row_cache(Rc::clone(&self.row_cache))
            .with_sources(
                source_provider.clone(),
                workspace.selections.conversation,
                Rc::new(move |action| {
                    source_actions
                        .borrow_mut()
                        .push_back(ConversationAction::Source(action));
                }),
            )
            .map_err(BuildError::Rows)?
            .with_stored(Rc::clone(&self.stored), source_provider)
            .map_err(BuildError::Rows)?
            .with_copy({
                let copy_actions = Rc::clone(&self.conversation_actions);
                let copy_scope = scope.clone();
                Rc::new(move |source| {
                    copy_actions
                        .borrow_mut()
                        .push_back(ConversationAction::Copy(copy_scope.clone(), source));
                })
            })
            .with_blob_images(
                blob_images,
                Rc::new(move |item, revision, action| {
                    blob_actions
                        .borrow_mut()
                        .push_back(ConversationAction::Blob(
                            blob_scope.clone(),
                            item,
                            revision,
                            action,
                        ));
                }),
            )
            .with_lazy_rows(&workspace.conversation, core, self.composer_height)
            .map_err(BuildError::Rows)?
            .with_reveals(
                &workspace.conversation,
                core,
                Rc::clone(&self.mounted_reveals),
                swift,
            )
            .map_err(BuildError::Rows)?
            .with_observations(self.observations.clone())
            .with_engine(Rc::clone(&self.markdown_engine))
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
        self.transcript_landing = Some(workspace.conversation.is_landing());
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
            self.replace_rows(Some(factory));
            return Ok(list);
        }
        self.latest.show(
            crate::shell::thread_navigation::latest_visible(
                &workspace.conversation,
                core,
                workspace.composer.is_some(),
            ),
            now,
            reduced_motion,
        );
        let presence = self.latest.value(now);
        let jump = (self.latest.shown() || presence > 0.0).then(|| {
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
            let mut notice = crate::ui::page::notice(&theme, unavailable.to_string());
            if matches!(unavailable, crate::core::ThreadUnavailable::Failed(_)) {
                let actions = Rc::clone(&self.conversation_actions);
                let scope = scope.clone();
                notice = notice.action(
                    crate::ui::kit::Button::text(
                        "retry-thread",
                        theme,
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
            let actions = Rc::clone(&self.conversation_actions);
            let scope = scope.clone();
            let retry = crate::ui::kit::Button::text(
                "retry-thread-controls",
                theme,
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
                crate::ui::page::notice(
                    &theme,
                    format!("Couldn't refresh questions and goals: {error}"),
                )
                .action(retry)
                .build(),
            );
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
            nested: workspace.conversation.agent.is_some(),
            placeholder: String::new(),
            questionnaires,
            attachments,
            mention_overlay: None,
            thread_error: None,
            shift: self.composer_glide.shift(now),
        };
        let center = if workspace.conversation.rows.is_empty() {
            self.thread_landing.reset();
            let loading = workspace.conversation.view.as_ref().is_some_and(|view| {
                let window = view.window();
                window.in_flight || window.oldest.is_none() && !window.exhausted
            });
            let body = match &workspace.conversation.load_error {
                Some(error) => {
                    self.thread_loading.reset();
                    view::conversation_notice(&theme, error.clone(), true)
                }
                None if loading => view::centered_body(self.thread_loading.build(
                    theme,
                    now,
                    reduced_motion,
                    "conversation-loading",
                )),
                None if unavailable.is_some() => {
                    self.thread_loading.reset();
                    view::centered_body(aqua_ui::element::Box::new().build())
                }
                None => {
                    self.thread_loading.reset();
                    view::conversation_notice(&theme, "No messages yet.".to_owned(), false)
                }
            };
            view::conversation_body(
                workspace,
                core,
                &theme,
                width,
                body,
                Vec::new(),
                parts,
                Rc::clone(&self.composer_measurement),
                above_composer,
            )?
        } else {
            self.thread_loading.reset();
            let mut overlays = vec![
                self.transcript_edges
                    .band(
                        "conversation-edge-top",
                        crate::ui::scroll_edge::Edge::Top,
                        theme.colors.background,
                    )
                    .style(view::conversation_edge_style(
                        crate::ui::scroll_edge::Edge::Top,
                    )),
                self.transcript_edges
                    .band(
                        "conversation-edge-bottom",
                        crate::ui::scroll_edge::Edge::Bottom,
                        theme.colors.background,
                    )
                    .style(view::conversation_edge_style(
                        crate::ui::scroll_edge::Edge::Bottom,
                    )),
            ];
            if let Some(error) = &workspace.conversation.load_error {
                overlays.push(view::conversation_load_error(&theme, error.clone()));
            }
            let drawer = self.transcript_viewport.and_then(|viewport| {
                self.navigation.build(
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
                            now,
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
                &theme,
                width,
                24.0,
                factory.layout_callback(),
                {
                    let revisions = Rc::clone(&factory);
                    Rc::new(move |index, key| revisions.revision(index, key))
                },
                viewport,
                Rc::new(move |_, key| heights.get(key).copied().unwrap_or(24.0)),
                overlays,
                drawer,
                parts,
                Rc::clone(&self.composer_measurement),
                above_composer,
            )?
        };
        self.replace_rows(Some(factory));
        Ok(center)
    }

    fn conversation_callbacks(
        &self,
        scope: crate::conversation::presentation::PresentationScope,
    ) -> (
        Rc<dyn Fn(crate::ui::markdown::visual::LinkAction)>,
        Rc<dyn Fn(crate::ui::rich::actions::ShowCopyMenu)>,
    ) {
        let links = Rc::clone(&self.conversation_actions);
        let colors = Rc::clone(&self.conversation_actions);
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

fn media_failure(error: &crate::core::client::CallError) -> crate::ui::media::source::MediaFailure {
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
        Some(_) => MediaFailure::Unreadable,
        None => MediaFailure::Unreachable,
    }
}
