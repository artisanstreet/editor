//! Visual-proof harness: mock transcript screen for screenshots.
//!
//! Renders two turns (user question + assistant markdown reply with a code
//! fence, then a follow-up) through the real ConversationSurface. No Forge,
//! no network: scene data is hardcoded. Run it, screenshot it, compare
//! against the old TS app. NOT shipped in any payload.

use artisan_domain::{ConversationLifecycle, TurnId};
use artisan_frontend::conversation_scene::{
    AssistantPhase, ConversationScene, SceneId, SceneItem, SceneItemKind, SceneTurn,
    TurnNarration, TurnNarrationEntry,
};
use artisan_frontend::conversation_surface::ConversationSurface;
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{
    App, Application, Bounds, Context, Entity, IntoElement, Render, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, size,
};
use std::borrow::Cow;

fn scene_id(value: &str) -> SceneId {
    SceneId::parse(value).expect("scene id is valid")
}

fn turn_id(value: &str) -> TurnId {
    TurnId::parse(value).expect("turn id is valid")
}

fn mock_scene() -> ConversationScene {
    let turn_a = SceneTurn::new(turn_id("turn_a"), 0, ConversationLifecycle::Completed);
    let turn_b = SceneTurn::new(turn_id("turn_b"), 1, ConversationLifecycle::Completed);
    let items = vec![
        SceneItem::new(
            scene_id("user-1"),
            turn_id("turn_a"),
            2,
            SceneItemKind::UserMessage {
                body: "How does the native port handle backdrop blur?".to_owned(),
            },
            None,
        )
        .expect("scene item"),
        SceneItem::new(
            scene_id("assistant-1"),
            turn_id("turn_a"),
            3,
            SceneItemKind::AssistantMessage {
                body: "With a dedicated scene primitive:\n\n```rust\nwindow.paint_backdrop_blur(bounds, corners, radius);\n```\n\nThe D3D11 pass snapshots the backdrop, runs dual-Kawase, and composites.".to_owned(),
                phase: AssistantPhase::Final,
            },
            None,
        )
        .expect("scene item"),
        SceneItem::new(
            scene_id("user-2"),
            turn_id("turn_b"),
            4,
            SceneItemKind::UserMessage {
                body: "And on macOS?".to_owned(),
            },
            None,
        )
        .expect("scene item"),
    ];
    let narration = vec![
        TurnNarrationEntry::new(turn_id("turn_a"), TurnNarration::Quiet),
        TurnNarrationEntry::new(turn_id("turn_b"), TurnNarration::Quiet),
    ];
    ConversationScene::build(vec![turn_a, turn_b], items, narration, Vec::new())
        .expect("mock scene")
}

struct TranscriptDemo {
    surface: Entity<ConversationSurface>,
}

impl Render for TranscriptDemo {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        div()
            .size_full()
            .bg(theme.colors.background.to_paint())
            .p(px(24.0))
            .child(self.surface.clone())
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        // Bundle the mono face: hosts without it installed must still render
        // code text instead of failing closed.
        cx.text_system()
            .add_fonts(vec![
                Cow::Borrowed(
                    include_bytes!("fonts/JetBrainsMono-Regular.ttf").as_slice(),
                ),
                Cow::Borrowed(include_bytes!("fonts/JetBrainsMono-Bold.ttf").as_slice()),
                Cow::Borrowed(include_bytes!("fonts/JetBrainsMono-Italic.ttf").as_slice()),
            ])
            .expect("bundled mono fonts must load");
        let bounds = Bounds::centered(None, size(px(1100.0), px(700.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx| {
                    let surface =
                        cx.new(|cx| ConversationSurface::new(mock_scene(), ThemeMode::Dark, cx));
                    TranscriptDemo { surface }
                })
            },
        )
        .unwrap();

        cx.activate(true);
    });
}
