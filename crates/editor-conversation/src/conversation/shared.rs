use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasher, Hash};
use std::ops::Deref;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

fn next_revision() -> u64 {
    static REVISION: AtomicU64 = AtomicU64::new(1);
    REVISION.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug)]
pub struct Shared<T> {
    value: Rc<T>,
    revision: u64,
}

impl<T> Shared<T> {
    pub fn new(value: T) -> Self {
        Self {
            value: Rc::new(value),
            revision: next_revision(),
        }
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }
}

impl<T: Clone> Shared<T> {
    pub fn make_mut(&mut self) -> &mut T {
        self.revision = next_revision();
        Rc::make_mut(&mut self.value)
    }
}

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self {
            value: Rc::clone(&self.value),
            revision: self.revision,
        }
    }
}

impl<T: Default> Default for Shared<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> Deref for Shared<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T: PartialEq> PartialEq<T> for Shared<T> {
    fn eq(&self, other: &T) -> bool {
        *self.value == *other
    }
}

impl<K: Clone + Eq + Hash, V: Clone, S: BuildHasher + Clone> Shared<HashMap<K, V, S>> {
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&K, &V) -> bool) {
        if self.value.iter().all(|(key, value)| keep(key, value)) {
            return;
        }
        self.make_mut().retain(|key, value| keep(key, value));
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        if self.value.contains_key(key) {
            self.make_mut().remove(key)
        } else {
            None
        }
    }
}

impl<K: Clone + Ord, V: Clone> Shared<BTreeMap<K, V>> {
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&K, &V) -> bool) {
        if self.value.iter().all(|(key, value)| keep(key, value)) {
            return;
        }
        self.make_mut().retain(|key, value| keep(key, value));
    }
}
