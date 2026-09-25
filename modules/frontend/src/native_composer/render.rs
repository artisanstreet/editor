//! GPUI render builders and the text-service input bridge for the native
//! composer: tray, viewer, root layout, input element, and `EntityInputHandler`.
//!
//! Extracted verbatim from `native_composer.rs` during the module split.

#![forbid(unsafe_code)]

use super::*;

impl NativeComposer {
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder composes the tray row, per-attachment chips, and viewer wiring in visual order"
    )]
    fn attachment_tray(
        &self,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        desktop_theme: DesktopTheme,
    ) -> Stateful<Div> {
        // Reference (`attachment-tray.svelte:28-30`): the open row carries
        // `px-1 pt-1 pb-2`. The tray only mounts while attachments exist,
        // which is exactly the reference open state.
        let mut row = div()
            .id("artisan-native-composer-attachment-tray-row")
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(4.0))
            .pt(px(4.0))
            .pb(px(8.0))
            .overflow_x_scroll();

        for (position, attachment) in self.attachments.iter().enumerate() {
            let attachment_id = attachment.id.clone();
            let name = attachment.name.clone();
            let view_entity = entity.clone();
            // Reference (`attachment-tray.svelte:32`): `card relative size-18
            // overflow-hidden rounded-xl`. The tile is a regular card, not a
            // card-glass surface.
            let mut tile = div()
                .id(format!("artisan-native-composer-attachment-{position}"))
                .relative()
                .size(px(NATIVE_COMPOSER_ATTACHMENT_SIZE))
                .flex_none()
                .overflow_hidden()
                .rounded(px(14.0))
                .shadow(card_shadows(theme))
                .bg(desktop_theme.field)
                .cursor_pointer()
                .role(gpui::Role::Button)
                .aria_label(format!("View {name}"))
                .debug_selector(|| "artisan-native-composer-attachment".to_owned())
                .on_click(move |_, _, cx| {
                    view_entity.update(cx, |composer, composer_cx| {
                        composer.view_attachment(&attachment_id, composer_cx);
                    });
                });

            if let Some(thumbnail) = attachment.thumbnail.clone() {
                tile = tile.child(
                    img(ImageSource::Render(thumbnail))
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                );
            } else {
                tile = tile.child(
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .px(px(5.0))
                        .text_color(desktop_theme.secondary)
                        .text_size(px(11.0))
                        .child("Preparing…"),
                );
            }

            let remove_click_id = attachment.id.clone();
            let remove_key_id = attachment.id.clone();
            let remove_click_entity = entity.clone();
            let remove_key_entity = entity.clone();
            let remove_label = format!("Remove {name}");
            // Reference (`attachment-tray.svelte:41-49`): `absolute
            // top/right 0.2rem`, `size-5.5`, secondary icon button, `X
            // size-3.5`.
            let remove = div()
                .id(format!(
                    "artisan-native-composer-attachment-remove-{position}"
                ))
                .absolute()
                .top(px(3.2))
                .right(px(3.2))
                .size(px(22.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(desktop_theme.chrome.opacity(0.9))
                .text_color(desktop_theme.foreground)
                .cursor_pointer()
                .tab_index(0)
                .role(gpui::Role::Button)
                .aria_label(remove_label)
                .debug_selector(|| "artisan-native-composer-attachment-remove".to_owned())
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    remove_click_entity.update(cx, |composer, composer_cx| {
                        composer.remove_attachment(&remove_click_id, composer_cx);
                    });
                })
                .on_key_down(move |event, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        cx.stop_propagation();
                        remove_key_entity.update(cx, |composer, composer_cx| {
                            composer.remove_attachment(&remove_key_id, composer_cx);
                        });
                    }
                })
                .child(asset_glyph(AssetId::TABLER_X).size(px(14.0)));
            tile = tile.child(remove);
            row = row.child(tile);
        }

        let mut tray = div()
            .id(NATIVE_COMPOSER_ATTACHMENT_TRAY_SELECTOR)
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .debug_selector(|| NATIVE_COMPOSER_ATTACHMENT_TRAY_SELECTOR.to_owned())
            .aria_label("Attachments")
            .child(row);
        if let Some(error) = self.attachment_error.clone() {
            tray = tray.child(
                div()
                    .text_color(desktop_theme.secondary)
                    .text_size(px(12.0))
                    .child(error),
            );
        }
        if !self.attachment_delivery_enabled {
            tray = tray.child(
                div()
                    .id(NATIVE_COMPOSER_ATTACHMENT_BLOCKED_SELECTOR)
                    .text_color(desktop_theme.secondary)
                    .text_size(px(12.0))
                    .debug_selector(|| NATIVE_COMPOSER_ATTACHMENT_BLOCKED_SELECTOR.to_owned())
                    .child("Images stay attached until image delivery is available."),
            );
        }
        tray
    }

    /// Fades a newly mounted tray in on the reference open clock.
    ///
    /// Each hidden-to-shown mount carries a fresh animation identity from
    /// `tray_entrance_generation`, so the entrance replays every time the
    /// tray opens. Reduced motion paints the settled tray immediately.
    /// Styling finishes first as `Stateful<Div>`; the animated and plain
    /// branches converge here to `AnyElement` for the card boundary.
    fn animate_tray_entrance(&self, tray: Stateful<Div>, cx: &mut Context<Self>) -> AnyElement {
        if cx.reduce_motion() {
            return tray.into_any_element();
        }
        let generation = self.tray_entrance_generation;
        tray.opacity(0.0)
            .with_animation(
                ElementId::Name(
                    format!("artisan-native-composer-tray-entrance-{generation}").into(),
                ),
                Animation::new(Duration::from_millis(COMPOSER_TRAY_MOTION_MS))
                    .with_easing(composer_smooth_out),
                move |tray, progress| tray.opacity(progress.clamp(0.0, 1.0)),
            )
            .into_any_element()
    }

    fn attachment_viewer(
        &self,
        entity: &Entity<Self>,
        theme: DesktopTheme,
    ) -> Option<impl IntoElement> {
        let viewed_id = self.viewed_attachment.as_ref()?;
        let preview = self
            .attachment_preview
            .as_ref()
            .filter(|preview| {
                preview.attachment_id == *viewed_id
                    && preview.draft_generation == self.draft_generation
                    && preview.request == self.attachment_preview_request
            })
            .map(|preview| preview.image.clone());
        let preview_error = self.attachment_preview_error.clone();

        let dismiss_entity = entity.clone();
        let dismiss = div()
            .id("artisan-native-composer-attachment-viewer-dismiss")
            .absolute()
            .left(Pixels::ZERO)
            .top(Pixels::ZERO)
            .right(Pixels::ZERO)
            .bottom(Pixels::ZERO)
            .on_click(move |_, _, cx| {
                dismiss_entity.update(cx, |composer, composer_cx| {
                    composer.close_attachment_viewer(composer_cx);
                });
            });

        let close_entity = entity.clone();
        let close = div()
            .id("artisan-native-composer-attachment-viewer-close")
            .absolute()
            .top(px(8.0))
            .right(px(8.0))
            .size(px(28.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(theme.chrome.opacity(0.9))
            .text_color(theme.foreground)
            .cursor_pointer()
            .tab_index(0)
            .role(gpui::Role::Button)
            .aria_label("Close image preview")
            .debug_selector(|| "artisan-native-composer-attachment-viewer-close".to_owned())
            .on_click(move |_, _, cx| {
                close_entity.update(cx, |composer, composer_cx| {
                    composer.close_attachment_viewer(composer_cx);
                });
            })
            .child(asset_glyph(AssetId::TABLER_X).size(px(16.0)));

        let mut content = div()
            .id("artisan-native-composer-attachment-viewer-content")
            .relative()
            .max_w(px(960.0))
            .max_h(px(720.0))
            .flex()
            .items_center()
            .justify_center()
            .on_click(|_, _, cx| cx.stop_propagation());
        if let Some(preview) = preview {
            content = content.child(
                img(ImageSource::Render(preview))
                    .max_w(px(960.0))
                    .max_h(px(720.0))
                    .object_fit(ObjectFit::Contain),
            );
        } else if let Some(error) = preview_error {
            content = content
                .px(px(16.0))
                .py(px(12.0))
                .text_color(theme.secondary)
                .child(format!("Preview unavailable: {error}"));
        } else {
            content = content
                .px(px(16.0))
                .py(px(12.0))
                .text_color(theme.secondary)
                .child("Preparing preview…");
        }

        Some(
            div()
                .id(NATIVE_COMPOSER_ATTACHMENT_VIEWER_SELECTOR)
                .absolute()
                .left(Pixels::ZERO)
                .top(Pixels::ZERO)
                .right(Pixels::ZERO)
                .bottom(Pixels::ZERO)
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.chrome.opacity(0.97))
                .occlude()
                .debug_selector(|| NATIVE_COMPOSER_ATTACHMENT_VIEWER_SELECTOR.to_owned())
                .child(dismiss)
                .child(content)
                .child(close),
        )
    }
}

impl Render for NativeComposer {
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI render builder assembles the composer chrome, text layout, and overlays that share reactive state"
    )]
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.prune_attachment_tasks();
        let entity = cx.entity();
        let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        let desktop_theme = DesktopTheme::neutral_dark();
        let draft = self.state.draft().to_owned();
        let styled_text = if self.selection.is_empty() {
            StyledText::new(SharedString::from(draft))
        } else {
            StyledText::new(SharedString::from(draft)).with_highlights([(
                self.selection.clone(),
                HighlightStyle {
                    color: Some(desktop_theme.foreground),
                    background_color: Some(desktop_theme.selected),
                    ..Default::default()
                },
            )])
        };
        self.painted_bounds = None;
        self.layout = Some(styled_text.layout().clone());

        let focus = self.focus_handle.clone();
        // Reference (`thread-composer.svelte:571-587`): `min-h-16 px-3 py-2
        // text-base`, uncapped with no internal scroll. Growth pushes the
        // absolute overlay taller while the transcript end space preserves
        // scroll-to-bottom (surface lane); no pixel cap lives here.
        let mut editor = div()
            .id("artisan-native-composer-editor")
            .debug_selector(|| NATIVE_COMPOSER_EDITOR_SELECTOR.to_string())
            .key_context(NATIVE_COMPOSER_KEY_CONTEXT)
            .w_full()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(64.0))
            .px(px(12.0))
            .py(px(8.0))
            .text_color(desktop_theme.foreground)
            .text_size(px(16.0))
            .line_height(px(24.0))
            .font_weight(ProseTypography::BODY_WEIGHT)
            .letter_spacing(px(ProseTypography::body_tracking_px(16.0)))
            .whitespace_normal()
            .track_focus(&focus)
            .child(styled_text);

        // Reference visibility (`thread-composer.svelte:180-187,559`): the
        // placeholder shows only while the composed value is empty, where an
        // attachment counts as content. Each fresh reveal walks the reference
        // vocabulary (`composer-placeholder.ts:54-67`). The per-character
        // `placeholder-reveal-in` keyframes are dead in the reference CSS (no
        // rule applies them), so the phrase paints statically.
        let placeholder_visible = self.state.draft().is_empty() && self.attachments.is_empty();
        if placeholder_visible && !self.placeholder_was_visible {
            self.placeholder_generation = self.placeholder_generation.wrapping_add(1);
        }
        self.placeholder_was_visible = placeholder_visible;
        if placeholder_visible {
            let phrase = composer_placeholder_phrase(self.placeholder_generation);
            editor = editor.child(
                div()
                    .absolute()
                    .top(px(8.0))
                    .left(px(12.0))
                    .text_color(desktop_theme.secondary)
                    .text_size(px(16.0))
                    .line_height(px(24.0))
                    .whitespace_normal()
                    .debug_selector(|| NATIVE_COMPOSER_PLACEHOLDER_SELECTOR.to_string())
                    .child(phrase),
            );
        }

        editor = editor
            .on_action(cx.listener(Self::undo_action))
            .on_action(cx.listener(Self::redo_action))
            .on_action(cx.listener(Self::delete_backward))
            .on_action(cx.listener(Self::delete_forward))
            .on_action(cx.listener(Self::move_left_action))
            .on_action(cx.listener(Self::move_right_action))
            .on_action(cx.listener(Self::select_left_action))
            .on_action(cx.listener(Self::select_right_action))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::move_home))
            .on_action(cx.listener(Self::move_end))
            .on_action(cx.listener(Self::move_up_action))
            .on_action(cx.listener(Self::move_down_action))
            .on_action(cx.listener(Self::select_home_action))
            .on_action(cx.listener(Self::select_end_action))
            .on_action(cx.listener(Self::select_up_action))
            .on_action(cx.listener(Self::select_down_action))
            .on_action(cx.listener(Self::move_document_home_action))
            .on_action(cx.listener(Self::move_document_end_action))
            .on_action(cx.listener(Self::select_document_home_action))
            .on_action(cx.listener(Self::select_document_end_action))
            .on_action(cx.listener(Self::request_send_action))
            .on_action(cx.listener(Self::insert_newline))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut));

        let editor =
            NativeComposerInputElement::new(editor.into_any_element(), entity.clone(), focus);
        let mounted_controls = self.controls.clone();
        let mounted_model_selector = self.model_selector.clone();
        let (controls_lip, controls_failure, controls_failed, controls_row, jump_to_latest) =
            if let Some((controls, model_selector)) = mounted_controls.zip(mounted_model_selector) {
                let controls_lip = controls.update(cx, |controls, controls_cx| {
                    controls.render_lip(theme, controls_cx)
                });
                let controls_failure = controls.update(cx, |controls, controls_cx| {
                    controls.render_failure(theme, controls_cx)
                });
                let controls_failed = controls.update(cx, |controls, controls_cx| {
                    controls.render_failed_dispatches(theme, controls_cx)
                });
                let controls_row = controls.update(cx, |controls, controls_cx| {
                    controls.render_control_row(theme, model_selector.clone(), controls_cx)
                });
                let jump_to_latest = controls.update(cx, |controls, controls_cx| {
                    controls.render_jump_to_latest(theme, controls_cx)
                });
                (
                    controls_lip,
                    controls_failure,
                    controls_failed,
                    Some(controls_row),
                    jump_to_latest,
                )
            } else {
                (None, None, None, None, None)
            };

        let legacy_toolbar = if controls_row.is_none() {
            let send_ready = self.send_ready();
            self.send_focus_handle = self.send_focus_handle.clone().tab_stop(send_ready);
            let send_entity = entity.clone();
            let send = AccessibleLabel::new("Send message").ok().and_then(|label| {
                Button::new(
                    NATIVE_COMPOSER_SEND_SELECTOR,
                    self.send_focus_handle.clone(),
                    theme,
                    MotionPolicy::Reduced,
                    ButtonVariant::Default,
                    ButtonSize::IconSmall,
                    ButtonContent::icon_only(AssetId::TABLER_ARROW_UP, label),
                )
                .ok()
            });
            let send = send.map(|button| {
                button
                    .focus_visibility(FocusVisibility::Visible)
                    .corner_radius(px(10.0))
                    .disabled(!send_ready)
                    .debug_selector(NATIVE_COMPOSER_SEND_SELECTOR)
                    .on_activate(move |_, _, cx| {
                        send_entity.update(cx, NativeComposer::request_send);
                    })
            });

            let model = div()
                .id("artisan-composer-model")
                .track_focus(&self.model_focus_handle)
                .tab_index(0)
                .h(px(32.0))
                .min_w(px(0.0))
                .px(px(8.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .rounded(px(10.0))
                .cursor_pointer()
                .hover(move |style| style.bg(desktop_theme.selected))
                .text_color(desktop_theme.secondary)
                .text_size(px(13.0))
                .debug_selector(|| "artisan-composer-model".to_owned())
                .child(div().truncate().child(self.model_label.clone()))
                .child(asset_glyph(AssetId::TABLER_CHEVRON_DOWN).size(px(14.0)))
                .on_click(cx.listener(|_, _, _, cx| cx.emit(NativeComposerEvent::ConfigureModel)))
                .on_key_down(cx.listener(|_, event: &gpui::KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        cx.stop_propagation();
                        cx.emit(NativeComposerEvent::ConfigureModel);
                    }
                }));

            let mut toolbar = div()
                .w_full()
                .flex()
                .h(px(32.0))
                .flex_shrink_0()
                .items_center()
                .justify_between()
                .child(model);
            if let Some(send) = send {
                toolbar = toolbar.child(send);
            }
            Some(toolbar)
        } else {
            None
        };

        let drop_entity = entity.clone();
        // The 112px minimum fits the 64px editor, 32px controls, and 8px
        // padding on each edge. The tray carries its own open padding.
        let mut root = div()
            .id("artisan-native-composer")
            .debug_selector(|| "artisan-native-composer".to_owned())
            .w_full()
            .flex()
            .flex_col()
            .min_h(px(112.0))
            .p(px(8.0))
            .rounded(px(18.0))
            .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
            .bg(glass_foreground_base(&theme))
            .shadow(glass_card_shadows())
            .relative()
            .child(glass_material_layer(GlassStrength::Quiet, px(18.0)))
            .child(glass_highlight_layer(GlassStrength::Quiet, px(18.0)))
            .on_drop::<ExternalPaths>(move |paths, _, cx| {
                let paths = paths.paths().to_vec();
                drop_entity.update(cx, |composer, composer_cx| {
                    composer.enqueue_file_drop(paths, composer_cx);
                });
            });

        if self.attachments.is_empty() || self.state.submission_is_eager() {
            self.tray_was_open = false;
            if let Some(error) = self.attachment_error.clone() {
                root = root.child(
                    div()
                        .id(NATIVE_COMPOSER_ATTACHMENT_BLOCKED_SELECTOR)
                        .text_color(desktop_theme.secondary)
                        .text_size(px(12.0))
                        .debug_selector(|| NATIVE_COMPOSER_ATTACHMENT_BLOCKED_SELECTOR.to_owned())
                        .child(error),
                );
            }
        } else {
            // Reference open motion (`attachment-tray.svelte:25`): the tray
            // fades in on `--composer-resize-dur` (300ms). The grid-track
            // height tween has no GPUI primitive (see the lane report), so
            // only the opacity half is reproduced. Close unmounts
            // immediately; a fade-out would need a retained tile snapshot
            // plus a settle timer for zero visual gain on a surface the user
            // just dismissed.
            if !self.tray_was_open {
                self.tray_entrance_generation = self.tray_entrance_generation.wrapping_add(1);
            }
            self.tray_was_open = true;
            let tray = self.attachment_tray(&entity, &theme, desktop_theme);
            root = root.child(self.animate_tray_entrance(tray, cx));
        }
        root = root.child(editor);
        if let Some(controls_row) = controls_row {
            root = root.child(controls_row);
        } else if let Some(legacy_toolbar) = legacy_toolbar {
            root = root.child(legacy_toolbar);
        }
        if let Some(viewer) = self.attachment_viewer(&entity, desktop_theme) {
            root = root.child(viewer);
        }
        // Reference (`thread-composer.svelte:526-543`): jump, failure, and
        // the queued lip are siblings above the composer card, spaced by the
        // frame's `gap-2`. The card holds only tray, editor, and controls.
        let mut shell = div().w_full().flex().flex_col().gap(px(8.0));
        if let Some(jump_to_latest) = jump_to_latest {
            shell = shell.child(jump_to_latest);
        }
        if let Some(failure) = controls_failure {
            shell = shell.child(failure);
        }
        if let Some(failed) = controls_failed {
            shell = shell.child(failed);
        }
        if let Some(lip) = controls_lip {
            shell = shell.child(lip);
        }
        shell.child(root)
    }
}

/// An element wrapper that registers the entity input handler in paint.
struct NativeComposerInputElement {
    child: AnyElement,
    view: Entity<NativeComposer>,
    focus_handle: FocusHandle,
}

impl NativeComposerInputElement {
    fn new(child: AnyElement, view: Entity<NativeComposer>, focus_handle: FocusHandle) -> Self {
        Self {
            child,
            view,
            focus_handle,
        }
    }
}

impl Element for NativeComposerInputElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.handle_input(
            &self.focus_handle,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );

        window.on_mouse_event({
            let view = self.view.clone();
            let focus_handle = self.focus_handle.clone();
            move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble
                    || event.button != MouseButton::Left
                    || !bounds.contains(&event.position)
                {
                    return;
                }

                cx.stop_propagation();
                window.focus(&focus_handle, cx);
                view.update(cx, |composer, composer_cx| {
                    composer.begin_selection_drag(
                        event.position,
                        event.modifiers.shift,
                        composer_cx,
                    );
                });
            }
        });
        window.on_mouse_event({
            let view = self.view.clone();
            move |event: &MouseMoveEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble || !event.dragging() {
                    return;
                }

                let is_dragging = view.read(cx).selection_dragging;
                if is_dragging {
                    view.update(cx, |composer, composer_cx| {
                        composer.update_selection_drag(event.position, composer_cx);
                    });
                }
            }
        });
        window.on_mouse_event({
            let view = self.view.clone();
            move |event: &MouseUpEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }

                if view.read(cx).selection_dragging {
                    view.update(cx, |composer, _| composer.end_selection_drag());
                }
            }
        });
        self.child.paint(window, cx);
        if let Some(caret) = self.view.read(cx).caret_quad(window) {
            window.paint_quad(caret);
        }
        self.view.update(cx, |composer, _| {
            composer.painted_bounds = valid_bounds(&bounds).then_some(bounds);
        });
    }
}

impl IntoElement for NativeComposerInputElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl gpui::EntityInputHandler for NativeComposer {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let utf8_range = utf16_range_to_utf8(self.state.draft(), range.clone())?;
        *adjusted_range = Some(range);
        Some(self.state.draft()[utf8_range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        if self.state.is_disabled() && !ignore_disabled_input {
            return None;
        }
        let draft = self.state.draft();
        let selection = self.current_selection();
        Some(UTF16Selection {
            range: utf8_range_to_utf16(draft, selection)?,
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .clone()
            .and_then(|range| utf8_range_to_utf16(self.state.draft(), range))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.marked_range.take().is_some() {
            self.advance_selection_revision();
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(range) = self.replacement_range(range) else {
            return;
        };
        self.replace_range(range, text, None, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(range) = self.replacement_range(range) else {
            return;
        };
        self.replace_range(range, new_text, new_selected_range, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let painted_bounds = self.painted_bounds.as_ref()?;
        if painted_bounds != &element_bounds || !valid_bounds(painted_bounds) {
            return None;
        }
        let draft = self.state.draft();
        let range = utf16_range_to_utf8(draft, range_utf16)?;
        let layout = self.layout.as_ref()?;
        let start = layout.position_for_index(range.start)?;
        let end = layout.position_for_index(range.end)?;
        let width = (end.x - start.x).max(px(1.0));
        let bounds = Bounds::new(start, size(width, layout.line_height()));
        valid_bounds(&bounds).then_some(bounds)
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let draft = self.state.draft();
        let byte_index = self.byte_index_for_global_point(point)?;
        utf8_offset_to_utf16(draft, byte_index)
    }
}
