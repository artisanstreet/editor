//! Shared file-tree rows own disclosure, focus, keyboard commands and row geometry.
use crate::{
    asset_seam::asset_glyph,
    theme::{ArtisanTheme, RadiusStep, RadiusTokens},
};
use editor_assets::AssetId;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

/// The tree model resolves commands against its visible node order.
#[derive(Clone, Copy)]
pub enum TreeNavigation {
    Previous,
    Next,
    First,
    Last,
    Parent,
}
type Activate = Rc<dyn Fn(&mut Window, &mut App)>;
type Expand = Rc<dyn Fn(bool, &mut Window, &mut App)>;
type Navigate = Box<dyn Fn(TreeNavigation, &mut Window, &mut App)>;
/// Retain with the tree state, then reconcile each native viewport batch.
/// Only visible handles and the current keyboard target survive scrolling.
#[derive(Clone, Default)]
pub struct TreeFocus(Rc<RefCell<HashMap<SharedString, FocusHandle>>>);
pub struct TreeNode {
    id: SharedString,
    focus: TreeFocus,
}
impl TreeFocus {
    pub fn node(&self, id: impl Into<SharedString>) -> TreeNode {
        TreeNode {
            id: id.into(),
            focus: self.clone(),
        }
    }
    fn handle(&self, id: SharedString, cx: &mut App) -> FocusHandle {
        self.0
            .borrow_mut()
            .entry(id)
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }
    /// Call once per native row batch, before rendering its rows. Preserve the
    /// focused target while scrolling it into view, even across measurement passes.
    pub fn retain_visible(&self, visible: impl IntoIterator<Item = SharedString>, window: &Window) {
        let visible: HashSet<_> = visible.into_iter().collect();
        self.0
            .borrow_mut()
            .retain(|id, handle| visible.contains(id) || handle.is_focused(window));
    }
    /// A new prepared tree owns a new node set. Drop prior handles and clear
    /// focus only when this tree owns it.
    pub fn reset(&self, window: &mut Window, cx: &mut App) {
        if self
            .0
            .borrow()
            .values()
            .any(|handle| handle.is_focused(window))
        {
            window.blur(cx);
        }
        self.0.borrow_mut().clear();
    }
    pub fn focus(&self, id: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
        window.focus(&self.handle(id.into(), cx), cx);
    }
}
enum Kind {
    Branch {
        expanded: bool,
        expand: Option<Expand>,
    },
    Leaf {
        activate: Option<Activate>,
    },
}

#[derive(IntoElement)]
pub struct TreeRow {
    node: TreeNode,
    theme: ArtisanTheme,
    depth: usize,
    label: SharedString,
    accessible_label: SharedString,
    kind: Kind,
    leading: Option<AnyElement>,
    deleted: bool,
    trailing: Option<AnyElement>,
    description: Option<SharedString>,
    navigate: Option<Navigate>,
    debug_selector: Option<SharedString>,
}
impl TreeRow {
    pub fn row_height() -> gpui::Pixels {
        px(26.0)
    }
    pub fn branch(
        node: TreeNode,
        theme: ArtisanTheme,
        depth: usize,
        label: impl Into<SharedString>,
        expanded: bool,
        expand: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        let label = label.into();
        Self {
            node,
            theme,
            depth: depth.min(32),
            accessible_label: label.clone(),
            label,
            kind: Kind::Branch {
                expanded,
                expand: Some(Rc::new(expand)),
            },
            leading: None,
            deleted: false,
            trailing: None,
            description: None,
            navigate: None,
            debug_selector: None,
        }
    }
    pub fn leaf(
        node: TreeNode,
        theme: ArtisanTheme,
        depth: usize,
        label: impl Into<SharedString>,
        accessible_label: impl Into<SharedString>,
        activate: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            node,
            theme,
            depth: depth.min(32),
            label: label.into(),
            accessible_label: accessible_label.into(),
            kind: Kind::Leaf {
                activate: Some(Rc::new(activate)),
            },
            leading: None,
            deleted: false,
            trailing: None,
            description: None,
            navigate: None,
            debug_selector: None,
        }
    }
    #[must_use]
    pub fn leading(mut self, leading: impl IntoElement) -> Self {
        self.leading = Some(leading.into_any_element());
        self
    }
    #[must_use]
    pub fn deleted(mut self, deleted: bool) -> Self {
        self.deleted = deleted;
        self
    }
    #[must_use]
    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }
    #[must_use]
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }
    #[must_use]
    pub fn on_navigate(
        mut self,
        navigate: impl Fn(TreeNavigation, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.navigate = Some(Box::new(navigate));
        self
    }
}
impl RenderOnce for TreeRow {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let handle = self.node.focus.handle(self.node.id.clone(), cx);
        let (expanded, leading, activate, expand) = match self.kind {
            Kind::Branch { expanded, expand } => (
                Some(expanded),
                asset_glyph(if expanded {
                    AssetId::TABLER_FOLDER_OPEN
                } else {
                    AssetId::TABLER_FOLDER
                })
                .size(px(14.0))
                .text_color(theme.colors.muted_foreground.to_paint())
                .into_any_element(),
                None,
                expand,
            ),
            Kind::Leaf { activate } => (
                None,
                asset_glyph(AssetId::TABLER_FILE_TEXT)
                    .size(px(14.0))
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .into_any_element(),
                activate,
                None,
            ),
        };
        let leading = self.leading.unwrap_or(leading);
        let deleted = self.deleted;
        let click_activate = activate.clone();
        let click_expand = expand.clone();
        let navigate = self.navigate;
        #[expect(
            clippy::cast_precision_loss,
            reason = "indent is bounded to the supported tree depth"
        )]
        let indent = px(4.0 + self.depth.min(32) as f32 * 16.0);
        let mut root = div()
            .id(self.node.id)
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(6.0))
            .h(Self::row_height())
            .pl(indent)
            .pr(px(8.0))
            .rounded(RadiusTokens::value(RadiusStep::Sm))
            .border_1()
            .border_color(gpui::transparent_black())
            .role(gpui::Role::TreeItem)
            .aria_label(self.accessible_label)
            .aria_level(self.depth.saturating_add(1))
            .track_focus(&handle)
            .tab_index(0)
            .cursor_pointer()
            .focus_visible(move |style| style.border_color(theme.colors.ring.to_paint()))
            .hover(move |style| {
                crate::hover_card::hover_fill_style(
                    style,
                    &theme,
                    RadiusTokens::value(RadiusStep::Sm),
                )
            });
        if let Some(selector) = self.debug_selector {
            root = root.debug_selector(move || selector.to_string());
        }
        if let Some(expanded) = expanded {
            root = root.aria_expanded(expanded);
        }
        root.child(
            div()
                .w(px(12.0))
                .flex_shrink_0()
                .children(expanded.map(|expanded| {
                    asset_glyph(if expanded {
                        AssetId::TABLER_CHEVRON_DOWN
                    } else {
                        AssetId::TABLER_CHEVRON_RIGHT
                    })
                    .size(px(12.0))
                    .text_color(theme.colors.muted_foreground.to_paint())
                })),
        )
        .child(
            div()
                .size(px(14.0))
                .flex_shrink_0()
                .overflow_hidden()
                .child(leading),
        )
        .child(
            div()
                .min_w_0()
                .flex_shrink_0()
                .max_w(gpui::relative(0.6))
                .truncate()
                .text_size(px(13.0))
                .text_color(if deleted {
                    theme.colors.muted_foreground.to_paint()
                } else {
                    theme.colors.foreground.to_paint()
                })
                .when(expanded.is_none(), |label| {
                    label
                        .font_family(theme.typography.mono.family)
                        .tracking(0.0)
                })
                .when(deleted, gpui::Styled::line_through)
                .child(self.label),
        )
        .children(
            self.trailing
                .map(|trailing| div().flex_shrink_0().child(trailing)),
        )
        .children(self.description.map(|description| {
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(12.0))
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(description)
        }))
        .on_click(move |_, window, cx| {
            if let (Some(expanded), Some(expand)) = (expanded, &click_expand) {
                expand(!expanded, window, cx);
            }
            if let Some(activate) = &click_activate {
                activate(window, cx);
            }
        })
        .on_key_down(move |event, window, cx| {
            let command = match event.keystroke.key.as_str() {
                "up" => Some(TreeNavigation::Previous),
                "down" => Some(TreeNavigation::Next),
                "home" => Some(TreeNavigation::First),
                "end" => Some(TreeNavigation::Last),
                "left" if expanded == Some(true) => {
                    if let Some(expand) = &expand {
                        expand(false, window, cx);
                    }
                    None
                }
                "left" => Some(TreeNavigation::Parent),
                "right" if expanded == Some(false) => {
                    if let Some(expand) = &expand {
                        expand(true, window, cx);
                    }
                    None
                }
                "right" if expanded == Some(true) => Some(TreeNavigation::Next),
                _ => return,
            };
            if let (Some(command), Some(navigate)) = (command, &navigate) {
                navigate(command, window, cx);
            }
            cx.stop_propagation();
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct View;
    impl gpui::Render for View {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            div()
        }
    }
    #[gpui::test]
    fn focus_retention_stays_bounded_across_scrolling_and_model_replacement(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_, cx) = cx.add_window_view(|_, _| View);
        cx.update(|window, cx| {
            let focus = TreeFocus::default();
            let target: SharedString = "keyboard-target".into();
            focus.focus(target.clone(), window, cx);
            for start in (0..5000).step_by(14) {
                let visible: Vec<SharedString> = (start..(start + 14).min(5000))
                    .map(|index| format!("node-{index}").into())
                    .collect();
                focus.retain_visible(visible.clone(), window);
                for id in &visible {
                    focus.handle(id.clone(), cx);
                }
                let handles = focus.0.borrow();
                assert_eq!(handles.len(), visible.len() + 1);
                assert!(handles[&target].is_focused(window));
            }
            focus.reset(window, cx);
            assert!(focus.0.borrow().is_empty());
            assert!(window.focused(cx).is_none());
            focus.focus("replacement-node", window, cx);
            focus.retain_visible(["replacement-node".into()], window);
            assert_eq!(focus.0.borrow().len(), 1);
            assert!(!focus.0.borrow().contains_key(&target));
        });
    }
}
