// RF-07 dynamic px mode: a pure controller that picks the target voxel pixels from frame-cost
// windows. It owns no clock and no timers; the caller closes a window every WINDOW_MS and calls
// `update`. Rule (docs/plans/mejoras-pendientes-13-11-2026.md, B4): coarsen one step when the
// window's p95 is over budget, refine one step after consecutive windows with headroom, and wait
// DWELL_MS after any change so the scene does not oscillate.

export const PX_LADDER = [1, 2, 4, 8];
export const WINDOW_MS = 1000;
export const DWELL_MS = 2000;
// GPU frame time p95 (ms): above COARSEN is overload, below REFINE for REFINE_WINDOWS is headroom.
// The frame-interval fallback is blunter, so it only refines under a stricter, longer condition.
const RULES = {
    gpu: { coarsenAbove: 16.7, refineBelow: 10, refineWindows: 2 },
    interval: { coarsenAbove: 20, refineBelow: 17.5, refineWindows: 3 },
};

/** The ladder rung nearest to `px` (in log scale), so a fixed target maps onto the ladder. */
export function snapToLadder(px) {
    let best = 0;
    for (let index = 1; index < PX_LADDER.length; index++) {
        if (Math.abs(Math.log2(PX_LADDER[index] / px)) < Math.abs(Math.log2(PX_LADDER[best] / px))) best = index;
    }
    return best;
}

export function createPxController({ initialPx = 4, startMs = 0 } = {}) {
    let index = snapToLadder(initialPx);
    let lastChangeMs = startMs - DWELL_MS;
    let goodWindows = 0;
    let goodSource = null;
    let last = { signalMs: null, source: null };

    return {
        get targetPx() { return PX_LADDER[index]; },
        get last() { return last; },
        /**
         * Closes one window. `gpuP95Ms` and `intervalP95Ms` are `null` when the window has no usable
         * samples; the GPU signal wins when present. Returns the new target when it changed, else null.
         */
        update(nowMs, { gpuP95Ms = null, intervalP95Ms = null }) {
            const source = gpuP95Ms !== null ? "gpu" : intervalP95Ms !== null ? "interval" : null;
            if (source === null) {
                // An empty window (idle scene) says nothing about cost: hold and restart the streak.
                last = { signalMs: null, source: null };
                goodWindows = 0;
                return null;
            }
            const signalMs = source === "gpu" ? gpuP95Ms : intervalP95Ms;
            last = { signalMs, source };
            const rule = RULES[source];
            if (source !== goodSource) { goodWindows = 0; goodSource = source; }
            const over = signalMs > rule.coarsenAbove;
            goodWindows = signalMs < rule.refineBelow ? goodWindows + 1 : 0;
            if (nowMs - lastChangeMs < DWELL_MS) return null;
            let next = index;
            if (over && index < PX_LADDER.length - 1) next = index + 1;
            else if (goodWindows >= rule.refineWindows && index > 0) next = index - 1;
            if (next === index) return null;
            index = next;
            lastChangeMs = nowMs;
            goodWindows = 0;
            return PX_LADDER[index];
        },
    };
}
