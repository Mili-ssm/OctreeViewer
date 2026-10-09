use std::collections::{BTreeSet, HashMap, HashSet};

use crate::{BlockId, PointBlock};

/// Statistics for decoded-block cache activity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlockCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub insertions: u64,
    pub evictions: u64,
    pub rejected_oversize: u64,
    pub rejected_protected: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheInsertRejection {
    Oversize,
    ProtectedEntries,
}

/// Result of adding a decoded block to the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheInsertResult {
    pub cached: bool,
    pub rejection: Option<CacheInsertRejection>,
    pub evicted_blocks: Vec<BlockId>,
    pub accounted_bytes: usize,
}

/// Byte-bounded LRU cache for decompressed spatial blocks.
///
/// The byte count estimates retained block payload allocations from vector capacities
/// and inline block data. It excludes allocator and cache-index overhead.
pub struct DecodedBlockCache {
    max_bytes: usize,
    used_bytes: usize,
    next_sequence: u64,
    entries: HashMap<BlockId, CacheEntry>,
    recency: BTreeSet<(u64, BlockId)>,
    protected: HashSet<BlockId>,
    stats: BlockCacheStats,
}

impl DecodedBlockCache {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            used_bytes: 0,
            next_sequence: 0,
            entries: HashMap::new(),
            recency: BTreeSet::new(),
            protected: HashSet::new(),
            stats: BlockCacheStats::default(),
        }
    }

    pub const fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    pub const fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub const fn stats(&self) -> BlockCacheStats {
        self.stats
    }

    /// Checks presence without changing LRU order or hit/miss counters.
    pub fn contains(&self, id: BlockId) -> bool {
        self.entries.contains_key(&id)
    }

    /// Inspects a resident block without changing LRU order or hit/miss counters.
    pub fn peek(&self, id: BlockId) -> Option<&PointBlock> {
        self.entries.get(&id).map(|entry| &entry.block)
    }

    /// Protects resident entries from eviction until the next protection set is applied.
    pub fn set_protected_blocks(&mut self, block_ids: impl IntoIterator<Item = BlockId>) {
        self.protected = block_ids
            .into_iter()
            .filter(|id| self.entries.contains_key(id))
            .collect();
    }

    /// Returns a cached block and marks it most recently used.
    pub fn get(&mut self, id: BlockId) -> Option<&PointBlock> {
        let Some(old_sequence) = self.entries.get(&id).map(|entry| entry.sequence) else {
            self.stats.misses = self.stats.misses.saturating_add(1);
            return None;
        };
        self.stats.hits = self.stats.hits.saturating_add(1);
        self.recency.remove(&(old_sequence, id));
        let sequence = self.allocate_sequence();
        let entry = self.entries.get_mut(&id).expect("entry was just observed");
        entry.sequence = sequence;
        self.recency.insert((sequence, id));
        self.entries.get(&id).map(|entry| &entry.block)
    }

    /// Inserts or replaces a block, evicting least-recently-used entries to fit.
    /// A block larger than the full budget is rejected and never retained.
    pub fn insert(&mut self, block: PointBlock) -> CacheInsertResult {
        let id = block.id();
        let accounted_bytes = block.estimated_memory_bytes();
        if accounted_bytes > self.max_bytes {
            self.stats.rejected_oversize = self.stats.rejected_oversize.saturating_add(1);
            return CacheInsertResult {
                cached: false,
                rejection: Some(CacheInsertRejection::Oversize),
                evicted_blocks: Vec::new(),
                accounted_bytes,
            };
        }
        if self.protected.contains(&id) {
            self.stats.rejected_protected = self.stats.rejected_protected.saturating_add(1);
            return CacheInsertResult {
                cached: false,
                rejection: Some(CacheInsertRejection::ProtectedEntries),
                evicted_blocks: Vec::new(),
                accounted_bytes,
            };
        }

        let replaced_bytes = self
            .entries
            .get(&id)
            .map_or(0, |entry| entry.accounted_bytes);
        let mut retained_bytes = self.used_bytes.saturating_sub(replaced_bytes);
        let mut evicted_blocks = Vec::new();
        if retained_bytes.saturating_add(accounted_bytes) > self.max_bytes {
            for (_, candidate_id) in &self.recency {
                if *candidate_id == id || self.protected.contains(candidate_id) {
                    continue;
                }
                retained_bytes =
                    retained_bytes.saturating_sub(self.entries[candidate_id].accounted_bytes);
                evicted_blocks.push(*candidate_id);
                if retained_bytes.saturating_add(accounted_bytes) <= self.max_bytes {
                    break;
                }
            }
        }
        if retained_bytes.saturating_add(accounted_bytes) > self.max_bytes {
            self.stats.rejected_protected = self.stats.rejected_protected.saturating_add(1);
            return CacheInsertResult {
                cached: false,
                rejection: Some(CacheInsertRejection::ProtectedEntries),
                evicted_blocks: Vec::new(),
                accounted_bytes,
            };
        }

        self.remove_entry(id);
        for evicted_id in &evicted_blocks {
            self.remove_entry(*evicted_id);
            self.stats.evictions = self.stats.evictions.saturating_add(1);
        }
        let sequence = self.allocate_sequence();
        self.used_bytes = self.used_bytes.saturating_add(accounted_bytes);
        self.entries.insert(
            id,
            CacheEntry {
                block,
                accounted_bytes,
                sequence,
            },
        );
        self.recency.insert((sequence, id));
        self.stats.insertions = self.stats.insertions.saturating_add(1);
        CacheInsertResult {
            cached: true,
            rejection: None,
            evicted_blocks,
            accounted_bytes,
        }
    }

    pub fn remove(&mut self, id: BlockId) -> Option<PointBlock> {
        self.remove_entry(id).map(|entry| entry.block)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.recency.clear();
        self.protected.clear();
        self.used_bytes = 0;
    }

    fn remove_entry(&mut self, id: BlockId) -> Option<CacheEntry> {
        let entry = self.entries.remove(&id)?;
        self.protected.remove(&id);
        self.recency.remove(&(entry.sequence, id));
        self.used_bytes = self.used_bytes.saturating_sub(entry.accounted_bytes);
        Some(entry)
    }

    fn allocate_sequence(&mut self) -> u64 {
        if self.next_sequence == u64::MAX {
            let ordered = self.recency.iter().copied().collect::<Vec<_>>();
            self.recency.clear();
            for (index, (old_sequence, id)) in ordered.into_iter().enumerate() {
                let sequence = index as u64;
                self.entries
                    .get_mut(&id)
                    .expect("recency index has a matching entry")
                    .sequence = sequence;
                self.recency.insert((sequence, id));
                debug_assert!(old_sequence < u64::MAX);
            }
            self.next_sequence = self.entries.len() as u64;
        }
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        sequence
    }
}

struct CacheEntry {
    block: PointBlock,
    accounted_bytes: usize,
    sequence: u64,
}

#[cfg(test)]
mod tests {
    use crate::{
        AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, ColorRgb, Point,
        PointBatch,
    };

    use super::{CacheInsertRejection, DecodedBlockCache};
    use crate::{BlockId, PointBlock};

    fn block(id: u64, point_count: usize) -> PointBlock {
        let points = (0..point_count)
            .map(|index| Point::new([index as f32, 0.0, 0.0], ColorRgb::new(1, 2, 3)))
            .collect();
        PointBlock::new(BlockId::new(id), PointBatch::new(points), Vec::new())
    }

    #[test]
    fn cache_accounts_retained_capacity_and_enforces_budget_with_lru_eviction() {
        let first = block(1, 3);
        let second = block(2, 3);
        let first_size = first.estimated_memory_bytes();
        let second_size = second.estimated_memory_bytes();
        let mut cache = DecodedBlockCache::new(first_size + second_size);

        assert!(cache.insert(first).cached);
        assert!(cache.insert(second).cached);
        assert_eq!(cache.used_bytes(), first_size + second_size);
        assert!(cache.get(BlockId::new(1)).is_some());

        let third = block(3, 2);
        let result = cache.insert(third);
        assert!(result.cached);
        assert_eq!(result.evicted_blocks, [BlockId::new(2)]);
        assert!(cache.get(BlockId::new(2)).is_none());
        assert!(cache.get(BlockId::new(1)).is_some());
        assert!(cache.get(BlockId::new(3)).is_some());
        assert!(cache.used_bytes() <= cache.max_bytes());
        assert_eq!(cache.stats().evictions, 1);
    }

    #[test]
    fn cache_counts_hits_misses_and_rejects_oversize_blocks() {
        let mut cache = DecodedBlockCache::new(1);
        let block = block(1, 2);
        let size = block.estimated_memory_bytes();
        let result = cache.insert(block);

        assert!(!result.cached);
        assert_eq!(result.accounted_bytes, size);
        assert_eq!(cache.used_bytes(), 0);
        assert!(cache.get(BlockId::new(1)).is_none());
        assert_eq!(cache.stats().rejected_oversize, 1);
        assert_eq!(cache.stats().misses, 1);
    }

    #[test]
    fn cache_replaces_removes_and_clears_without_stale_accounting() {
        let first = block(1, 2);
        let size = first.estimated_memory_bytes();
        let mut cache = DecodedBlockCache::new(size * 3);
        cache.insert(first);
        cache.insert(block(1, 1));
        assert_eq!(cache.len(), 1);
        assert!(cache.used_bytes() < size);

        assert!(cache.remove(BlockId::new(1)).is_some());
        assert_eq!(cache.used_bytes(), 0);
        cache.insert(block(2, 1));
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.used_bytes(), 0);
    }

    #[test]
    fn cache_accounts_for_attribute_payload_allocations() {
        let descriptor = AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap();
        let column =
            AttributeColumn::new(descriptor, AttributeValues::U16(vec![4, 8, 12]), 3).unwrap();
        let block = PointBlock::new(
            BlockId::new(7),
            PointBatch::new(vec![Point::new([0.0; 3], ColorRgb::new(0, 0, 0)); 3]),
            vec![column],
        );
        let expected_minimum = std::mem::size_of::<PointBlock>()
            + 3 * std::mem::size_of::<Point>()
            + std::mem::size_of::<AttributeColumn>()
            + "intensity".len()
            + 3 * std::mem::size_of::<u16>();
        assert!(block.estimated_memory_bytes() >= expected_minimum);
    }

    #[test]
    fn protected_entries_survive_pressure_and_rejected_insert_is_atomic() {
        let first = block(1, 2);
        let size = first.estimated_memory_bytes();
        let mut cache = DecodedBlockCache::new(size);
        cache.insert(first);
        cache.set_protected_blocks([BlockId::new(1)]);

        let result = cache.insert(block(2, 2));
        assert!(!result.cached);
        assert_eq!(
            result.rejection,
            Some(CacheInsertRejection::ProtectedEntries)
        );
        assert!(result.evicted_blocks.is_empty());
        assert!(cache.contains(BlockId::new(1)));
        assert!(!cache.contains(BlockId::new(2)));
        assert_eq!(cache.used_bytes(), size);
        assert_eq!(cache.stats().rejected_protected, 1);
    }
}
