//! A small most-recently-used cache.
//!
//! Capacities here are tens of entries, so a linear scan beats hashing and
//! keeps eviction exact: the least recently used entry goes first, instead of
//! the whole cache emptying when it fills.

pub struct Recent<K, V> {
    entries: Vec<(K, V)>,
    capacity: usize,
}

impl<K: PartialEq, V> Recent<K, V> {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            capacity: capacity.max(1),
        }
    }

    /// Look up `key` and mark it as just used.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        let at = self.entries.iter().position(|(k, _)| k == key)?;
        let entry = self.entries.remove(at);
        self.entries.push(entry);
        self.entries.last().map(|(_, v)| v)
    }

    /// Whether `key` is cached, without changing its order.
    pub fn contains(&self, key: &K) -> bool {
        self.entries.iter().any(|(k, _)| k == key)
    }

    pub fn insert(&mut self, key: K, value: V) {
        self.entries.retain(|(k, _)| *k != key);
        if self.entries.len() == self.capacity {
            self.entries.remove(0);
        }
        self.entries.push((key, value));
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn weight(&self, weight: impl Fn(&V) -> usize) -> usize {
        self.entries.iter().map(|(_, v)| weight(v)).sum()
    }

    pub fn trim_weight(&mut self, limit: usize, weight: impl Fn(&V) -> usize) {
        let mut total = self.weight(&weight);
        while total > limit && !self.entries.is_empty() {
            total -= weight(&self.entries.remove(0).1);
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl<K: PartialEq, V> Default for Recent<K, V> {
    fn default() -> Self {
        Self::new(16)
    }
}

#[cfg(test)]
mod tests {
    use super::Recent;

    #[test]
    fn the_least_recently_used_entry_is_evicted_first() {
        let mut cache = Recent::new(2);
        cache.insert("a", 1);
        cache.insert("b", 2);
        // Using "a" makes "b" the oldest.
        assert_eq!(cache.get(&"a"), Some(&1));
        cache.insert("c", 3);
        assert!(cache.contains(&"a") && cache.contains(&"c"));
        assert!(!cache.contains(&"b"));
        cache.insert("a", 4);
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get(&"a"), Some(&4));
        cache.clear();
        assert!(cache.is_empty());
    }
}
