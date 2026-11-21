use aqua_core::{InputEvent, Key, Modifiers, NamedKey, Role};
use aqua_ui::element::Element;
use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput};

pub(crate) fn navigation_group(
    content: Element,
    id: &str,
    label: String,
    value: Option<String>,
    handle: impl Fn(NamedKey, bool) -> bool + 'static,
) -> Element {
    let group = content
        .with_key(format!("{id}-navigation"))
        .role(Role::Group)
        .focusable(true)
        .tab_index(0)
        .aria_label(label)
        .on_input(InputKind::KeyDown, EventPhase::Bubble, move |event| {
            let RoutedInput::Native(InputEvent::KeyDown(key_event)) = &event.input else {
                return InputResponse::default();
            };
            let Key::Named(code) = key_event.key else {
                return InputResponse::default();
            };
            if !handle(code, key_event.modifiers.contains(Modifiers::ALT)) {
                return InputResponse::default();
            }
            InputResponse {
                stop_propagation: true,
                ..InputResponse::default()
            }
        });
    match value {
        Some(value) => group.aria_value(value),
        None => group,
    }
}
