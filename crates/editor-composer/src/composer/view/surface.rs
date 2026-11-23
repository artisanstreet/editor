use super::{
    CardParts, ComposerAction, ComposerFailure, ComposerOwner, MODEL_BAR_GAP, MODEL_BAR_HEIGHT,
    MODEL_ITEM_GAP, MODEL_LIST_HEIGHT, ModelMenuFrame, PROMPT_NAME, composer_card, draft_error,
    goal_label, model_item_height, model_menu, model_menu_items, model_trigger, primary,
    project_tab,
};
use aqua_ui::{
    AlignItems,
    element::{Box as UiBox, Element},
    style::Style,
};
use editor_ui::{
    kit::{Space, Spaced, TextRole},
    theme::ArtisanTheme,
};
use std::rc::Rc;

impl ComposerOwner {
    #[expect(
        clippy::too_many_lines,
        reason = "the owner measures tabs, scroll position and the engine light before building the menu"
    )]
    pub fn model_menu(
        &self,
        theme: &ArtisanTheme,
        host: &editor_core::HostData,
        viewport: aqua_core::Size,
        kept_scroll: f32,
        assets: &aqua::images::ImageAssets,
        now: std::time::Instant,
    ) -> Result<Element, aqua::images::ImageAssetError> {
        use super::super::model_list::ListLayout;
        let controller = self.controller.borrow();
        let picker = &controller.picker;
        let current = controller.effective_config(host);
        let engines = super::super::picker::offered_engines(host);
        let layout = ListLayout::new(theme, &picker.models);
        let items = model_menu_items(host, picker, current.clone());
        let pane = picker.axis.filter(|axis| {
            items
                .iter()
                .any(|item| item.pane == Some(*axis) && !item.locked)
        });
        let opening = self.model_menu_serial.replace(picker.open_serial) != picker.open_serial;
        let max = layout.max_scroll(MODEL_LIST_HEIGHT);
        let mut scrolling = self.model_scroll.borrow_mut();
        let kept = if opening {
            let centred = current
                .as_ref()
                .and_then(|config| {
                    layout.centred(
                        &picker.models,
                        config.engine,
                        config.model.as_deref()?,
                        MODEL_LIST_HEIGHT,
                    )
                })
                .unwrap_or(0.0);
            scrolling.open(centred);
            centred
        } else {
            kept_scroll
        };
        let scroll = if pane.is_none() {
            scrolling.offset(kept, max, now, self.reduced_motion)
        } else {
            scrolling.hide();
            0.0
        };
        if opening && scroll > 0.0 {
            picker.model_edges.restore(editor_ui::scroll_edge::Hidden {
                above: scroll,
                below: max - scroll,
            });
        }
        let spied = scrolling
            .target()
            .or_else(|| layout.spied(&picker.models, scroll, MODEL_LIST_HEIGHT))
            .filter(|engine| engines.iter().any(|status| status.engine == *engine));
        drop(scrolling);
        *self.model_layout.borrow_mut() = Some(layout);
        let active = super::rows(
            items
                .iter()
                .position(|item| item.pane == pane)
                .unwrap_or_default(),
        );
        let tab = {
            let mut tab = self.model_tab.borrow_mut();
            let height = model_item_height(theme);
            tab.resize(height);
            tab.aim(active * (height + MODEL_ITEM_GAP), now);
            if opening || self.reduced_motion {
                tab.snap();
            } else {
                tab.advance(now);
            }
            *tab
        };
        let engine_light = if let Some((index, engine)) = spied.and_then(|engine| {
            engines
                .iter()
                .position(|status| status.engine == engine)
                .map(|index| (index, engine))
        }) {
            let mut light = self.engine_light.borrow_mut();
            light.measure(
                engine,
                super::rows(index) * (MODEL_BAR_HEIGHT + MODEL_BAR_GAP.px()),
                MODEL_BAR_HEIGHT,
                now,
                self.reduced_motion,
            );
            match light.frame(now) {
                Some((left, _)) => Some(
                    self.engine_light_image
                        .borrow_mut()
                        .element(theme, assets, left)?,
                ),
                None => None,
            }
        } else {
            None
        };
        Ok(model_menu(
            theme,
            host,
            picker,
            current.as_ref(),
            viewport,
            ModelMenuFrame {
                items,
                pane,
                scroll,
                spied,
                tab,
                engine_light,
            },
            (host.instance(), controller.generation()),
            &self.dispatch,
        ))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the composer surface is placed by the workspace with its theme, frame clock, viewport and live question state"
    )]
    pub fn surface(
        &self,
        theme: &ArtisanTheme,
        host: &editor_core::HostData,
        placeholder: &str,
        questionnaires: Option<Element>,
        attachments: Option<Element>,
        mention_overlay: Option<Element>,
        thread_error: Option<Element>,
        now: std::time::Instant,
    ) -> Element {
        let suggestion = {
            let composer = self.controller.borrow();
            match &composer.target {
                super::super::controller::ComposerTarget::Thread { thread, .. }
                    if composer.matches_host(host)
                        && !composer.questionnaire.writing_answer()
                        && !host.is_working(thread) =>
                {
                    host.suggestion(thread).map(str::to_owned)
                }
                super::super::controller::ComposerTarget::Thread { .. }
                | super::super::controller::ComposerTarget::NewThread { .. } => None,
            }
        };
        self.editor.set_suggestion(suggestion);
        let (tab, strip) = {
            let mut controller = self.controller.borrow_mut();
            let project = match &controller.target {
                super::super::controller::ComposerTarget::NewThread { .. } => {
                    project_tab(theme, &controller, host, Rc::clone(&self.dispatch))
                }
                super::super::controller::ComposerTarget::Thread { .. } => None,
            };
            let tab = Some(
                UiBox::new()
                    .height(editor_ui::tab_card::TAB_CARD_TAB_HEIGHT_PX)
                    .spaced(Space::S16)
                    .style(Style::new().align(AlignItems::CENTER))
                    .child(self.model(theme, &controller, host))
                    .children(project)
                    .build(),
            );
            let strip = if tab.is_some() {
                if let Some(frame) = controller.strip.motion.frame(now, self.reduced_motion) {
                    self.strip_moving.set(frame.moving);
                    let mut ticker = self.note_ticker.borrow_mut();
                    ticker.set_text(frame.goal.objective.clone());
                    let content = goal_label(
                        theme,
                        &frame,
                        UiBox::new()
                            .style(TextRole::Label.inherit(theme))
                            .child(ticker.build(TextRole::Label.line_px(theme)))
                            .build(),
                        Rc::clone(&self.dispatch),
                    );
                    Some(editor_ui::tab_card::TabCardStrip {
                        reveal: frame.reveal,
                        content,
                    })
                } else {
                    self.strip_moving.set(false);
                    None
                }
            } else {
                self.strip_moving.set(false);
                None
            };
            (tab, strip)
        };
        let nested = tab.is_some();
        let card = self.card(
            theme,
            host,
            nested,
            placeholder,
            questionnaires,
            attachments,
            mention_overlay,
            thread_error,
        );
        match tab {
            Some(tab) => editor_ui::tab_card::TabCard {
                id: "composer-tab-card".into(),
                tab,
                body: card,
                strip,
            }
            .build(*theme, Rc::clone(&self.tab_geometry)),
            None => card,
        }
    }

    fn model(
        &self,
        theme: &ArtisanTheme,
        controller: &super::super::controller::Composer,
        host: &editor_core::HostData,
    ) -> Element {
        let config = controller.effective_config(host);
        let model = config.as_ref().and_then(|config| {
            host.engines.get(&config.engine).and_then(|status| {
                status
                    .models
                    .iter()
                    .find(|model| Some(&model.id) == config.model.as_ref())
            })
        });
        model_trigger(theme, config.as_ref(), model, Rc::clone(&self.dispatch))
    }

    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "the composer card is placed by the workspace with its theme, frame clock, viewport and live question state; the card assembles the strip, prompt, attachments, toolbar and failure callouts in order"
    )]
    pub fn card(
        &self,
        theme: &ArtisanTheme,
        host: &editor_core::HostData,
        nested: bool,
        placeholder: &str,
        questionnaires: Option<Element>,
        attachments: Option<Element>,
        mention_overlay: Option<Element>,
        thread_error: Option<Element>,
    ) -> Element {
        let controller = self.controller.borrow();
        let (face, disabled) = controller.primary(host);
        let now = std::time::Instant::now();
        let mut swap = self.primary_swap.get();
        swap.observe(face, now, self.reduced_motion);
        self.primary_swap.set(swap);
        let outgoing = swap.progress(now);
        let instance = controller.host_instance();
        let generation = controller.generation();
        let host_id = controller.target.host().clone();
        let dispatch = Rc::clone(&self.dispatch);
        let refused = controller
            .refused
            .map(|refusal| editor_core::Problem::new(refusal.to_string()));
        let draft_error = controller
            .session
            .as_ref()
            .and_then(|session| {
                session.read(|state| state.error().map(|error| (error, state.conflict())))
            })
            .map(|(error, conflict)| {
                draft_error(
                    theme,
                    ComposerFailure::Draft {
                        error: &error,
                        conflict,
                    },
                    &self.dispatch,
                )
            })
            .or_else(|| {
                host.preference_failure().map(|error| {
                    draft_error(
                        theme,
                        ComposerFailure::ModelChoice { error },
                        &self.dispatch,
                    )
                })
            })
            .or_else(|| {
                refused
                    .as_ref()
                    .or(controller.notice.as_ref())
                    .or_else(|| {
                        controller
                            .attachments
                            .as_ref()
                            .and_then(|attachments| attachments.notice.as_ref())
                    })
                    .map(|message| {
                        draft_error(theme, ComposerFailure::Notice { message }, &self.dispatch)
                    })
            });
        composer_card(
            theme,
            nested,
            CardParts {
                prompt: self.editor.prompt_with_scroll_edges(
                    "composer-prompt",
                    theme,
                    PROMPT_NAME,
                    placeholder,
                    &self.prompt_edges(theme),
                ),
                primary: primary(theme, face, outgoing, disabled, Rc::clone(&self.dispatch)),
                questionnaires,
                attachments,
                mention_overlay,
                thread_error,
                draft_error,
            },
        )
        .on_input(
            aqua_ui::input::InputKind::DragOver,
            aqua_ui::input::EventPhase::Bubble,
            {
                let host = host_id;
                move |event| {
                    use aqua_ui::input::{InputResponse, RoutedInput};
                    let acceptable = matches!(
                        &event.input,
                        RoutedInput::DragOver { payload, .. }
                            if payload.get::<editor_core::DraggedThread>().is_some_and(|thread| {
                                thread.host == host
                                    && artisan_mentions::valid_thread_title(&thread.title)
                            })
                    );
                    InputResponse {
                        accept_drop: acceptable,
                        ..InputResponse::default()
                    }
                }
            },
        )
        .on_input(
            aqua_ui::input::InputKind::Drop,
            aqua_ui::input::EventPhase::Bubble,
            {
                let dispatch = Rc::clone(&self.dispatch);
                move |event| {
                    use aqua_ui::input::{InputResponse, RoutedInput};
                    match &event.input {
                        RoutedInput::Drop { payload, .. } => {
                            match payload.get::<editor_core::DraggedThread>() {
                                Some(thread) => {
                                    dispatch(ComposerAction::DroppedThread(thread.clone()));
                                    InputResponse::handled()
                                }
                                None => InputResponse::default(),
                            }
                        }
                        RoutedInput::ClipboardPaste(_)
                        | RoutedInput::ClipboardCut
                        | RoutedInput::TextVertical { .. }
                        | RoutedInput::TextMotion { .. }
                        | RoutedInput::Native(_)
                        | RoutedInput::PointerEnter(_)
                        | RoutedInput::PointerLeave(_)
                        | RoutedInput::TextPointer { .. }
                        | RoutedInput::TextContext { .. }
                        | RoutedInput::TextRangeActivated { .. }
                        | RoutedInput::FilesDropped { .. }
                        | RoutedInput::DragEnter { .. }
                        | RoutedInput::DragOver { .. }
                        | RoutedInput::DragLeave { .. }
                        | RoutedInput::DragEnd { .. } => InputResponse::default(),
                    }
                }
            },
        )
        .on_input(
            aqua_ui::input::InputKind::FilesDropped,
            aqua_ui::input::EventPhase::Bubble,
            move |event| {
                use aqua_ui::input::{InputResponse, RoutedInput};
                if let (
                    Some(instance),
                    RoutedInput::FilesDropped {
                        paths,
                        at: Some(at),
                    },
                ) = (instance, &event.input)
                    && !paths.is_empty()
                    && event.bounds.contains(*at)
                {
                    dispatch(ComposerAction::DroppedPaths {
                        paths: paths.to_vec(),
                        instance,
                        generation,
                    });
                    return InputResponse::handled();
                }
                InputResponse::default()
            },
        )
    }
}
