//! Byte format of one node's packed levels (R3 Task 10a1): the payload of a `nodeCells` message
//! from the build worker to the render worker.
//!
//! Layout, little endian, no padding:
//!
//! | Part | Bytes | Content |
//! | --- | --- | --- |
//! | header | 4 | magic `R3NC` |
//! | | 4 | `u32` format version (3) |
//! | | 8 | `u64` node id |
//! | | 8 | `u64` point count |
//! | | 48 | `f64` bounds min x y z, max x y z (world frame) |
//! | | 4 | `u32` level count |
//! | level | 4 | `u32` depth |
//! | | 4 | `u32` page count (at least 1) |
//! | page ×N | 58 | origin 3×`u64`, `first_cell` `u64`, `cells` `u32`, `box_max` 3×`u16`, `flags` `u32`, `rel_origin` 3×`u32` |
//! | | 512 × 16 | the page's instance slots, 4×`u32` each |
//!
//! Levels are in strictly increasing depth. The node and depth of a page are the ones of its
//! header and level, so they cannot disagree. Decoding validates everything it can see (lengths,
//! counts, depth ranges, `first_cell` continuity, the sentinel padding, relative coordinates
//! inside the page box) and never panics. `rel_origin` is not cross-checked against `origin` and
//! the node corner: the blob comes from the build worker and is trusted like the rest of the
//! geometry. Every count is checked against the bytes left before
//! anything is allocated, so a hostile length cannot reserve more than the input holds.

use std::fmt;

use viewer_core::{NodeId, SpatialBounds};

use crate::{
    CellInstance, MAX_PAGE_EXTENT, PAGE_SLOTS, PackedLevel, PageInfo, cells::driver::PackedNode,
};

const MAGIC: [u8; 4] = *b"R3NC";
/// Version of the byte format.
pub const CODEC_VERSION: u32 = 3;
const HEADER_BYTES: usize = 4 + 4 + 8 + 8 + 48 + 2 + 4;
const LEVEL_BYTES: usize = 8;
const PAGE_RECORD_BYTES: usize = 58;
const INSTANCE_BYTES: usize = size_of::<CellInstance>();
const PAGE_BYTES: usize = PAGE_RECORD_BYTES + PAGE_SLOTS * INSTANCE_BYTES;
/// Deepest level a cell coordinate (`u64`) can address.
const MAX_DEPTH: u32 = 63;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    /// The input ends before a field or an array does.
    Truncated,
    /// Bytes remain after the last level.
    TrailingBytes,
    BadMagic,
    UnsupportedVersion(u32),
    /// Which rule the input breaks.
    Invalid(&'static str),
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => write!(f, "node cells payload is truncated"),
            Self::TrailingBytes => write!(f, "node cells payload has trailing bytes"),
            Self::BadMagic => write!(f, "node cells payload has a wrong magic"),
            Self::UnsupportedVersion(version) => {
                write!(f, "node cells payload version {version} is not supported")
            }
            Self::Invalid(rule) => write!(f, "invalid node cells payload: {rule}"),
        }
    }
}

impl std::error::Error for CodecError {}

/// Checks one level as the decoder and the encoder both require it, so they accept the same
/// levels: pages and instances sized together, `first_cell` continuous, cell counts in range,
/// every page box inside the `2^depth` grid, sentinel padding, relative coordinates in the box.
fn validate_level(
    node: NodeId,
    depth: u8,
    pages: &[PageInfo],
    instances: &[CellInstance],
) -> Result<(), CodecError> {
    if u32::from(depth) > MAX_DEPTH
        || pages.is_empty()
        || pages.len().checked_mul(PAGE_SLOTS) != Some(instances.len())
    {
        return Err(CodecError::Invalid("level shape"));
    }
    let mut next_cell = 0_u64;
    for (page, slots) in pages.iter().zip(instances.chunks(PAGE_SLOTS)) {
        if page.node != node || page.depth != depth {
            return Err(CodecError::Invalid("page does not match its level"));
        }
        if page.cells == 0 || page.cells as usize > PAGE_SLOTS {
            return Err(CodecError::Invalid("page cell count"));
        }
        if page.first_cell as u64 != next_cell {
            return Err(CodecError::Invalid("first cell"));
        }
        next_cell += u64::from(page.cells);
        if page.flags > 0xffff {
            return Err(CodecError::Invalid("page flags"));
        }
        for axis in 0..3 {
            let extent = u64::from(page.box_max[axis]) + 1;
            // In `u128` so that depth 63 is checked like the others.
            let end = u128::from(page.origin[axis]) + u128::from(extent);
            if extent > MAX_PAGE_EXTENT || end > 1_u128 << depth {
                return Err(CodecError::Invalid("page box"));
            }
        }
        for (slot, instance) in slots.iter().enumerate() {
            if slot >= page.cells as usize {
                if *instance != CellInstance::EMPTY {
                    return Err(CodecError::Invalid("padding slot"));
                }
            } else {
                let rel = [
                    instance.0[0] & 0xffff,
                    instance.0[0] >> 16,
                    instance.0[1] & 0xffff,
                ];
                if instance.is_empty()
                    || (0..3).any(|axis| rel[axis] > u32::from(page.box_max[axis]))
                {
                    return Err(CodecError::Invalid("cell outside its page"));
                }
            }
        }
    }
    Ok(())
}

fn check_buildable((first, last): (u8, u8)) -> Result<(), CodecError> {
    if first > last || u32::from(last) > MAX_DEPTH {
        return Err(CodecError::Invalid("buildable range"));
    }
    Ok(())
}

/// Encodes a node. Levels without pages are left out (the driver drops them too); the others must
/// pass the checks of the decoder (see `validate_level`) and be in strictly increasing depth, so
/// the output always decodes.
pub fn encode_node_levels(node: &PackedNode) -> Result<Vec<u8>, CodecError> {
    let too_big = CodecError::Invalid("node too large");
    check_buildable(node.buildable)?;
    let levels: Vec<&PackedLevel> = node
        .levels
        .iter()
        .filter(|level| !level.pages.is_empty())
        .collect();
    let mut previous: Option<u8> = None;
    let mut total = HEADER_BYTES;
    for level in &levels {
        if level.node != node.id || previous.is_some_and(|depth| depth >= level.depth) {
            return Err(CodecError::Invalid("level does not match its node"));
        }
        previous = Some(level.depth);
        validate_level(node.id, level.depth, &level.pages, &level.instances)?;
        total = level
            .pages
            .len()
            .checked_mul(PAGE_BYTES)
            .and_then(|bytes| bytes.checked_add(LEVEL_BYTES))
            .and_then(|bytes| bytes.checked_add(total))
            .ok_or(too_big.clone())?;
    }
    let level_count = u32::try_from(levels.len()).map_err(|_| too_big.clone())?;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&CODEC_VERSION.to_le_bytes());
    out.extend_from_slice(&node.id.get().to_le_bytes());
    out.extend_from_slice(&node.points.to_le_bytes());
    for value in node.bounds.min().into_iter().chain(node.bounds.max()) {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&[node.buildable.0, node.buildable.1]);
    out.extend_from_slice(&level_count.to_le_bytes());
    for level in levels {
        out.extend_from_slice(&u32::from(level.depth).to_le_bytes());
        // Fits: `validate_level` bounded it by the checked `total`, which fits in `usize`, and the
        // page count is below `u32::MAX` on every target that can hold it.
        let pages = u32::try_from(level.pages.len()).map_err(|_| too_big.clone())?;
        out.extend_from_slice(&pages.to_le_bytes());
        for (page, slots) in level.pages.iter().zip(level.instances.chunks(PAGE_SLOTS)) {
            for value in page.origin {
                out.extend_from_slice(&value.to_le_bytes());
            }
            out.extend_from_slice(&(page.first_cell as u64).to_le_bytes());
            out.extend_from_slice(&page.cells.to_le_bytes());
            for value in page.box_max {
                out.extend_from_slice(&value.to_le_bytes());
            }
            out.extend_from_slice(&page.flags.to_le_bytes());
            for value in page.rel_origin {
                out.extend_from_slice(&value.to_le_bytes());
            }
            for instance in slots {
                for word in instance.0 {
                    out.extend_from_slice(&word.to_le_bytes());
                }
            }
        }
    }
    debug_assert_eq!(out.len(), total);
    Ok(out)
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], CodecError> {
        if self.0.len() < len {
            return Err(CodecError::Truncated);
        }
        let (head, tail) = self.0.split_at(len);
        self.0 = tail;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], CodecError> {
        Ok(self.take(N)?.try_into().expect("take returned N bytes"))
    }

    fn u16(&mut self) -> Result<u16, CodecError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, CodecError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, CodecError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn f64(&mut self) -> Result<f64, CodecError> {
        Ok(f64::from_le_bytes(self.array()?))
    }
}

/// Decodes and validates a node. The result is what `encode_node_levels` was given, minus the
/// levels without pages.
pub fn decode_node_levels(bytes: &[u8]) -> Result<PackedNode, CodecError> {
    let mut reader = Reader(bytes);
    if reader.array::<4>()? != MAGIC {
        return Err(CodecError::BadMagic);
    }
    let version = reader.u32()?;
    if version != CODEC_VERSION {
        return Err(CodecError::UnsupportedVersion(version));
    }
    let id = NodeId::new(reader.u64()?);
    let points = reader.u64()?;
    let mut bounds = [0.0_f64; 6];
    for value in &mut bounds {
        *value = reader.f64()?;
    }
    let bounds = SpatialBounds::new(
        [bounds[0], bounds[1], bounds[2]],
        [bounds[3], bounds[4], bounds[5]],
    )
    .map_err(|_| CodecError::Invalid("bounds"))?;
    let buildable = (reader.array::<1>()?[0], reader.array::<1>()?[0]);
    check_buildable(buildable)?;
    let level_count = reader.u32()? as usize;
    // Every level takes at least its header and one page, so this caps the allocation below.
    if level_count > reader.0.len() / (LEVEL_BYTES + PAGE_BYTES) {
        return Err(if level_count > MAX_DEPTH as usize + 1 {
            CodecError::Invalid("level count")
        } else {
            CodecError::Truncated
        });
    }
    let mut levels = Vec::with_capacity(level_count);
    let mut previous: Option<u32> = None;
    for _ in 0..level_count {
        let depth = reader.u32()?;
        if depth > MAX_DEPTH || previous.is_some_and(|before| before >= depth) {
            return Err(CodecError::Invalid("level depth"));
        }
        previous = Some(depth);
        let depth = depth as u8;
        let page_count = reader.u32()? as usize;
        if page_count == 0 {
            return Err(CodecError::Invalid("level without pages"));
        }
        let needed = page_count
            .checked_mul(PAGE_BYTES)
            .ok_or(CodecError::Invalid("page count"))?;
        if needed > reader.0.len() {
            return Err(CodecError::Truncated);
        }
        let mut pages = Vec::with_capacity(page_count);
        // Bounded by the remaining input through `needed` above.
        let slots = page_count
            .checked_mul(PAGE_SLOTS)
            .ok_or(CodecError::Invalid("page count"))?;
        let mut instances = Vec::with_capacity(slots);
        for _ in 0..page_count {
            let origin = [reader.u64()?, reader.u64()?, reader.u64()?];
            let first_cell =
                usize::try_from(reader.u64()?).map_err(|_| CodecError::Invalid("first cell"))?;
            let cells = reader.u32()?;
            let box_max = [reader.u16()?, reader.u16()?, reader.u16()?];
            let flags = reader.u32()?;
            let rel_origin = [reader.u32()?, reader.u32()?, reader.u32()?];
            for _ in 0..PAGE_SLOTS {
                instances.push(CellInstance([
                    reader.u32()?,
                    reader.u32()?,
                    reader.u32()?,
                    reader.u32()?,
                ]));
            }
            pages.push(PageInfo {
                node: id,
                depth,
                origin,
                first_cell,
                cells,
                box_max,
                flags,
                rel_origin,
            });
        }
        validate_level(id, depth, &pages, &instances)?;
        levels.push(PackedLevel {
            node: id,
            depth,
            instances,
            pages,
        });
    }
    if !reader.0.is_empty() {
        return Err(CodecError::TrailingBytes);
    }
    Ok(PackedNode {
        id,
        bounds,
        points,
        buildable,
        levels,
    })
}

#[cfg(test)]
#[path = "codec_tests.rs"]
mod tests;
