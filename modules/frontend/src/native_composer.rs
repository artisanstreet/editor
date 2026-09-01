//! Editable GPUI composer for the native first-message workflow.
//!
//! [`ComposerState`] remains the only draft and submission authority. This
//! entity owns the toolkit state needed to bridge GPUI 0.2.2 text services:
//! focus, UTF-8 selection, marked text, and the current text layout. It never
//! normalizes, trims, or otherwise rewrites authored text.

#![forbid(unsafe_code)]

use std::{ops::Range, panic};

use artisan_ui::{
    button::{Button, ButtonContent, ButtonSize, ButtonVariant, FocusVisibility},
    input::InputStyle,
    motion::MotionPolicy,
    theme::{ArtisanTheme, ThemeMode},
};
use gpui::{
    actions, div, point,
    prelude::{
        InteractiveElement as _, ParentElement as _, StatefulInteractiveElement as _, Styled as _,
    },
    px, size, AnyElement, App, Bounds, ClipboardItem, Context, Element, ElementId,
    ElementInputHandler, Entity, EventEmitter, FocusHandle, Focusable, GlobalElementId,
    InspectorElementId, IntoElement, KeyBinding, LayoutId, MouseButton, Pixels, Point, Render,
    SharedString, StyledText, UTF16Selection, Window,
};

use crate::composer::{ComposerState, DraftDisposition, SubmissionBlocked, SubmissionToken};

actions!(
    native_composer,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Paste,
        Copy,
        Cut,
        Home,
        End,
        RequestSend,
        InsertNewline,
    ]
);

const NATIVE_COMPOSER_KEY_CONTEXT: &str = "artisan-native-composer";
const NATIVE_COMPOSER_PLACEHOLDER: &str = "Do anything";
const NATIVE_COMPOSER_PLACEHOLDER_SELECTOR: &str = "artisan-native-composer-placeholder";
const NATIVE_COMPOSER_SEND_SELECTOR: &str = "artisan-native-composer-send";

/// One bounded application event emitted by the send control.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NativeComposerEvent {
    /// The user invoked the send control; the application owns admission.
    SendRequested,
}

/// Native GPUI owner of the exact composer draft and its text-service state.
pub(crate) struct NativeComposer {
    state: ComposerState,
    focus_handle: FocusHandle,
    send_focus_handle: FocusHandle,
    selection: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    layout: Option<gpui::TextLayout>,
    painted_bounds: Option<Bounds<Pixels>>,
}

impl EventEmitter<NativeComposerEvent> for NativeComposer {}

impl Focusable for NativeComposer {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl NativeComposer {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            state: ComposerState::new(),
            focus_handle: cx.focus_handle().tab_index(0).tab_stop(true),
            send_focus_handle: cx.focus_handle().tab_index(1).tab_stop(true),
            selection: 0..0,
            selection_reversed: false,
            marked_range: None,
            layout: None,
            painted_bounds: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn draft(&self) -> &str {
        self.state.draft()
    }

    /// Returns whether the authored draft is byte-identical to `body`.
    ///
    /// This read-only seam keeps draft comparison out of the submission
    /// lifecycle: it does not expose text, allocate, mint a token, mutate
    /// editor state, or notify observers.
    pub(crate) fn draft_matches_body(&self, body: &artisan_domain::MessageBody) -> bool {
        self.state.draft().as_bytes() == body.as_str().as_bytes()
    }

    #[cfg(test)]
    pub(crate) fn set_draft(&mut self, draft: impl Into<String>) {
        self.state.set_draft(draft);
        self.layout = None;
        self.painted_bounds = None;
        let end = self.state.draft().len();
        self.selection = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
    }

    pub(crate) fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        if self.state.is_disabled() == disabled {
            return;
        }
        self.state.set_disabled(disabled);
        cx.notify();
    }

    pub(crate) fn send_ready(&self) -> bool {
        self.state.send_ready() && self.marked_range.is_none()
    }

    pub(crate) fn is_submitting(&self) -> bool {
        self.state.is_submitting()
    }

    pub(crate) fn begin_submission(
        &mut self,
    ) -> Result<(artisan_domain::MessageBody, SubmissionToken), SubmissionBlocked> {
        self.state.begin_submission()
    }

    pub(crate) fn finish_submission(
        &mut self,
        token: SubmissionToken,
        disposition: DraftDisposition,
        cx: &mut Context<Self>,
    ) {
        let was_submitting = self.state.is_submitting();
        self.state.finish_submission(token, disposition);
        if was_submitting && !self.state.is_submitting() {
            self.layout = None;
            self.painted_bounds = None;
            let end = self.selection.end.min(self.state.draft().len());
            self.selection = end..end;
            self.selection_reversed = false;
            self.marked_range = None;
            cx.notify();
        }
    }

    fn request_send(&mut self, cx: &mut Context<Self>) {
        if self.send_ready() {
            cx.emit(NativeComposerEvent::SendRequested);
        }
    }

    fn request_send_action(&mut self, _: &RequestSend, _: &mut Window, cx: &mut Context<Self>) {
        self.request_send(cx);
    }

    fn replace_range(
        &mut self,
        range: Range<usize>,
        replacement: &str,
        marked_selection: Option<Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        if self.state.is_disabled() {
            return;
        }
        let Some(next) =
            replace_text_preserving_raw(self.state.draft(), range.clone(), replacement)
        else {
            return;
        };
        let selected_offsets = match marked_selection {
            Some(selected_range) if selected_range.start <= selected_range.end => Some((
                match utf16_offset_to_utf8(replacement, selected_range.start) {
                    Some(offset) => offset,
                    None => return,
                },
                match utf16_offset_to_utf8(replacement, selected_range.end) {
                    Some(offset) => offset,
                    None => return,
                },
            )),
            Some(_) => return,
            None => None,
        };
        self.state.set_draft(next);
        self.layout = None;
        self.painted_bounds = None;
        let replacement_end = range.start.saturating_add(replacement.len());
        if let Some((start, end)) = selected_offsets {
            self.selection = range.start + start..range.start + end;
            self.selection_reversed = false;
            self.marked_range = Some(range.start..replacement_end);
        } else {
            self.selection = replacement_end..replacement_end;
            self.selection_reversed = false;
            self.marked_range = None;
        }
        cx.notify();
    }

    fn replacement_range(&self, range: Option<Range<usize>>) -> Option<Range<usize>> {
        let draft = self.state.draft();
        let range = match range {
            Some(range) => utf16_range_to_utf8(draft, range),
            None => self
                .marked_range
                .clone()
                .or_else(|| Some(self.selection.clone())),
        }?;
        (range.start <= range.end
            && range.end <= draft.len()
            && draft.is_char_boundary(range.start)
            && draft.is_char_boundary(range.end))
        .then_some(range)
    }

    fn current_selection(&self) -> Range<usize> {
        self.selection.clone()
    }

    fn set_selection_for_point(
        &mut self,
        point: Point<Pixels>,
        extend: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(byte_index) = self.byte_index_for_global_point(point) else {
            return;
        };
        if extend {
            self.select_to(byte_index);
        } else {
            self.selection = byte_index..byte_index;
            self.selection_reversed = false;
        }
        self.marked_range = None;
        cx.notify();
    }

    pub(crate) fn bind_actions(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new("backspace", Backspace, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("delete", Delete, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("left", Left, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("right", Right, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("shift-left", SelectLeft, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new(
                "shift-right",
                SelectRight,
                Some(NATIVE_COMPOSER_KEY_CONTEXT),
            ),
            KeyBinding::new("cmd-a", SelectAll, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("ctrl-a", SelectAll, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("cmd-v", Paste, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("ctrl-v", Paste, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("cmd-c", Copy, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("ctrl-c", Copy, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("cmd-x", Cut, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("ctrl-x", Cut, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("home", Home, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("end", End, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("enter", RequestSend, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new(
                "shift-enter",
                InsertNewline,
                Some(NATIVE_COMPOSER_KEY_CONTEXT),
            ),
        ]);
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selection = offset..offset;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize) {
        let anchor = if self.selection_reversed {
            self.selection.end
        } else {
            self.selection.start
        };
        if offset < anchor {
            self.selection = offset..anchor;
            self.selection_reversed = true;
        } else {
            self.selection = anchor..offset;
            self.selection_reversed = false;
        }
    }

    fn move_left(&mut self, extend: bool, cx: &mut Context<Self>) {
        let target = if !extend && !self.selection.is_empty() {
            self.selection.start
        } else {
            previous_character_boundary(self.state.draft(), self.cursor_offset())
        };
        if extend {
            self.select_to(target);
            self.marked_range = None;
            cx.notify();
        } else {
            self.move_to(target, cx);
        }
    }

    fn move_right(&mut self, extend: bool, cx: &mut Context<Self>) {
        let target = if !extend && !self.selection.is_empty() {
            self.selection.end
        } else {
            next_character_boundary(self.state.draft(), self.cursor_offset())
        };
        if extend {
            self.select_to(target);
            self.marked_range = None;
            cx.notify();
        } else {
            self.move_to(target, cx);
        }
    }

    fn delete_backward(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_disabled() {
            return;
        }
        let range = self
            .marked_range
            .clone()
            .filter(|range| !range.is_empty())
            .or_else(|| (!self.selection.is_empty()).then_some(self.selection.clone()))
            .or_else(|| {
                let cursor = self.cursor_offset();
                (cursor > 0)
                    .then(|| previous_character_boundary(self.state.draft(), cursor)..cursor)
            });
        if let Some(range) = range {
            self.replace_range(range, "", None, cx);
        }
    }

    fn delete_forward(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_disabled() {
            return;
        }
        let range = self
            .marked_range
            .clone()
            .filter(|range| !range.is_empty())
            .or_else(|| (!self.selection.is_empty()).then_some(self.selection.clone()))
            .or_else(|| {
                let cursor = self.cursor_offset();
                (cursor < self.state.draft().len())
                    .then(|| cursor..next_character_boundary(self.state.draft(), cursor))
            });
        if let Some(range) = range {
            self.replace_range(range, "", None, cx);
        }
    }

    fn move_left_action(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if !self.state.is_disabled() {
            self.move_left(false, cx);
        }
    }

    fn move_right_action(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if !self.state.is_disabled() {
            self.move_right(false, cx);
        }
    }

    fn select_left_action(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        if !self.state.is_disabled() {
            self.move_left(true, cx);
        }
    }

    fn select_right_action(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        if !self.state.is_disabled() {
            self.move_right(true, cx);
        }
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        if !self.state.is_disabled() {
            self.selection = 0..self.state.draft().len();
            self.selection_reversed = false;
            self.marked_range = None;
            cx.notify();
        }
    }

    fn move_home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        if !self.state.is_disabled() {
            let cursor = self.cursor_offset();
            let line_start = self.state.draft()[..cursor]
                .rfind('\n')
                .map_or(0, |index| index + 1);
            self.move_to(line_start, cx);
        }
    }

    fn move_end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        if !self.state.is_disabled() {
            let cursor = self.cursor_offset();
            let line_end = self.state.draft()[cursor..]
                .find('\n')
                .map_or(self.state.draft().len(), |index| cursor + index);
            self.move_to(line_end, cx);
        }
    }

    fn insert_newline(&mut self, _: &InsertNewline, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_disabled() {
            return;
        }
        let Some(range) = self.replacement_range(None) else {
            return;
        };
        self.replace_range(range, "\n", None, cx);
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_disabled() {
            return;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let Some(range) = self.replacement_range(None) else {
            return;
        };
        self.replace_range(range, &text, None, cx);
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_disabled() || self.selection.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.state.draft()[self.selection.clone()].to_owned(),
        ));
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_disabled() || self.selection.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.state.draft()[self.selection.clone()].to_owned(),
        ));
        let range = self.selection.clone();
        self.replace_range(range, "", None, cx);
    }

    fn byte_index_for_global_point(&self, point: Point<Pixels>) -> Option<usize> {
        let bounds = self.painted_bounds.as_ref()?;
        let point = localize_painted_point(bounds, point)?;
        let layout = self.layout.as_ref()?;
        let index = match layout.index_for_position(point) {
            Ok(index) | Err(index) => index,
        };
        (index <= self.state.draft().len())
            .then(|| previous_char_boundary(self.state.draft(), index))
    }
}

impl Render for NativeComposer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        let style = InputStyle::resolve(theme, false);
        let draft = self.state.draft().to_owned();
        let styled_text = StyledText::new(SharedString::from(draft));
        self.painted_bounds = None;
        self.layout = Some(styled_text.layout().clone());

        let focus = self.focus_handle.clone();
        let mut editor = div()
            .id("artisan-native-composer-editor")
            .key_context(NATIVE_COMPOSER_KEY_CONTEXT)
            .w_full()
            .min_w(px(0.0))
            .min_h(style.height)
            .max_h(px(180.0))
            .px(style.horizontal_padding)
            .py(style.vertical_padding)
            .rounded(style.corner_radius)
            .border(style.border_width)
            .border_color(style.border)
            .bg(style.background)
            .text_color(style.foreground)
            .text_size(style.text_size)
            .line_height(style.line_height)
            .whitespace_normal()
            .overflow_y_scroll()
            .track_focus(&focus)
            .child(styled_text);

        if self.state.draft().is_empty() {
            editor = editor.child(
                div()
                    .absolute()
                    .top(style.vertical_padding)
                    .left(style.horizontal_padding)
                    .text_color(style.placeholder_foreground)
                    .text_size(style.text_size)
                    .line_height(style.line_height)
                    .whitespace_normal()
                    .debug_selector(|| NATIVE_COMPOSER_PLACEHOLDER_SELECTOR.to_string())
                    .child(NATIVE_COMPOSER_PLACEHOLDER),
            );
        }

        editor = editor.focus(move |focused| focused.border_color(style.focus_border));

        let mouse_entity = entity.clone();
        editor = editor.on_mouse_down(MouseButton::Left, move |event, _, cx| {
            let point = event.position;
            mouse_entity.update(cx, |composer, composer_cx| {
                composer.set_selection_for_point(point, event.modifiers.shift, composer_cx);
            });
        });

        editor = editor
            .on_action(cx.listener(Self::delete_backward))
            .on_action(cx.listener(Self::delete_forward))
            .on_action(cx.listener(Self::move_left_action))
            .on_action(cx.listener(Self::move_right_action))
            .on_action(cx.listener(Self::select_left_action))
            .on_action(cx.listener(Self::select_right_action))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::move_home))
            .on_action(cx.listener(Self::move_end))
            .on_action(cx.listener(Self::request_send_action))
            .on_action(cx.listener(Self::insert_newline))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut));

        let editor =
            NativeComposerInputElement::new(editor.into_any_element(), entity.clone(), focus);
        let send_ready = self.send_ready();
        let sending = self.is_submitting();
        self.send_focus_handle = self.send_focus_handle.clone().tab_stop(send_ready);
        let send_entity = entity.clone();
        let send = Button::new(
            NATIVE_COMPOSER_SEND_SELECTOR,
            self.send_focus_handle.clone(),
            theme,
            MotionPolicy::Reduced,
            ButtonVariant::Ghost,
            ButtonSize::Small,
            ButtonContent::text(if sending { "Sending…" } else { "Send" }),
        )
        .expect("the native composer send button configuration is valid")
        .focus_visibility(FocusVisibility::Visible)
        .disabled(!send_ready)
        .debug_selector(NATIVE_COMPOSER_SEND_SELECTOR)
        .on_activate(move |_, _, cx| {
            send_entity.update(cx, NativeComposer::request_send);
        });

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .child(editor)
            .child(send)
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
        self.child.paint(window, cx);
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
        self.marked_range = None;
        cx.notify();
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
        offset_layout_bounds(painted_bounds, start, end, layout.line_height())
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

fn valid_pixels(value: Pixels) -> bool {
    f32::from(value).is_finite()
}

fn valid_bounds(bounds: &Bounds<Pixels>) -> bool {
    !bounds.is_empty()
        && valid_pixels(bounds.origin.x)
        && valid_pixels(bounds.origin.y)
        && valid_pixels(bounds.size.width)
        && valid_pixels(bounds.size.height)
        && valid_pixels(bounds.right())
        && valid_pixels(bounds.bottom())
}

fn valid_point(point: Point<Pixels>) -> bool {
    valid_pixels(point.x) && valid_pixels(point.y)
}

fn localize_painted_point(
    painted_bounds: &Bounds<Pixels>,
    global_point: Point<Pixels>,
) -> Option<Point<Pixels>> {
    valid_bounds(painted_bounds)
        .then_some(())
        .and_then(|()| valid_point(global_point).then_some(()))?;
    painted_bounds.localize(&global_point)
}

fn offset_layout_bounds(
    element_bounds: &Bounds<Pixels>,
    local_start: Point<Pixels>,
    local_end: Point<Pixels>,
    line_height: Pixels,
) -> Option<Bounds<Pixels>> {
    if !valid_bounds(element_bounds)
        || !valid_point(local_start)
        || !valid_point(local_end)
        || !valid_pixels(line_height)
        || line_height <= Pixels::ZERO
    {
        return None;
    }

    let width = (local_end.x - local_start.x).max(px(1.0));
    let bounds = Bounds::new(
        point(
            element_bounds.left() + local_start.x,
            element_bounds.top() + local_start.y,
        ),
        size(width, line_height),
    );
    valid_bounds(&bounds).then_some(bounds)
}

fn replace_text_preserving_raw(
    draft: &str,
    range: Range<usize>,
    replacement: &str,
) -> Option<String> {
    if range.start > range.end
        || range.end > draft.len()
        || !draft.is_char_boundary(range.start)
        || !draft.is_char_boundary(range.end)
    {
        return None;
    }
    let mut next = draft.to_owned();
    next.replace_range(range, replacement);
    Some(next)
}

fn utf16_range_to_utf8(text: &str, range: Range<usize>) -> Option<Range<usize>> {
    if range.start > range.end {
        return None;
    }
    Some(utf16_offset_to_utf8(text, range.start)?..utf16_offset_to_utf8(text, range.end)?)
}

fn utf16_offset_to_utf8(text: &str, target: usize) -> Option<usize> {
    let mut utf16_offset = 0;
    if target == 0 {
        return Some(0);
    }
    for (byte_offset, character) in text.char_indices() {
        if utf16_offset == target {
            return Some(byte_offset);
        }
        utf16_offset = utf16_offset.checked_add(character.len_utf16())?;
        if utf16_offset == target {
            return Some(byte_offset + character.len_utf8());
        }
        if utf16_offset > target {
            return None;
        }
    }
    (utf16_offset == target).then_some(text.len())
}

fn utf8_range_to_utf16(text: &str, range: Range<usize>) -> Option<Range<usize>> {
    if range.start > range.end
        || range.end > text.len()
        || !text.is_char_boundary(range.start)
        || !text.is_char_boundary(range.end)
    {
        return None;
    }
    Some(utf8_offset_to_utf16(text, range.start)?..utf8_offset_to_utf16(text, range.end)?)
}

fn utf8_offset_to_utf16(text: &str, byte_offset: usize) -> Option<usize> {
    if byte_offset > text.len() || !text.is_char_boundary(byte_offset) {
        return None;
    }
    text.get(..byte_offset)?
        .chars()
        .try_fold(0usize, |offset, character| {
            offset.checked_add(character.len_utf16())
        })
}

fn previous_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn previous_character_boundary(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_character_boundary(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    text[offset..]
        .chars()
        .next()
        .map_or(text.len(), |character| offset + character.len_utf8())
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use super::{
        localize_painted_point, offset_layout_bounds, replace_text_preserving_raw,
        utf16_offset_to_utf8, utf16_range_to_utf8, utf8_offset_to_utf16, NativeComposer,
        NativeComposerEvent, NATIVE_COMPOSER_PLACEHOLDER, NATIVE_COMPOSER_PLACEHOLDER_SELECTOR,
        NATIVE_COMPOSER_SEND_SELECTOR,
    };
    use crate::composer::DraftDisposition;
    use artisan_ui::button::{Button, ButtonContent, ButtonSize, ButtonVariant, FocusVisibility};
    use artisan_ui::motion::MotionPolicy;
    use artisan_ui::theme::{ArtisanTheme, ThemeMode};
    use gpui::{
        point, px, size, Bounds, Entity, EntityInputHandler as _, KeyUpEvent, Keystroke, Modifiers,
        Subscription, TestAppContext, VisualTestContext,
    };

    fn set_draft(cx: &mut VisualTestContext, view: &Entity<NativeComposer>, draft: &str) {
        let draft = draft.to_owned();
        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_draft(draft);
                composer_cx.notify();
            });
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn draft_body_match_is_exact_and_does_not_change_editor_state(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        let draft = "  exact\n😀  ";
        set_draft(cx, &view, draft);

        cx.update(|_, app| {
            view.update(app, |composer, _| {
                composer.selection = 2..7;
                composer.selection_reversed = true;
                composer.marked_range = Some(2..7);
                let selection = composer.selection.clone();
                let selection_reversed = composer.selection_reversed;
                let marked_range = composer.marked_range.clone();
                let layout_present = composer.layout.is_some();
                let painted_bounds_present = composer.painted_bounds.is_some();
                let submitting = composer.is_submitting();
                let body =
                    artisan_domain::MessageBody::parse(draft.to_owned()).expect("draft body");
                let changed = artisan_domain::MessageBody::parse("  exact\n😀  !".to_owned())
                    .expect("changed body");

                assert!(composer.draft_matches_body(&body));
                assert!(!composer.draft_matches_body(&changed));
                assert_eq!(composer.selection, selection);
                assert_eq!(composer.selection_reversed, selection_reversed);
                assert_eq!(composer.marked_range, marked_range);
                assert_eq!(composer.layout.is_some(), layout_present);
                assert_eq!(composer.painted_bounds.is_some(), painted_bounds_present);
                assert_eq!(composer.is_submitting(), submitting);
            });
        });
    }

    fn bind_actions(cx: &mut VisualTestContext) {
        cx.update(|_, app| NativeComposer::bind_actions(app));
    }

    fn focus_editor(cx: &mut VisualTestContext, view: &Entity<NativeComposer>) {
        cx.update(|window, app| {
            let focus = view.read(app).focus_handle.clone();
            window.focus(&focus);
        });
        cx.run_until_parked();
    }

    fn observe_send_requests(
        cx: &mut VisualTestContext,
        view: &Entity<NativeComposer>,
    ) -> (Rc<Cell<usize>>, Subscription) {
        let requests = Rc::new(Cell::new(0));
        let observed_requests = requests.clone();
        let subscription = cx.update(|_, app| {
            app.subscribe(view, move |_, event: &NativeComposerEvent, _| {
                if *event == NativeComposerEvent::SendRequested {
                    observed_requests.set(observed_requests.get() + 1);
                }
            })
        });
        cx.run_until_parked();
        (requests, subscription)
    }

    fn send_button(focus: gpui::FocusHandle) -> Button {
        Button::new(
            NATIVE_COMPOSER_SEND_SELECTOR,
            focus,
            ArtisanTheme::for_mode(ThemeMode::Dark),
            MotionPolicy::Reduced,
            ButtonVariant::Ghost,
            ButtonSize::Small,
            ButtonContent::text("Send"),
        )
        .expect("the native composer send button configuration is valid")
        .focus_visibility(FocusVisibility::Visible)
    }

    #[gpui::test]
    fn plain_enter_requests_send_and_shift_enter_inserts_a_newline(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        set_draft(cx, &view, "draft");
        bind_actions(cx);
        focus_editor(cx, &view);
        let (requests, _subscription) = observe_send_requests(cx, &view);

        cx.simulate_keystrokes("enter");
        cx.update(|_, app| assert_eq!(view.read(app).draft(), "draft"));
        assert_eq!(requests.get(), 1);

        cx.simulate_keystrokes("shift-enter");
        cx.update(|_, app| assert_eq!(view.read(app).draft(), "draft\n"));
        assert_eq!(requests.get(), 1);
    }

    #[gpui::test]
    fn modified_and_unready_enter_requests_are_refused(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        bind_actions(cx);
        focus_editor(cx, &view);
        let (requests, _subscription) = observe_send_requests(cx, &view);

        set_draft(cx, &view, "draft");
        cx.simulate_keystrokes("ctrl-enter alt-enter cmd-enter");
        cx.update(|_, app| assert_eq!(view.read(app).draft(), "draft"));
        assert_eq!(requests.get(), 0);

        for draft in ["", " \t\n"] {
            set_draft(cx, &view, draft);
            cx.simulate_keystrokes("enter");
            assert_eq!(requests.get(), 0);
        }

        set_draft(cx, &view, "disabled");
        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_disabled(true, composer_cx);
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        assert_eq!(requests.get(), 0);

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_disabled(false, composer_cx);
                composer.set_draft("in flight");
                assert!(composer.begin_submission().is_ok());
                composer_cx.notify();
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        assert_eq!(requests.get(), 0);
        cx.update(|_, app| assert!(view.read(app).is_submitting()));
    }

    #[gpui::test]
    fn marked_ime_text_refuses_enter_until_composition_is_unmarked(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        bind_actions(cx);
        focus_editor(cx, &view);
        let (requests, _subscription) = observe_send_requests(cx, &view);

        cx.update(|window, app| {
            view.update(app, |composer, composer_cx| {
                composer.replace_and_mark_text_in_range(
                    Some(0..0),
                    "preedit",
                    Some(0..7),
                    window,
                    composer_cx,
                );
            });
        });
        cx.run_until_parked();
        cx.update(|_, app| {
            let composer = view.read(app);
            assert_eq!(composer.draft(), "preedit");
            assert!(composer.marked_range.is_some());
            assert!(!composer.send_ready());
        });

        cx.simulate_keystrokes("enter");
        assert_eq!(requests.get(), 0);

        cx.update(|window, app| {
            view.update(app, |composer, composer_cx| {
                composer.unmark_text(window, composer_cx);
            });
        });
        cx.run_until_parked();
        cx.update(|_, app| assert!(view.read(app).send_ready()));

        cx.simulate_keystrokes("enter");
        assert_eq!(requests.get(), 1);
    }

    #[gpui::test]
    fn send_has_stable_identity_focusability_visible_focus_and_local_tab_order(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        set_draft(cx, &view, "draft");

        assert!(
            cx.debug_bounds(NATIVE_COMPOSER_SEND_SELECTOR).is_some(),
            "the native Send button must retain its stable selector"
        );
        cx.update(|_, app| {
            let composer = view.read(app);
            assert_eq!(composer.focus_handle.tab_index, 0);
            assert_eq!(composer.send_focus_handle.tab_index, 1);
            assert!(composer.focus_handle.tab_stop);
            assert!(composer.send_focus_handle.tab_stop);
        });

        let ring_visible = cx.update(|window, app| {
            let focus = view.read(app).send_focus_handle.clone();
            window.focus(&focus);
            send_button(focus).focus_ring_visible(window)
        });
        assert!(ring_visible);

        cx.update(|window, app| {
            let composer = view.read(app);
            let editor_focus = composer.focus_handle.clone();
            let send_focus = composer.send_focus_handle.clone();
            window.focus(&editor_focus);
            window.focus_next();
            assert!(send_focus.is_focused(window));
            window.focus_prev();
            assert!(editor_focus.is_focused(window));
        });
    }

    #[gpui::test]
    fn pointer_enter_and_space_activate_through_the_same_send_path(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        set_draft(cx, &view, "draft");
        let (requests, _subscription) = observe_send_requests(cx, &view);
        let send_bounds = cx
            .debug_bounds(NATIVE_COMPOSER_SEND_SELECTOR)
            .expect("the native Send button must paint");

        cx.simulate_click(send_bounds.center(), Modifiers::none());
        cx.update(|window, app| {
            assert!(
                view.read(app).send_focus_handle.is_focused(window),
                "a pointer activation must focus the shared Send button"
            );
        });
        for key in ["enter", "space"] {
            cx.simulate_event(KeyUpEvent {
                keystroke: Keystroke::parse(key).expect("known keyboard activation key"),
            });
        }

        assert_eq!(requests.get(), 3);
    }

    #[gpui::test]
    fn repeated_activation_is_blocked_by_the_composer_single_flight(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        set_draft(cx, &view, "draft");
        let (requests, _subscription) = observe_send_requests(cx, &view);
        let send_bounds = cx
            .debug_bounds(NATIVE_COMPOSER_SEND_SELECTOR)
            .expect("the native Send button must paint");

        cx.simulate_click(send_bounds.center(), Modifiers::none());
        assert_eq!(requests.get(), 1);

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                assert!(composer.begin_submission().is_ok());
                composer_cx.notify();
            });
        });
        cx.run_until_parked();
        cx.update(|_, app| assert!(!view.read(app).send_focus_handle.tab_stop));

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.request_send(composer_cx);
            });
        });
        cx.simulate_click(send_bounds.center(), Modifiers::none());
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse("enter").expect("known keyboard activation key"),
        });
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse("space").expect("known keyboard activation key"),
        });

        assert_eq!(requests.get(), 1);
        cx.update(|_, app| assert!(view.read(app).is_submitting()));
    }

    #[test]
    fn painted_geometry_translates_global_points_and_layout_bounds() {
        let painted_bounds = Bounds::new(point(px(120.0), px(48.0)), size(px(300.0), px(96.0)));
        assert_eq!(
            localize_painted_point(&painted_bounds, point(px(137.0), px(79.0))),
            Some(point(px(17.0), px(31.0)))
        );
        assert!(localize_painted_point(&painted_bounds, point(px(119.0), px(79.0))).is_none());
        assert!(localize_painted_point(&painted_bounds, point(px(137.0), px(145.0))).is_none());

        let text_bounds = offset_layout_bounds(
            &painted_bounds,
            point(px(8.0), px(14.0)),
            point(px(88.0), px(14.0)),
            px(18.0),
        )
        .expect("valid painted geometry");
        assert_eq!(text_bounds.origin, point(px(128.0), px(62.0)));
        assert_eq!(text_bounds.size, size(px(80.0), px(18.0)));
    }

    #[test]
    fn invalid_painted_geometry_fails_closed() {
        let empty_bounds = Bounds::new(point(px(120.0), px(48.0)), size(px(0.0), px(96.0)));
        assert!(localize_painted_point(&empty_bounds, point(px(120.0), px(48.0))).is_none());
        assert!(
            offset_layout_bounds(
                &empty_bounds,
                point(px(0.0), px(0.0)),
                point(px(4.0), px(0.0)),
                px(18.0),
            )
            .is_none()
        );
    }

    #[test]
    fn raw_editing_preserves_whitespace_newlines_and_unicode() {
        let original = "\u{200b}  café\r\n\t第二行  ";
        let range = utf16_range_to_utf8(original, 3..7).expect("valid range");
        let edited = replace_text_preserving_raw(original, range, "é").expect("edit");
        assert_eq!(edited, "\u{200b}  é\r\n\t第二行  ");
        assert_eq!(edited.as_bytes(), "\u{200b}  é\r\n\t第二行  ".as_bytes());
    }

    #[test]
    fn utf16_and_utf8_offsets_reject_surrogate_splits_and_round_trip() {
        let text = "a😀b";
        assert_eq!(utf16_offset_to_utf8(text, 1), Some(1));
        assert_eq!(utf16_offset_to_utf8(text, 2), None);
        assert_eq!(utf16_offset_to_utf8(text, 3), Some(5));
        assert_eq!(utf8_offset_to_utf16(text, 5), Some(3));
        assert!(utf16_range_to_utf8(text, 2..3).is_none());
        assert!(replace_text_preserving_raw(text, 2..2, "x").is_none());
    }

    #[gpui::test]
    fn empty_composer_paints_exact_placeholder_selector_and_phrase(cx: &mut TestAppContext) {
        assert_eq!(NATIVE_COMPOSER_PLACEHOLDER, "Do anything");
        assert_eq!(
            NATIVE_COMPOSER_PLACEHOLDER_SELECTOR,
            "artisan-native-composer-placeholder"
        );

        let (_view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        assert!(
            cx.debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
                .is_some()
        );
    }

    #[gpui::test]
    fn nonempty_and_whitespace_drafts_hide_placeholder_without_rewriting(cx: &mut TestAppContext) {
        for draft in ["message", " \t\n"] {
            let (view, window_cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));

            window_cx.update(|_, app| {
                view.update(app, |composer, composer_cx| {
                    composer.set_draft(draft);
                    composer_cx.notify();
                });
            });
            window_cx.run_until_parked();

            assert!(
                window_cx
                    .debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
                    .is_none()
            );
            window_cx.update(|_, app| assert_eq!(view.read(app).draft(), draft));
        }
    }

    #[gpui::test]
    fn clearing_a_nonempty_draft_restores_placeholder(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_draft("message");
                composer_cx.notify();
            });
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
                .is_none()
        );

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_draft("");
                composer_cx.notify();
            });
        });
        cx.run_until_parked();

        cx.update(|_, app| assert_eq!(view.read(app).draft(), ""));
        assert!(
            cx.debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
                .is_some()
        );
    }

    #[gpui::test]
    fn accepted_submission_clear_restores_placeholder(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_draft("accepted");
                composer_cx.notify();
            });
        });
        cx.run_until_parked();

        let token = cx.update(|_, app| {
            view.update(app, |composer, _| {
                composer
                    .begin_submission()
                    .expect("nonempty draft begins a submission")
                    .1
            })
        });
        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.finish_submission(token, DraftDisposition::Accepted, composer_cx);
            });
        });
        cx.run_until_parked();

        cx.update(|_, app| {
            let composer = view.read(app);
            assert_eq!(composer.draft(), "");
            assert!(!composer.is_submitting());
        });
        assert!(
            cx.debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
                .is_some()
        );
    }

    #[gpui::test]
    fn disabled_and_submitting_empty_states_keep_one_unchanged_placeholder(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_disabled(true, composer_cx);
            });
        });
        cx.run_until_parked();
        cx.update(|_, app| {
            let composer = view.read(app);
            assert_eq!(composer.draft(), "");
            assert!(!composer.is_submitting());
        });
        assert!(
            cx.debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
                .is_some()
        );

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_disabled(false, composer_cx);
                composer.set_draft("in flight");
                composer_cx.notify();
            });
        });
        cx.run_until_parked();
        let token = cx.update(|_, app| {
            view.update(app, |composer, _| {
                composer
                    .begin_submission()
                    .expect("nonempty draft begins a submission")
                    .1
            })
        });

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_draft("");
                composer_cx.notify();
            });
        });
        cx.run_until_parked();
        cx.update(|_, app| {
            let composer = view.read(app);
            assert_eq!(composer.draft(), "");
            assert!(composer.is_submitting());
        });
        assert!(
            cx.debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
                .is_some()
        );

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.finish_submission(token, DraftDisposition::Accepted, composer_cx);
            });
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
                .is_some()
        );
    }

    #[gpui::test]
    fn clicking_painted_placeholder_uses_editor_selection_surface(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        let placeholder = cx
            .debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
            .expect("empty composer paints the placeholder");

        cx.update(|window, app| {
            let focus = view.read(app).focus_handle.clone();
            window.focus(&focus);
            view.update(app, |composer, _| {
                composer.selection_reversed = true;
            });
        });

        cx.simulate_click(placeholder.center(), Modifiers::none());
        cx.run_until_parked();
        cx.update(|window, app| {
            let composer = view.read(app);
            assert!(composer.focus_handle.is_focused(window));
            assert_eq!(composer.selection, 0..0);
            assert!(!composer.selection_reversed);
        });
    }
}
