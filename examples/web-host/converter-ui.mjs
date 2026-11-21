// Pure helpers for the converter panel: option validation, mapping to core-engine options and
// result formatting. Kept DOM-free so Node can test them.

const GB = 1e9;

/** Options V8 fixes (format, row order, points per block, spatial divisions): any value is an error. */
export const REMOVED_OPTIONS = Object.freeze(["formatVersion", "rowOrder", "pointsPerBlock", "spatialDivisions"]);

/** Form strings for the options a user can still set. */
export const OPTION_DEFAULTS = Object.freeze({
    maxError: "0.01",
    plannerBudgetMiB: "512",
    ramBudgetGB: "4",
});

const MAX_PLANNER_BUDGET_MIB = 4096;
const MAX_RAM_BUDGET_GB = 8;

function parseNumber(text) {
    const trimmed = String(text ?? "").trim();
    return trimmed === "" ? Number.NaN : Number(trimmed);
}

function integerInRange(text, min, max, label, unit = "") {
    const value = parseNumber(text);
    if (!Number.isInteger(value) || value < min || value > max) {
        return { error: `${label} must be a whole number from ${min.toLocaleString("en-US")} to ${max.toLocaleString("en-US")}${unit}.` };
    }
    return { value };
}

/**
 * Validates the raw form strings. Returns `{ errors, options }`: `errors` maps a field name to its
 * message (empty when valid) and `options` holds the core-engine options (`ramBudget` in bytes).
 */
export function validateConverterOptions(raw = {}) {
    const errors = {};
    const options = {};
    const input = { ...OPTION_DEFAULTS, ...raw };

    for (const name of REMOVED_OPTIONS) {
        if (input[name] !== undefined) errors[name] = `option ${name} was removed: V8 is the only format and fixes it`;
    }

    // Removed with the zstd codecs (ADR 0060): attribute payloads are LZ4 only, so no value is valid.
    if (input.attrCodec !== undefined) errors.attrCodec = "option attrCodec was removed (ADR 0060): attribute payloads are stored with LZ4 only";

    const maxError = parseNumber(input.maxError);
    if (!Number.isFinite(maxError) || maxError < 0) errors.maxError = "Max error must be a number of 0 or more.";
    else options.maxError = maxError;

    const planner = integerInRange(input.plannerBudgetMiB, 1, MAX_PLANNER_BUDGET_MIB, "Planner budget", " MiB");
    if (planner.error) errors.plannerBudgetMiB = planner.error;
    else options.maxMemoryMiB = planner.value;

    const ram = integerInRange(input.ramBudgetGB, 1, MAX_RAM_BUDGET_GB, "RAM budget", " GB");
    if (ram.error) errors.ramBudgetGB = ram.error;
    else options.ramBudget = ram.value * GB;

    return { errors, options };
}

const STAGE_LABELS = {
    inspecting: "Inspecting input",
    "planning-metadata": "Planning metadata",
    planning: "Planning spatial index",
    hierarchy: "Building hierarchy",
    writing: "Writing blocks",
    validating: "Validating output",
    publishing: "Publishing output",
    complete: "Complete",
};

/** "Writing blocks · 42%" (or just the label while the total is unknown). */
export function progressText(stage, processedPoints, totalPoints) {
    const label = STAGE_LABELS[stage] ?? "Converting";
    const processed = Number(processedPoints);
    const total = Number(totalPoints);
    if (!(total > 0)) return label;
    const percent = Math.min(100, Math.floor((processed / total) * 100));
    return `${label} · ${percent}%`;
}

const seconds = (ms) => `${(ms / 1000).toFixed(1)}s`;

/** Per-phase times that the result carries; only the phases that are present are listed. */
export function timingsText(timings) {
    if (!timings) return "";
    const phases = [["planning", timings.planningMs], ["writing", timings.writingMs], ["total", timings.totalMs]];
    return phases.filter(([, ms]) => Number.isFinite(ms)).map(([name, ms]) => `${name} ${seconds(ms)}`).join(" · ");
}

function bytesText(bytes) {
    if (bytes >= GB) return `${(bytes / GB).toFixed(1)} GB`;
    return `${(bytes / 1e6).toFixed(1)} MB`;
}

/** Compact spool statistics line, or "" when the result has none. */
export function spoolText(spool) {
    if (!spool) return "";
    const parts = [];
    if (Number.isFinite(spool.spilledBytes)) parts.push(`spilled ${bytesText(spool.spilledBytes)}`);
    if (Number.isFinite(spool.pagesPeak)) parts.push(`pages peak ${spool.pagesPeak.toLocaleString("en-US")}`);
    return parts.length ? `Spool: ${parts.join(" · ")}` : "";
}
