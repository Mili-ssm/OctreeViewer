export const FRAME_RATE_WINDOW_MS = 10_000;
export const FRAME_RATE_WARMUP_MS = 5_000;

export function measureFrameRate(frameReplies, benchmarkStartedAt, now = performance.now()) {
    if (!Number.isFinite(benchmarkStartedAt)) {
        return { sampleCount: 0, durationMs: 0, windowDurationMs: 0, meanFps: null, intervalP50Ms: null, intervalP95Ms: null };
    }
    const start = Math.max(now - FRAME_RATE_WINDOW_MS, benchmarkStartedAt + FRAME_RATE_WARMUP_MS);
    const windowDurationMs = Math.max(0, now - start);
    const samples = frameReplies.filter((sample) => sample.time >= start && sample.time <= now);
    if (samples.length < 2) {
        return {
            sampleCount: samples.length,
            durationMs: 0,
            windowDurationMs,
            meanFps: null,
            intervalP50Ms: null,
            intervalP95Ms: null,
            renderCallP50Ms: null,
            renderCallP95Ms: null,
            renderCallP99Ms: null,
        };
    }
    const times = samples.map((sample) => sample.time);
    const intervals = times.slice(1).map((time, index) => time - times[index]).sort((a, b) => a - b);
    const renderCalls = samples.map((sample) => sample.renderCallMs).filter(Number.isFinite).sort((a, b) => a - b);
    const percentile = (values, fraction) => values[Math.min(values.length - 1, Math.ceil(values.length * fraction) - 1)];
    const durationMs = times.at(-1) - times[0];
    return {
        sampleCount: intervals.length,
        durationMs,
        windowDurationMs,
        meanFps: durationMs > 0 ? intervals.length * 1000 / durationMs : null,
        intervalP50Ms: percentile(intervals, 0.50),
        intervalP95Ms: percentile(intervals, 0.95),
        intervalMaxMs: intervals.at(-1),
        renderCallP50Ms: renderCalls.length ? percentile(renderCalls, 0.50) : null,
        renderCallP95Ms: renderCalls.length ? percentile(renderCalls, 0.95) : null,
        renderCallP99Ms: renderCalls.length ? percentile(renderCalls, 0.99) : null,
    };
}

export function summarizeSeries(values) {
    const ordered = values.filter(Number.isFinite).sort((a, b) => a - b);
    if (ordered.length === 0) return { count: 0, p50: null, p95: null, max: null };
    const at = (fraction) => ordered[Math.min(ordered.length - 1, Math.ceil(ordered.length * fraction) - 1)];
    return { count: ordered.length, p50: at(0.50), p95: at(0.95), max: ordered.at(-1) };
}

/**
 * Primary latency metric (E2): gaps between consecutive frame replies seen on the main thread,
 * plus the request-to-reply round trip (queue wait + render). `renderCallMs` stays secondary.
 * Replies are `{time, renderCallMs, roundTripMs}`; `from`/`to` bound `time` (inclusive).
 */
export function summarizeFrameReplies(replies, from = -Infinity, to = Infinity) {
    const window = replies.filter((reply) => reply.time >= from && reply.time <= to);
    const intervals = window.slice(1).map((reply, index) => reply.time - window[index].time);
    return {
        replies: window.length,
        intervalMs: summarizeSeries(intervals),
        roundTripMs: summarizeSeries(window.map((reply) => reply.roundTripMs)),
        renderCallMs: summarizeSeries(window.map((reply) => reply.renderCallMs)),
    };
}

/** Compact timestamped entry for one worker `lod` message (E3). */
export function lodLogEntry(time, source, lod) {
    return {
        time,
        source,
        stagePx: lod.lodStagePixels,
        targetPx: lod.targetVoxelPixels,
        oversizedVoxels: lod.oversizedVoxels,
        uncoveredNodes: lod.uncoveredNodes,
        pendingBlocks: lod.pendingBlockIds?.length ?? 0,
        visibleBlocks: lod.visibleBlocks,
    };
}

/** Deterministic counters (E4) read from a `lodStats()` object. */
export function lodCounters(lod) {
    if (!lod) return null;
    const work = lod.refinementWork ?? {};
    return {
        sceneBuilds: work.sceneBuilds ?? null,
        voxelSourcePoints: work.voxelSourcePoints ?? null,
        loadedBlocks: work.loadedBlocks ?? null,
        evictedPendingBlocks: work.evictedPendingBlocks ?? null,
        uncoveredNodes: lod.uncoveredNodes ?? null,
        oversizedVoxels: lod.oversizedVoxels ?? null,
    };
}

// Whether a LOD stats object (`lod` of getStats / lodProgress) is at rest at the requested detail.
// Hierarchy pages of a paged format count like blocks: an anchor whose page is still pending keeps
// the target unreached. `pageBudgetExhausted` marks pending pages that can no longer be grafted
// because the page budget is pinned by the cut; nothing more will load until the view changes.
export function lodTargetReached(lod) {
    return Boolean(lod && !lod.lodError && lod.coverageComplete
        && !lod.pendingBlockIds?.length && !lod.pendingPageAnchors?.length
        && lod.lodStagePixels <= lod.targetVoxelPixels);
}

// Whether more LOD work is expected: false once the page budget blocks the remaining pages.
export function lodStillLoading(lod) {
    return Boolean(lod && !lod.lodError && !lod.pageBudgetExhausted
        && (lod.pendingBlockIds?.length || lod.pendingPageAnchors?.length
            || lod.lodStagePixels > lod.targetVoxelPixels));
}
