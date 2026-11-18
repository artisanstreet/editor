use super::{
    ComposerOwner, MODEL_EDGE_BOTTOM, MODEL_EDGE_TOP, PROJECT_EDGE_BOTTOM, PROJECT_EDGE_TOP,
    PROJECT_SCROLL, PROMPT_EDGE_BOTTOM, PROMPT_EDGE_TOP, PROMPT_SCROLL,
};
use editor_ui::theme::ArtisanTheme;

impl ComposerOwner {
    pub fn needs_animation(&self, app: &aqua::UiApp) -> bool {
        self.strip_moving.get()
            || self
                .primary_swap
                .get()
                .progress(std::time::Instant::now())
                .is_some()
            || self.controller.borrow().cleared.is_some()
            || self.note_ticker.borrow().is_animating()
            || (self.controller.borrow().picker.menu == Some(super::super::picker::Menu::Model)
                && (self.engine_light.borrow().animating()
                    || !self.model_tab.borrow().settled()
                    || self.model_scroll.borrow().jumping()))
            || self
                .model_popup
                .as_ref()
                .is_some_and(|popup| popup.is_animating(app))
            || self
                .mention_popup
                .as_ref()
                .is_some_and(|popup| popup.is_animating(app))
            || self
                .project_popup
                .as_ref()
                .is_some_and(|popup| popup.is_animating(app))
    }

    pub fn advance_edges(
        &self,
        app: &mut aqua::UiApp,
        theme: &ArtisanTheme,
        reduced: bool,
        now: std::time::Instant,
    ) -> bool {
        let prompt = self.prompt_edges(theme).advance(app, reduced, now);
        prompt | self.advance_picker_edges(app, theme, reduced, now)
    }

    pub(super) fn prompt_edges(&self, theme: &ArtisanTheme) -> editor_ui::scroll_edge::EdgeBands {
        editor_ui::scroll_edge::EdgeBands {
            scroll: PROMPT_SCROLL,
            top: PROMPT_EDGE_TOP,
            bottom: PROMPT_EDGE_BOTTOM,
            color: theme.colors.surface,
            edges: self.prompt_edges.clone(),
        }
    }

    pub(super) fn advance_picker_edges(
        &self,
        app: &mut aqua::UiApp,
        theme: &ArtisanTheme,
        reduced: bool,
        now: std::time::Instant,
    ) -> bool {
        let controller = self.controller.borrow();
        let picker = &controller.picker;
        match picker.menu {
            Some(super::super::picker::Menu::Project) => editor_ui::scroll_edge::EdgeSurface {
                edges: &picker.project_edges,
                scroller: PROJECT_SCROLL,
                top: PROJECT_EDGE_TOP,
                bottom: PROJECT_EDGE_BOTTOM,
                top_color: theme.colors.gray.step(7),
                bottom_color: theme.colors.gray.step(4),
                paint: editor_ui::scroll_edge::EdgePaint::Gradient,
            }
            .advance(app, reduced, now),
            Some(super::super::picker::Menu::Model) if picker.axis.is_none() => {
                editor_ui::scroll_edge::EdgeSurface {
                    edges: &picker.model_edges,
                    scroller: editor_ui::scroll::MODEL_MENU,
                    top: MODEL_EDGE_TOP,
                    bottom: MODEL_EDGE_BOTTOM,
                    top_color: theme.colors.background,
                    bottom_color: theme.colors.background,
                    paint: editor_ui::scroll_edge::EdgePaint::InsetShadow,
                }
                .advance(app, reduced, now)
            }
            _ => false,
        }
    }

    pub fn advance_primary(&self, app: &mut aqua::UiApp, now: std::time::Instant) {
        let mut swap = self.primary_swap.get();
        if let Some(frame) = swap.progress(now) {
            super::super::swap::advance(app, Some(frame));
        } else if swap.settle(now) {
            super::super::swap::advance(app, None);
        }
        self.primary_swap.set(swap);
        let cleared = self.controller.borrow().cleared;
        if let Some(cleared) = cleared {
            match super::super::swap::settle_progress(cleared, now).filter(|_| !self.reduced_motion)
            {
                Some(progress) => super::super::swap::advance_prompt(app, progress),
                None => {
                    super::super::swap::advance_prompt(app, 1.0);
                    self.controller.borrow_mut().cleared = None;
                }
            }
        }
    }

    pub fn advance_note_ticker(&self, app: &mut aqua::UiApp, now: f64) {
        let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from("composer-note-ticker"));
        let node = app.tree().nodes_with_key(key).into_iter().next();
        if let Some(node) = node {
            self.note_ticker
                .borrow_mut()
                .advance(app, node, now, self.reduced_motion);
        } else {
            *self.note_ticker.borrow_mut() =
                editor_ui::marquee::Marquee::new("composer-note-ticker", "");
        }
    }
}
