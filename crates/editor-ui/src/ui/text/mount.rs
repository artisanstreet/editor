use std::cell::RefCell;
use std::rc::Rc;

use aqua_core::{ImeEvent, InputEvent, Key, Modifiers, NamedKey};
use aqua_text::input_service::{InputChange, TextInputService};
use aqua_ui::element::{Box as UiBox, Element, Scroll, Text};
use aqua_ui::input::{
    EventPhase, InputKind, InputResponse, RoutedEvent, RoutedInput, TextEdit, TextPointerPhase,
};
use aqua_ui::style::{Axis, Style, pct, px};
use aqua_ui::tree::UiCommand;

use super::editor::{CommandOutcome, EditOutcome, Editor, Mode, PasteOutcome, utf8_range_to_utf16};
use super::metrics::EditorMetrics;
use crate::ui::scroll_edge::EdgeBands;
use crate::ui::theme::ArtisanTheme;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorEvent {
    Changed,
    PresentationChanged,
    SelectionChanged {
        range: std::ops::Range<usize>,
        reversed: bool,
        caret: Option<usize>,
    },
    Submitted,
    List(super::editor::ListKey),
    PastedFiles {
        images: Vec<aqua_ui::input::PasteImage>,
        paths: Vec<std::path::PathBuf>,
    },
}

#[derive(Clone)]
pub struct EditorController {
    state: Rc<RefCell<Editor>>,
    event: Rc<dyn Fn(EditorEvent)>,
    text_key: Rc<RefCell<Option<aqua_ui::element::Key>>>,
    theme: Rc<RefCell<Option<ArtisanTheme>>>,
    native_follow_caret: Rc<std::cell::Cell<bool>>,
}

impl std::fmt::Debug for EditorController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditorController").finish_non_exhaustive()
    }
}

#[derive(Clone, Copy)]
struct Framing {
    mode: Mode,
    bare: bool,
    inset: bool,
}

struct NativeEditorModel {
    state: std::rc::Weak<RefCell<Editor>>,
    event: Rc<dyn Fn(EditorEvent)>,
    follow_caret: Rc<std::cell::Cell<bool>>,
}

impl aqua_ui::tree::NativeTextModel for NativeEditorModel {
    fn paint_revision(&self) -> Option<u64> {
        let state = self.state.upgrade()?;
        let state = state.borrow();
        (!state.is_disabled()).then(|| state.paint_revision())
    }

    fn matches_text(&self, text: &str) -> bool {
        self.state
            .upgrade()
            .is_some_and(|state| state.borrow().text() == text)
    }

    fn selected_bytes(&self) -> Option<std::ops::Range<usize>> {
        let state = self.state.upgrade()?;
        let state = state.borrow();
        (!state.is_disabled()).then(|| state.selection())
    }

    fn replace_utf16(&self, text: &str, selected: Option<std::ops::Range<usize>>) {
        self.edit(|state| input_change(state.replace_utf16_range(None, text, selected)));
    }

    fn replace_all(&self, text: &str) {
        self.edit(|state| {
            let everything = 0..state.text().len();
            state.replace(everything, text)
        });
    }
}

impl NativeEditorModel {
    fn edit(&self, apply: impl FnOnce(&mut Editor) -> EditOutcome) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let mut state = state.borrow_mut();
        let previous_selection = (state.selection(), state.selection_reversed(), state.caret());
        let previous_limit = state.limit_notice();
        let outcome = apply(&mut state);
        self.follow_caret.set(true);
        let next_selection = (state.selection(), state.selection_reversed(), state.caret());
        let next_limit = state.limit_notice();
        drop(state);
        if outcome == EditOutcome::Changed {
            (self.event)(EditorEvent::Changed);
        }
        if previous_limit != next_limit || outcome == EditOutcome::Unchanged {
            (self.event)(EditorEvent::PresentationChanged);
        }
        if previous_selection != next_selection {
            (self.event)(EditorEvent::SelectionChanged {
                range: next_selection.0,
                reversed: next_selection.1,
                caret: next_selection.2,
            });
        }
    }
}

impl EditorController {
    pub fn new(editor: Editor, event: impl Fn(EditorEvent) + 'static) -> Self {
        Self::from_shared(Rc::new(RefCell::new(editor)), event)
    }

    pub fn from_shared(state: Rc<RefCell<Editor>>, event: impl Fn(EditorEvent) + 'static) -> Self {
        Self {
            state,
            event: Rc::new(event),
            text_key: Rc::new(RefCell::new(None)),
            theme: Rc::new(RefCell::new(None)),
            native_follow_caret: Rc::new(std::cell::Cell::new(false)),
        }
    }

    pub fn text_key(&self) -> Option<aqua_ui::element::Key> {
        self.text_key.borrow().clone()
    }

    pub fn sync(&self, tree: &mut aqua_ui::tree::UiTree, node: aqua_core::NodeId) -> bool {
        if !tree.get(node).is_some_and(|record| {
            matches!(
                &record.kind, aqua_ui::tree::NodeKind::Text(text) if text.external_text_input
            )
        }) {
            return false;
        }
        let state = self.state.borrow();
        let range = state.selection();
        let selection = if state.selection_reversed() {
            aqua_text::Selection::new(range.end, range.start)
        } else {
            aqua_text::Selection::new(range.start, range.end)
        };
        let style = tree.get(node).and_then(|record| match &record.kind {
            aqua_ui::tree::NodeKind::Text(text) => Some(text.effective_text_style()),
            aqua_ui::tree::NodeKind::Box(_)
            | aqua_ui::tree::NodeKind::Media(_)
            | aqua_ui::tree::NodeKind::Scroll(_)
            | aqua_ui::tree::NodeKind::Canvas(_)
            | aqua_ui::tree::NodeKind::Viewport3D(_) => None,
        });
        let presentation = self
            .theme
            .borrow()
            .as_ref()
            .zip(style.as_ref())
            .and_then(|(theme, style)| token_presentation(&state, theme, style));
        let changed = tree
            .edit_editor(node, |editor| {
                editor.set_external_projection(state.text(), selection, state.marked_range())
            })
            .unwrap_or(false);
        if let Some((spans, paints)) = presentation {
            tree.edit_text(node, |text| {
                text.spans = spans;
                text.inline_glyphs.clear();
                text.inline_paints = paints;
            });
        }
        if self.native_follow_caret.replace(false) {
            tree.push_command(UiCommand::FollowTextCaret { node });
        }
        changed
    }

    pub fn set_disabled(
        &self,
        tree: &mut aqua_ui::tree::UiTree,
        node: aqua_core::NodeId,
        disabled: bool,
    ) -> bool {
        let Some(record) = tree.get(node) else {
            return false;
        };
        if !matches!(&record.kind, aqua_ui::tree::NodeKind::Text(text) if text.external_text_input)
        {
            return false;
        }
        let mut semantics = record.semantics.clone();
        let mut paint = record.paint().clone();
        semantics.disabled = disabled;
        paint.opacity = disabled.then_some(crate::ui::theme::Opacity::Disabled.alpha());
        let (text, selection, marked, changed) = {
            let mut state = self.state.borrow_mut();
            let changed = state.is_disabled() != disabled;
            state.set_disabled(disabled);
            let range = state.selection();
            let selection = if state.selection_reversed() {
                aqua_text::Selection::new(range.end, range.start)
            } else {
                aqua_text::Selection::new(range.start, range.end)
            };
            (
                state.text().to_owned(),
                selection,
                state.marked_range(),
                changed,
            )
        };
        tree.edit_editor(node, |editor| {
            editor.set_external_projection(&text, selection, marked)
        });
        tree.set_semantics(node, semantics);
        tree.set_paint(node, paint);
        if let Some(record) = tree.get_mut(node) {
            record.focusable = !disabled;
        }
        if changed {
            (self.event)(EditorEvent::PresentationChanged);
        }
        true
    }

    pub fn set_suggestion(&self, suggestion: Option<String>) {
        let mut state = self.state.borrow_mut();
        let before = state.shown_suggestion().map(str::to_owned);
        state.set_suggestion(suggestion);
        let changed = before.as_deref() != state.shown_suggestion();
        drop(state);
        if changed {
            (self.event)(EditorEvent::PresentationChanged);
        }
    }

    pub fn set_tokenizer(&self, tokenizer: Option<super::editor::StyledTokenizer>) {
        self.state.borrow_mut().set_styled_tokenizer(tokenizer);
        (self.event)(EditorEvent::PresentationChanged);
    }

    pub fn state(&self) -> Rc<RefCell<Editor>> {
        Rc::clone(&self.state)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one match handling every routed text input event against the mounted state"
    )]
    fn handle(
        &self,
        event: &RoutedEvent,
        theme: &ArtisanTheme,
        text_style: &aqua_text::TextStyle,
    ) -> InputResponse {
        let mut state = self.state.borrow_mut();
        let previous_limit = state.limit_notice();
        let previous_selection = (state.selection(), state.selection_reversed(), state.caret());
        let mut pending_event = None;
        let outcome = match &event.input {
            RoutedInput::Native(InputEvent::TextInput { text }) => Some(CommandOutcome::Edit(
                input_change(state.replace_utf16_range(None, text, None)),
            )),
            RoutedInput::Native(InputEvent::Ime(ime)) => match ime {
                ImeEvent::Start => Some(CommandOutcome::Ignored),
                ImeEvent::Update { marked, cursor } => {
                    let selected = cursor
                        .and_then(|(start, end)| utf8_range_to_utf16(marked, start..end))
                        .or_else(|| utf8_range_to_utf16(marked, marked.len()..marked.len()));
                    Some(CommandOutcome::Edit(input_change(
                        state.replace_utf16_range(None, marked, selected),
                    )))
                }
                ImeEvent::Commit { text } => Some(CommandOutcome::Edit(input_change(
                    state.replace_utf16_range(None, text, None),
                ))),
                ImeEvent::Cancel => Some(CommandOutcome::Edit(state.cancel_composition())),
            },
            RoutedInput::Native(InputEvent::ImeReplace { start, end, text }) => {
                Some(CommandOutcome::Edit(state.replace_utf16(
                    usize::try_from(*start).unwrap_or(usize::MAX)
                        ..usize::try_from(*end).unwrap_or(usize::MAX),
                    text,
                )))
            }
            RoutedInput::Native(InputEvent::ImeSelect {
                start,
                end,
                reversed,
            }) => {
                state.select_utf16(
                    usize::try_from(*start).unwrap_or(usize::MAX)
                        ..usize::try_from(*end).unwrap_or(usize::MAX),
                    *reversed,
                );
                Some(CommandOutcome::Ignored)
            }
            RoutedInput::Native(InputEvent::KeyDown(key)) => {
                let shift = key.modifiers.contains(Modifiers::SHIFT);
                let word = key.modifiers.contains(Modifiers::CONTROL)
                    || key.modifiers.contains(Modifiers::ALT);
                let command = key.modifiers.contains(Modifiers::CONTROL)
                    || key.modifiers.contains(Modifiers::LOGO);
                match key.key {
                    Key::Named(NamedKey::Enter) => Some(state.enter(shift)),
                    Key::Named(NamedKey::Tab) if !shift => Some(state.tab()),
                    Key::Named(NamedKey::Escape) => Some(state.dismiss()),
                    Key::Named(NamedKey::Left | NamedKey::Right)
                        if word || (!shift && !state.selection().is_empty()) =>
                    {
                        state.move_horizontal(
                            matches!(key.key, Key::Named(NamedKey::Right)),
                            word,
                            shift,
                        );
                        Some(CommandOutcome::Ignored)
                    }
                    Key::Named(NamedKey::Backspace) => {
                        Some(CommandOutcome::Edit(state.delete_toward(false, word)))
                    }
                    Key::Named(NamedKey::Delete) => {
                        Some(CommandOutcome::Edit(state.delete_toward(true, word)))
                    }
                    Key::Named(NamedKey::Up | NamedKey::Down)
                        if key.modifiers.contains(Modifiers::LOGO) =>
                    {
                        state.move_document_edge(
                            matches!(key.key, Key::Named(NamedKey::Down)),
                            shift,
                        );
                        Some(CommandOutcome::Ignored)
                    }
                    Key::Named(NamedKey::Up | NamedKey::Down) if !shift && !word => state
                        .list_vertical(matches!(key.key, Key::Named(NamedKey::Up)))
                        .map(CommandOutcome::List),
                    Key::Named(NamedKey::Home) if command => {
                        state.move_document_edge(false, shift);
                        Some(CommandOutcome::Ignored)
                    }
                    Key::Named(NamedKey::End) if command => {
                        state.move_document_edge(true, shift);
                        Some(CommandOutcome::Ignored)
                    }
                    Key::Char('a' | 'A') if command => {
                        state.select_all();
                        Some(CommandOutcome::Ignored)
                    }
                    Key::Char('z' | 'Z') if command => {
                        let changed = if shift { state.redo() } else { state.undo() };
                        Some(CommandOutcome::Edit(if changed {
                            EditOutcome::Changed
                        } else {
                            EditOutcome::Unchanged
                        }))
                    }
                    Key::Char('y' | 'Y') if command => {
                        let changed = state.redo();
                        Some(CommandOutcome::Edit(if changed {
                            EditOutcome::Changed
                        } else {
                            EditOutcome::Unchanged
                        }))
                    }
                    Key::Char(_) | Key::Named(_) => None,
                }
            }
            RoutedInput::TextVertical { up, extend, target } => {
                state.move_vertical(*up, *extend, Some(*target));
                Some(CommandOutcome::Ignored)
            }
            RoutedInput::TextMotion { extend, target, .. } => {
                state.move_caret(*target, *extend);
                Some(CommandOutcome::Ignored)
            }
            RoutedInput::TextPointer { byte, phase, .. } => {
                match phase {
                    TextPointerPhase::Down { click_count, shift } => {
                        state.pointer_down(*byte, *shift, *click_count);
                    }
                    TextPointerPhase::Drag => state.pointer_drag(*byte),
                    TextPointerPhase::Up => state.pointer_up(),
                }
                Some(CommandOutcome::Ignored)
            }
            RoutedInput::ClipboardCut => {
                let changed = state.cut().is_some();
                Some(CommandOutcome::Edit(if changed {
                    EditOutcome::Changed
                } else {
                    EditOutcome::Unchanged
                }))
            }
            RoutedInput::ClipboardPaste(item) => match state.paste(item) {
                PasteOutcome::Edit(edit) => Some(CommandOutcome::Edit(edit)),
                PasteOutcome::Files { images, paths } => {
                    pending_event = Some(EditorEvent::PastedFiles { images, paths });
                    Some(CommandOutcome::Ignored)
                }
            },
            RoutedInput::Native(_)
            | RoutedInput::PointerEnter(_)
            | RoutedInput::PointerLeave(_)
            | RoutedInput::TextContext { .. }
            | RoutedInput::TextRangeActivated { .. }
            | RoutedInput::FilesDropped { .. }
            | RoutedInput::DragEnter { .. }
            | RoutedInput::DragOver { .. }
            | RoutedInput::DragLeave { .. }
            | RoutedInput::Drop { .. }
            | RoutedInput::DragEnd { .. } => None,
        };
        let Some(outcome) = outcome else {
            return InputResponse::default();
        };
        if outcome == CommandOutcome::Propagate {
            return InputResponse::default();
        }
        let follow = matches!(
            &outcome,
            CommandOutcome::Edit(EditOutcome::Changed | EditOutcome::Unchanged)
        ) || matches!(
            &event.input,
            RoutedInput::TextVertical { .. }
                | RoutedInput::TextMotion { .. }
                | RoutedInput::Native(InputEvent::KeyDown(aqua_core::KeyEvent {
                    key: Key::Named(
                        NamedKey::Left | NamedKey::Right | NamedKey::Home | NamedKey::End
                    ),
                    ..
                }))
        );
        if pending_event.is_none() {
            pending_event = match outcome {
                CommandOutcome::Edit(EditOutcome::Changed) => Some(EditorEvent::Changed),
                CommandOutcome::Submitted => Some(EditorEvent::Submitted),
                CommandOutcome::List(key) => Some(EditorEvent::List(key)),
                CommandOutcome::Edit(_) | CommandOutcome::Propagate | CommandOutcome::Ignored => {
                    None
                }
            };
        }
        let next_limit = state.limit_notice();
        let next_selection = (state.selection(), state.selection_reversed(), state.caret());
        drop(state);
        if let Some(pending_event) = pending_event {
            (self.event)(pending_event);
        }
        if previous_limit != next_limit {
            (self.event)(EditorEvent::PresentationChanged);
        }
        if previous_selection != next_selection {
            (self.event)(EditorEvent::SelectionChanged {
                range: next_selection.0,
                reversed: next_selection.1,
                caret: next_selection.2,
            });
        }
        let state = self.state.borrow();
        let range = state.selection();
        let selection = if state.selection_reversed() {
            aqua_text::Selection::new(range.end, range.start)
        } else {
            aqua_text::Selection::new(range.start, range.end)
        };
        let text = state.text().to_owned();
        let marked = state.marked_range();
        let presentation = token_presentation(&state, theme, text_style);
        drop(state);
        let mut commands = vec![UiCommand::EditText {
            node: event.current_target,
            revision: None,
            edit: TextEdit::SetExternalProjection {
                text,
                selection,
                marked,
            },
        }];
        if follow {
            commands.push(UiCommand::FollowTextCaret {
                node: event.current_target,
            });
        }
        if let Some((spans, paints)) = presentation {
            commands.push(UiCommand::SetTextPresentation {
                node: event.current_target,
                spans,
                glyphs: Vec::new(),
            });
            commands.push(UiCommand::SetTextInlinePaint {
                node: event.current_target,
                paints,
            });
        }
        InputResponse {
            commands,
            ..InputResponse::handled()
        }
    }

    pub fn element(&self, mode: Mode, theme: &ArtisanTheme, placeholder: &str) -> Element {
        wrap_prompt_scroll(
            self.element_with_chrome(mode, theme, placeholder, false, false),
            mode,
            theme,
            false,
            false,
            self.state.borrow().shown_suggestion().map(str::to_owned),
            None,
        )
    }

    pub fn element_with_notice_keyed(
        &self,
        key: impl Into<aqua_ui::element::Key>,
        mode: Mode,
        theme: &ArtisanTheme,
        placeholder: &str,
        bare: bool,
        inset: bool,
    ) -> Element {
        let framing = Framing { mode, bare, inset };
        self.keyed(key.into(), theme, placeholder, placeholder, framing, None)
    }

    pub fn prompt_with_scroll_edges(
        &self,
        key: impl Into<aqua_ui::element::Key>,
        theme: &ArtisanTheme,
        name: &str,
        placeholder: &str,
        edges: &EdgeBands,
    ) -> Element {
        let framing = Framing {
            mode: Mode::Prompt,
            bare: false,
            inset: false,
        };
        self.keyed(key.into(), theme, name, placeholder, framing, Some(edges))
    }

    fn keyed(
        &self,
        key: aqua_ui::element::Key,
        theme: &ArtisanTheme,
        name: &str,
        placeholder: &str,
        Framing { mode, bare, inset }: Framing,
        edges: Option<&EdgeBands>,
    ) -> Element {
        *self.text_key.borrow_mut() = Some(key.clone());
        let show_notice = self.state.borrow().limit_notice();
        let leaf = self
            .named_leaf(mode, theme, name, placeholder, bare, inset)
            .with_key(key);
        let mut root = UiBox::new()
            .style(Style::new().axis(Axis::Column).min_width(px(0.0)))
            .child(wrap_prompt_scroll(
                leaf,
                mode,
                theme,
                bare,
                inset,
                self.state.borrow().shown_suggestion().map(str::to_owned),
                edges,
            ));
        if show_notice {
            root = root.child(
                crate::ui::kit::ProblemNotice::new(
                    "prompt-limit-problem",
                    *theme,
                    &editor_core::Problem::new(
                        "Text is limited to 64 KiB. Attach a file for larger content.",
                    ),
                )
                .build(),
            );
        }
        root.build()
    }

    pub fn element_with_chrome(
        &self,
        mode: Mode,
        theme: &ArtisanTheme,
        placeholder: &str,
        bare: bool,
        inset: bool,
    ) -> Element {
        self.named_leaf(mode, theme, placeholder, placeholder, bare, inset)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one named text leaf: mode, placeholder, bare and inset styling"
    )]
    fn named_leaf(
        &self,
        mode: Mode,
        theme: &ArtisanTheme,
        name: &str,
        placeholder: &str,
        bare: bool,
        inset: bool,
    ) -> Element {
        let metrics = EditorMetrics::resolve(theme, mode, bare, inset);
        *self.theme.borrow_mut() = Some(*theme);
        let state = self.state.borrow();
        let text = state.text().to_owned();
        let range = state.selection();
        let selection = if state.selection_reversed() {
            aqua_text::Selection::new(range.end, range.start)
        } else {
            aqua_text::Selection::new(range.start, range.end)
        };
        let marked = state.marked_range();
        let disabled = state.is_disabled();
        let has_suggestion = state.shown_suggestion().is_some();
        let presentation = token_presentation(&state, theme, &metrics.text_style);
        drop(state);
        let mut style = Style::new()
            .width(pct(100.0))
            .min_width(px(0.0))
            .min_height(px(metrics.min_height()))
            .padding_insets(aqua_core::Insets::hv(
                metrics.padding_x + metrics.border_width,
                metrics.padding_y + metrics.border_width,
            ));
        if mode != Mode::Prompt && !bare {
            style = style
                .background(metrics.background)
                .radius(px(metrics.radius))
                .border_of(metrics.border_width, metrics.border)
                .focus_ring(metrics.border_width, metrics.focus_border);
        }
        if inset && mode != Mode::Prompt && !bare {
            let gradient = crate::ui::glass::glass_control_fill(theme);
            style = style
                .gpui_background(aqua::GpuiBackground::LinearGradient {
                    angle: gradient.angle_degrees,
                    color_space: aqua::GpuiGradientColorSpace::Oklab,
                    stops: [
                        aqua::GpuiGradientStop {
                            percentage: 0.0,
                            color: gradient.start.to_reference_paint(),
                        },
                        aqua::GpuiGradientStop {
                            percentage: gradient.end_position,
                            color: gradient.end.to_reference_paint(),
                        },
                    ],
                })
                .shadows(
                    crate::ui::glass::card_shadows(theme)
                        .into_iter()
                        .map(aqua::ShadowLayer::from)
                        .collect(),
                );
        }
        if disabled {
            style = style.opacity(metrics.disabled_opacity);
        }
        if mode == Mode::Prompt {
            style = style.flex_shrink(0.0);
        }
        let mut leaf = Text::new(text);
        if mode == Mode::Prompt {
            leaf = leaf.viewport_scroll_max(0.4, metrics.min_height() * 2.0);
        }
        if let Some((spans, paints)) = presentation {
            leaf = leaf.spans(spans).inline_paints(paints);
        }
        let mut element = leaf
            .editable(true)
            .external_text_input(true)
            .native_text_model(Rc::new(NativeEditorModel {
                state: Rc::downgrade(&self.state),
                event: Rc::clone(&self.event),
                follow_caret: Rc::clone(&self.native_follow_caret),
            }))
            .initial_external_selection(selection, marked)
            .multiline(mode != Mode::SingleLine)
            .text_style(metrics.text_style.clone())
            .style(style)
            .color(metrics.foreground)
            .gpui_source_color(theme.colors.text.to_reference_text_color())
            .text_appearance(metrics.text_appearance())
            .gpui_text_appearance(EditorMetrics::gpui_text_appearance(theme))
            .selection_auto_scroll(false)
            .placeholder(if has_suggestion { "" } else { placeholder })
            .placeholder_color(metrics.placeholder)
            .gpui_source_placeholder_color(if mode == Mode::Prompt {
                theme.colors.text_muted.to_reference_text_color()
            } else {
                theme.colors.text_faint.to_reference_text_color()
            })
            .build()
            .focusable(!disabled)
            .disabled(disabled);
        if !name.is_empty() {
            element = element.aria_label(name);
        }
        if name != placeholder && !placeholder.is_empty() && !has_suggestion {
            element = element.aria_description(placeholder);
        }
        for kind in [
            InputKind::KeyDown,
            InputKind::TextInput,
            InputKind::Ime,
            InputKind::ClipboardPaste,
            InputKind::ClipboardCut,
            InputKind::TextPointer,
            InputKind::TextVertical,
            InputKind::TextMotion,
        ] {
            let controller = self.clone();
            let theme = *theme;
            let text_style = metrics.text_style.clone();
            element = element.on_input(kind, EventPhase::Capture, move |event| {
                controller.handle(event, &theme, &text_style)
            });
        }
        element
    }
}

fn input_change(change: InputChange) -> EditOutcome {
    match change {
        InputChange::Unchanged => EditOutcome::Unchanged,
        InputChange::Changed => EditOutcome::Changed,
        InputChange::Rejected => EditOutcome::LimitReached,
    }
}

fn token_presentation(
    state: &Editor,
    theme: &ArtisanTheme,
    style: &aqua_text::TextStyle,
) -> Option<(
    Vec<aqua_text::StyledSpan>,
    Vec<aqua_ui::tree::TextInlinePaint>,
)> {
    let tokens = state.styled_tokens();
    let compiled =
        match super::tokens::compile_tokens(state.text(), style, theme.colors.text, &tokens, |_| {
            super::tokens::MENTION_GLYPH_CELL_PX
        }) {
            Ok(compiled) => compiled,
            Err(error) => {
                tracing::warn!(?error, "style editor tokens; showing plain text");
                return None;
            }
        };
    let glyphs = compiled
        .glyphs
        .into_iter()
        .map(|glyph| {
            let asset = editor_assets::get(glyph.asset);
            let tint = match asset.presentation {
                editor_assets::Presentation::Tinted => Some(glyph.color.to_reference_paint()),
                editor_assets::Presentation::FullColor => None,
            };
            aqua_ui::tree::TextInlinePaint {
                bytes: glyph.range,
                content: aqua_ui::tree::TextInlinePaintContent::FixedSvg {
                    source: asset.source.into(),
                    tint,
                    size: 15.0,
                    intrinsic: asset.intrinsic_dimensions(),
                },
            }
        })
        .collect();
    Some((compiled.spans, glyphs))
}

fn wrap_prompt_scroll(
    leaf: Element,
    mode: Mode,
    theme: &ArtisanTheme,
    bare: bool,
    inset: bool,
    suggestion: Option<String>,
    edges: Option<&EdgeBands>,
) -> Element {
    if mode != Mode::Prompt {
        return leaf;
    }
    let metrics = EditorMetrics::resolve(theme, mode, bare, inset);
    let scroll = Scroll::new()
        .preserve_offset(true)
        .style(
            Style::new()
                .axis(Axis::Column)
                .width(pct(100.0))
                .min_width(px(0.0))
                .min_height(px(metrics.min_height())),
        )
        .child(leaf)
        .build();
    let scroll = match edges {
        Some(edges) => scroll.with_key(edges.scroll),
        None => scroll,
    };
    let wrapper = UiBox::new().style(
        Style::new()
            .axis(Axis::Column)
            .width(pct(100.0))
            .min_width(px(0.0)),
    );
    let mut hint = UiBox::new().style(
        Style::new()
            .absolute(aqua_ui::style::Absolute {
                left: Some(metrics.padding_x),
                top: Some(metrics.padding_y),
                right: Some(metrics.padding_x),
                bottom: None,
            })
            .height(px(metrics.line_height))
            .align(aqua_ui::AlignItems::CENTER)
            .gap(8.0),
    );
    if let Some(suggestion) = suggestion {
        hint = hint
            .child(
                Text::new(suggestion)
                    .text_style(metrics.text_style.clone())
                    .color(metrics.placeholder)
                    .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                    .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
                    .build(),
            )
            .child(
                crate::ui::badge::Badge::new(*theme, "Tab")
                    .variant(crate::ui::badge::BadgeVariant::Keycap)
                    .build(),
            );
    }
    wrapper
        .child(hint.build())
        .child(scroll)
        .children(edges.map(EdgeBands::bands).into_iter().flatten())
        .build()
}
