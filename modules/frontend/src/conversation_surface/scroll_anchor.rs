//! Rendered scroll-anchor registry and transcript viewport geometry for
//! [`ConversationSurface`].
//!
//! Extracted verbatim from `conversation_surface.rs` during the phase-3 module
//! split; visibility was widened to `pub(super)` for parent- and sibling-owned
//! access.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ViewportGeometry {
    pub(super) scroll_top: f64,
    pub(super) viewport_height: f64,
    pub(super) scroll_height: f64,
}

pub(super) struct RenderedScrollAnchor {
    scene_id: Option<SceneId>,
    item_id: Option<ItemId>,
    pub(super) anchor: ScrollAnchor,
    pub(super) painted: bool,
}

impl RenderedScrollAnchor {
    pub(super) fn matches(&self, target: &ConversationSurfaceTarget) -> bool {
        match target {
            ConversationSurfaceTarget::Scene(scene_id) => self.scene_id.as_ref() == Some(scene_id),
            ConversationSurfaceTarget::Item(item_id) => self.item_id.as_ref() == Some(item_id),
        }
    }

    fn has_identity(&self) -> bool {
        self.scene_id.is_some() || self.item_id.is_some()
    }

    fn same_identity(&self, scene_id: Option<&SceneId>, item_id: Option<&ItemId>) -> bool {
        self.has_identity()
            && self.scene_id.as_ref() == scene_id
            && self.item_id.as_ref() == item_id
    }
}

const SCROLL_ANCHOR_ELEMENT_PREFIX: &str = "artisan-conversation-scroll-anchor";

pub(super) struct ScrollAnchorRegistry<'a> {
    pub(super) handle: &'a ScrollHandle,
    pub(super) previous: &'a [RenderedScrollAnchor],
    pub(super) next_element_id: usize,
    pub(super) rendered: &'a mut Vec<RenderedScrollAnchor>,
}

impl ScrollAnchorRegistry<'_> {
    pub(super) fn attach(
        &mut self,
        element: Div,
        scene_id: Option<&SceneId>,
        item_id: Option<&ItemId>,
    ) -> Stateful<Div> {
        let scene_id = scene_id.cloned();
        let item_id = item_id.cloned();
        let previous = self
            .previous
            .iter()
            .find(|rendered| rendered.same_identity(scene_id.as_ref(), item_id.as_ref()));
        let (anchor, painted) = previous.map_or_else(
            || (ScrollAnchor::for_handle(self.handle.clone()), false),
            |rendered| (rendered.anchor.clone(), rendered.painted),
        );
        self.rendered.push(RenderedScrollAnchor {
            scene_id: scene_id.clone(),
            item_id: item_id.clone(),
            anchor: anchor.clone(),
            painted,
        });
        let element_id = self.next_element_id;
        self.next_element_id = self
            .next_element_id
            .checked_add(1)
            .expect("conversation scroll anchor element id space exhausted");
        let internal_id = ElementId::named_usize(SCROLL_ANCHOR_ELEMENT_PREFIX, element_id);
        element.id(internal_id).anchor_scroll(Some(anchor))
    }

    pub(super) fn anchor_for_item(&self, item_id: &ItemId) -> Option<(ScrollAnchor, bool)> {
        self.rendered
            .iter()
            .find(|rendered| rendered.item_id.as_ref() == Some(item_id))
            .map(|rendered| (rendered.anchor.clone(), rendered.painted))
    }

    pub(super) fn register_item_alias(
        &mut self,
        item_id: ItemId,
        anchor: ScrollAnchor,
        painted: bool,
    ) {
        self.rendered.push(RenderedScrollAnchor {
            scene_id: None,
            item_id: Some(item_id),
            anchor,
            painted,
        });
    }
}
