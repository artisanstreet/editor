use aqua::UiApp;
use aqua_core::{
    InputEvent, Key, KeyEvent, Modifiers, NamedKey, NodeId, Point, PointerButton, PointerEvent,
    PointerKind, Rect, Role,
};
use aqua_text::document::{SelectionDocumentId, SelectionSourceId};
use aqua_ui::element::{Key as ElementKey, ui};
use aqua_ui::style::{Length, Style};
use aqua_ui::tree::{NodeKind, hash_key};
use editor_ui::markdown::presenter::{
    BodyServices, HeatmapServices, MarkdownPresenter, SelectionPlace,
};
use editor_ui::markdown::visual::{BodyContext, BodyTone};
use editor_ui::markdown::{GlyphFactory, MarkdownEngine};
use editor_ui::theme::{ArtisanTheme, ThemeMode};
use std::cell::Cell;
use std::rc::Rc;

pub const ITEM: &str = "item";

pub struct MountedBody {
    app: UiApp,
    engine: MarkdownEngine,
    presenter: MarkdownPresenter,
    services: HeatmapServices,
    invalidated: Rc<Cell<bool>>,
    mounted: bool,
}

impl std::fmt::Debug for MountedBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MountedBody").finish_non_exhaustive()
    }
}

#[must_use]
pub fn fence(kind: &str, json: &str) -> String {
    format!("```artisan-{kind}\n{json}\n```")
}

impl MountedBody {
    #[must_use]
    pub fn new(source: &str) -> Self {
        let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        let mut app = UiApp::new(aqua_text::SharedFonts::new(), aqua_text::SharedRuns::new());
        app.set_viewport(720.0, 900.0, 1.0);
        let mut body = Self {
            app,
            engine: MarkdownEngine::new().expect("the markdown engine starts"),
            presenter: MarkdownPresenter::default(),
            services: HeatmapServices::new(*theme),
            invalidated: Rc::new(Cell::new(false)),
            mounted: false,
        };
        body.edit(source);
        body
    }

    #[must_use]
    pub fn block(kind: &str, json: &str) -> Self {
        Self::new(&fence(kind, json))
    }

    pub fn edit(&mut self, source: &str) {
        self.presenter
            .prepare(&self.engine, source)
            .expect("the body prepares");
        self.render();
    }

    pub fn replace(&mut self, kind: &str, json: &str) {
        self.edit(&fence(kind, json));
    }

    #[must_use]
    pub fn presenter(&self) -> &MarkdownPresenter {
        &self.presenter
    }

    fn render(&mut self) {
        self.invalidated.set(false);
        let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        let glyph: GlyphFactory = Rc::new(|_, _, _| ui::Box::new().build());
        let flag = Rc::clone(&self.invalidated);
        let invalidate: Rc<dyn Fn()> = Rc::new(move || flag.set(true));
        let rows = self
            .presenter
            .projected_count()
            .expect("invariant: the body was prepared before it renders");
        let mut column = ui::Box::new()
            .layout("column")
            .style(Style::new().width(Length::Pct(100.0)));
        for row in 0..rows {
            let drawn = self
                .presenter
                .render_projected_with_code(
                    row,
                    ITEM,
                    BodyContext {
                        theme,
                        tone: BodyTone::Foreground,
                        on_link: None,
                        on_color: None,
                        scope: None,
                    },
                    SelectionPlace {
                        document: SelectionDocumentId(1),
                        order: &[0],
                    },
                    |index| SelectionSourceId(index as u64 + 1),
                    BodyServices {
                        glyph: &glyph,
                        invalidate: &invalidate,
                        charts: Some(&self.services),
                        code: None,
                    },
                )
                .unwrap_or_else(|error| panic!("row {row} renders: {error}"));
            column = column.child(drawn.element);
        }
        let element = column.build();
        if self.mounted {
            self.app
                .refresh(vec![element])
                .expect("refresh the rendered body");
        } else {
            self.app
                .mount(|_| element)
                .expect("mount the rendered body");
            self.mounted = true;
        }
        self.app.build_frame().expect("draw the rendered body");
    }

    fn settle(&mut self) {
        self.app.build_frame().expect("draw after input");
        if self.invalidated.get() {
            self.render();
        }
    }

    fn navigation(&self) -> NodeId {
        let tree = self.app.tree();
        tree.traverse()
            .into_iter()
            .find(|node| {
                tree.get(*node)
                    .is_some_and(|record| record.focusable && record.semantics.role == Role::Group)
            })
            .expect("a chart's keyboard navigation is mounted")
    }

    pub fn press(&mut self, key: NamedKey) {
        self.press_with(key, Modifiers::NONE);
    }

    pub fn press_alt(&mut self, key: NamedKey) {
        self.press_with(key, Modifiers::ALT);
    }

    pub fn press_on(&mut self, control: &str, key: NamedKey) {
        assert!(self.app.focus_key(control), "{control:?} takes focus");
        self.send(key, Modifiers::NONE);
    }

    fn press_with(&mut self, key: NamedKey, modifiers: Modifiers) {
        let navigation = self.navigation();
        self.app.tree_mut().set_focused(Some(navigation));
        self.send(key, modifiers);
    }

    fn send(&mut self, key: NamedKey, modifiers: Modifiers) {
        let event = KeyEvent {
            key: Key::Named(key),
            modifiers,
            repeat: false,
        };
        self.app.handle_input(&InputEvent::KeyDown(event));
        self.app.handle_input(&InputEvent::KeyUp(event));
        self.settle();
    }

    #[must_use]
    pub fn label(&self) -> String {
        let navigation = self.navigation();
        self.app
            .tree()
            .get(navigation)
            .map(|record| record.semantics.label.clone())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn texts(&self) -> Vec<String> {
        super::ui::painted_texts(&self.app)
    }

    #[must_use]
    pub fn shows(&self, text: &str) -> bool {
        self.texts().iter().any(|painted| painted == text)
    }

    #[must_use]
    pub fn text_rect(&self, text: &str) -> Rect {
        let tree = self.app.tree();
        let node = tree
            .traverse()
            .into_iter()
            .find(|node| {
                tree.get(*node).is_some_and(
                    |record| matches!(&record.kind, NodeKind::Text(content) if content.text() == text),
                )
            })
            .unwrap_or_else(|| panic!("{text:?} is not painted"));
        tree.visual_rect(node)
            .unwrap_or_else(|| panic!("{text:?} is not laid out"))
    }

    #[must_use]
    pub fn keyed(&self, key: &str) -> Option<Rect> {
        let node = self
            .app
            .tree()
            .last_node_with_key(hash_key(ElementKey::from(key)))?;
        self.app.tree().visual_rect(node)
    }

    #[must_use]
    pub fn rect(&self, key: &str) -> Rect {
        self.keyed(key)
            .unwrap_or_else(|| panic!("no mounted node is keyed {key:?}"))
    }

    #[must_use]
    pub fn block_id(&self, kind: &str) -> String {
        (0..64)
            .map(|index| format!("{ITEM}-rich-{index}-{kind}"))
            .find(|id| self.keyed(&format!("{id}-navigation")).is_some())
            .unwrap_or_else(|| panic!("no {kind} block is mounted"))
    }

    pub fn move_to(&mut self, x: f32, y: f32) {
        self.pointer(&InputEvent::PointerMove(pointer(x, y, 0)));
    }

    pub fn move_within(&mut self, rect: Rect, fx: f32, fy: f32) {
        self.move_to(rect.w.mul_add(fx, rect.x), rect.h.mul_add(fy, rect.y));
    }

    pub fn drag(&mut self, from: (f32, f32), to: (f32, f32)) {
        self.pointer(&InputEvent::PointerMove(pointer(from.0, from.1, 0)));
        self.pointer(&InputEvent::PointerDown(
            pointer(from.0, from.1, 1),
            PointerButton::Primary,
        ));
        self.pointer(&InputEvent::PointerMove(pointer(to.0, to.1, 1)));
        self.pointer(&InputEvent::PointerUp(
            pointer(to.0, to.1, 0),
            PointerButton::Primary,
        ));
    }

    pub fn click_at(&mut self, x: f32, y: f32) {
        self.drag((x, y), (x, y));
    }

    pub fn click(&mut self, key: &str) {
        let rect = self.rect(key);
        self.click_at(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
    }

    fn pointer(&mut self, event: &InputEvent) {
        self.app.handle_input(event);
        self.settle();
    }
}

fn pointer(x: f32, y: f32, buttons: u8) -> PointerEvent {
    PointerEvent {
        contact: 1,
        kind: PointerKind::Mouse,
        position: Point::new(x, y),
        buttons,
    }
}
