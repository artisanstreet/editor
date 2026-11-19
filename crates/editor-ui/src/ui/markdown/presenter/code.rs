use super::{
    CodeServices, MarkdownPresenter, PresenterError, ReferenceServices, RetainedRich, enveloped,
};
use crate::ui::kit::{Space, Spaced, TextRole};
use crate::ui::rich::{self};
use crate::ui::theme::ArtisanTheme;
use aqua_text::document::SelectionSource;
use aqua_ui::element::{Element, ui};
use aqua_ui::style::Length;
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn code_card(
    fence: &super::super::fence::Fence,
    id: &str,
    theme: &ArtisanTheme,
    document: aqua_text::document::SelectionDocumentId,
    sources: &[SelectionSource],
    code: Option<&CodeServices>,
    on_color: Option<Rc<dyn Fn(rich::actions::ShowCopyMenu)>>,
    note: Option<&str>,
) -> Result<Element, PresenterError> {
    let services = code.ok_or(PresenterError::Unavailable("code copy services"))?;
    let card = fence.render(
        id,
        theme,
        document,
        sources,
        services.copy_enabled.then(|| Rc::clone(&services.copy)),
        on_color,
    );
    Ok(if let Some(note) = note {
        ui::Box::new()
            .layout("column")
            .width(Length::Pct(100.0))
            .spaced(Space::S6)
            .child(card)
            .child(rich::narrative::callout::text(
                theme,
                note,
                TextRole::Label,
                theme.colors.text_muted,
            ))
            .build()
    } else {
        card
    })
}

fn reference_services<'a>(
    code: Option<&'a CodeServices>,
    scope: Option<&rich::actions::ResourceScope>,
) -> Result<(&'a CodeServices, &'a ReferenceServices), PresenterError> {
    let services = code.ok_or(PresenterError::Unavailable("code services"))?;
    let reference = services
        .reference
        .as_ref()
        .ok_or(PresenterError::Unavailable("reference services"))?;
    if scope.is_some() && reference.instance.is_none() {
        return Err(PresenterError::Unavailable("reference host instance"));
    }
    Ok((services, reference))
}

fn code_envelope(
    theme: &ArtisanTheme,
    envelope: &rich::Envelope,
    content: Element,
    services: &CodeServices,
    block_id: &str,
    copy_text: &str,
) -> Element {
    let content = rich::code::card::envelope(
        theme,
        envelope.title.as_deref(),
        envelope.subtitle.as_deref(),
        envelope.caption.as_deref(),
        content,
    );
    let copy = services.copy_enabled.then(|| {
        (
            (services.copy)(&format!("{block_id}-copy"), copy_text),
            rich::code::card::CopyPlacement::TopRight,
        )
    });
    rich::code::card::render(theme, content, copy, false)
}

impl MarkdownPresenter {
    pub(super) fn render_annotated_code(
        &self,
        block: &rich::code::annotated::AnnotatedCode,
        entry: &RetainedRich,
        envelope: &rich::Envelope,
        id: &str,
        theme: &ArtisanTheme,
        invalidate: Rc<dyn Fn()>,
        code: Option<&CodeServices>,
        reference_scope: Option<rich::actions::ResourceScope>,
    ) -> Result<Element, PresenterError> {
        let (services, reference) = reference_services(code, reference_scope.as_ref())?;
        let block_id = format!("{id}-rich-{}-annotated", entry.id);
        self.annotated_ids
            .borrow_mut()
            .entry(id.to_owned())
            .or_default()
            .insert(block_id.clone());
        let state = Rc::clone(
            self.annotated
                .borrow_mut()
                .entry(block_id.clone())
                .or_insert_with(|| {
                    Rc::new(RefCell::new(
                        rich::code::annotated::view::AnnotatedState::new(block),
                    ))
                }),
        );
        let content = block.render(
            theme,
            &block_id,
            reference_scope,
            reference.instance,
            Rc::clone(&reference.open),
            state,
            Rc::clone(&invalidate),
        );
        Ok(code_envelope(
            theme,
            envelope,
            content,
            services,
            &block_id,
            &block.copy(),
        ))
    }

    pub(super) fn render_file_tree(
        &self,
        tree: &rich::code::file_tree::FileTree,
        entry: &RetainedRich,
        envelope: &rich::Envelope,
        id: &str,
        theme: &ArtisanTheme,
        invalidate: Rc<dyn Fn()>,
        code: Option<&CodeServices>,
        reference_scope: Option<rich::actions::ResourceScope>,
    ) -> Result<Element, PresenterError> {
        let (_, reference) = reference_services(code, reference_scope.as_ref())?;
        let block_id = format!("{id}-rich-{}-file-tree", entry.id);
        self.file_tree_ids
            .borrow_mut()
            .entry(id.to_owned())
            .or_default()
            .insert(block_id.clone());
        let state = Rc::clone(
            self.file_trees
                .borrow_mut()
                .entry(block_id.clone())
                .or_default(),
        );
        if state.borrow_mut().reconcile(tree) {
            self.file_tree_focus.borrow_mut().remove(&block_id);
        }
        let tree_for_events = tree.clone();
        let state_for_events = Rc::clone(&state);
        let event_id = block_id.clone();
        let pending = Rc::clone(&self.file_tree_focus);
        let open = Rc::clone(&reference.open);
        let instance = reference.instance;
        let refresh = Rc::clone(&invalidate);
        let emit = Rc::new(move |action| {
            let effect = state_for_events.borrow_mut().dispatch(
                &tree_for_events,
                &event_id,
                reference_scope.clone(),
                action,
            );
            if let Some(focus) = effect.focus {
                pending.borrow_mut().insert(event_id.clone(), focus);
            }
            if let Some(request) = effect.open {
                open(request, instance);
            }
            if effect.changed || pending.borrow().contains_key(&event_id) {
                refresh();
            }
        });
        let content = tree.render(theme, &block_id, state, emit);
        Ok(enveloped(theme, envelope, content))
    }

    pub(super) fn render_code_ref(
        &self,
        reference: &rich::code::code_ref::CodeRefBlock,
        entry: &RetainedRich,
        envelope: &rich::Envelope,
        id: &str,
        theme: &ArtisanTheme,
        code: Option<&CodeServices>,
        reference_scope: Option<rich::actions::ResourceScope>,
    ) -> Result<Element, PresenterError> {
        let (services, open) = reference_services(code, reference_scope.as_ref())?;
        let block_id = format!("{id}-rich-{}-code-ref", entry.id);
        let content = reference.render(
            theme,
            &block_id,
            reference_scope,
            open.instance,
            Rc::clone(&open.open),
        );
        Ok(code_envelope(
            theme,
            envelope,
            content,
            services,
            &block_id,
            &reference.copy(),
        ))
    }

    pub(super) fn remove_file_tree(&self, id: &str) {
        if let Some(state) = self.file_trees.borrow_mut().remove(id) {
            state.borrow_mut().deactivate();
        }
        self.file_tree_focus.borrow_mut().remove(id);
    }

    pub fn file_tree_focus_requests(&self, owner: &str) -> Vec<(String, String, String, usize)> {
        let ids = self.file_tree_ids.borrow();
        let pending = self.file_tree_focus.borrow();
        ids.get(owner)
            .into_iter()
            .flatten()
            .filter_map(|block| {
                let (collection, row, index) = pending.get(block)?;
                Some((block.clone(), collection.clone(), row.clone(), *index))
            })
            .collect()
    }

    pub fn file_tree_focused(&self, block: &str, collection: &str, row: &str, index: usize) {
        let mut pending = self.file_tree_focus.borrow_mut();
        if pending
            .get(block)
            .is_some_and(|(c, r, i)| c == collection && r == row && *i == index)
        {
            pending.remove(block);
        }
    }

    pub fn code_services(
        &self,
        owner: &str,
        theme: ArtisanTheme,
        reduced_motion: bool,
        effect: Rc<dyn Fn(crate::ui::code_copy::CopyEffect)>,
        copy_enabled: bool,
        reference: Option<ReferenceServices>,
    ) -> CodeServices {
        let states = Rc::clone(&self.copies);
        let owner = owner.to_owned();
        CodeServices {
            copy_enabled,
            reference,
            copy: Rc::new(move |id, source| {
                let controller = states
                    .borrow_mut()
                    .entry(owner.clone())
                    .or_default()
                    .entry(id.to_owned())
                    .or_default()
                    .clone();
                let now = std::time::Instant::now();
                if controller.frame(now, reduced_motion).animating {
                    effect(crate::ui::code_copy::CopyEffect::RequestFrame);
                }
                let emit = Rc::clone(&effect);
                crate::ui::code_copy::CodeCopyButton::new(
                    id.to_owned(),
                    source.to_owned(),
                    theme,
                    controller,
                    reduced_motion,
                    move |event| emit(event),
                )
                .build(now)
            }),
        }
    }
}
