//! Typed mounted collection owners. All screens use Core's same bounded native
//! viewport; this module chooses only canonical record labels and source fields.
use artisan_api::{Collection, CollectionRef, Item, ItemId, ItemKind, Question, TextContent};
use editor_core::HostData;
use editor_sources::{CollectionBrowser, CollectionPresenter, CollectionPreview};
use editor_ui::theme::ArtisanTheme;
use gpui::{
    App, AppContext as _, Entity, EntityId, IntoElement, ParentElement as _, RenderOnce,
    SharedString, WeakEntity, Window, div,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

type Owner<T> = Rc<RefCell<Option<(EntityId, ItemId, Entity<CollectionBrowser<T>>)>>>;
#[derive(Default)]
pub struct CollectionOwners {
    questions: Owner<Question>,
}
impl CollectionOwners {
    pub(crate) fn reconcile(&self, items: &HashMap<ItemId, Arc<Item>>, cx: &mut App) {
        fn sync<T: serde::de::DeserializeOwned + Send + 'static>(
            owner: &Owner<T>,
            items: &HashMap<ItemId, Arc<Item>>,
            source: impl Fn(&Item) -> Option<&CollectionRef>,
            cx: &mut App,
        ) {
            let mut retained = owner.borrow_mut();
            let Some((_, id, view)) = retained.as_ref() else {
                return;
            };
            let Some(source) = items.get(id).and_then(|item| source(item)) else {
                *retained = None;
                return;
            };
            view.update(cx, |view, cx| view.set_source(source.clone(), cx));
        }
        sync(
            &self.questions,
            items,
            |item| match &item.kind {
                ItemKind::Question {
                    questions: Collection::Stored(source),
                    ..
                } => Some(source),
                ItemKind::UserMessage { .. }
                | ItemKind::AssistantMessage { .. }
                | ItemKind::Tool { .. }
                | ItemKind::Agent { .. }
                | ItemKind::Question { .. }
                | ItemKind::Plan { .. }
                | ItemKind::Image { .. }
                | ItemKind::Compaction { .. }
                | ItemKind::Notice { .. }
                | ItemKind::Error { .. } => None,
            },
            cx,
        );
    }
    pub(crate) fn question(
        &self,
        id: ItemId,
        source: CollectionRef,
        host: Option<WeakEntity<HostData>>,
        theme: &ArtisanTheme,
    ) -> gpui::AnyElement {
        Mounted {
            owner: Rc::clone(&self.questions),
            id,
            source,
            host,
            theme: *theme,
            title: "Questions".into(),
            present: Rc::new(|question: &Question, index, _| CollectionPreview {
                label: format!("{}. {}", index + 1, preview(&question.text)).into(),
                detail: Some(question.text.clone()),
                selected: false,
                disabled: false,
            }),
        }
        .into_any_element()
    }
}
fn preview(text: &TextContent) -> String {
    match text {
        TextContent::Inline(text) => text.chars().take(160).collect(),
        TextContent::Stored(_) => "Read content".into(),
    }
}
#[derive(IntoElement)]
struct Mounted<T: serde::de::DeserializeOwned + Send + 'static> {
    owner: Owner<T>,
    id: ItemId,
    source: CollectionRef,
    host: Option<WeakEntity<HostData>>,
    theme: ArtisanTheme,
    title: SharedString,
    present: CollectionPresenter<T>,
}
impl<T: serde::de::DeserializeOwned + Send + 'static> RenderOnce for Mounted<T> {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(host) = self.host.and_then(|host| host.upgrade()) else {
            self.owner.borrow_mut().take();
            return div()
                .child("This Forge is no longer connected.")
                .into_any_element();
        };
        let mut owner = self.owner.borrow_mut();
        if let Some((host_id, item_id, view)) = owner.as_ref()
            && *host_id == host.entity_id()
            && *item_id == self.id
        {
            view.update(cx, |view, cx| {
                view.set_source(self.source, cx);
                view.set_theme(self.theme, cx);
            });
            return view.clone().into_any_element();
        }
        let view = cx.new(|cx| {
            CollectionBrowser::new(
                &host,
                self.source,
                self.title,
                self.theme,
                self.present,
                None,
                cx,
            )
        });
        *owner = Some((host.entity_id(), self.id, view.clone()));
        view.into_any_element()
    }
}
