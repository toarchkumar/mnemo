//! A bounded, least-recently-used cache of decrypted page payloads.
//!
//! The pager keeps decrypted page contents here so repeated reads skip a
//! decrypt and writes can be batched until a flush. Left unbounded the cache
//! would grow with the database; this LRU caps it.
//!
//! One rule is absolute: a **dirty** page — one holding the only copy of an
//! un-flushed write — is never evicted. Eviction targets the
//! least-recently-used *clean* page instead. If every cached page is dirty
//! the cache is allowed to exceed its cap until the next flush turns those
//! pages clean again; the cap bounds retained *clean* pages, and is never a
//! licence to drop unwritten data.
//!
//! **Keyed by page and generation** (v0.20.0, v0200-3). A v9 page is
//! sealed under its writer's generation, and every pointer to it says which
//! generation it expects. The cache keeps each page's generation and serves
//! a hit only for the same one, so a page another process freed and reused
//! under a newer generation is never served stale. That lets a shared
//! handle keep its cache across an incremental catch-up
//! (`docs/design/catch-up.md`, section 4b). A v8 pager passes generation 0
//! everywhere.
//!
//! The recency order is an intrusive doubly linked list threaded through a
//! slab (`Vec<Node>` plus a free list), so every operation is O(1) and the
//! whole thing stays within `#![forbid(unsafe_code)]` — list links are slab
//! indices, not pointers. Eviction walks from the LRU end skipping dirty
//! nodes; because freshly written pages sit at the MRU end, that walk almost
//! always stops at the first node.

use std::collections::HashMap;

/// One cached page plus its intrusive LRU-list links.
struct Node {
    page: u64,
    /// The generation the page was read or written under.
    gen: u64,
    payload: Vec<u8>,
    dirty: bool,
    /// Toward the more-recently-used end (`None` at the MRU head).
    prev: Option<usize>,
    /// Toward the less-recently-used end (`None` at the LRU tail).
    next: Option<usize>,
}

/// Bounded LRU cache of decrypted pages, keyed by page number and served
/// only for the generation each was cached under.
pub struct PageCache {
    slots: Vec<Node>,
    free: Vec<usize>,
    map: HashMap<u64, usize>,
    head: Option<usize>, // most-recently-used
    tail: Option<usize>, // least-recently-used
    capacity: usize,
    /// Count of non-dirty entries — the pool eviction can draw from.
    clean: usize,
}

impl PageCache {
    /// A cache holding at most `capacity` pages (clamped to at least one).
    pub fn new(capacity: usize) -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            map: HashMap::new(),
            head: None,
            tail: None,
            capacity: capacity.max(1),
            clean: 0,
        }
    }

    /// Number of pages currently cached (clean plus dirty).
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// The configured clean-page cap.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Change the cap, evicting clean pages immediately if it shrank.
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.max(1);
        self.trim();
    }

    // --- intrusive list plumbing -----------------------------------------

    fn detach(&mut self, i: usize) {
        let (p, n) = (self.slots[i].prev, self.slots[i].next);
        match p {
            Some(pi) => self.slots[pi].next = n,
            None => self.head = n,
        }
        match n {
            Some(ni) => self.slots[ni].prev = p,
            None => self.tail = p,
        }
        self.slots[i].prev = None;
        self.slots[i].next = None;
    }

    fn push_front(&mut self, i: usize) {
        self.slots[i].prev = None;
        self.slots[i].next = self.head;
        match self.head {
            Some(h) => self.slots[h].prev = Some(i),
            None => self.tail = Some(i),
        }
        self.head = Some(i);
    }

    /// Move an existing node to the most-recently-used position.
    fn touch(&mut self, i: usize) {
        self.detach(i);
        self.push_front(i);
    }

    fn alloc(&mut self, node: Node) -> usize {
        if let Some(i) = self.free.pop() {
            self.slots[i] = node;
            i
        } else {
            self.slots.push(node);
            self.slots.len() - 1
        }
    }

    /// Unlink and forget node `i`, freeing its slot and payload.
    fn remove_node(&mut self, i: usize) {
        self.detach(i);
        let page = self.slots[i].page;
        self.map.remove(&page);
        if !self.slots[i].dirty {
            self.clean -= 1;
        }
        self.slots[i].payload = Vec::new();
        self.slots[i].dirty = false;
        self.free.push(i);
    }

    /// Evict the least-recently-used *clean* page. Returns `false` when no
    /// clean page exists (the cache then legitimately exceeds its cap).
    fn evict_one(&mut self) -> bool {
        if self.clean == 0 {
            return false;
        }
        let mut cur = self.tail;
        while let Some(i) = cur {
            if !self.slots[i].dirty {
                self.remove_node(i);
                return true;
            }
            cur = self.slots[i].prev;
        }
        false
    }

    /// Evict clean pages until the cache is within its cap (or out of clean
    /// pages to give up).
    pub fn trim(&mut self) {
        while self.map.len() > self.capacity {
            if !self.evict_one() {
                break;
            }
        }
    }

    // --- public cache operations -----------------------------------------

    /// Fetch a page's payload cached under `gen`, marking it
    /// most-recently-used. A clean entry under another generation is stale
    /// (the page was freed and reused since): it's dropped, and this is a
    /// miss. A dirty one is this transaction's own write and stays.
    pub fn get(&mut self, page: u64, gen: u64) -> Option<Vec<u8>> {
        let i = *self.map.get(&page)?;
        if self.slots[i].gen != gen {
            if !self.slots[i].dirty {
                self.remove_node(i);
            }
            return None;
        }
        self.touch(i);
        Some(self.slots[i].payload.clone())
    }

    /// Read a page's payload cached under `gen` **without** affecting
    /// recency order. Another generation is a miss.
    pub fn peek(&self, page: u64, gen: u64) -> Option<&[u8]> {
        self.map
            .get(&page)
            .filter(|&&i| self.slots[i].gen == gen)
            .map(|&i| self.slots[i].payload.as_slice())
    }

    /// Insert or replace a page under generation `gen`. `dirty` marks it as
    /// holding an un-flushed write (and so pinned against eviction). A fresh
    /// insertion may evict the least-recently-used clean page to stay within
    /// the cap.
    pub fn insert(&mut self, page: u64, gen: u64, payload: Vec<u8>, dirty: bool) {
        if let Some(&i) = self.map.get(&page) {
            match (self.slots[i].dirty, dirty) {
                (true, false) => self.clean += 1,
                (false, true) => self.clean -= 1,
                _ => {}
            }
            self.slots[i].gen = gen;
            self.slots[i].payload = payload;
            self.slots[i].dirty = dirty;
            self.touch(i);
            return;
        }
        let i = self.alloc(Node {
            page,
            gen,
            payload,
            dirty,
            prev: None,
            next: None,
        });
        self.map.insert(page, i);
        self.push_front(i);
        if !dirty {
            self.clean += 1;
        }
        self.trim();
    }

    /// Whether any cached page is dirty.
    pub fn has_dirty(&self) -> bool {
        self.clean < self.map.len()
    }

    /// Whether `page` is cached dirty (under any generation).
    pub fn is_dirty(&self, page: u64) -> bool {
        self.map.get(&page).is_some_and(|&i| self.slots[i].dirty)
    }

    /// Every dirty page's number and generation, sorted by page — the
    /// flush work list.
    pub fn dirty_pages(&self) -> Vec<(u64, u64)> {
        let mut v: Vec<(u64, u64)> = self
            .map
            .iter()
            .filter(|&(_, &i)| self.slots[i].dirty)
            .map(|(&p, &i)| (p, self.slots[i].gen))
            .collect();
        v.sort_unstable();
        v
    }

    /// v0200-3r: drop every clean page cached under a generation above
    /// `gen`. A dirty page is this transaction's own write and stays.
    pub fn drop_newer_than(&mut self, gen: u64) {
        let doomed: Vec<usize> = self
            .map
            .values()
            .copied()
            .filter(|&i| self.slots[i].gen > gen && !self.slots[i].dirty)
            .collect();
        for i in doomed {
            self.remove_node(i);
        }
    }

    /// Mark every cached page clean — called after a flush has written them.
    /// Evictable pages then become available again.
    pub fn mark_all_clean(&mut self) {
        for &i in self.map.values() {
            self.slots[i].dirty = false;
        }
        self.clean = self.map.len();
    }
}

#[cfg(test)]
mod tests {
    use super::PageCache;

    fn page(n: u8) -> Vec<u8> {
        vec![n; 4]
    }

    #[test]
    fn evicts_least_recently_used_clean_page() {
        let mut c = PageCache::new(3);
        for n in 1..=3u64 {
            c.insert(n, 1, page(n as u8), false);
        }
        c.insert(4, 1, page(4), false); // over cap → evict LRU (page 1)
        assert!(c.peek(1, 1).is_none());
        assert!(c.peek(4, 1).is_some());

        c.get(2, 1); // page 2 becomes most-recently-used
        c.insert(5, 1, page(5), false); // evict the new LRU (page 3)
        assert!(c.peek(3, 1).is_none());
        assert_eq!(c.len(), 3);
        for n in [2u64, 4, 5] {
            assert!(c.peek(n, 1).is_some());
        }
    }

    #[test]
    fn dirty_pages_are_never_evicted() {
        let mut c = PageCache::new(2);
        for n in 1..=3u64 {
            c.insert(n, 1, page(n as u8), true);
        }
        // All three are dirty: the cap is exceeded rather than lose a write.
        assert_eq!(c.len(), 3);
        for n in 1..=3u64 {
            assert!(c.peek(n, 1).is_some());
        }
        assert_eq!(c.dirty_pages(), vec![(1, 1), (2, 1), (3, 1)]);

        // Once a flush marks them clean, the cache trims back to the cap.
        c.mark_all_clean();
        c.trim();
        assert_eq!(c.len(), 2);
        assert!(c.dirty_pages().is_empty());
    }

    #[test]
    fn shrinking_capacity_trims_immediately() {
        let mut c = PageCache::new(8);
        for n in 1..=8u64 {
            c.insert(n, 1, page(n as u8), false);
        }
        assert_eq!(c.len(), 8);
        c.set_capacity(3);
        assert_eq!(c.len(), 3);
        // The three most-recently-used entries survive.
        for n in [6u64, 7, 8] {
            assert!(c.peek(n, 1).is_some());
        }
    }

    #[test]
    fn reinsert_updates_payload_and_recency() {
        let mut c = PageCache::new(2);
        c.insert(1, 1, page(1), false);
        c.insert(2, 1, page(2), false);
        c.insert(1, 1, page(9), false); // updates page 1 and makes it MRU
        c.insert(3, 1, page(3), false); // evicts the LRU — page 2, not page 1
        assert_eq!(c.get(1, 1), Some(page(9)));
        assert!(c.peek(2, 1).is_none());
        assert!(c.peek(3, 1).is_some());
    }

    /// v0200-3: a page cached under one generation isn't served for
    /// another. A clean stale entry is dropped on the miss; a dirty one
    /// (this transaction's own write) stays.
    /// v0200-3r: only clean pages newer than the generation go.
    #[test]
    fn drop_newer_than_keeps_older_and_dirty_pages() {
        let mut c = PageCache::new(8);
        c.insert(1, 3, page(1), false);
        c.insert(2, 5, page(2), false);
        c.insert(3, 6, page(3), true);
        c.insert(4, 4, page(4), false);
        c.drop_newer_than(4);
        assert!(c.peek(1, 3).is_some());
        assert!(c.peek(4, 4).is_some());
        assert!(c.peek(2, 5).is_none());
        assert!(c.peek(3, 6).is_some(), "a dirty page stays");
        assert_eq!(c.len(), 3);
    }

    #[test]
    fn another_generation_is_a_miss() {
        let mut c = PageCache::new(4);
        c.insert(7, 3, page(1), false);
        assert_eq!(c.peek(7, 3), Some(&page(1)[..]));
        assert!(c.peek(7, 4).is_none());
        assert!(c.get(7, 4).is_none());
        assert_eq!(c.len(), 0, "the stale clean entry is dropped");
        c.insert(7, 4, page(2), false);
        assert_eq!(c.get(7, 4), Some(page(2)));
        assert!(c.get(7, 3).is_none());

        c.insert(8, 5, page(3), true);
        assert!(c.get(8, 4).is_none());
        assert_eq!(c.len(), 1, "a dirty entry stays on a miss");
        assert_eq!(c.dirty_pages(), vec![(8, 5)]);
        assert_eq!(c.get(8, 5), Some(page(3)));
    }
}
