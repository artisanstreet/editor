use crate::inspector::{Layout, Row, TerminalDialog, TerminalPalette, shell_prompt};
use aqua_text::StyledSpan;
use aqua_ui::{
    collection::{
        CollectionDescriptor, CollectionError, CollectionLayout, CollectionRole, CollectionSource,
        CollectionViewport,
    },
    element::{Box as UiBox, Element, Key},
    style::{Length, Style},
};
use editor_assets::AssetId;
use editor_ui::kit::{Button, ButtonTone, Finish, IconButton, IconButtonSize, Surface, TextRole};
use editor_ui::text::slice::slice;
use editor_ui::theme::ArtisanTheme;
use std::{cell::RefCell, rc::Rc};

const DIALOG: Surface = Surface::Dialog;
const WELL: Surface = Surface::Inset;
const TYPE: TextRole = TextRole::Code;
const CARET: &str = "█";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TerminalAction {
    Retry,
    Copy,
    JumpToLatest,
    Hover(bool),
    Viewport(CollectionViewport),
}

fn line_height() -> f32 {
    TYPE.spec().line
}

struct Rows {
    dialog: Rc<RefCell<TerminalDialog>>,
    layout: Layout,
    theme: ArtisanTheme,
}

impl CollectionSource for Rows {
    fn len(&self) -> usize {
        self.layout.total()
    }
    fn key(&self, index: usize) -> Key {
        Key::Usize(index)
    }
    fn layout(&self, available_width: f32) -> CollectionLayout {
        CollectionLayout {
            content_width: available_width,
            column_widths: Vec::new(),
        }
    }
    fn role(&self) -> CollectionRole {
        CollectionRole::List
    }
    fn header(&self, _: &CollectionLayout) -> Option<Element> {
        None
    }
    fn validate(&self) -> Result<(), CollectionError> {
        Ok(())
    }
    fn row(&self, index: usize, _: &CollectionLayout) -> Element {
        let state = self.dialog.borrow();
        let Some(job) = &state.job else {
            return UiBox::new().height(line_height()).build();
        };
        let theme = self.theme;
        let font = TYPE.style(&theme);
        let mut row = UiBox::new()
            .width(Length::Pct(100.0))
            .style(Style::new().min_height(Length::Px(line_height())));
        match self.layout.row(index) {
            Some(Row::Command(index)) => {
                let prompt = shell_prompt(job.cwd.as_deref());
                let prefix = if index == 0 {
                    prompt.prompt.as_str()
                } else {
                    prompt.continuation
                };
                let command = job
                    .commands
                    .get(index)
                    .map_or("", |range| slice(&job.command, range.clone()));
                let mut spans = Vec::new();
                let highlighted = if index == 0 && prompt.coloured {
                    prefix.trim_end().trim_end_matches('$')
                } else {
                    ""
                };
                let rest = prefix.strip_prefix(highlighted).unwrap_or_else(|| {
                    panic!(
                        "invariant: {highlighted:?} is trimmed from the end of prompt {prefix:?}"
                    )
                });
                if !highlighted.is_empty() {
                    spans.push(
                        StyledSpan::new(highlighted, TerminalPalette::bold(&font))
                            .gpui_source_color(TerminalPalette::for_theme(&theme).base[12]),
                    );
                }
                spans.push(StyledSpan::new(format!("{rest}{command}"), font));
                row = row.child(TYPE.text(&theme, "").spans(spans).build());
            }
            Some(Row::Missing) => {
                row = row.child(
                    TYPE.text(
                        &theme,
                        if job.unverified {
                            "Live output only; Forge supplied no final snapshot."
                        } else {
                            "Earlier output is not kept; showing the newest."
                        },
                    )
                    .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                    .build(),
                );
            }
            Some(Row::Output(index)) => {
                if let Some(line) = job.screen.line(index) {
                    let fg = theme.colors.text.to_reference_text_color();
                    let spans = TerminalPalette::for_theme(&theme).spans(
                        line,
                        &font,
                        [fg.hue_degrees, fg.saturation, fg.lightness, fg.alpha],
                        [0.0, 0.0, 0.0, 0.0],
                    );
                    row = row.child(TYPE.text(&theme, "").spans(spans).build());
                }
            }
            Some(Row::Status) => {
                if job.status == artisan_api::Status::Running {
                    row = row.child(
                        TYPE.text(&theme, CARET)
                            .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                            .build(),
                    );
                } else {
                    row = row.child(
                        TYPE.text(&theme, shell_prompt(job.cwd.as_deref()).prompt)
                            .build(),
                    );
                }
            }
            None => {}
        }
        row.build()
    }
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one terminal pane view built from the session state it is handed"
)]
pub(crate) fn terminal_view(
    dialog: Rc<RefCell<TerminalDialog>>,
    theme: &ArtisanTheme,
    viewport_height: f32,
    hovered: bool,
    copied: bool,
    command_source: Option<Element>,
    output_source: Option<Element>,
    action: &Rc<dyn Fn(TerminalAction)>,
) -> Element {
    let state = dialog.borrow();
    let job = state
        .job
        .as_ref()
        .expect("invariant: the terminal dialog is rendered only while a job is open");
    assert!(
        job.command_source.is_none() || command_source.is_some(),
        "invariant: a stored terminal command is rendered with its source view"
    );
    assert!(
        job.output_source.is_none() || output_source.is_some(),
        "invariant: stored terminal output is rendered with its source view"
    );
    let output_stored = job.output_source.is_some();
    let layout = Layout::of(job);
    let height = (viewport_height * 0.62)
        .min(640.0)
        .min((viewport_height - 160.0).max(32.0));
    let mut content = UiBox::new().layout("column").width(Length::Pct(100.0));
    if let Some(error) = state.error() {
        let retry = Rc::clone(action);
        content = content.child(
            editor_ui::kit::ProblemNotice::new("terminal-problem", *theme, error)
                .action(
                    Button::text("terminal-retry", *theme, ButtonTone::Neutral, "Try again")
                        .on_activate(move |_| retry(TerminalAction::Retry))
                        .build(),
                )
                .build(),
        );
    } else if state.loading() && job.data.is_none() {
        content = content.child(TextRole::Label.text(theme, "Loading shell output…").build());
    }
    if let Some(source) = command_source {
        content = content.child(source);
    }
    if let Some(source) = output_source {
        content = content.child(source);
    }
    let following = state.following;
    drop(state);
    let source = Rc::new(Rows {
        dialog,
        layout,
        theme: *theme,
    });
    let mut descriptor = CollectionDescriptor::new(
        source,
        vec![
            Style::new()
                .height(Length::Px(height))
                .width(Length::Pct(100.0)),
        ],
        true,
        line_height(),
        4,
    );
    let viewport_callback = Rc::clone(action);
    descriptor.on_viewport = Some(Rc::new(move |viewport| {
        viewport_callback(TerminalAction::Viewport(viewport));
    }));
    let mut well = vec![
        content
            .child(Element::collection(descriptor).with_key("inspector-terminal-lines"))
            .build(),
    ];
    let edge = WELL.padding_px();
    let copy = Rc::clone(action);
    well.push(
        UiBox::new()
            .opacity(if hovered { 1.0 } else { 0.0 })
            .style(Style::new().absolute_at(None, Some(edge), Some(edge), None))
            .child(
                IconButton::new(
                    "inspector-terminal-copy",
                    *theme,
                    if copied {
                        AssetId::TABLER_CHECK
                    } else {
                        AssetId::TABLER_COPY
                    },
                    "Copy output",
                    IconButtonSize::Md,
                    Finish::Plain,
                )
                .within(WELL)
                .disabled(output_stored)
                .on_activate(move |_| copy(TerminalAction::Copy))
                .build(),
            )
            .build(),
    );
    if !following {
        let latest = Rc::clone(action);
        well.push(
            UiBox::new()
                .style(Style::new().absolute_at(None, None, Some(edge), Some(edge)))
                .child(
                    IconButton::new(
                        "inspector-terminal-latest",
                        *theme,
                        AssetId::TABLER_ARROW_BAR_TO_DOWN,
                        "Jump to latest",
                        IconButtonSize::Md,
                        Finish::Plain,
                    )
                    .within(WELL)
                    .on_activate(move |_| latest(TerminalAction::JumpToLatest))
                    .build(),
                )
                .build(),
        );
    }
    let hover = Rc::clone(action);
    DIALOG
        .build(theme, vec![WELL.build(theme, well)])
        .style(Style::new().width(Length::Pct(100.0)))
        .with_key("inspector-terminal-card")
        .on_hover_change(move |value| hover(TerminalAction::Hover(value)))
}
