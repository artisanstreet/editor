use accesskit_consumer::{NodeRef, Tree, TreeChangeHandler};
use aqua::AccessSink;
use aqua_a11y_uia::accesskit::TreeUpdate;
use aqua_a11y_uia::{A11yBridge, Connection};
use aqua_protocol::{A11yActionRequest, AccessTreeUpdate};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Default)]
pub(super) struct Published {
    tree: Option<Tree>,
}

struct Unobserved;

impl TreeChangeHandler for Unobserved {
    fn node_added(&mut self, _: &NodeRef<'_>) {}
    fn node_updated(&mut self, _: &NodeRef<'_>, _: &NodeRef<'_>) {}
    fn focus_moved(&mut self, _: Option<&NodeRef<'_>>, _: Option<&NodeRef<'_>>) {}
    fn node_removed(&mut self, _: &NodeRef<'_>) {}
}

impl Published {
    fn apply(&mut self, update: TreeUpdate) {
        match &mut self.tree {
            Some(tree) => tree.update_and_process_changes(update, &mut Unobserved),
            None => self.tree = Some(Tree::new(update, true)),
        }
    }

    pub(super) fn tree(&self) -> Option<&Tree> {
        self.tree.as_ref()
    }
}

pub(super) struct BridgeSink {
    bridge: Arc<A11yBridge>,
    published: Rc<RefCell<Published>>,
}

impl BridgeSink {
    pub(super) const fn new(bridge: Arc<A11yBridge>, published: Rc<RefCell<Published>>) -> Self {
        Self { bridge, published }
    }
}

impl AccessSink for BridgeSink {
    fn needs_snapshot(&self) -> bool {
        self.bridge.connection() == Connection::Connected
    }

    fn connection_epoch(&self) -> u64 {
        u64::from(self.needs_snapshot())
    }

    fn apply(&mut self, update: &AccessTreeUpdate) {
        if let Some(snapshot) = self.bridge.snapshot(update) {
            self.published.borrow_mut().apply(snapshot);
        }
    }

    fn drain_actions(&mut self) -> Vec<A11yActionRequest> {
        self.bridge.drain_actions()
    }
}
