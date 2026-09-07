//! A controlled expand/collapse primitive for native GPUI surfaces.
//!
//! [`Collapsible`] owns the caller's trigger and content elements after they
//! are converted to [`gpui::AnyElement`]. The open value remains controlled by
//! the caller: activation reports the requested value and never mutates the
//! component's copy of that value.

use gpui::prelude::Refineable;
use gpui::{
    AnyElement, App, ClickEvent, ElementId, FocusHandle, InteractiveElement, IntoElement,
    ParentElement, RenderOnce, SharedString, StatefulInteractiveElement, StyleRefinement, Styled,
    Window, div, px,
};

type ChangeHandler = Box<dyn Fn(bool, &ClickEvent, &mut Window, &mut App) + 'static>;

/// The controlled open and disabled state used by a [`Collapsible`].
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct CollapsibleState {
    open: bool,
    disabled: bool,
}

impl CollapsibleState {
    /// Creates a state value from the caller's open and disabled values.
    #[must_use]
    pub const fn new(open: bool, disabled: bool) -> Self {
        Self { open, disabled }
    }

    /// Returns the controlled open value.
    #[must_use]
    pub const fn open(self) -> bool {
        self.open
    }

    /// Returns the controlled open value.
    #[must_use]
    pub const fn is_open(self) -> bool {
        self.open
    }

    /// Returns whether activation is disabled.
    #[must_use]
    pub const fn disabled(self) -> bool {
        self.disabled
    }

    /// Returns whether activation is disabled.
    #[must_use]
    pub const fn is_disabled(self) -> bool {
        self.disabled
    }

    /// Returns the value the caller should apply after an activation request.
    ///
    /// Disabled state is intentionally represented by `None`; a caller never
    /// receives a toggle request that it must filter separately.
    #[must_use]
    pub const fn requested_toggle(self) -> Option<bool> {
        if self.disabled {
            None
        } else {
            Some(!self.open)
        }
    }
}

/// A controlled expand/collapse primitive with caller-owned slots.
///
/// The trigger is always mounted. Content is mounted while open, or while
/// closed when [`Self::force_mount`] is enabled. This primitive does not
/// animate either slot; callers can apply their own motion recipe around the
/// content slot.
#[derive(IntoElement)]
pub struct Collapsible {
    id: ElementId,
    focus: FocusHandle,
    state: CollapsibleState,
    force_mount: bool,
    hidden_until_found: bool,
    trigger: AnyElement,
    content: AnyElement,
    on_change: Option<ChangeHandler>,
    debug_selector: Option<SharedString>,
    root_style: StyleRefinement,
}

impl Collapsible {
    /// Constructs a controlled collapsible from caller-owned slots.
    ///
    /// The trigger and content are converted to owned GPUI elements here, so
    /// the component does not retain a lifetime tied to the caller's element
    /// expressions.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        open: bool,
        trigger: impl IntoElement,
        content: impl IntoElement,
    ) -> Self {
        Self {
            id: id.into(),
            focus,
            state: CollapsibleState::new(open, false),
            force_mount: false,
            hidden_until_found: false,
            trigger: trigger.into_any_element(),
            content: content.into_any_element(),
            on_change: None,
            debug_selector: None,
            root_style: StyleRefinement::default(),
        }
    }

    /// Sets whether activation is disabled.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.state = CollapsibleState::new(self.state.open(), disabled);
        self
    }

    /// Keeps content mounted while closed.
    #[must_use]
    pub const fn force_mount(mut self, force_mount: bool) -> Self {
        self.force_mount = force_mount;
        self
    }

    /// Requests the closed-content hidden policy when force-mounted.
    ///
    /// GPUI has no browser find-in-page lifecycle to implement here. When this
    /// option is active with [`Self::force_mount`], closed content uses the same
    /// nonpainted, noninteractive native `display: none` policy as other
    /// force-mounted closed content; it does not claim browser reveal behavior.
    #[must_use]
    pub const fn hidden_until_found(mut self, hidden_until_found: bool) -> Self {
        self.hidden_until_found = hidden_until_found;
        self
    }

    /// Reports a requested open value after a valid activation.
    #[must_use]
    pub fn on_change(
        mut self,
        handler: impl Fn(bool, &ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Box::new(handler));
        self
    }

    /// Adds a stable debug-selector prefix for root, trigger, and content.
    ///
    /// The supplied selector names the root. The trigger and content bounds
    /// use `-trigger` and `-content` suffixes respectively.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }

    /// Alias for [`Self::debug_selector`] that makes the prefix intent clear
    /// at call sites.
    #[must_use]
    pub fn debug_selector_prefix(self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector(selector)
    }

    /// Returns the controlled state captured by this render value.
    #[must_use]
    pub const fn state(&self) -> CollapsibleState {
        self.state
    }

    /// Returns the controlled open value captured by this render value.
    #[must_use]
    pub const fn open(&self) -> bool {
        self.state.open()
    }

    /// Returns the controlled open value captured by this render value.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.state.is_open()
    }

    /// Returns whether activation is disabled.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.state.is_disabled()
    }

    /// Returns whether closed content is kept mounted.
    #[must_use]
    pub const fn is_force_mounted(&self) -> bool {
        self.force_mount
    }

    /// Returns whether the hidden-until-found policy was requested.
    #[must_use]
    pub const fn uses_hidden_until_found(&self) -> bool {
        self.hidden_until_found
    }

    /// Returns the next controlled open value, if this instance is enabled.
    #[must_use]
    pub const fn requested_toggle(&self) -> Option<bool> {
        self.state.requested_toggle()
    }
}

impl Styled for Collapsible {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.root_style
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContentMode {
    Unmounted,
    Visible,
    Hidden,
    HiddenUntilFound,
}

const fn content_mode(open: bool, force_mount: bool, hidden_until_found: bool) -> ContentMode {
    if open {
        ContentMode::Visible
    } else if !force_mount {
        ContentMode::Unmounted
    } else if hidden_until_found {
        ContentMode::HiddenUntilFound
    } else {
        ContentMode::Hidden
    }
}

impl RenderOnce for Collapsible {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let Self {
            id,
            focus,
            state,
            force_mount,
            hidden_until_found,
            trigger,
            content,
            on_change,
            debug_selector,
            root_style,
        } = self;

        let requested_open = state.requested_toggle();
        let root_selector = debug_selector.map(|selector| selector.to_string());
        let trigger_selector = root_selector
            .as_ref()
            .map(|selector| format!("{selector}-trigger"));
        let content_selector = root_selector
            .as_ref()
            .map(|selector| format!("{selector}-content"));

        let mut trigger_shell = div().min_w_0().child(trigger).id(id);
        if let Some(selector) = trigger_selector {
            trigger_shell = trigger_shell.debug_selector(move || selector);
        }

        if !state.disabled()
            && let Some(requested_open) = requested_open
        {
            trigger_shell = trigger_shell
                .track_focus(&focus)
                .on_click(move |event, window, cx| {
                    if event.standard_click()
                        && let Some(handler) = on_change.as_ref()
                    {
                        handler(requested_open, event, window, cx);
                    }
                });
        }

        let mut root = div().flex().flex_col().min_w_0();
        root.style().refine(&root_style);
        if let Some(selector) = root_selector {
            root = root.debug_selector(move || selector);
        }
        root = root.child(trigger_shell);

        match content_mode(state.open(), force_mount, hidden_until_found) {
            ContentMode::Unmounted => root,
            ContentMode::Visible => {
                let mut content_shell = div().min_w_0().child(content);
                if let Some(selector) = content_selector {
                    content_shell = content_shell.debug_selector(move || selector);
                }
                root.child(content_shell)
            }
            ContentMode::Hidden | ContentMode::HiddenUntilFound => {
                // `hidden` removes the subtree from paint, hit testing, and
                // focus traversal. The explicit zero height also documents
                // the layout contract and keeps this recipe safe if a future
                // GPUI display implementation preserves a stale bound.
                let mut content_shell = div()
                    .min_w_0()
                    .h(px(0.0))
                    .overflow_hidden()
                    .hidden()
                    .child(content);
                if let Some(selector) = content_selector {
                    content_shell = content_shell.debug_selector(move || selector);
                }
                root.child(content_shell)
            }
        }
    }
}
