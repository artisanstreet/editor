use artisan_api::Thread;

use editor_core::hosts::HostId;
use editor_core::screen::DraggedThread;

use super::list::{RowData, RowState};

pub const STATE_DOT: f32 = 6.0;
pub const WORKING_PULSE_MILLIS: u64 = 1_600;
pub const WORKING_PULSE_FLOOR: f32 = 0.4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Settle,
    Rename,
    MarkUnread,
    Archive,
}

pub fn menu_actions(row: &RowData, thread: &Thread) -> Vec<MenuAction> {
    let mut actions = Vec::new();
    if row.settleable {
        actions.push(MenuAction::Settle);
    }
    actions.push(MenuAction::Rename);
    if !thread.is_unread() && thread.activity_seq != 0 {
        actions.push(MenuAction::MarkUnread);
    }
    actions.push(MenuAction::Archive);
    actions
}

pub fn opens_keyboard_menu(key: &str, shift: bool) -> bool {
    matches!(key, "contextmenu" | "menu") || (key == "f10" && shift)
}

pub fn opens_aqua_keyboard_menu(event: &aqua_core::KeyEvent) -> bool {
    match event.key {
        aqua_core::Key::Named(aqua_core::NamedKey::ContextMenu) => true,
        aqua_core::Key::Named(aqua_core::NamedKey::Function(10)) => {
            event.modifiers.contains(aqua_core::Modifiers::SHIFT)
        }
        _ => false,
    }
}

pub fn drag(host: HostId, thread: &Thread) -> Option<DraggedThread> {
    let title = thread
        .title
        .as_ref()
        .filter(|title| artisan_mentions::valid_thread_title(title))?;
    Some(DraggedThread {
        host,
        thread: thread.id.clone(),
        title: title.clone(),
    })
}

pub const fn state_label(state: RowState) -> &'static str {
    match state {
        RowState::Idle => "",
        RowState::Working => "Working",
        RowState::Waiting => "Needs answer",
        RowState::Unread => "Unread",
        RowState::Failed => "Failed",
        RowState::Parked => "Waiting to pick up",
    }
}

pub fn rename_value(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

pub struct RowVisuals {
    pub title: aqua_ui::element::Element,
    pub subtitle: aqua_ui::element::Element,
    pub project_mark: aqua_ui::element::Element,
    pub dot: Option<aqua_ui::element::Element>,
}

pub type TruncatedText =
    std::rc::Rc<dyn Fn(String, f32, aqua::GpuiTextColor, String) -> aqua_ui::element::Element>;
pub type WorkingPulse = std::rc::Rc<
    dyn Fn(
        aqua_ui::element::Element,
        &'static str,
        std::time::Duration,
        f32,
    ) -> aqua_ui::element::Element,
>;

pub fn visual_factory(
    theme: editor_ui::theme::ArtisanTheme,
    reduced_motion: bool,
    truncate: TruncatedText,
    pulse: WorkingPulse,
) -> super::view::ThreadVisualFactory {
    use aqua_ui::element::Box as UiBox;
    std::rc::Rc::new(move |data, image, repository_host| {
        let title_color = if data.selected {
            theme.colors.thread_title_active
        } else {
            theme.colors.text
        };
        let selector = format!("sidebar-thread-{}", data.id);
        let title = truncate(
            data.title.clone(),
            theme.typography.control_text,
            title_color.to_reference_text_color(),
            format!("{selector}-title"),
        );
        let subtitle = truncate(
            data.subtitle.clone(),
            theme.typography.label_text,
            theme.colors.text_muted.to_reference_text_color(),
            format!("{selector}-subtitle"),
        );
        let project_mark = super::project_mark::project_mark(
            image,
            repository_host.as_ref().map(|host| host.as_deref()),
            14.0,
            theme.colors.text_muted.to_reference_paint(),
        );
        let tone = match data.state {
            RowState::Idle => None,
            RowState::Working => Some(theme.colors.text),
            RowState::Waiting => Some(theme.colors.needs_answer),
            RowState::Unread => Some(theme.colors.unread),
            RowState::Failed => Some(theme.colors.red.solid),
            RowState::Parked => Some(theme.colors.amber.solid),
        };
        let dot = tone.map(|color| {
            let dot = UiBox::new()
                .width(STATE_DOT)
                .height(STATE_DOT)
                .radius(STATE_DOT / 2.0)
                .style(
                    aqua_ui::style::Style::new()
                        .flex_shrink(0.0)
                        .gpui_background(aqua_core::GpuiBackground::Solid(
                            color.to_reference_paint(),
                        )),
                )
                .build();
            if data.state == RowState::Working && !reduced_motion {
                pulse(
                    dot,
                    "working-pulse",
                    std::time::Duration::from_millis(WORKING_PULSE_MILLIS),
                    WORKING_PULSE_FLOOR,
                )
            } else {
                dot
            }
        });
        RowVisuals {
            title: UiBox::new()
                .width(aqua_ui::style::Length::Pct(100.0))
                .child(title)
                .build(),
            subtitle: UiBox::new()
                .style(
                    aqua_ui::style::Style::new()
                        .flex_grow(1.0)
                        .min_width(aqua_ui::style::Length::Px(0.0)),
                )
                .child(subtitle)
                .build(),
            project_mark,
            dot,
        }
    })
}

pub fn build(
    data: &RowData,
    host: HostId,
    instance: editor_core::HostInstance,
    drag: Option<DraggedThread>,
    visuals: RowVisuals,
    layout: super::geometry::ThreadLayout,
    hover: aqua_ui::element::Element,
    emit: std::rc::Rc<dyn Fn(super::view::ViewAction)>,
) -> aqua_ui::element::Element {
    use super::view::ViewAction;
    use editor_ui::theme::{RadiusStep, RadiusTokens};
    use aqua_core::{CursorIcon, InputEvent, Insets, PointerButton, Role};
    use aqua_ui::{
        AlignItems,
        element::{Box as UiBox, Button},
        input::{
            CaptureRequest, DragPayload, DragRequest, EventPhase, InputKind, InputResponse,
            RoutedInput,
        },
        style::{Length, Style},
    };

    let open_host = host.clone();
    let open_thread = data.id.clone();
    let open = emit.clone();
    let content = UiBox::new()
        .layout("column")
        .gap(2.0)
        .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
        .child(visuals.title)
        .children(match layout {
            super::geometry::ThreadLayout::Grouped => None,
            super::geometry::ThreadLayout::Standalone => Some(
                UiBox::new()
                    .width(Length::Pct(100.0))
                    .gap(5.0)
                    .style(
                        Style::new()
                            .align(AlignItems::CENTER)
                            .min_width(Length::Px(0.0)),
                    )
                    .child(visuals.project_mark)
                    .child(visuals.subtitle)
                    .build(),
            ),
        });
    let mut root = Button::new()
        .width(Length::Pct(100.0))
        .height(layout.height())
        .gap(8.0)
        .radius(RadiusTokens::value(RadiusStep::Md))
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .flex_shrink(0.0)
                .min_width(Length::Px(0.0))
                .padding_insets(Insets {
                    left: 8.0,
                    right: 8.0,
                    top: 0.0,
                    bottom: 0.0,
                }),
        )
        .child(hover)
        .child(content.build());
    if let Some(dot) = visuals.dot {
        root = root.child(dot);
    }
    let pointer_host = host.clone();
    let pointer_thread = data.id.clone();
    let pointer_emit = emit.clone();
    let key_host = host.clone();
    let key_thread = data.id.clone();
    let key_emit = emit.clone();
    let hover_thread = data.id.clone();
    let hover_emit = emit.clone();
    let leave_thread = data.id.clone();
    let leave_emit = emit.clone();
    let pending = std::rc::Rc::new(std::cell::Cell::new(None::<(u64, aqua_core::Point)>));
    let down_pending = pending.clone();
    let move_pending = pending.clone();
    let up_pending = pending.clone();
    let cancel_pending = pending;
    let draggable = drag.is_some();
    let drag_emit = emit.clone();
    let drag_end = emit.clone();
    let drag_end_host = host.clone();
    let drag_end_thread = data.id.clone();
    let mut element = root
        .on_activate(move || {
            open(ViewAction::OpenThread {
                host: open_host.clone(),
                instance,
                thread: open_thread.clone(),
            })
        })
        .build()
        .with_key(format!("sidebar-thread-{}", data.id))
        .role(Role::Button)
        .aria_label(data.title.clone())
        .aria_selected(data.selected)
        .aria_description(format!(
            "{}; {}; Thread actions: Shift+F10",
            data.subtitle,
            state_label(data.state)
        ))
        .cursor(CursorIcon::Pointer)
        .on_input(InputKind::PointerEnter, EventPhase::Bubble, move |event| {
            hover_emit(ViewAction::ThreadHover {
                thread: hover_thread.clone(),
                bounds: event.bounds,
                hovered: true,
            });
            InputResponse::default()
        })
        .on_input(InputKind::PointerLeave, EventPhase::Bubble, move |event| {
            leave_emit(ViewAction::ThreadHover {
                thread: leave_thread.clone(),
                bounds: event.bounds,
                hovered: false,
            });
            InputResponse::default()
        })
        .on_input(InputKind::KeyDown, EventPhase::Bubble, move |event| {
            if let RoutedInput::Native(InputEvent::KeyDown(key)) = &event.input
                && opens_aqua_keyboard_menu(key)
            {
                key_emit(ViewAction::OpenThreadMenu {
                    host: key_host.clone(),
                    instance,
                    thread: key_thread.clone(),
                    position: aqua_core::Point::new(event.bounds.x, event.bounds.bottom()),
                });
                InputResponse {
                    stop_propagation: true,
                    ..InputResponse::handled()
                }
            } else {
                InputResponse::default()
            }
        })
        .on_input(
            InputKind::PointerDown,
            EventPhase::Bubble,
            move |event| match &event.input {
                RoutedInput::Native(InputEvent::PointerDown(pointer, PointerButton::Secondary)) => {
                    pointer_emit(ViewAction::OpenThreadMenu {
                        host: pointer_host.clone(),
                        instance,
                        thread: pointer_thread.clone(),
                        position: pointer.position,
                    });
                    InputResponse {
                        stop_propagation: true,
                        ..InputResponse::handled()
                    }
                }
                RoutedInput::Native(InputEvent::PointerDown(pointer, PointerButton::Primary)) => {
                    if !draggable {
                        return InputResponse::default();
                    }
                    down_pending.set(Some((pointer.contact, pointer.position)));
                    InputResponse {
                        pointer_capture: CaptureRequest::Capture,
                        ..InputResponse::default()
                    }
                }
                _ => InputResponse::default(),
            },
        )
        .on_input(InputKind::PointerUp, EventPhase::Bubble, move |_| {
            up_pending.set(None);
            InputResponse::default()
        })
        .on_input(InputKind::PointerCancel, EventPhase::Bubble, move |_| {
            cancel_pending.set(None);
            InputResponse::default()
        })
        .on_input(InputKind::DragEnd, EventPhase::Bubble, move |_| {
            drag_end(ViewAction::ThreadDragEnded {
                host: drag_end_host.clone(),
                instance,
                thread: drag_end_thread.clone(),
            });
            InputResponse::default()
        });
    if let Some(drag) = drag {
        element = element.on_input(InputKind::PointerMove, EventPhase::Bubble, move |event| {
            let Some((contact, origin)) = move_pending.get() else {
                return InputResponse::default();
            };
            let RoutedInput::Native(InputEvent::PointerMove(pointer)) = &event.input else {
                return InputResponse::default();
            };
            if pointer.contact != contact {
                return InputResponse::default();
            }
            let dx = pointer.position.x - origin.x;
            let dy = pointer.position.y - origin.y;
            if (dx * dx + dy * dy).sqrt() <= 2.0 {
                return InputResponse::default();
            }
            move_pending.set(None);
            drag_emit(ViewAction::ThreadDragStarted {
                payload: drag.clone(),
                instance,
                position: pointer.position,
            });
            InputResponse {
                drag: DragRequest::Start(DragPayload::new(drag.clone())),
                ..InputResponse::handled()
            }
        });
    }
    element
}

pub fn drag_preview(
    title: &str,
    theme: editor_ui::theme::ArtisanTheme,
    _viewport: aqua_core::Size,
) -> aqua_ui::element::Element {
    use editor_ui::{
        asset::source_asset_glyph,
        glass::{GlassStrength, glass_card_shadows, glass_layers},
        theme::{Elevation, RadiusStep, RadiusTokens},
    };
    use aqua_core::Insets;
    use aqua_ui::{
        AlignItems,
        element::{Box as UiBox, Text},
        style::{Length, Style},
    };
    use editor_assets::AssetId;
    let content = UiBox::new()
        .height(32.0)
        .gap(8.0)
        .style(
            Style::new()
                .max_width(Length::Px(260.0))
                .align(AlignItems::CENTER)
                .padding_insets(Insets {
                    left: 10.0,
                    right: 10.0,
                    top: 0.0,
                    bottom: 0.0,
                }),
        )
        .child(
            UiBox::new()
                .style(Style::new().flex_shrink(0.0))
                .child(source_asset_glyph(
                    AssetId::TABLER_MESSAGE_CIRCLE,
                    14.0,
                    theme.colors.text_muted.to_reference_paint(),
                ))
                .build(),
        )
        .child(
            UiBox::new()
                .style(Style::new().min_width(Length::Px(0.0)).flex_shrink(1.0))
                .child(
                    Text::new(title.to_owned())
                        .font_size(14.0)
                        .gpui_source_color(theme.colors.text.to_reference_text_color())
                        .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
                        .style(Style::new().min_width(Length::Px(0.0)).flex_shrink(1.0))
                        .build(),
                )
                .build(),
        )
        .build();
    let radius = RadiusTokens::value(RadiusStep::LgMid);
    UiBox::new()
        .radius(radius)
        .clip(true)
        .style(
            Style::new().shadows(
                glass_card_shadows(&theme, Elevation::Overlay)
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            ),
        )
        .child(glass_layers(&theme, GlassStrength::Strong, radius))
        .child(content)
        .build()
        .with_key("sidebar-thread-drag")
}
