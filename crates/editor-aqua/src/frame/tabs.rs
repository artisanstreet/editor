use artisan_api::ThreadId;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Tabs {
    order: Vec<ThreadId>,
}

impl Tabs {
    #[must_use]
    pub fn order(&self) -> &[ThreadId] {
        &self.order
    }

    pub fn sync(&mut self, working: &[ThreadId], open: Option<&ThreadId>) -> bool {
        let kept = |id: &ThreadId| working.contains(id) || open == Some(id);
        let before = self.order.clone();
        self.order.retain(kept);
        for id in working.iter().chain(open) {
            if !self.order.contains(id) {
                self.order.push(id.clone());
            }
        }
        before != self.order
    }

    #[must_use]
    pub fn successor(&self, closing: &ThreadId) -> Option<ThreadId> {
        let index = self.order.iter().position(|id| id == closing)?;
        self.order
            .get(index + 1)
            .or_else(|| index.checked_sub(1).and_then(|before| self.order.get(before)))
            .cloned()
    }
}
