use crate::EngineEvent;
use artisan_api::ItemKind;
use std::collections::{HashSet, VecDeque};

const RETIRED_IDS: usize = 4096;

#[derive(Debug, Default)]
pub(crate) struct Retired {
    order: VecDeque<String>,
    ids: HashSet<String>,
}

impl Retired {
    pub(crate) fn insert(&mut self, id: String) {
        if !self.ids.insert(id.clone()) {
            return;
        }
        self.order.push_back(id);
        while self.order.len() > RETIRED_IDS
            && let Some(oldest) = self.order.pop_front()
        {
            self.ids.remove(&oldest);
        }
    }

    pub(crate) fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }
}

pub(crate) trait Retains {
    fn retained(&mut self, key: &str) -> Option<&mut ItemKind>;
}

pub(crate) fn settle(decoder: &mut impl Retains, events: Vec<EngineEvent>) -> Vec<EngineEvent> {
    for event in &events {
        let (EngineEvent::Item { key, status, .. } | EngineEvent::Status { key, status }) = event
        else {
            continue;
        };
        if !status.is_terminal() {
            continue;
        }
        if let Some(kind) = decoder.retained(key)
            && let Err(error) = forge_content::compact_settled(kind)
        {
            tracing::warn!(key, %error, "keeping a settled item uncompacted");
        }
    }
    events
}

#[cfg(test)]
#[path = "../tests/retained/unit.rs"]
mod tests;
