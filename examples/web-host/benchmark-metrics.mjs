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
        renderCallP50Ms: renderCalls.length ? percentile(renderCalls, 0.50) : null,
        renderCallP95Ms: renderCalls.length ? percentile(renderCalls, 0.95) : null,
        renderCallP99Ms: renderCalls.length ? percentile(renderCalls, 0.99) : null,
    };
}
