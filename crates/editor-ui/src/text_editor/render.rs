//! Rendering, the paint-time input element, and the text-service bridge
//! ([`gpui::EntityInputHandler`]) for the native text editor.

#![forbid(unsafe_code)]

use crate::theme::ActiveTheme as _;
use gpui::prelude::FluentBuilder as _;
use std::{ops::Range, panic};

use crate::{
    asset_seam::asset_glyph,
    text_runs::{TextRunOverride, compile_text_runs},
    textarea::TextareaStyle,
    theme::{ColorTokens, ProseTypography},
};
use gpui::{
    AnyElement, App, Bounds, Context, CursorStyle, DispatchPhase, Element, ElementId,
    ElementInputHandler, Entity, FocusHandle, GlobalElementId, HighlightStyle, Hitbox,
    HitboxBehavior, InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, SharedString, StyledText, UTF16Selection,
    UnderlineStyle, Window, div,
    prelude::{
        InteractiveElement as _, ParentElement as _, StatefulInteractiveElement as _, Styled as _,
    },
    px, size,
};

use super::text::{utf8_offset_to_utf16, utf8_range_to_utf16, utf16_range_to_utf8, valid_bounds};
use super::{
    Mode, TEXT_EDITOR_KEY_CONTEXT, TEXT_EDITOR_PLACEHOLDER_SELECTOR, TEXT_EDITOR_SELECTOR,
    TextEditor,
};

/// Text size, matching the composer's `text-base`.
const TEXT_SIZE_PX: f32 = 16.0;
/// Line height, matching the composer's leading.
const LINE_HEIGHT_PX: f32 = 24.0;
/// Visible lines of an empty editor.
const MIN_VISIBLE_LINES: f32 = 8.0;
/// Text size and line height of a single-line field, matching control text.
const SINGLE_LINE_TEXT_SIZE_PX: f32 = 14.0;
const SINGLE_LINE_HEIGHT_PX: f32 = 20.0;
/// Visible lines of an empty prompt (64px with its padding).
const PROMPT_VISIBLE_LINES: f32 = 2.0;
/// Past this share of the window a prompt scrolls inside.
const PROMPT_MAX_SHARE: f32 = 0.4;

/// The editor's type size and chrome: a document, a compact one-line
/// field, or a prompt (no chrome; its card provides it).
struct EditorMetrics {
    text_size: Pixels,
    line_height: Pixels,
    visible_lines: f32,
    padding_x: Pixels,
    padding_y: Pixels,
    radius: Pixels,
}

impl EditorMetrics {
    const fn of(mode: Mode, style: &TextareaStyle) -> Self {
        match mode {
            Mode::SingleLine => Self {
                text_size: px(SINGLE_LINE_TEXT_SIZE_PX),
                line_height: px(SINGLE_LINE_HEIGHT_PX),
                visible_lines: 1.0,
                padding_x: px(6.0),
                padding_y: px(2.0),
                radius: px(6.0),
            },
            Mode::Document => Self {
                text_size: px(TEXT_SIZE_PX),
                line_height: px(LINE_HEIGHT_PX),
                visible_lines: MIN_VISIBLE_LINES,
                padding_x: style.horizontal_padding,
                padding_y: style.vertical_padding,
                radius: style.corner_radius,
            },
            Mode::Prompt => Self {
                text_size: px(TEXT_SIZE_PX),
                line_height: px(LINE_HEIGHT_PX),
                visible_lines: PROMPT_VISIBLE_LINES,
                padding_x: px(12.0),
                padding_y: px(8.0),
                radius: px(0.0),
            },
        }
    }
}

impl TextEditor {
    /// The root's box: a bordered field, or a borderless prompt that
    /// scrolls inside once it reaches its share of the window.
    fn chrome(
        &self,
        metrics: &EditorMetrics,
        style: &TextareaStyle,
        window: &Window,
        cx: &Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let prompt = self.mode == Mode::Prompt;
        let border_width = if prompt || self.bare {
            px(0.0)
        } else {
            style.border_width
        };
        let min_height = metrics.line_height * metrics.visible_lines
            + metrics.padding_y * 2.0
            + border_width * 2.0;
        let root = div()
            .id(("text-editor", cx.entity_id()))
            .debug_selector(|| TEXT_EDITOR_SELECTOR.to_owned())
            .key_context(TEXT_EDITOR_KEY_CONTEXT)
            .w_full()
            .min_w(px(0.0))
            .min_h(min_height)
            .px(metrics.padding_x)
            .py(metrics.padding_y);
        if prompt {
            let scroll = self.scroll.clone();
            root.max_h((window.viewport_size().height * PROMPT_MAX_SHARE).max(min_height * 2.0))
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                // Keep the wheel from scrolling what lies behind a long prompt.
                .on_scroll_wheel(move |_, _, cx| {
                    if scroll.max_offset().y > Pixels::ZERO {
                        cx.stop_propagation();
                    }
                })
        } else if self.bare {
            root
        } else {
            let focus_border = style.focus_border;
            root.rounded(metrics.radius)
                .border(border_width)
                .border_color(style.border)
                .bg(style.background)
                .focus(move |focused| focused.border_color(focus_border))
        }
    }

    /// Moves a prompt's scroll offset so the selection head is visible;
    /// true when it moved.
    fn scroll_caret_into_view(&self, viewport: Bounds<Pixels>, padding_y: Pixels) -> bool {
        let Some(layout) = self.layout.as_ref() else {
            return false;
        };
        let head = if self.selection_reversed {
            self.selection.start
        } else {
            self.selection.end
        };
        let Some(caret) = super::visual_position(layout, head) else {
            return false;
        };
        let line_height = layout.line_height();
        let mut offset = self.scroll.offset();
        let (top, bottom) = (viewport.top() + padding_y, viewport.bottom() - padding_y);
        if caret.y < top {
            offset.y += top - caret.y;
        } else if caret.y + line_height > bottom {
            offset.y -= caret.y + line_height - bottom;
        } else {
            return false;
        }
        offset.y = offset.y.min(Pixels::ZERO).max(-self.scroll.max_offset().y);
        self.scroll.set_offset(offset);
        true
    }

    /// Token styles under the selection wash or the IME underline. A
    /// selected token keeps its own colour.
    fn highlights(&mut self, theme: &ColorTokens) -> Vec<(Range<usize>, HighlightStyle)> {
        let tokens = self.tokens();
        let mut overlay = self.overlay_highlights(theme);
        if tokens.is_empty() {
            return overlay;
        }
        for (_, style) in &mut overlay {
            style.color = None;
        }
        let tokens = tokens
            .iter()
            .map(|token| (token.range.clone(), token.style))
            .collect::<Vec<_>>();
        gpui::combine_highlights(tokens, overlay).collect()
    }

    /// Selection highlight, or the underline of an IME composition.
    fn overlay_highlights(&self, theme: &ColorTokens) -> Vec<(Range<usize>, HighlightStyle)> {
        if let Some(marked) = self.marked_range.clone().filter(|range| !range.is_empty()) {
            return vec![(
                marked,
                HighlightStyle {
                    underline: Some(UnderlineStyle {
                        thickness: px(1.0),
                        color: Some(theme.text.to_paint()),
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
                color: Some(theme.text.to_paint()),
                background_color: Some(theme.text_selection.to_paint()),
                ..HighlightStyle::default()
            },
        )]
    }
}

impl TextEditor {
    /// The sigil range, sigil, icon and icon colour of each glyph token.
    fn glyphs(&mut self, desktop: &ColorTokens) -> Vec<Glyph> {
        self.tokens()
            .iter()
            .filter_map(|token| {
                let glyph = token.glyph?;
                let sigil = self.text[token.range.clone()].chars().next()?;
                let mut color = token.style.color.unwrap_or_else(|| desktop.text.to_paint());
                color.alpha *= crate::theme::Opacity::MentionGlyph.alpha();
                Some((
                    token.range.start..token.range.start + sigil.len_utf8(),
                    sigil,
                    glyph,
                    color,
                ))
            })
            .collect()
    }
}

/// A glyph token's sigil range, sigil, icon and icon colour.
type Glyph = (Range<usize>, char, crate::AssetId, gpui::Hsla);

impl Render for TextEditor {
    #[expect(
        clippy::too_many_lines,
        reason = "one element tree; splitting it would only thread its locals through helpers"
    )]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let style = TextareaStyle::resolve(&theme);
        let desktop = theme.colors;

        let highlights = self.highlights(&desktop);
        let glyphs = self.glyphs(&desktop);
        let text = GlyphText::new(self.shared_text(), highlights, glyphs);
        // The layout is read for hit-testing and the caret only after the
        // next paint records `painted_bounds`; GPUI's line-layout cache
        // reuses shaped lines whose text and wrap width are unchanged.
        self.layout = Some(text.layout());
        self.painted_bounds = None;

        let metrics = EditorMetrics::of(self.mode, &style);
        let line_height = metrics.line_height;
        let mut content = div()
            .relative()
            .w_full()
            .min_w(px(0.0))
            .whitespace_normal()
            .child(text);
        if self.text.is_empty() {
            // A prompt's placeholder is the composer's secondary text.
            let placeholder = if self.mode == Mode::Prompt {
                desktop.text_muted.to_paint()
            } else {
                style.placeholder_foreground
            };
            let hint = div()
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .w_full()
                .text_color(placeholder)
                .debug_selector(|| TEXT_EDITOR_PLACEHOLDER_SELECTOR.to_owned());
            content = content.child(match self.shown_suggestion() {
                // The keycap sits on the first line, after the words.
                Some(suggestion) => hint
                    .h(line_height)
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().min_w(px(0.0)).truncate().child(suggestion.clone()))
                    .child(crate::keycap::Keycap::new(theme, "Tab")),
                None => hint.child(self.placeholder.clone()),
            });
        }

        let root = self.chrome(&metrics, &style, window, cx);
        let mut root = root
            .text_color(desktop.text.to_paint())
            .text_size(metrics.text_size)
            .line_height(line_height)
            .font_weight(ProseTypography::BODY_WEIGHT)
            .tracking(theme.typography.tracking_em)
            .child(content);
        if self.disabled {
            root = root.opacity(style.disabled_opacity);
        } else {
            root = root
                .track_focus(&self.focus_handle)
                .on_action(cx.listener(Self::undo_action))
                .on_action(cx.listener(Self::redo_action))
                .on_action(cx.listener(Self::backspace))
                .on_action(cx.listener(Self::delete))
                .on_action(cx.listener(Self::left_action))
                .on_action(cx.listener(Self::right_action))
                .on_action(cx.listener(Self::select_left_action))
                .on_action(cx.listener(Self::select_right_action))
                .on_action(cx.listener(Self::word_left_action))
                .on_action(cx.listener(Self::word_right_action))
                .on_action(cx.listener(Self::select_word_left_action))
                .on_action(cx.listener(Self::select_word_right_action))
                .on_action(cx.listener(Self::backspace_word))
                .on_action(cx.listener(Self::delete_word))
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
                .on_action(cx.listener(Self::insert_line_break))
                .on_action(cx.listener(Self::insert_tab))
                .on_action(cx.listener(Self::dismiss))
                .on_action(cx.listener(Self::paste))
                .on_action(cx.listener(Self::copy))
                .on_action(cx.listener(Self::cut));
        }

        let root = div()
            .flex()
            .flex_col()
            .min_w_0()
            .child(root)
            .when(self.limit_notice, |root| {
                root.child(
                    div()
                        .text_xs()
                        .text_color(theme.colors.destructive_text.to_paint())
                        .child("Text is limited to 64 KiB. Attach a file for larger content."),
                )
            });
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
    view: Entity<TextEditor>,
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
        self.view.update(cx, |editor, cx| {
            editor.painted_bounds = valid_bounds(&bounds).then_some(bounds);
            if std::mem::take(&mut editor.follow_caret)
                && editor.mode == Mode::Prompt
                && editor.scroll_caret_into_view(bounds, px(8.0))
            {
                cx.notify();
            }
        });
        if let Some(caret) = self
            .view
            .read(cx)
            .caret_quad(window, cx.theme().colors.text.to_paint())
        {
            window.paint_quad(caret);
        }
    }
}

fn register_mouse_listeners(
    view: &Entity<TextEditor>,
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

impl gpui::EntityInputHandler for TextEditor {
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
            self.replace_range(range, new_text, new_selected_range.as_ref(), cx);
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

/// Width a token's glyph takes in place of its first character, gap
/// included.
const GLYPH_CELL_PX: f32 = 20.0;
/// Painted size of a token's glyph.
const GLYPH_SIZE_PX: f32 = 15.0;

/// The editor's styled text with icons painted over the sigils of glyph
/// tokens. Runs compile at layout against the inherited text style, so
/// each sigil widens to its glyph's cell measured in the font in use.
struct GlyphText {
    styled: StyledText,
    text: SharedString,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    /// Sigil range, sigil, glyph and glyph colour.
    glyphs: Vec<Glyph>,
}

impl GlyphText {
    fn new(
        text: SharedString,
        mut highlights: Vec<(Range<usize>, HighlightStyle)>,
        glyphs: Vec<Glyph>,
    ) -> Self {
        if !glyphs.is_empty() {
            let sigils = glyphs
                .iter()
                .map(|(range, ..)| {
                    (
                        range.clone(),
                        HighlightStyle {
                            color: Some(gpui::transparent_black()),
                            ..HighlightStyle::default()
                        },
                    )
                })
                .collect::<Vec<_>>();
            highlights = gpui::combine_highlights(highlights, sigils).collect();
        }
        Self {
            styled: StyledText::new(text.clone()),
            text,
            highlights,
            glyphs,
        }
    }

    fn layout(&self) -> gpui::TextLayout {
        self.styled.layout().clone()
    }
}

impl Element for GlyphText {
    type RequestLayoutState = ();
    type PrepaintState = Vec<AnyElement>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&style.font());
        let overrides = self
            .glyphs
            .iter()
            .map(|(range, sigil, ..)| {
                let advance = text_system
                    .advance(font_id, font_size, *sigil)
                    .map_or(px(0.0), |advance| advance.width);
                TextRunOverride {
                    range: range.clone(),
                    font_family: None,
                    letter_spacing: Some((px(GLYPH_CELL_PX) - advance).max(px(0.0))),
                }
            })
            .collect::<Vec<_>>();
        let runs = compile_text_runs(&self.text, &style, &self.highlights, &overrides);
        let styled = std::mem::replace(&mut self.styled, StyledText::new(""));
        self.styled = styled.with_runs(runs);
        self.styled.request_layout(None, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.styled
            .prepaint(None, inspector_id, bounds, state, window, cx);
        let layout = self.styled.layout().clone();
        let line_height = layout.line_height();
        let glyph = px(GLYPH_SIZE_PX);
        let mut glyphs = Vec::new();
        for (range, _, asset, color) in &self.glyphs {
            let Some(origin) = layout.position_for_index(range.start) else {
                continue;
            };
            let mut element = asset_glyph(*asset)
                .size(glyph)
                .text_color(*color)
                .into_any_element();
            let top = origin.y + (line_height - glyph) / 2.0;
            element.prepaint_as_root(
                gpui::point(origin.x, top),
                size(glyph, glyph).into(),
                window,
                cx,
            );
            glyphs.push(element);
        }
        glyphs
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        glyphs: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.styled.paint(
            None,
            inspector_id,
            bounds,
            request_layout,
            &mut (),
            window,
            cx,
        );
        for glyph in glyphs.iter_mut() {
            glyph.paint(window, cx);
        }
    }
}

impl IntoElement for GlyphText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}
