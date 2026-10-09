use std::collections::{BTreeSet, HashMap};

use crate::{BlockId, PointBlock};

/// Statistics for decoded-block cache activity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlockCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub insertions: u64,
    pub evictions: u64,
    pub rejected_oversize: u64,
}

/// Result of adding a decoded block to the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheInsertResult {
    pub cached: bool,
    pub evicted_blocks: usize,
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
        self.remove_entry(id);
        if accounted_bytes > self.max_bytes {
            self.stats.rejected_oversize = self.stats.rejected_oversize.saturating_add(1);
            return CacheInsertResult {
                cached: false,
                evicted_blocks: 0,
                accounted_bytes,
            };
        }

        let mut evicted_blocks = 0;
        while self.used_bytes.saturating_add(accounted_bytes) > self.max_bytes {
            if !self.evict_oldest() {
                break;
            }
            evicted_blocks += 1;
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
        self.used_bytes = 0;
    }

    fn remove_entry(&mut self, id: BlockId) -> Option<CacheEntry> {
        let entry = self.entries.remove(&id)?;
        self.recency.remove(&(entry.sequence, id));
        self.used_bytes = self.used_bytes.saturating_sub(entry.accounted_bytes);
        Some(entry)
    }

    fn evict_oldest(&mut self) -> bool {
        let Some((sequence, id)) = self.recency.pop_first() else {
            return false;
        };
        let Some(entry) = self.entries.remove(&id) else {
            return false;
        };
        debug_assert_eq!(entry.sequence, sequence);
        self.used_bytes = self.used_bytes.saturating_sub(entry.accounted_bytes);
        self.stats.evictions = self.stats.evictions.saturating_add(1);
        true
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

    use super::DecodedBlockCache;
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
        assert_eq!(result.evicted_blocks, 1);
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
}
