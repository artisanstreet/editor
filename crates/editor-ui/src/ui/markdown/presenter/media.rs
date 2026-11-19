use super::{MarkdownPresenter, MediaServices, PresenterError, RetainedRich, enveloped};
use crate::ui::rich::{self};
use crate::ui::theme::ArtisanTheme;
use aqua_core::GpuiHsla;
use aqua_ui::element::Element;
use editor_assets::AssetId;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

impl MarkdownPresenter {
    pub(super) fn render_model(
        &self,
        model: &rich::media::model::ModelBlock,
        entry: &RetainedRich,
        envelope: &rich::Envelope,
        id: &str,
        theme: &ArtisanTheme,
        glyph: Rc<dyn Fn(AssetId, f32, GpuiHsla) -> Element>,
        invalidate: Rc<dyn Fn()>,
    ) -> Result<Element, PresenterError> {
        let media = self
            .media_services
            .borrow()
            .get(id)
            .cloned()
            .ok_or(PresenterError::Unavailable("measured media services"))?;
        if !media.scale.is_finite() || media.scale <= 0.0 {
            return Err(PresenterError::Unavailable("model device scale"));
        }
        let block_id = format!("{id}-rich-{}-model", entry.id);
        self.model_ids
            .borrow_mut()
            .entry(id.to_owned())
            .or_default()
            .insert(block_id.clone());
        let hovered = Rc::clone(
            self.model_hover
                .borrow_mut()
                .entry(block_id.clone())
                .or_insert_with(|| Rc::new(Cell::new(false))),
        );
        let services = rich::media::model::services(
            &block_id,
            *theme,
            media.model_assets.clone(),
            Rc::clone(&media.model_capability),
            Rc::clone(&glyph),
            Rc::clone(&media.effect),
            hovered,
            Rc::clone(&invalidate),
        );
        let content = model.view(
            &block_id,
            media.width,
            media.scale,
            Some(&media.fetcher),
            self.models.clone(),
            &services,
        );
        Ok(enveloped(theme, envelope, content))
    }

    pub(super) fn render_image(
        &self,
        model: &Arc<rich::media::image::ImageBlock>,
        entry: &RetainedRich,
        envelope: &rich::Envelope,
        id: &str,
        theme: &ArtisanTheme,
        invalidate: Rc<dyn Fn()>,
    ) -> Result<Element, PresenterError> {
        let services = self
            .media_services
            .borrow()
            .get(id)
            .cloned()
            .ok_or(PresenterError::Unavailable("measured media services"))?;
        let block_id = format!("{id}-rich-{}-image", entry.id);
        self.image_ids
            .borrow_mut()
            .entry(id.to_owned())
            .or_default()
            .insert(block_id.clone());
        let scope_key = format!(
            "{}:{}",
            services.fetcher.scope(),
            services.fetcher.asset_owner("image", false)
        );
        let state = {
            let mut states = self.images.borrow_mut();
            let state = states.entry(block_id.clone()).or_insert_with(|| {
                let state = rich::media::image::ImageState::new(
                    model,
                    services.fetcher.clone(),
                    services.images.clone(),
                );
                state.wake_mailbox().set_waker(Arc::clone(&services.wake));
                (
                    Arc::clone(model),
                    scope_key.clone(),
                    Rc::new(RefCell::new(state)),
                )
            });
            if !Arc::ptr_eq(&state.0, model) || state.1 != scope_key {
                let replacement = rich::media::image::ImageState::new(
                    model,
                    services.fetcher.clone(),
                    services.images.clone(),
                );
                replacement
                    .wake_mailbox()
                    .set_waker(Arc::clone(&services.wake));
                *state = (
                    Arc::clone(model),
                    scope_key,
                    Rc::new(RefCell::new(replacement)),
                );
            }
            state
                .2
                .borrow()
                .wake_mailbox()
                .set_waker(Arc::clone(&services.wake));
            Rc::clone(&state.2)
        };
        state.borrow_mut().refresh();
        let event_state = Rc::downgrade(&state);
        let event_id = block_id.clone();
        let effect = Rc::clone(&services.effect);
        let refresh = Rc::clone(&invalidate);
        let emit = Rc::new(move |action| {
            let Some(state) = event_state.upgrade() else {
                return;
            };
            match action {
                rich::media::image::Action::Open { index, image } => {
                    let (fetcher, sources) = state.borrow().preview_sources();
                    effect(crate::ui::media::preview::Effect::Open(
                        crate::ui::media::preview::OpenPreview {
                            fetcher,
                            sources,
                            index,
                            image,
                            opener_key: format!("{event_id}-image-{index}"),
                        },
                    ));
                }
                rich::media::image::Action::Retry(index) => {
                    if state.borrow_mut().retry(index) {
                        refresh();
                    }
                }
                rich::media::image::Action::AllowRemote => {
                    state.borrow_mut().allow_remote();
                    refresh();
                }
                rich::media::image::Action::CopyPath { index, position } => {
                    if let Some(source) = state.borrow().preview_sources().1.get(index) {
                        effect(crate::ui::media::preview::Effect::ShowCopyMenu(
                            rich::actions::ShowCopyMenu {
                                position,
                                entries: vec![rich::actions::CopyEntry::primary(
                                    "Copy Image Path",
                                    source.raw().to_owned(),
                                )],
                            },
                        ));
                    }
                }
            }
        });
        let content = state
            .borrow()
            .view(model, *theme, &block_id, services.width, emit)
            .map_err(PresenterError::Unavailable)?;
        Ok(enveloped(theme, envelope, content))
    }

    pub(super) fn render_audio(
        &self,
        block: &rich::media::audio::AudioBlock,
        entry: &RetainedRich,
        envelope: &rich::Envelope,
        id: &str,
        theme: &ArtisanTheme,
        glyph: Rc<dyn Fn(AssetId, f32, GpuiHsla) -> Element>,
        invalidate: Rc<dyn Fn()>,
    ) -> Result<Element, PresenterError> {
        let services = self
            .media_services
            .borrow()
            .get(id)
            .cloned()
            .ok_or(PresenterError::Unavailable("media services"))?;
        let block_id = format!("{id}-rich-{}-audio", entry.id);
        self.audio_ids
            .borrow_mut()
            .entry(id.to_owned())
            .or_default()
            .insert(block_id.clone());
        let content = block.view(
            &block_id,
            theme,
            Some(&services.fetcher),
            &services.audio_assets,
            &services.player,
            self.audio.clone(),
            Rc::clone(&glyph),
            Rc::clone(&invalidate),
        );
        Ok(enveloped(theme, envelope, content))
    }

    pub(super) fn render_image_compare(
        &self,
        model: &rich::media::image_compare::ImageCompare,
        entry: &RetainedRich,
        envelope: &rich::Envelope,
        id: &str,
        theme: &ArtisanTheme,
        invalidate: Rc<dyn Fn()>,
    ) -> Result<Element, PresenterError> {
        let services = self
            .media_services
            .borrow()
            .get(id)
            .cloned()
            .ok_or(PresenterError::Unavailable("measured media services"))?;
        let block_id = format!("{id}-rich-{}-image-compare", entry.id);
        self.image_comparison_ids
            .borrow_mut()
            .entry(id.to_owned())
            .or_default()
            .insert(block_id.clone());
        let scope_key = format!(
            "{}:{}",
            services.fetcher.scope(),
            services.fetcher.asset_owner("before", false)
        );
        let state = {
            let mut states = self.image_comparisons.borrow_mut();
            let entry = states.entry(block_id.clone()).or_insert_with(|| {
                let state = rich::media::image_compare::view::CompareState::new(
                    model,
                    services.fetcher.clone(),
                    services.images.clone(),
                );
                state.wake_mailbox().set_waker(Arc::clone(&services.wake));
                (scope_key.clone(), Rc::new(RefCell::new(state)))
            });
            let changed = entry.0 != scope_key;
            entry
                .1
                .borrow_mut()
                .reconcile(model, services.fetcher.clone(), changed);
            entry.0 = scope_key;
            entry
                .1
                .borrow()
                .wake_mailbox()
                .set_waker(Arc::clone(&services.wake));
            Rc::clone(&entry.1)
        };
        state.borrow_mut().refresh();
        let state_for_events = Rc::downgrade(&state);
        let refresh = Rc::clone(&invalidate);
        let emit = Rc::new(move |action| {
            if let Some(state) = state_for_events.upgrade()
                && state.borrow_mut().dispatch(action)
            {
                refresh();
            }
        });
        let content = state
            .borrow()
            .view(model, *theme, &block_id, services.width, emit)
            .map_err(PresenterError::Unavailable)?;
        Ok(enveloped(theme, envelope, content))
    }

    pub fn bind_media(&self, ctx: &aqua::AppCtx<'_>, window: aqua::prelude::AquaWindowId) {
        self.audio.bind(ctx, window);
        self.models.bind(ctx, window);
    }

    pub fn drain_media(&self) -> bool {
        let mut changed = self.audio.drain();
        if let Some(services) = self.media_services.borrow().values().next() {
            changed |= self.models.drain(&services.image_assets);
        }
        for (_, _, state) in self.images.borrow().values() {
            changed |= state.borrow_mut().refresh();
        }
        for (_, state) in self.image_comparisons.borrow().values() {
            changed |= state.borrow_mut().refresh();
        }
        if changed {
            let invalidations: Vec<_> = self
                .media_invalidations
                .borrow()
                .values()
                .cloned()
                .collect();
            for invalidate in invalidations {
                invalidate();
            }
        }
        changed
    }

    pub fn set_media_services(
        &self,
        owner: &str,
        services: MediaServices,
    ) -> Result<(), PresenterError> {
        if !services.width.is_finite() || services.width <= 0.0 {
            return Err(PresenterError::Unavailable("measured media width"));
        }
        self.media_services
            .borrow_mut()
            .insert(owner.to_owned(), services);
        Ok(())
    }
}
