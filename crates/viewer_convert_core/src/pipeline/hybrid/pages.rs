//! Page allocator over spill zones. Pages are handed out lowest index first so a zone empties as
//! soon as its pages are freed; an empty zone is reported so the caller can truncate it.

use std::{collections::BTreeSet, io};

use crate::error::ConvertError;

pub struct PageAllocator {
    page_bytes: u64,
    zone_bytes: u64,
    pages_per_zone: u32,
    total_pages: u32,
    free: BTreeSet<u32>,
    next_unused: u32,
    live_per_zone: Vec<u32>,
    in_use: u32,
    peak: u32,
    zones_released: u32,
}

impl PageAllocator {
    pub fn new(zone_count: u32, zone_bytes: u64, page_bytes: u32) -> Result<Self, ConvertError> {
        let pages_per_zone = if page_bytes == 0 {
            0
        } else {
            u32::try_from(zone_bytes / u64::from(page_bytes)).unwrap_or(u32::MAX)
        };
        if pages_per_zone == 0 {
            return Err(ConvertError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a spill zone must hold at least one page",
            )));
        }
        Ok(Self {
            page_bytes: u64::from(page_bytes),
            zone_bytes,
            pages_per_zone,
            total_pages: pages_per_zone.saturating_mul(zone_count),
            free: BTreeSet::new(),
            next_unused: 0,
            live_per_zone: vec![0; zone_count as usize],
            in_use: 0,
            peak: 0,
            zones_released: 0,
        })
    }

    pub fn alloc(&mut self) -> Result<u32, ConvertError> {
        let page = if let Some(page) = self.free.pop_first() {
            page
        } else if self.next_unused < self.total_pages {
            self.next_unused += 1;
            self.next_unused - 1
        } else {
            return Err(ConvertError::Io(io::Error::other(format!(
                "spill zones exhausted: all {} zones of {} MiB are in use; the pre-run estimate was too low",
                self.live_per_zone.len(),
                self.zone_bytes >> 20
            ))));
        };
        self.live_per_zone[(page / self.pages_per_zone) as usize] += 1;
        self.in_use += 1;
        self.peak = self.peak.max(self.in_use);
        Ok(page)
    }

    /// Frees `page`; returns the zone to release when that was its last live page.
    pub fn free(&mut self, page: u32) -> Option<u32> {
        debug_assert!(page < self.next_unused && !self.free.contains(&page));
        let zone = page / self.pages_per_zone;
        self.free.insert(page);
        self.in_use -= 1;
        let live = &mut self.live_per_zone[zone as usize];
        *live -= 1;
        (*live == 0).then(|| {
            self.zones_released += 1;
            zone
        })
    }

    /// `(zone, byte offset in the zone)` of `page`.
    pub fn locate(&self, page: u32) -> (u32, u64) {
        (
            page / self.pages_per_zone,
            u64::from(page % self.pages_per_zone) * self.page_bytes,
        )
    }

    pub fn in_use(&self) -> u32 {
        self.in_use
    }

    pub fn peak(&self) -> u32 {
        self.peak
    }

    pub fn zones_released(&self) -> u32 {
        self.zones_released
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reuses_the_lowest_free_page_first() {
        let mut pages = PageAllocator::new(2, 4 * 64, 64).unwrap();
        let all: Vec<u32> = (0..5).map(|_| pages.alloc().unwrap()).collect();
        assert_eq!(all, [0, 1, 2, 3, 4]);
        pages.free(3);
        pages.free(1);
        assert_eq!(pages.alloc().unwrap(), 1);
        assert_eq!(pages.alloc().unwrap(), 3);
        assert_eq!(pages.alloc().unwrap(), 5);
        assert_eq!((pages.in_use(), pages.peak()), (6, 6));
    }

    #[test]
    fn maps_pages_to_zones_and_releases_empty_zones() {
        let mut pages = PageAllocator::new(2, 4 * 64, 64).unwrap();
        for _ in 0..6 {
            pages.alloc().unwrap();
        }
        assert_eq!(pages.locate(0), (0, 0));
        assert_eq!(pages.locate(3), (0, 192));
        assert_eq!(pages.locate(4), (1, 0));
        assert_eq!(pages.locate(5), (1, 64));
        assert_eq!(pages.free(4), None);
        assert_eq!(pages.free(5), Some(1));
        assert_eq!(pages.zones_released(), 1);
        // The emptied zone is reused before a fresh page is taken.
        assert_eq!(pages.alloc().unwrap(), 4);
    }

    #[test]
    fn reports_exhaustion_with_the_estimate_hint() {
        let mut pages = PageAllocator::new(2, 1 << 20, 1 << 19).unwrap();
        for _ in 0..4 {
            pages.alloc().unwrap();
        }
        assert_eq!(
            pages.alloc().unwrap_err().to_string(),
            "spill zones exhausted: all 2 zones of 1 MiB are in use; the pre-run estimate was too low"
        );
    }

    #[test]
    fn rejects_zones_smaller_than_a_page() {
        assert!(PageAllocator::new(1, 10, 64).is_err());
        assert!(PageAllocator::new(1, 64, 0).is_err());
    }
}
