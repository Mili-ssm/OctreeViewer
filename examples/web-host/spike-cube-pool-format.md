# Cube-pool spike exchange format (R3 Task 5, throwaway)

Written by `coverage_spike --export DIR`, read by `spike-cube-pool.mjs`. All binary files are little-endian, tightly packed, no headers. One directory per case: `DIR/<dataset>-<pose>-<target>px/`.

## `case.json`

```json
{
  "dataset": "belvedere", "pose": "inside-leaf", "target_px": 4,
  "viewport": [1280, 720],
  "view_proj": [16 f32, column-major, camera-relative: maps (world - eye) to clip],
  "eye": [3 f64, dataset frame],
  "visual_scale": f64,
  "nodes": N, "levels": L,
  "variants": {
    "w10": { "slots_per_page": 682, "pages": P10, "slots_used": S10, "empty_slots": E10 },
    "w16": { "slots_per_page": 512, "pages": P16, "slots_used": S16, "empty_slots": E16 }
  },
  "drawn_cells": D, "resident_cells": R,
  "ranges_w10": R10, "ranges_w16": R16,
  "f32_max_error_px": f64
}
```

## Files

| File | Record | Bytes |
| --- | --- | --- |
| `w10_instances.bin` | per slot: `u32` position (x bits 0-9, y 10-19, z 20-29, relative to the page origin), `u32` rgba8 (r in the low byte), `u32` (intensity `u16` low, class `i16` high). Empty slot: position `0xFFFFFFFF` | 12 × slots |
| `w16_instances.bin` | per slot: `u32` (x `u16` low, y `u16` high), `u32` (z `u16` low, flags `u16` high, 0), `u32` rgba8, `u32` (intensity low, class high). Empty slot: first word `0xFFFFFFFF` and second word `0xFFFFFFFF` | 16 × slots |
| `w10_pages.bin`, `w16_pages.bin` | per page, 12 × `u32`: origin `f32`×3 (page min corner relative to the eye, dataset units × visual_scale), cell side `f32` (× visual_scale), node index `u32`, level depth `u32`, node-relative cell origin of the page at that depth `u32`×3, `octant_bits u32` (octants of this node that draw this level), mask shift `i32` stored as two's-complement `u32` (= depth − (node depth + 3); negative for leaf levels at node + 1 or + 2), pad `u32` | 48 × pages |
| `nodes.bin` | per node: 8³ mask as 16 × `u32` (bit `x + 8y + 64z`; 1 = hidden) | 64 × nodes |
| `w10_ranges.bin`, `w16_ranges.bin` | per draw range: `first_slot u32`, `slot_count u32` | 8 × ranges |
| `i40_instances.bin` | per drawn cell (no empty slots): the current `InstanceRaw` (`center f32×3` relative to the eye × visual_scale, `size f32`, `color f32×4` linear, `intensity f32`, `classification f32`) | 40 × drawn cells |

## Vertex-stage contract

- Slot `s` (= `instance_index`, which includes `firstInstance`) → page `s / slots_per_page`.
- Discard (emit a degenerate position) when the slot is empty; when the page's octant bit for the cell's octant is 0; or when the 8³ mask bit of the cell's mask cell is 1.
- The cell's mask cell is `(page_cell_origin + rel) >> mask_shift`, or `<< -mask_shift` when the shift is negative, and its octant is `4·(mx >> 2) + 2·(my >> 2) + (mz >> 2)`.
- Centre: `origin + (rel + 0.5) × side`; size: `side`. Colour: rgba8 → linear like `scene.rs`.
