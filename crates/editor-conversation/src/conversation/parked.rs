use std::collections::VecDeque;

pub struct Shelf<K, V> {
    entries: VecDeque<(K, V, usize)>,
    count: usize,
    bytes: usize,
}

impl<K: PartialEq, V> Shelf<K, V> {
    #[must_use]
    pub const fn new(count: usize, bytes: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            count,
            bytes,
        }
    }

    pub fn put(&mut self, key: K, value: V, size: usize) {
        self.entries.retain(|(known, _, _)| known != &key);
        if size > self.bytes || self.count == 0 {
            return;
        }
        self.entries.push_back((key, value, size));
        while self.entries.len() > self.count || self.used() > self.bytes {
            self.entries.pop_front();
        }
    }

    pub fn take(&mut self, key: &K) -> Option<V> {
        let index = self.entries.iter().position(|(known, _, _)| known == key)?;
        self.entries.remove(index).map(|(_, value, _)| value)
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        self.entries.retain(|(key, _, _)| keep(key));
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn used(&self) -> usize {
        self.entries.iter().map(|(_, _, size)| size).sum()
    }
}
