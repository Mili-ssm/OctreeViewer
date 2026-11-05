// THROWAWAY SPIKE CODE (R3 Task 5a): writes a small synthetic case in the format of
// examples/web-host/spike-cube-pool-format.md so the browser page can be tested without the Rust
// exporter. Usage: node examples/web-host/tests/spike-cube-pool-synthetic.mjs [OUT_DIR]
// Three nodes (side 1, depth 0; the third behind the first), two full levels (depth 3 draws octants 0-3, depth 4 octants 4-7),
// some cells hidden by the 8^3 mask and one empty tail slot.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const out = path.resolve(process.argv[2] ?? path.join(root, "target/r3-t5-synthetic/synthetic-pose-4px"));
fs.mkdirSync(out, { recursive: true });

const WIDTH = 1280, HEIGHT = 720;
const eye = [0.6, 0.5, 2.5];
const visualScale = 1;
// The third node sits behind the first one (the camera looks down -z), so Hi-Z occlusion has pages to cull.
const nodeMins = [[0, 0, 0], [1.2, 0, 0], [0.1, 0, -4]];
const levels = [{ depth: 3, shift: 0, octants: 0x0f }, { depth: 4, shift: 1, octants: 0xf0 }];
const SPP = { w10: 682, w16: 512 };

// Right-handed perspective (glam perspective_rh, depth 0..1), camera looking down -z, column-major.
function perspective(fovY, aspect, near, far) {
    const f = 1 / Math.tan(fovY / 2);
    const m = new Float32Array(16);
    m[0] = f / aspect; m[5] = f;
    m[10] = far / (near - far); m[11] = -1; m[14] = (near * far) / (near - far);
    return m;
}

let seed = 12345;
const rand = () => { seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0; return seed / 2 ** 32; };

const maskWords = nodeMins.map(() => {
    const w = new Uint32Array(16);
    for (let k = 0; k < 40; k++) { const bit = Math.floor(rand() * 512); w[bit >> 5] |= 1 << (bit & 31); }
    return w;
});

// Cells per (node, level): the complete level, in Morton-like (x, y, z) order.
const pages = [];
for (let node = 0; node < nodeMins.length; node++) {
    for (const level of levels) {
        const n = 2 ** level.depth;
        const cells = [];
        for (let x = 0; x < n; x++) for (let y = 0; y < n; y++) for (let z = 0; z < n; z++) {
            cells.push({ x, y, z, rgba: ((rand() * 256) | (rand() * 256) << 8 | (rand() * 256) << 16 | 255 << 24) >>> 0,
                intensity: (rand() * 65535) | 0, cls: (rand() * 10) | 0 });
        }
        pages.push({ node, level, cells });
    }
}

function visible(node, level, c) {
    const m = [c.x >> level.shift, c.y >> level.shift, c.z >> level.shift];
    const octant = 4 * (m[0] >> 2) + 2 * (m[1] >> 2) + (m[2] >> 2);
    if (!((level.octants >> octant) & 1)) return false;
    const bit = m[0] + 8 * m[1] + 64 * m[2];
    return !((maskWords[node][bit >> 5] >> (bit & 31)) & 1);
}

const f32 = new Float32Array(1);
function pack(widthName) {
    const spp = SPP[widthName];
    const pageList = [];
    for (const p of pages) {
        for (let start = 0; start < p.cells.length; start += spp) pageList.push({ ...p, chunk: p.cells.slice(start, start + spp) });
    }
    const wordsPerSlot = widthName === "w10" ? 3 : 4;
    const slots = pageList.length * spp;
    const inst = new Uint32Array(slots * wordsPerSlot).fill(0xffffffff);
    const pageTable = new ArrayBuffer(pageList.length * 48);
    const pf = new Float32Array(pageTable), pu = new Uint32Array(pageTable);
    const ranges = [];
    pageList.forEach((page, pi) => {
        const side = (1 / 2 ** page.level.depth) * visualScale;
        pf.set(nodeMins[page.node].map((v, a) => (v - eye[a]) * visualScale), pi * 12);
        pf[pi * 12 + 3] = side;
        pu.set([page.node, page.level.depth, 0, 0, 0, page.level.octants, page.level.shift, 0], pi * 12 + 4);
        page.chunk.forEach((c, i) => {
            const s = pi * spp + i, b = s * wordsPerSlot;
            if (widthName === "w10") {
                inst[b] = c.x | (c.y << 10) | (c.z << 20); inst[b + 1] = c.rgba; inst[b + 2] = c.intensity | ((c.cls & 0xffff) << 16);
            } else {
                inst[b] = c.x | (c.y << 16); inst[b + 1] = c.z; inst[b + 2] = c.rgba; inst[b + 3] = c.intensity | ((c.cls & 0xffff) << 16);
            }
        });
        ranges.push(pi * spp, page.chunk.length);
    });
    // slots_used ends one slot after the last filled slot of the last page: an empty tail slot.
    const last = pageList.length - 1;
    const slotsUsed = last * spp + pageList[last].chunk.length + 1;
    return { spp, pages: pageList.length, slotsUsed, inst: inst.subarray(0, slotsUsed * wordsPerSlot), pageTable, ranges: new Uint32Array(ranges), pageList };
}

const w10 = pack("w10"), w16 = pack("w16");
const i40 = [];
for (const page of w10.pageList) {
    const side = (1 / 2 ** page.level.depth) * visualScale;
    for (const c of page.chunk) {
        if (!visible(page.node, page.level, c)) continue;
        const o = nodeMins[page.node].map((v, a) => (v - eye[a]) * visualScale + (([c.x, c.y, c.z][a]) + 0.5) * side);
        const lin = ch => { const s = ch / 255; return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4; };
        i40.push(...o, side, lin(c.rgba & 255), lin((c.rgba >> 8) & 255), lin((c.rgba >> 16) & 255), 1, c.intensity, c.cls);
    }
}
const empty = w => w.pages * w.spp - w.slotsUsed + 1;
const meta = {
    dataset: "synthetic", pose: "pose", target_px: 4, viewport: [WIDTH, HEIGHT],
    view_proj: Array.from(perspective(Math.PI / 3, WIDTH / HEIGHT, 0.05, 20)),
    eye, visual_scale: visualScale, nodes: nodeMins.length, levels: levels.length,
    variants: {
        w10: { slots_per_page: SPP.w10, pages: w10.pages, slots_used: w10.slotsUsed, empty_slots: empty(w10) },
        w16: { slots_per_page: SPP.w16, pages: w16.pages, slots_used: w16.slotsUsed, empty_slots: empty(w16) },
    },
    drawn_cells: i40.length / 10, resident_cells: pages.reduce((s, p) => s + p.cells.length, 0),
    ranges_w10: w10.ranges.length / 2, ranges_w16: w16.ranges.length / 2, f32_max_error_px: 0,
};
const write = (name, data) => fs.writeFileSync(path.join(out, name), Buffer.from(data.buffer ?? data, data.byteOffset ?? 0, data.byteLength));
fs.writeFileSync(path.join(out, "case.json"), JSON.stringify(meta, null, 2));
write("w10_instances.bin", w10.inst); write("w16_instances.bin", w16.inst);
write("w10_pages.bin", new Uint8Array(w10.pageTable)); write("w16_pages.bin", new Uint8Array(w16.pageTable));
write("nodes.bin", Uint32Array.from(maskWords.flatMap(w => [...w])));
write("w10_ranges.bin", w10.ranges); write("w16_ranges.bin", w16.ranges);
write("i40_instances.bin", Float32Array.from(i40));
console.log(`wrote ${out}: drawn_cells=${meta.drawn_cells} w10 slots=${w10.slotsUsed} w16 slots=${w16.slotsUsed}`);
