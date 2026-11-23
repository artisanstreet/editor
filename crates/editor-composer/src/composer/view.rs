use std::rc::Rc;

use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Element, Scroll},
    style::{Length, Style},
};
use editor_assets::AssetId;
use editor_ui::{
    kit::{
        Button, ButtonTone, Finish, IconButton, IconButtonSize, IconSize, Inset, Key, KeyFace,
        KeySize, Lead, Space, Spaced, TextRole, Trigger, TriggerSize,
    },
    theme::ArtisanTheme,
};
mod answers;
mod dispatch;
mod mentions;
mod model_menu;
mod motion;
mod popups;
mod project;
mod questionnaire;
mod surface;
pub(crate) use self::model_menu::model_menu;
use self::model_menu::{
    MODEL_BAR_GAP, MODEL_BAR_HEIGHT, MODEL_EDGE_BOTTOM, MODEL_EDGE_TOP, MODEL_ITEM_GAP,
    MODEL_LIST_HEIGHT, axis_options, choose_axis_config, engine_icon, model_item_height,
    model_menu_items,
};
pub use self::project::project_menu;
pub(crate) use self::project::project_tab;
pub use self::questionnaire::questionnaire;

const MODEL_TRIGGER_WIDTH: f32 = 360.0;
const MENTION_DETAIL_WIDTH: f32 = 180.0;
const MENTION_ROWS_SHOWN: f32 = 9.5;
const PROJECT_MENU_WIDTH: f32 = 320.0;
const PROJECT_TAB_WIDTH: f32 = 240.0;
const PROJECT_ROWS_SHOWN: usize = 8;
const MENU_VIEWPORT_MARGIN: f32 = 32.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerAction {
    Editor(editor_ui::text::mount::EditorEvent),
    DroppedPaths {
        paths: Vec<std::path::PathBuf>,
        instance: editor_core::HostInstance,
        generation: u64,
    },
    DroppedThread(editor_core::DraggedThread),
    TooltipHover {
        instance: editor_core::HostInstance,
        generation: u64,
        anchor: String,
        content: Option<super::picker::TooltipContent>,
        hovered: bool,
    },
    Send,
    Stop,
    Continue,
    ToggleModel,
    ToggleProject,
    JumpEngine(artisan_api::EngineId),
    OpenEngineSettings(artisan_api::EngineId),
    ToggleModelGroup {
        engine: artisan_api::EngineId,
        name: String,
        open: bool,
    },
    SelectPane(Option<super::picker::Axis>),
    AxisPointer {
        axis: super::picker::Axis,
        index: usize,
    },
    AxisKey {
        axis: super::picker::Axis,
        key: super::picker::AxisKey,
    },
    ChooseAxis {
        axis: super::picker::Axis,
        index: usize,
    },
    ClosePicker,
    DismissMentions,
    ChooseModel {
        engine: artisan_api::EngineId,
        id: String,
    },
    ChooseProject(artisan_api::ProjectId),
    NewProject,
    SelectMention(usize),
    HoverMention(usize, bool),
    RemoveImage(u64),
    OpenImage(u64),
    RetryImage(u64),
    RemoveFile(u64),
    RetryDraft,
    KeepDraft,
    LoadOtherDraft,
    RetryModelChoice,
    RetryThreadSettings,
    DismissNotice,
    CloseNote,
    ClearGoal,
    Unpark,
    DismissQuestionnaire,
    ActivateQuestionnaire(artisan_api::ItemId),
    ChooseQuestionOption {
        item: artisan_api::ItemId,
        question: String,
        step: usize,
        id: String,
        multi: bool,
    },
    ContinueQuestionnaire,
    RetryQuestionnaire,
    RetryQuestionPage,
    QuestionBack,
    OpenQuestionSource(artisan_api::ContentRef),
}

pub type Dispatch = Rc<dyn Fn(ComposerAction)>;

fn rows(count: usize) -> f32 {
    f32::from(u16::try_from(count).unwrap_or(u16::MAX))
}

pub(crate) const PROMPT_NAME: &str = "Message";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Primary {
    Send,
    Stop,
    Continue,
}

const fn primary_icon(face: Primary) -> AssetId {
    match face {
        Primary::Stop => AssetId::TABLER_PLAYER_STOP_FILLED,
        Primary::Continue => AssetId::TABLER_PLAYER_PLAY_FILLED,
        Primary::Send => AssetId::TABLER_ARROW_UP,
    }
}

pub(crate) fn primary(
    theme: &ArtisanTheme,
    face: Primary,
    outgoing: Option<super::swap::SwapFrame>,
    disabled: bool,
    dispatch: Dispatch,
) -> Element {
    let (icon, label, id, action) = match face {
        Primary::Stop => (
            AssetId::TABLER_PLAYER_STOP_FILLED,
            "Stop",
            "composer-stop",
            ComposerAction::Stop,
        ),
        Primary::Continue => (
            AssetId::TABLER_PLAYER_PLAY_FILLED,
            "Continue where it stopped",
            "composer-continue",
            ComposerAction::Continue,
        ),
        Primary::Send => (
            AssetId::TABLER_ARROW_UP,
            "Send message",
            "composer-send",
            ComposerAction::Send,
        ),
    };
    let content = super::swap::face(
        icon,
        outgoing.map(|frame| (primary_icon(frame.previous), frame)),
    );
    Key::new(id, *theme, label, KeySize::Md, KeyFace::Content(content))
        .primary()
        .disabled(disabled)
        .on_activate(move |_| dispatch(action.clone()))
        .build()
        .debug_selector(id)
}

fn frame(theme: &ArtisanTheme, nested: bool, content: Element) -> Element {
    editor_ui::glass::ComposerFrame::new(*theme, content)
        .nested(nested)
        .build()
}

pub(crate) fn model_trigger(
    theme: &ArtisanTheme,
    config: Option<&artisan_api::EngineConfig>,
    model: Option<&artisan_api::Model>,
    dispatch: Dispatch,
) -> Element {
    let migrated = model.and_then(|model| config.and_then(|config| model.migrate_config(config)));
    let config = migrated.as_ref().or(config);
    let name = config.map_or_else(
        || "No model".to_owned(),
        |config| {
            model.map_or_else(
                || config.model.clone().unwrap_or_else(|| "No model".into()),
                |model| model.name.clone(),
            )
        },
    );
    let details = config
        .map(|config| model_details(config, model))
        .unwrap_or_default();
    let mut trigger = Trigger::new("composer-model", *theme, TriggerSize::Sm, name)
        .on_activate(move || dispatch(ComposerAction::ToggleModel));
    if !details.is_empty() {
        trigger = trigger.detail(details.join(" "));
    }
    if let Some(config) = config {
        trigger = trigger.lead(Lead::Glyph(engine_icon(theme, config.engine)));
    }
    UiBox::new()
        .style(
            Style::new()
                .min_width(Length::Pct(0.0))
                .max_width(Length::Px(MODEL_TRIGGER_WIDTH)),
        )
        .child(trigger.build())
        .build()
}

fn model_details(
    config: &artisan_api::EngineConfig,
    model: Option<&artisan_api::Model>,
) -> Vec<String> {
    let context = model
        .and_then(|model| config.context_window_override().or(model.context_window))
        .map(artisan_api::token_label);
    let compaction = config
        .compaction_window_override()
        .map(|compaction| format!("{} compact", artisan_api::token_label(compaction)));
    let effort = config
        .effort
        .as_ref()
        .or_else(|| {
            model.and_then(|model| {
                model
                    .default_effort
                    .as_ref()
                    .or_else(|| (model.efforts.len() == 1).then(|| &model.efforts[0]))
            })
        })
        .filter(|_| config.engine != artisan_api::EngineId::OpenCode)
        .map(|effort| {
            artisan_api::ThinkingLevel::from_native(effort)
                .map_or_else(|| effort.clone(), |level| level.label().into())
        });
    let variant = model
        .and_then(|model| {
            model
                .variant_for(config.variant.as_deref())
                .and_then(|id| model.variants.iter().find(|variant| variant.id == id))
        })
        .map(|variant| {
            artisan_api::ThinkingLevel::from_native(&variant.id)
                .map_or_else(|| variant.label.clone(), |level| level.label().into())
        });
    let speed = model
        .and_then(|model| {
            config.speed.as_ref().and_then(|id| {
                model
                    .speeds
                    .iter()
                    .find(|speed| &speed.id == id && !speed.default)
            })
        })
        .map(|speed| {
            artisan_api::SpeedLevel::from_native(&speed.id)
                .map_or_else(|| speed.label.clone(), |level| level.label().into())
        });
    [context, compaction, effort, variant, speed]
        .into_iter()
        .flatten()
        .collect()
}

fn controls(primary: Element) -> Element {
    UiBox::new()
        .height(KeySize::Md.edge())
        .spaced(Space::S8)
        .style(
            Style::new()
                .flex_shrink(0.0)
                .align(AlignItems::CENTER)
                .justify(JustifyContent::END),
        )
        .child(primary)
        .build()
        .with_key("composer-controls")
}

pub const MENTION_SCROLL: &str = "composer-mention-scroll";

pub(crate) struct CardParts {
    prompt: Element,
    primary: Element,
    questionnaires: Option<Element>,
    attachments: Option<Element>,
    mention_overlay: Option<Element>,
    thread_error: Option<Element>,
    draft_error: Option<Element>,
}

const TEXT_INSET: f32 = Inset::Menu.px() + Space::S8.px();
const PROMPT_PADDING_X: f32 = 8.0;
const PROMPT_GUTTER: f32 = TEXT_INSET - editor_ui::tab_card::TAB_CARD_PADDING_PX - PROMPT_PADDING_X;

#[expect(
    clippy::disallowed_methods,
    reason = "the extras inset only the sides and top, and the prompt gutter only the sides; the kit pads all four sides alike"
)]
pub(crate) fn composer_card(theme: &ArtisanTheme, nested: bool, parts: CardParts) -> Element {
    let extras: Vec<Element> = [
        parts.thread_error,
        parts.draft_error,
        parts.questionnaires,
        parts.attachments,
    ]
    .into_iter()
    .flatten()
    .collect();
    let extras = (!extras.is_empty()).then(|| {
        let inset = if nested { Inset::Frame.px() } else { 0.0 };
        UiBox::new()
            .layout("column")
            .width(Length::Pct(100.0))
            .style(Style::new().padding_insets(aqua_core::Insets {
                left: inset,
                right: inset,
                top: inset,
                bottom: 0.0,
            }))
            .children(extras)
            .build()
    });
    let gutter = if nested { PROMPT_GUTTER } else { 0.0 };
    let entry = UiBox::new()
        .width(Length::Pct(100.0))
        .style(
            Style::new()
                .wrap(true)
                .column_gap(Space::S8.px())
                .row_gap(0.0)
                .align(AlignItems::END)
                .justify(JustifyContent::END),
        )
        .child(
            UiBox::new()
                .layout("column")
                .style(
                    Style::new()
                        .flex_grow(1.0)
                        .flex_shrink(1.0)
                        .min_width(Length::Pct(0.0))
                        .max_width(Length::Pct(100.0))
                        .padding_insets(aqua_core::Insets {
                            left: gutter,
                            right: gutter,
                            top: 0.0,
                            bottom: 0.0,
                        }),
                )
                .child(parts.prompt.style(Style::new().width(Length::Auto)))
                .build()
                .with_key(super::swap::PROMPT),
        )
        .child(controls(parts.primary))
        .build();
    let card = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .children(extras)
        .child(entry)
        .children(parts.mention_overlay)
        .build();
    frame(theme, nested, card).with_key("composer")
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ComposerFailure<'a> {
    Draft {
        error: &'a editor_core::Problem,
        conflict: bool,
    },
    ModelChoice {
        error: &'a editor_core::Problem,
    },
    Notice {
        message: &'a editor_core::Problem,
    },
}

pub(crate) fn draft_error(
    theme: &ArtisanTheme,
    failure: ComposerFailure<'_>,
    dispatch: &Dispatch,
) -> Element {
    let choice = |key: &'static str, label: &'static str, action: ComposerAction| {
        let dispatch = Rc::clone(dispatch);
        Button::text(key, *theme, ButtonTone::Neutral, label)
            .on_activate(move |_| dispatch(action.clone()))
            .build()
    };
    let (problem, actions) = match failure {
        ComposerFailure::Draft {
            error,
            conflict: true,
        } => (
            error,
            vec![
                choice(
                    "draft-keep-local",
                    "Keep my draft",
                    ComposerAction::KeepDraft,
                ),
                choice(
                    "draft-use-remote",
                    "Load other draft",
                    ComposerAction::LoadOtherDraft,
                ),
            ],
        ),
        ComposerFailure::Draft {
            error,
            conflict: false,
        } => (
            error,
            vec![choice(
                "draft-retry",
                "Try again",
                ComposerAction::RetryDraft,
            )],
        ),
        ComposerFailure::ModelChoice { error } => (
            error,
            vec![choice(
                "model-choice-retry",
                "Try again",
                ComposerAction::RetryModelChoice,
            )],
        ),
        ComposerFailure::Notice { message } => (
            message,
            vec![choice(
                "notice-dismiss",
                "Dismiss",
                ComposerAction::DismissNotice,
            )],
        ),
    };
    let notice = actions.into_iter().fold(
        editor_ui::kit::ProblemNotice::new("composer-problem", *theme, problem),
        editor_ui::kit::ProblemNotice::action,
    );
    UiBox::new()
        .layout("column")
        .padded(Space::S8)
        .child(notice.build())
        .build()
}

#[expect(
    clippy::too_many_lines,
    reason = "the goal label lays out its status glyph, ticker, budget and actions for every goal phase"
)]
pub(crate) fn goal_label(
    theme: &ArtisanTheme,
    frame: &super::goal_strip::GoalStripFrame,
    ticker: Element,
    dispatch: Dispatch,
) -> Element {
    use super::goal_strip::ComposerGoalState as State;
    use aqua_core::Transform2D;
    let goal = &frame.goal;
    let safety = matches!(
        goal.state,
        State::SafetyHolding | State::SafetyRerouted | State::SafetyRefused
    );
    let note = matches!(goal.state, State::Note | State::HeadsUp);
    let (icon, tone) = match goal.state {
        State::Blocked => (
            AssetId::TABLER_TARGET_ARROW,
            theme.colors.banner_warning.text,
        ),
        State::SafetyHolding | State::SafetyRerouted => (
            AssetId::TABLER_SHIELD_LOCK,
            theme.colors.banner_warning.text,
        ),
        State::SafetyRefused => (AssetId::TABLER_SHIELD_LOCK, theme.colors.banner_error.text),
        State::Parked => (AssetId::TABLER_RUN_SPRINT, theme.colors.banner_warning.text),
        State::Note => (
            AssetId::TABLER_BELL_EXCLAMATION,
            theme.colors.banner_info.text,
        ),
        State::HeadsUp => (
            AssetId::TABLER_BELL_EXCLAMATION,
            theme.colors.banner_warning.text,
        ),
        State::Active | State::Paused | State::UsageLimited | State::BudgetLimited => {
            (AssetId::TABLER_TARGET_ARROW, theme.colors.text_muted)
        }
    };
    let muted = theme.colors.text_muted.to_reference_text_color();
    let mut style = Style::new()
        .align(AlignItems::CENTER)
        .opacity(frame.label_opacity)
        .transform(Transform2D::translate(0.0, frame.label_offset));
    let blur = (frame.label_offset.abs() / 2.0).min(2.0);
    if blur > 0.0 {
        style = style.content_blur(blur);
    }
    let mut row = UiBox::new()
        .height(Length::Pct(100.0))
        .padded(Space::S6)
        .spaced(Space::S6)
        .style(style)
        .child(IconSize::Md.glyph(icon, tone.to_reference_paint()));
    if note {
        row = row.child(
            UiBox::new()
                .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)))
                .child(ticker)
                .build(),
        );
    } else {
        let headline = if safety { tone } else { theme.colors.text };
        row = row.child(
            UiBox::new()
                .style(Style::new().min_width(Length::Pct(0.0)))
                .child(
                    TextRole::Label
                        .text(theme, goal.objective.clone())
                        .gpui_source_color(headline.to_reference_text_color())
                        .wrap(false)
                        .build(),
                )
                .build(),
        );
        if matches!(
            goal.state,
            State::Active | State::SafetyHolding | State::SafetyRerouted | State::SafetyRefused
        ) && let Some(detail) = &goal.detail
        {
            row = row.child(
                TextRole::Label
                    .text(theme, detail.clone())
                    .gpui_source_color(muted)
                    .wrap(false)
                    .build(),
            );
        }
        let status = match goal.state {
            State::Paused => Some(("Paused", theme.colors.banner_info.text)),
            State::Blocked => Some(("Blocked", theme.colors.banner_warning.text)),
            State::UsageLimited => Some(("Usage limit reached", theme.colors.banner_warning.text)),
            State::BudgetLimited => Some(("Budget spent", theme.colors.banner_warning.text)),
            State::Active
            | State::SafetyHolding
            | State::SafetyRerouted
            | State::SafetyRefused
            | State::Note
            | State::HeadsUp
            | State::Parked => None,
        };
        if let Some((text, tone)) = status {
            row = row.child(
                TextRole::Caption
                    .text(theme, text)
                    .gpui_source_color(tone.to_reference_text_color())
                    .wrap(false)
                    .build(),
            );
        }
        row = row.child(UiBox::new().style(Style::new().flex_grow(1.0)).build());
    }
    if !safety {
        let (id, label, action) = if note {
            (
                "composer-note-close",
                "Close note",
                ComposerAction::CloseNote,
            )
        } else if goal.state == State::Parked {
            (
                "composer-park-cancel",
                "Cancel the parked run",
                ComposerAction::Unpark,
            )
        } else {
            (
                "composer-goal-clear",
                "Clear goal",
                ComposerAction::ClearGoal,
            )
        };
        row = row.child(
            IconButton::new(
                id,
                *theme,
                AssetId::TABLER_X,
                label,
                IconButtonSize::Sm,
                Finish::Plain,
            )
            .muted()
            .on_activate(move |_| dispatch(action.clone()))
            .build(),
        );
    }
    row.build()
}

pub struct ComposerOwner {
    pub controller: Rc<std::cell::RefCell<super::controller::Composer>>,
    pub editor: editor_ui::text::mount::EditorController,
    dispatch: Dispatch,
    answers: editor_core::delivery::Delivery<AnswerCompletion>,
    reduced_motion: bool,
    model_popup: Option<editor_ui::popover::AnimatedPickerHost>,
    mention_popup: Option<editor_ui::popover::AnimatedPickerHost>,
    project_popup: Option<editor_ui::popover::AnimatedPickerHost>,
    engine_light: std::cell::RefCell<super::engine_light::EngineLight>,
    engine_light_image: std::cell::RefCell<super::engine_light::EngineLightImage>,
    tab_geometry: Rc<std::cell::RefCell<editor_ui::tab_card::GeometryCache>>,
    strip_moving: std::cell::Cell<bool>,
    note_ticker: std::cell::RefCell<editor_ui::marquee::Marquee>,
    primary_swap: std::cell::Cell<super::swap::PrimarySwap>,
    model_menu_serial: std::cell::Cell<u64>,
    prompt_edges: editor_ui::scroll_edge::ScrollEdges,
    model_scroll: std::cell::RefCell<super::model_list::ModelScroll>,
    model_layout: std::cell::RefCell<Option<super::model_list::ListLayout>>,
    model_tab: std::cell::RefCell<super::sliding_tab::SlidingTab>,
    last_visible: std::cell::RefCell<ComposerVisible>,
    tooltip: std::cell::RefCell<super::picker::PickerTooltip>,
}

impl std::fmt::Debug for ComposerOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComposerOwner")
            .field("controller", &self.controller)
            .field("reduced_motion", &self.reduced_motion)
            .finish_non_exhaustive()
    }
}

struct AnswerCompletion {
    host: editor_core::HostId,
    instance: editor_core::HostInstance,
    generation: u64,
    item: artisan_api::ItemId,
    result: Result<(), editor_core::client::CallError>,
}

#[derive(Clone, Debug)]
pub enum ComposerEffect {
    Stopping(editor_core::HostId, artisan_api::ThreadId),
    StopFailed(editor_core::HostId, artisan_api::ThreadId),
    OpenSent {
        host: editor_core::HostId,
        thread: artisan_api::ThreadId,
        replaced: Vec<artisan_api::ThreadId>,
    },
    EditorChanged,
    FocusEditor,
    NewProject,
    OpenImage(u64),
    OpenQuestionSource(artisan_api::ContentRef),
    RetryQuestionPage,
    CloseNote,
    RetryThreadSettings,
}

#[derive(Debug)]
pub struct ComposerDrain {
    pub events: Vec<super::persistence::SessionEvent>,
    pub effects: Vec<ComposerEffect>,
    pub changed: bool,
}

#[derive(Debug)]
pub struct PickerTooltipPresentation {
    pub side: aqua_ui::overlay::Side,
    pub snap_to_window: bool,
}

pub struct PickerTooltipSurface {
    pub placement: editor_ui::hover_tooltip::TooltipPlacement,
    pub surface: Rc<dyn Fn(f32, bool) -> Element>,
}

impl std::fmt::Debug for PickerTooltipSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PickerTooltipSurface")
            .field("placement", &self.placement)
            .finish_non_exhaustive()
    }
}

type QuestionMark = (
    Option<artisan_api::ItemId>,
    usize,
    Vec<String>,
    Option<artisan_api::ItemId>,
    Option<editor_core::Problem>,
    Option<String>,
);

type AttachmentMark = (
    u64,
    super::attachment::State,
    Option<String>,
    bool,
    bool,
    Option<std::time::Instant>,
);

type AttachmentsMark = (
    Vec<AttachmentMark>,
    Option<(u64, std::time::Instant)>,
    Option<editor_core::Problem>,
);

#[derive(PartialEq)]
struct ComposerRest {
    picker: super::picker::PickerState,
    mention: (Option<super::mentions::Trigger>, Vec<usize>, usize),
    question: QuestionMark,
    strip: super::goal_strip::StripState,
    config: Option<artisan_api::EngineConfig>,
    project: Option<artisan_api::ProjectId>,
    attachments: Option<AttachmentsMark>,
    files: Vec<(u64, std::path::PathBuf, bool)>,
    file_shake: Option<(u64, std::time::Instant)>,
    notice: Option<editor_core::Problem>,
    refused: Option<super::controller::SendRefusal>,
    session: Option<(bool, bool, Option<editor_core::Problem>, bool)>,
    cancelling: bool,
    continuing: bool,
}

#[derive(PartialEq)]
struct ComposerVisible {
    rest: ComposerRest,
    editor: EditorVisible,
}

impl ComposerVisible {
    fn same_chrome(&self, other: &Self) -> bool {
        self.rest == other.rest && self.editor.same_chrome(&other.editor)
    }
}

#[derive(Clone, PartialEq)]
struct EditorVisible {
    text: String,
    selection: std::ops::Range<usize>,
    reversed: bool,
    marked: Option<std::ops::Range<usize>>,
    disabled: bool,
    limit_notice: bool,
    suggestion: Option<String>,
    tokens: Vec<editor_ui::text::editor::StyledToken>,
}

impl EditorVisible {
    fn capture(editor: &editor_ui::text::editor::Editor) -> Self {
        Self {
            text: editor.text().to_owned(),
            selection: editor.selection(),
            reversed: editor.selection_reversed(),
            marked: editor.marked_range(),
            disabled: editor.is_disabled(),
            limit_notice: editor.limit_notice(),
            suggestion: editor.shown_suggestion().map(str::to_owned),
            tokens: editor.styled_tokens(),
        }
    }

    fn same_chrome(&self, other: &Self) -> bool {
        self.text.is_empty() == other.text.is_empty()
            && self.text.trim().is_empty() == other.text.trim().is_empty()
            && self.disabled == other.disabled
            && self.limit_notice == other.limit_notice
            && self.suggestion == other.suggestion
            && self.tokens == other.tokens
    }
}

impl ComposerVisible {
    fn capture(composer: &super::controller::Composer) -> Self {
        let editor = composer.editor.borrow();
        Self {
            rest: ComposerRest::capture(composer),
            editor: EditorVisible::capture(&editor),
        }
    }
}

impl ComposerRest {
    fn capture(composer: &super::controller::Composer) -> Self {
        let mut picker = composer.picker.clone();
        picker.axis_cursor = picker
            .axis_cursor
            .filter(|cursor| matches!(cursor, super::picker::AxisCursor::Keys(_)));
        Self {
            picker,
            mention: (
                composer.mentions.current.clone(),
                composer.mentions.rows.clone(),
                composer.mentions.highlighted,
            ),
            question: (
                composer
                    .questionnaire
                    .active()
                    .map(|entry| entry.item.clone()),
                composer.questionnaire.step,
                composer.questionnaire.ticked.clone(),
                composer.questionnaire.sending.clone(),
                composer.questionnaire.error.clone(),
                composer.questionnaire.aside_text().map(str::to_owned),
            ),
            strip: composer.strip.clone(),
            config: composer.config.clone(),
            project: composer.project.clone(),
            attachments: composer.attachments.as_ref().map(|attachments| {
                (
                    attachments
                        .items
                        .iter()
                        .map(|item| {
                            (
                                item.key,
                                item.state.clone(),
                                item.preview_error.clone(),
                                item.shown,
                                item.preview.is_some(),
                                item.revealed,
                            )
                        })
                        .collect(),
                    attachments.shake,
                    attachments.notice.clone(),
                )
            }),
            files: composer
                .files
                .iter()
                .map(|file| (file.key, file.path.as_path().to_path_buf(), file.folder))
                .collect(),
            file_shake: composer.file_shake,
            notice: composer.notice.clone(),
            refused: composer.refused,
            session: composer.session.as_ref().map(|session| {
                session.read(|state| {
                    (
                        state.is_loaded(),
                        state.conflict(),
                        state.error(),
                        state.sending(),
                    )
                })
            }),
            cancelling: composer.cancelling,
            continuing: composer.continuing,
        }
    }
}

impl ComposerOwner {
    pub fn new(
        core: &editor_core::Core,
        sessions: &mut super::persistence::DraftSessions,
        target: super::controller::ComposerTarget,
        dispatch: Dispatch,
    ) -> Self {
        let controller = Rc::new(std::cell::RefCell::new(super::controller::Composer::new(
            core, sessions, target,
        )));
        let send = Rc::clone(&dispatch);
        let editor = controller
            .borrow()
            .mount_editor(move |event| send(ComposerAction::Editor(event)));
        let last_visible = ComposerVisible::capture(&controller.borrow());
        Self {
            controller,
            editor,
            dispatch,
            answers: editor_core::delivery::Delivery::new(
                std::num::NonZeroUsize::new(16)
                    .expect("invariant: the answer delivery capacity is non-zero"),
            ),
            reduced_motion: false,
            model_popup: None,
            mention_popup: None,
            project_popup: None,
            engine_light: std::cell::RefCell::new(super::engine_light::EngineLight::default()),
            engine_light_image: std::cell::RefCell::new(
                super::engine_light::EngineLightImage::default(),
            ),
            tab_geometry: Rc::new(std::cell::RefCell::new(
                editor_ui::tab_card::GeometryCache::default(),
            )),
            strip_moving: std::cell::Cell::new(false),
            note_ticker: std::cell::RefCell::new(editor_ui::marquee::Marquee::new(
                "composer-note-ticker",
                "",
            )),
            primary_swap: std::cell::Cell::new(super::swap::PrimarySwap::default()),
            model_menu_serial: std::cell::Cell::new(0),
            prompt_edges: editor_ui::scroll_edge::ScrollEdges::default(),
            model_scroll: std::cell::RefCell::default(),
            model_layout: std::cell::RefCell::default(),
            model_tab: std::cell::RefCell::new(super::sliding_tab::SlidingTab::new(0.0, 0.0)),
            last_visible: std::cell::RefCell::new(last_visible),
            tooltip: std::cell::RefCell::new(super::picker::PickerTooltip::default()),
        }
    }

    pub fn set_target(
        &self,
        core: &editor_core::Core,
        sessions: &mut super::persistence::DraftSessions,
        target: super::controller::ComposerTarget,
    ) {
        let changed = {
            let controller = self.controller.borrow();
            controller.target != target
                || core
                    .host(target.host())
                    .map(editor_core::HostData::instance)
                    != controller.host_instance()
        };
        if changed {
            self.tooltip.borrow_mut().clear();
            *self.engine_light.borrow_mut() = super::engine_light::EngineLight::default();
            *self.tab_geometry.borrow_mut() = editor_ui::tab_card::GeometryCache::default();
            self.strip_moving.set(false);
            *self.note_ticker.borrow_mut() =
                editor_ui::marquee::Marquee::new("composer-note-ticker", "");
        }
        self.controller
            .borrow_mut()
            .set_target(core, sessions, target);
        *self.last_visible.borrow_mut() = ComposerVisible::capture(&self.controller.borrow());
    }

    pub fn refresh_mentions(&self, host: &editor_core::HostData, theme: &ArtisanTheme) {
        let tokenizer = {
            let mut controller = self.controller.borrow_mut();
            controller.refresh_mentions(host);
            controller.mentions.tokenizer(theme)
        };
        self.editor.set_tokenizer(Some(tokenizer));
    }

    pub fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
    }
}

fn attachment_problem(image: &super::attachment::Attachment) -> Option<editor_core::Problem> {
    image
        .state
        .failure()
        .map(|reason| {
            editor_core::Problem::detailed(format!("Couldn't attach {}.", image.name), reason)
        })
        .or_else(|| {
            image.preview_error.as_ref().map(|reason| {
                editor_core::Problem::detailed(
                    format!("Couldn't show a preview of {}.", image.name),
                    reason.clone(),
                )
            })
        })
}

pub fn attachments(
    theme: &ArtisanTheme,
    controller: &super::controller::Composer,
    now: std::time::Instant,
    reduced_motion: bool,
    dispatch: &Dispatch,
) -> Option<Element> {
    use editor_ui::attachment_tile::AttachmentTile;
    if controller.questionnaire.writing_answer() {
        return None;
    }
    let images = controller.attachments.as_ref();
    if images.is_none_or(|images| images.items.iter().all(|image| !image.shown))
        && controller.files.is_empty()
    {
        return None;
    }
    let editable = controller
        .session
        .as_ref()
        .is_some_and(|session| session.read(|state| !state.sending()));
    let mut row = UiBox::new()
        .padded(Space::S4)
        .spaced(Space::S8)
        .style(Style::new().align(AlignItems::CENTER));
    if let Some(images) = images {
        for image in images.items.iter().filter(|image| image.shown) {
            let key = image.key;
            let remove = Rc::clone(dispatch);
            let open = Rc::clone(dispatch);
            let mut tile = AttachmentTile::new(
                format!("composer-attachment-{key}"),
                *theme,
                image.name.clone(),
            )
            .preview(
                image.preview.clone(),
                image
                    .revealed
                    .map(|instant| now.saturating_duration_since(instant)),
            )
            .preparing(matches!(
                image.state,
                super::attachment::State::Encoding | super::attachment::State::Uploading
            ))
            .error(attachment_problem(image))
            .shaking(
                images
                    .shake
                    .filter(|(shaken, _)| *shaken == key)
                    .map(|(_, instant)| now.saturating_duration_since(instant)),
            )
            .reduced_motion(reduced_motion)
            .removable(editable)
            .on_remove(move || remove(ComposerAction::RemoveImage(key)))
            .on_activate(move || open(ComposerAction::OpenImage(key)));
            if image.preview_error.is_some() {
                let retry = Rc::clone(dispatch);
                tile = tile.on_retry(move || retry(ComposerAction::RetryImage(key)));
            }
            row = row.child(tile.build());
        }
    }
    for file in &controller.files {
        let key = file.key;
        let path = file.path.as_path();
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let glyph = if file.folder {
            AssetId::TABLER_FOLDER
        } else {
            AssetId::TABLER_FILE_TEXT
        };
        let remove = Rc::clone(dispatch);
        row = row.child(
            AttachmentTile::new(format!("composer-file-{key}"), *theme, name)
                .file(glyph)
                .shaking(
                    controller
                        .file_shake
                        .filter(|(shaken, _)| *shaken == key)
                        .map(|(_, instant)| now.saturating_duration_since(instant)),
                )
                .reduced_motion(reduced_motion)
                .removable(editable)
                .on_remove(move || remove(ComposerAction::RemoveFile(key)))
                .build(),
        );
    }
    Some(
        Scroll::new()
            .width(Length::Pct(100.0))
            .child(row.build())
            .build()
            .with_key("composer-attachments"),
    )
}

const PROJECT_SCROLL: &str = "composer-project-scroll";
const PROMPT_SCROLL: &str = "composer-prompt-scroll";
const PROMPT_EDGE_TOP: &str = "composer-prompt-edge-top";
const PROMPT_EDGE_BOTTOM: &str = "composer-prompt-edge-bottom";
const PROJECT_EDGE_TOP: &str = "composer-project-edge-top";
const PROJECT_EDGE_BOTTOM: &str = "composer-project-edge-bottom";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupKind {
    Model,
    Project,
    Mentions,
}
impl PopupKind {
    const fn gap(self) -> f32 {
        match self {
            Self::Project | Self::Model | Self::Mentions => Space::S8.px(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelMenuItem {
    pane: Option<super::picker::Axis>,
    label: &'static str,
    value: String,
    locked: bool,
}

impl ModelMenuItem {
    fn name(&self) -> String {
        if self.value.is_empty() {
            self.label.to_owned()
        } else {
            format!("{}: {}", self.label, self.value)
        }
    }
}

pub(crate) struct ModelMenuFrame {
    items: Vec<ModelMenuItem>,
    pane: Option<super::picker::Axis>,
    scroll: f32,
    spied: Option<artisan_api::EngineId>,
    tab: super::sliding_tab::SlidingTab,
    engine_light: Option<Element>,
}

#[cfg(test)]
#[path = "../../tests/composer/view.rs"]
mod tests;
