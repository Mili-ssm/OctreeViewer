
@group(1) @binding(0) var<storage, read> inst: array<vec4<u32>>;
@group(1) @binding(1) var<storage, read> pages: array<CellPage>;
@group(1) @binding(2) var<storage, read> masks: array<u32>;
@group(1) @binding(3) var<storage, read_write> page_list: array<atomic<u32>>;
@group(1) @binding(4) var<storage, read_write> visible: array<u32>;
@group(1) @binding(5) var<storage, read_write> draw_args: array<atomic<u32>>;
@group(1) @binding(6) var<storage, read_write> bits: array<u32>;
@group(2) @binding(0) var<storage, read_write> dispatch_args: array<u32>;

// Visibility bits: one word per page in each half, then sixteen words (512 slot bits) per page.
fn page_bit_index(half: u32, page: u32) -> u32 {
    return half * frame.capacity + page;
}

fn slot_word_index(half: u32, page: u32, word: u32) -> u32 {
    return 2u * frame.capacity + half * frame.capacity * 16u + page * 16u + word;
}

const COUNTER_BASE: u32 = 10u;

// Phase 2 writes its draw arguments after those of phase 1.
fn args_base() -> u32 {
    return select(0u, 5u, PHASE == 2u);
}

fn page_box_hi(page: CellPage) -> vec3<f32> {
    let box_max = vec3<f32>(
        f32(page.box_xy & 0xffffu), f32(page.box_xy >> 16u), f32(page.box_z_flags & 0xffffu));
    return fma(box_max + vec3<f32>(1.0), vec3<f32>(page.side), page.origin);
}

@compute @workgroup_size(64)
fn page_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = id.x;
    if p >= frame.num_pages { return; }
    let page = pages[p];
    // The page box starts at the page origin: its minimum cell has relative coordinate 0.
    let hi = page_box_hi(page);
    // An inert page (octant word 0) is never kept, but phase 2 still writes its bit.
    var keep = page.octants != 0u && !cell_outside_frustum(page.origin, hi);
    if PHASE == 1u && keep {
        keep = bits[page_bit_index(frame.prev, p)] != 0u;
    }
    if PHASE == 2u {
        if keep { keep = !cell_occluded(page.origin, hi); }
        bits[page_bit_index(1u - frame.prev, p)] = select(0u, 1u, keep);
    }
    if keep {
        let i = atomicAdd(&page_list[0], 1u);
        atomicStore(&page_list[1u + i], p);
    }
}

@compute @workgroup_size(1)
fn finalize_main() {
    dispatch_args[0] = atomicLoad(&page_list[0]);
    dispatch_args[1] = 1u;
    dispatch_args[2] = 1u;
    let base = args_base();
    atomicStore(&draw_args[base], 36u);
    atomicStore(&draw_args[base + 1u], 0u);
    atomicStore(&draw_args[base + 2u], 0u);
    atomicStore(&draw_args[base + 3u], 0u);
    atomicStore(&draw_args[base + 4u], 0u);
}

// Cell mode (`lod_mode == 1`): the WGSL twin of `cell_lod`. Sums and products are written in the
// same order as the Rust, with no `fma`. A corner at or behind the eye plane gives a finite value
// far above any target (3.0e38, just under the f32 maximum of about 3.4e38; WGSL does not guarantee
// `inf`), which refines like the Rust `+inf`.
const CELL_NEAR_EDGE: f32 = 3.0e38;

fn cell_dot(a: vec3<f32>, b: vec3<f32>) -> f32 {
    return a.x * b.x + a.y * b.y + a.z * b.z;
}

// The smallest projected edge among the 8 children of the cell: see `min_child_edge_px` in
// cell_lod.rs, whose operation order this copies.
fn cell_min_child_edge_px(lo: vec3<f32>, side: f32) -> f32 {
    let half = side * 0.5;
    var projected: array<vec2<f32>, 27>;
    var behind = 0u;
    for (var index = 0u; index < 27u; index++) {
        let steps = vec3<u32>(index % 3u, (index / 3u) % 3u, index / 9u);
        let point = vec3<f32>(
            select(select(lo.x, lo.x + half, steps.x == 1u), (lo.x + half) + half, steps.x == 2u),
            select(select(lo.y, lo.y + half, steps.y == 1u), (lo.y + half) + half, steps.y == 2u),
            select(select(lo.z, lo.z + half, steps.z == 1u), (lo.z + half) + half, steps.z == 2u));
        let depth = cell_dot(point, frame.forward);
        if depth <= 0.0 {
            behind |= 1u << index;
            continue;
        }
        projected[index] = vec2<f32>(
            frame.focal * (cell_dot(point, frame.right) / depth),
            frame.focal * (cell_dot(point, frame.up) / depth));
    }
    var smallest = CELL_NEAR_EDGE;
    for (var child = 0u; child < 8u; child++) {
        let base = (child & 1u) + ((child >> 1u) & 1u) * 3u + ((child >> 2u) & 1u) * 9u;
        var blocked = false;
        for (var corner = 0u; corner < 8u; corner++) {
            let point = base + (corner & 1u) + ((corner >> 1u) & 1u) * 3u + (corner >> 2u) * 9u;
            if ((behind >> point) & 1u) != 0u { blocked = true; }
        }
        if blocked { continue; }
        var maximum = 0.0;
        for (var corner = 0u; corner < 8u; corner++) {
            for (var axis = 0u; axis < 3u; axis++) {
                if ((corner >> axis) & 1u) != 0u { continue; }
                let first = base + (corner & 1u) + ((corner >> 1u) & 1u) * 3u + (corner >> 2u) * 9u;
                let other = corner | (1u << axis);
                let second = base + (other & 1u) + ((other >> 1u) & 1u) * 3u + (other >> 2u) * 9u;
                let dx = projected[first].x - projected[second].x;
                let dy = projected[first].y - projected[second].y;
                maximum = max(maximum, sqrt(dx * dx + dy * dy));
            }
        }
        smallest = min(smallest, maximum);
    }
    return smallest;
}

fn cell_refines(lo: vec3<f32>, side: f32) -> bool {
    return side * 0.5 >= frame.min_side && cell_min_child_edge_px(lo, side) >= frame.target_px;
}

// Is any set mask bit inside the cell `rel` of the node record at word `record`, `level` below the
// node depth? From level 3 the 8^3 bit; above it the 4^3, 2^3 and 1^3 mips (words 22..25).
fn cell_masked_any(record: u32, level: u32, rel: vec3<u32>) -> bool {
    if level >= 3u {
        let m = rel >> vec3<u32>(level - 3u);
        let bit = m.x + 8u * m.y + 64u * m.z;
        return ((masks[record + (bit >> 5u)] >> (bit & 31u)) & 1u) != 0u;
    }
    if level == 0u { return ((masks[record + 24u] >> 8u) & 1u) != 0u; }
    if level == 1u {
        return ((masks[record + 24u] >> (rel.x + 2u * rel.y + 4u * rel.z)) & 1u) != 0u;
    }
    let bit = rel.x + 4u * rel.y + 16u * rel.z;
    return ((masks[record + 22u + (bit >> 5u)] >> (bit & 31u)) & 1u) != 0u;
}

// E4/E5: does this cell draw its points? Mirrors `cell_lod::draws`, ancestors from the coarsest.
fn cell_draws(page: CellPage, cell: vec4<u32>) -> bool {
    if all(page.rel == vec3<u32>(0xffffffffu)) { return false; }
    let record = page.node * 32u;
    let node_depth = masks[record + 19u];
    if page.depth < node_depth || page.depth - node_depth > 31u { return false; }
    let level = page.depth - node_depth;
    let active_depths = masks[record + 20u];
    if ((active_depths >> level) & 1u) == 0u { return false; }
    let rel = page.rel + cell_rel(cell);
    if cell_masked_any(record, level, rel) { return false; }
    let anchor = vec3<f32>(
        bitcast<f32>(masks[record + 16u]),
        bitcast<f32>(masks[record + 17u]),
        bitcast<f32>(masks[record + 18u]));
    let finer = ((active_depths >> level) >> 1u) != 0u || masks[record + 21u] != 0u;
    if finer && cell_refines(anchor + vec3<f32>(rel) * page.side, page.side) { return false; }
    var coarser = active_depths & ((1u << level) - 1u);
    while coarser != 0u {
        let ancestor_level = countTrailingZeros(coarser);
        coarser &= coarser - 1u;
        let up = level - ancestor_level;
        let ancestor_rel = rel >> vec3<u32>(up);
        let ancestor_side = page.side * bitcast<f32>((127u + up) << 23u);
        let ancestor_lo = anchor + vec3<f32>(ancestor_rel) * ancestor_side;
        if !(cell_refines(ancestor_lo, ancestor_side)
            || cell_masked_any(record, ancestor_level, ancestor_rel)) {
            return false;
        }
    }
    return true;
}

var<workgroup> scan: array<u32, 256>;
var<workgroup> next_words: array<atomic<u32>, 16>;
var<workgroup> wg_base: u32;

// Bit 0: draw the slot in this phase. Bit 1: the slot is visible for the next frame (phase 2).
// Phase 2 skips what phase 1 drew with the predicate phase 1 used: the page was kept on its
// previous bit (the page pass of phase 1) and the slot passed on its previous bit.
fn slot_state(page: CellPage, page_index: u32, local: u32, page_was_drawn: bool) -> u32 {
    let cell = inst[page_index * 512u + local];
    if cell_is_empty(cell) { return 0u; }
    // Without a LOD view (tests and benches) every unmasked cell of the active pages draws.
    if frame.lod_mode == 1u {
        if !cell_draws(page, cell) { return 0u; }
    } else if !cell_spans_octants(cell) {
        if ((page.octants >> cell_octant(cell)) & 1u) == 0u { return 0u; }
        let bit = cell_mask_index(cell);
        if ((masks[page.node * 32u + (bit >> 5u)] >> (bit & 31u)) & 1u) == 1u { return 0u; }
    }
    let lo = cell_lo(page, cell);
    let hi = cell_hi(page, cell);
    if cell_outside_frustum(lo, hi) { return 0u; }
    if PHASE == 0u { return 1u; }
    let was_visible =
        ((bits[slot_word_index(frame.prev, page_index, local >> 5u)] >> (local & 31u)) & 1u) == 1u;
    if PHASE == 1u { return select(0u, 1u, was_visible); }
    if cell_occluded(lo, hi) { return 0u; }
    return select(3u, 2u, page_was_drawn && was_visible);
}

@compute @workgroup_size(256)
fn slot_main(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_index) lid: u32) {
    let page_index = atomicLoad(&page_list[1u + wid.x]);
    let page = pages[page_index];
    var page_was_drawn = false;
    if PHASE == 2u {
        page_was_drawn = bits[page_bit_index(frame.prev, page_index)] != 0u;
        if lid < 16u { atomicStore(&next_words[lid], 0u); }
        workgroupBarrier();
    }
    let l0 = lid * 2u;
    let l1 = l0 + 1u;
    let s0 = slot_state(page, page_index, l0, page_was_drawn);
    let s1 = slot_state(page, page_index, l1, page_was_drawn);
    if PHASE == 2u {
        if (s0 & 2u) != 0u { atomicOr(&next_words[l0 >> 5u], 1u << (l0 & 31u)); }
        if (s1 & 2u) != 0u { atomicOr(&next_words[l1 >> 5u], 1u << (l1 & 31u)); }
    }
    let c0 = s0 & 1u;
    let c1 = s1 & 1u;
    scan[lid] = c0 + c1;
    workgroupBarrier();
    for (var off = 1u; off < 256u; off = off << 1u) {
        var v = scan[lid];
        if lid >= off { v += scan[lid - off]; }
        workgroupBarrier();
        scan[lid] = v;
        workgroupBarrier();
    }
    let total = scan[255];
    if frame.counters != 0u {
        let counter = COUNTER_BASE + min(page.depth, 63u);
        if c0 == 1u { atomicAdd(&draw_args[counter], 1u); }
        if c1 == 1u { atomicAdd(&draw_args[counter], 1u); }
    }
    let exclusive = scan[lid] - (c0 + c1);
    if lid == 0u && total > 0u { wg_base = atomicAdd(&draw_args[args_base() + 1u], total); }
    workgroupBarrier();
    if total > 0u {
        let out = wg_base + exclusive;
        if c0 == 1u { visible[out] = page_index * 512u + l0; }
        if c1 == 1u { visible[out + c0] = page_index * 512u + l1; }
    }
    if PHASE == 2u && lid < 16u {
        bits[slot_word_index(1u - frame.prev, page_index, lid)] = atomicLoad(&next_words[lid]);
    }
}
