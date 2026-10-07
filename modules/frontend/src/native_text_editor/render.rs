//! Rendering, the paint-time input element, and the text-service bridge
//! ([`gpui::EntityInputHandler`]) for the native text editor.

#![forbid(unsafe_code)]

use std::{ops::Range, panic};

use artisan_ui::{
    textarea::TextareaStyle,
    theme::{ArtisanTheme, DesktopTheme, ProseTypography, ThemeMode},
};
use gpui::{
    AnyElement, App, Bounds, Context, CursorStyle, DispatchPhase, Element, ElementId,
    ElementInputHandler, Entity, FocusHandle, GlobalElementId, HighlightStyle, Hitbox,
    HitboxBehavior, InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, StyledText, UTF16Selection,
    UnderlineStyle, Window, div,
    prelude::{InteractiveElement as _, ParentElement as _, Styled as _},
    px, size,
};

use super::text::{utf8_offset_to_utf16, utf8_range_to_utf16, utf16_range_to_utf8, valid_bounds};
use super::{
    NATIVE_TEXT_EDITOR_KEY_CONTEXT, NATIVE_TEXT_EDITOR_PLACEHOLDER_SELECTOR,
    NATIVE_TEXT_EDITOR_SELECTOR, NativeTextEditor,
};

/// Text size, matching the composer's `text-base`.
const TEXT_SIZE_PX: f32 = 16.0;
/// Line height, matching the composer's leading.
const LINE_HEIGHT_PX: f32 = 24.0;
/// Visible lines of an empty editor.
const MIN_VISIBLE_LINES: f32 = 8.0;

impl NativeTextEditor {
    /// Selection highlight, or the underline of an IME composition.
    fn highlights(&self, theme: DesktopTheme) -> Vec<(Range<usize>, HighlightStyle)> {
        if let Some(marked) = self.marked_range.clone().filter(|range| !range.is_empty()) {
            return vec![(
                marked,
                HighlightStyle {
                    underline: Some(UnderlineStyle {
                        thickness: px(1.0),
                        color: Some(theme.foreground),
                        wavy: false,
                    }),
                    ..HighlightStyle::default()
                },
            )];
        }
        if self.selection.is_empty() {
            return Vec::new();
        }
        vec![(
            self.selection.clone(),
            HighlightStyle {
                color: Some(theme.foreground),
                background_color: Some(theme.selected),
                ..HighlightStyle::default()
            },
        )]
    }
}

impl Render for NativeTextEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        let style = TextareaStyle::resolve(theme, false);
        let desktop = DesktopTheme::neutral_dark();

        let highlights = self.highlights(desktop);
        let text = StyledText::new(self.shared_text());
        let text = if highlights.is_empty() {
            text
        } else {
            text.with_highlights(highlights)
        };
        // The layout is read for hit-testing and the caret only after the
        // next paint records `painted_bounds`; GPUI's line-layout cache
        // reuses shaped lines whose text and wrap width are unchanged.
        self.layout = Some(text.layout().clone());
        self.painted_bounds = None;

        let line_height = px(LINE_HEIGHT_PX);
        let mut content = div()
            .relative()
            .w_full()
            .min_w(px(0.0))
            .whitespace_normal()
            .child(text);
        if self.text.is_empty() {
            content = content.child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .left(px(0.0))
                    .w_full()
                    .text_color(style.placeholder_foreground)
                    .debug_selector(|| NATIVE_TEXT_EDITOR_PLACEHOLDER_SELECTOR.to_owned())
                    .child(self.placeholder.clone()),
            );
        }

        let min_height = line_height * MIN_VISIBLE_LINES
            + style.vertical_padding * 2.0
            + style.border_width * 2.0;
        let focus_border = style.focus_border;
        let mut root = div()
            .debug_selector(|| NATIVE_TEXT_EDITOR_SELECTOR.to_owned())
            .key_context(NATIVE_TEXT_EDITOR_KEY_CONTEXT)
            .w_full()
            .min_w(px(0.0))
            .min_h(min_height)
            .px(style.horizontal_padding)
            .py(style.vertical_padding)
            .rounded(style.corner_radius)
            .border(style.border_width)
            .border_color(style.border)
            .bg(style.background)
            .text_color(desktop.foreground)
            .text_size(px(TEXT_SIZE_PX))
            .line_height(line_height)
            .font_weight(ProseTypography::BODY_WEIGHT)
            .letter_spacing(px(ProseTypography::body_tracking_px(TEXT_SIZE_PX)))
            .child(content);
        if self.disabled {
            root = root.opacity(style.disabled_opacity);
        } else {
            root = root
                .track_focus(&self.focus_handle)
                .focus(move |focused| focused.border_color(focus_border))
                .on_action(cx.listener(Self::undo_action))
                .on_action(cx.listener(Self::redo_action))
                .on_action(cx.listener(Self::backspace))
                .on_action(cx.listener(Self::delete))
                .on_action(cx.listener(Self::left_action))
                .on_action(cx.listener(Self::right_action))
                .on_action(cx.listener(Self::select_left_action))
                .on_action(cx.listener(Self::select_right_action))
                .on_action(cx.listener(Self::select_all))
                .on_action(cx.listener(Self::home))
                .on_action(cx.listener(Self::end))
                .on_action(cx.listener(Self::select_home))
                .on_action(cx.listener(Self::select_end))
                .on_action(cx.listener(Self::document_home))
                .on_action(cx.listener(Self::document_end))
                .on_action(cx.listener(Self::select_document_home))
                .on_action(cx.listener(Self::select_document_end))
                .on_action(cx.listener(Self::up))
                .on_action(cx.listener(Self::down))
                .on_action(cx.listener(Self::select_up))
                .on_action(cx.listener(Self::select_down))
                .on_action(cx.listener(Self::insert_newline))
                .on_action(cx.listener(Self::insert_tab))
                .on_action(cx.listener(Self::paste))
                .on_action(cx.listener(Self::copy))
                .on_action(cx.listener(Self::cut));
        }

        InputElement {
            child: root.into_any_element(),
            view: cx.entity(),
            focus_handle: self.focus_handle.clone(),
        }
    }
}

/// Wraps the editor chrome and registers the input handler, the mouse
/// listeners, and the caret during paint.
struct InputElement {
    child: AnyElement,
    view: Entity<NativeTextEditor>,
    focus_handle: FocusHandle,
}

impl IntoElement for InputElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for InputElement {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

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
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.child.prepaint(window, cx);
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let disabled = self.view.read(cx).disabled;
        if !disabled {
            window.handle_input(
                &self.focus_handle,
                ElementInputHandler::new(bounds, self.view.clone()),
                cx,
            );
            window.set_cursor_style(CursorStyle::IBeam, hitbox);
            register_mouse_listeners(&self.view, &self.focus_handle, hitbox, window);
        }
        self.child.paint(window, cx);
        self.view.update(cx, |editor, _| {
            editor.painted_bounds = valid_bounds(&bounds).then_some(bounds);
        });
        if let Some(caret) = self.view.read(cx).caret_quad(window) {
            window.paint_quad(caret);
        }
    }
}

fn register_mouse_listeners(
    view: &Entity<NativeTextEditor>,
    focus_handle: &FocusHandle,
    hitbox: &Hitbox,
    window: &mut Window,
) {
    window.on_mouse_event({
        let view = view.clone();
        let focus_handle = focus_handle.clone();
        let hitbox = hitbox.clone();
        move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || event.button != MouseButton::Left
                || !hitbox.is_hovered(window)
            {
                return;
            }
            cx.stop_propagation();
            window.focus(&focus_handle, cx);
            view.update(cx, |editor, editor_cx| {
                editor.mouse_down(
                    event.position,
                    event.modifiers.shift,
                    event.click_count,
                    editor_cx,
                );
            });
        }
    });
    window.on_mouse_event({
        let view = view.clone();
        move |event: &MouseMoveEvent, phase, _window, cx| {
            if phase != DispatchPhase::Bubble || !event.dragging() {
                return;
            }
            if view.read(cx).selection_dragging {
                view.update(cx, |editor, editor_cx| {
                    editor.mouse_drag(event.position, editor_cx);
                });
            }
        }
    });
    window.on_mouse_event({
        let view = view.clone();
        move |event: &MouseUpEvent, phase, _window, cx| {
            if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                return;
            }
            if view.read(cx).selection_dragging {
                view.update(cx, |editor, _| editor.mouse_up());
            }
        }
    });
}

impl gpui::EntityInputHandler for NativeTextEditor {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let utf8_range = utf16_range_to_utf8(&self.text, range.clone())?;
        *adjusted_range = Some(range);
        self.text.get(utf8_range).map(str::to_owned)
    }

    fn selected_text_range(
        &mut self,
        ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        if self.disabled && !ignore_disabled_input {
            return None;
        }
        Some(UTF16Selection {
            range: utf8_range_to_utf16(&self.text, self.selection.clone())?,
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
            .and_then(|range| utf8_range_to_utf16(&self.text, range))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.marked_range.take().is_some() {
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
        if let Some(range) = self.replacement_range(range) {
            self.replace_range(range, text, None, cx);
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(range) = self.replacement_range(range) {
            self.replace_range(range, new_text, new_selected_range, cx);
        }
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
        let range = utf16_range_to_utf8(&self.text, range_utf16)?;
        let layout = self.layout.as_ref()?;
        let start = super::visual_position(layout, range.start)?;
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
        let index = self.index_for_point(point, false)?;
        utf8_offset_to_utf16(&self.text, index)
    }
}
