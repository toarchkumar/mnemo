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
    payload: Vec<u8>,
    dirty: bool,
    /// Toward the more-recently-used end (`None` at the MRU head).
    prev: Option<usize>,
    /// Toward the less-recently-used end (`None` at the LRU tail).
    next: Option<usize>,
}

/// Bounded LRU cache of decrypted pages, keyed by page number.
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

    /// Fetch a page's payload, marking it most-recently-used.
    pub fn get(&mut self, page: u64) -> Option<Vec<u8>> {
        let i = *self.map.get(&page)?;
        self.touch(i);
        Some(self.slots[i].payload.clone())
    }

    /// Read a page's payload **without** affecting recency order.
    pub fn peek(&self, page: u64) -> Option<&[u8]> {
        self.map.get(&page).map(|&i| self.slots[i].payload.as_slice())
    }

    /// Insert or replace a page. `dirty` marks it as holding an un-flushed
    /// write (and so pinned against eviction). A fresh insertion may evict
    /// the least-recently-used clean page to stay within the cap.
    pub fn insert(&mut self, page: u64, payload: Vec<u8>, dirty: bool) {
        if let Some(&i) = self.map.get(&page) {
            match (self.slots[i].dirty, dirty) {
                (true, false) => self.clean += 1,
                (false, true) => self.clean -= 1,
                _ => {}
            }
            self.slots[i].payload = payload;
            self.slots[i].dirty = dirty;
            self.touch(i);
            return;
        }
        let i = self.alloc(Node {
            page,
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

    /// Sorted list of every dirty page's number — the flush work list.
    pub fn dirty_pages(&self) -> Vec<u64> {
        let mut v: Vec<u64> = self
            .map
            .iter()
            .filter(|&(_, &i)| self.slots[i].dirty)
            .map(|(&p, _)| p)
            .collect();
        v.sort_unstable();
        v
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
            c.insert(n, page(n as u8), false);
        }
        c.insert(4, page(4), false); // over cap → evict LRU (page 1)
        assert!(c.peek(1).is_none());
        assert!(c.peek(4).is_some());

        c.get(2); // page 2 becomes most-recently-used
        c.insert(5, page(5), false); // evict the new LRU (page 3)
        assert!(c.peek(3).is_none());
        assert_eq!(c.len(), 3);
        for n in [2u64, 4, 5] {
            assert!(c.peek(n).is_some());
        }
    }

    #[test]
    fn dirty_pages_are_never_evicted() {
        let mut c = PageCache::new(2);
        for n in 1..=3u64 {
            c.insert(n, page(n as u8), true);
        }
        // All three are dirty: the cap is exceeded rather than lose a write.
        assert_eq!(c.len(), 3);
        for n in 1..=3u64 {
            assert!(c.peek(n).is_some());
        }
        assert_eq!(c.dirty_pages(), vec![1, 2, 3]);

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
            c.insert(n, page(n as u8), false);
        }
        assert_eq!(c.len(), 8);
        c.set_capacity(3);
        assert_eq!(c.len(), 3);
        // The three most-recently-used entries survive.
        for n in [6u64, 7, 8] {
            assert!(c.peek(n).is_some());
        }
    }

    #[test]
    fn reinsert_updates_payload_and_recency() {
        let mut c = PageCache::new(2);
        c.insert(1, page(1), false);
        c.insert(2, page(2), false);
        c.insert(1, page(9), false); // updates page 1 and makes it MRU
        c.insert(3, page(3), false); // evicts the LRU — page 2, not page 1
        assert_eq!(c.get(1), Some(page(9)));
        assert!(c.peek(2).is_none());
        assert!(c.peek(3).is_some());
    }
}
