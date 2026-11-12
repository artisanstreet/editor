use super::{Highlight, Hover};
use crate::ui::theme::ArtisanTheme;
use aqua_core::{NodeId, Transform2D};
use aqua_ui::element::{Box as UiBox, Element, Key};
use aqua_ui::input::{EventPhase, InputKind, InputResponse};
use aqua_ui::style::{Length, Style};
use aqua_ui::tree::{KeyHash, hash_key};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

const LAYER: &str = "kit-hover-layer";
const SELECTED: &str = "kit-hover-layer-selected";

thread_local! {
    static LIVE: RefCell<HashMap<NodeId, Highlight>> = RefCell::default();
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Frame {
    pub changed: bool,
    pub animating: bool,
}

fn key_of(name: &str) -> KeyHash {
    hash_key(Key::from(name))
}

fn pose(highlight: Highlight, now: Instant, height: f32) -> (f32, Transform2D) {
    let scale = highlight.scale(now);
    (
        highlight.opacity(now),
        Transform2D::scale(1.0, scale).then(Transform2D::translate(0.0, height * (1.0 - scale))),
    )
}

fn light(owner: NodeId, hovered: bool) {
    let now = Instant::now();
    LIVE.with(|live| {
        let mut live = live.borrow_mut();
        let highlight = live.get(&owner).copied().unwrap_or_default();
        live.insert(owner, highlight.light(hovered, now, false));
    });
}

#[must_use]
pub fn layer(hover: Hover, theme: &ArtisanTheme, radius: f32, selected: bool) -> Element {
    let (opacity, transform) = match selected {
        true => (1.0, Transform2D::IDENTITY),
        false => pose(Highlight::default(), Instant::now(), 0.0),
    };
    UiBox::new()
        .style(hover.style(theme))
        .style(
            Style::new()
                .absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .radius(Length::Px(radius))
                .opacity(opacity)
                .transform(transform),
        )
        .build()
        .with_key(if selected { SELECTED } else { LAYER })
        .exclude_from_hit_testing()
}

#[must_use]
pub fn track(element: Element) -> Element {
    element
        .on_input(InputKind::PointerEnter, EventPhase::Bubble, |event| {
            light(event.current_target, true);
            InputResponse::default()
        })
        .on_input(InputKind::PointerLeave, EventPhase::Bubble, |event| {
            light(event.current_target, false);
            InputResponse::default()
        })
}

fn layer_of(app: &aqua::UiApp, owner: NodeId) -> Option<NodeId> {
    let tree = app.tree();
    let key = key_of(LAYER);
    tree.get(owner)?.children.iter().copied().find(|child| {
        tree.get(*child)
            .is_some_and(|record| record.key == Some(key))
    })
}

pub fn advance(app: &mut aqua::UiApp, now: Instant, reduced: bool) -> Frame {
    let entries: Vec<(NodeId, Highlight)> = LIVE.with(|live| {
        live.borrow()
            .iter()
            .map(|(owner, lit)| (*owner, *lit))
            .collect()
    });
    let mut frame = Frame::default();
    let mut kept = HashMap::new();
    for (owner, highlight) in entries {
        let highlight = match reduced {
            true => Highlight::default().light(highlight.lit(), now, true),
            false => highlight.settle(now),
        };
        let Some(node) = layer_of(app, owner) else {
            continue;
        };
        if let Some(mut paint) = app.tree().paint(node) {
            let height = app.tree().rect(node).map_or(0.0, |rect| rect.h);
            let (opacity, transform) = pose(highlight, now, height);
            paint.opacity = Some(opacity);
            paint.transform = Some(transform);
            frame.changed |= app.tree_mut().set_paint(node, paint);
        }
        frame.animating |= highlight.running();
        if highlight.lit() || highlight.running() {
            kept.insert(owner, highlight);
        }
    }
    LIVE.with(|live| *live.borrow_mut() = kept);
    frame
}
