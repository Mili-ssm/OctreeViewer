// Pure helpers for the converter panel: option validation, mapping to core-engine options and
// result formatting. Kept DOM-free so Node can test them.

const GB = 1e9;

/** Writable output revisions: v8 (the default) first, then v7, the v7.N-pre revisions in ascending order, then v5, the v6.N-pre revisions and the experimental v8.1-pre and v8.2-pre. */
export const FORMAT_VERSIONS = ["v8", "v7", "v7.1-pre", "v7.2-pre", "v7.3-pre", "v7.4-pre", "v7.5-pre", "v5", "v6.1-pre", "v6.2-pre", "v6.3-pre", "v6.4-pre", "v8.1-pre", "v8.2-pre"];
export const SPATIAL_DIVISIONS = [2, 4, 8];
/** Experimental per-column-class entropy coder of `v8.2-pre` (`--attr-codec`); reachable by API only, with no form field. */
export const ATTR_CODECS = ["lz4", "cold", "cold-rgb", "all"];

const GLOBAL_FRAME_FORMATS = new Set(["v8", "v7", "v7.4-pre", "v7.5-pre", "v8.1-pre", "v8.2-pre"]);

/** Global-frame formats fix points per block and spatial divisions; the core rejects other values. */
export const isGlobalFrameFormat = (formatVersion) => GLOBAL_FRAME_FORMATS.has(formatVersion);

/** Values the global-frame formats fix (the core's 64,000 points per block and 2 divisions), as shown in the form. */
export const FIXED_FRAME_VALUES = Object.freeze({ pointsPerBlock: "64000", spatialDivisions: "2" });

/** Hint shown next to a disabled field, derived from `FIXED_FRAME_VALUES`. */
export const fixedFrameHint = (name) => `Fixed by the selected format (${Number(FIXED_FRAME_VALUES[name]).toLocaleString("en-US")}).`;

/**
 * Value to display in a fixed-frame field: the fixed one for a global-frame format, otherwise the
 * user's `saved` value (or the non-global default when none was saved).
 */
export const fixedFrameDisplayValue = (name, formatVersion, saved) =>
    isGlobalFrameFormat(formatVersion) ? FIXED_FRAME_VALUES[name] : (saved ?? OPTION_DEFAULTS[name]);

/**
 * Form strings for the options that formats do not fix; global-frame formats show
 * `FIXED_FRAME_VALUES` instead (see `fixedFrameDisplayValue`).
 */
export const OPTION_DEFAULTS = Object.freeze({
    formatVersion: "v8",
    pointsPerBlock: "262144",
    maxError: "0.01",
    spatialDivisions: "2",
    plannerBudgetMiB: "512",
    ramBudgetGB: "4",
});

const MAX_POINTS_PER_BLOCK = 1_000_000;
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

    if (FORMAT_VERSIONS.includes(input.formatVersion)) options.formatVersion = input.formatVersion;
    else errors.formatVersion = "Choose one of the listed format versions.";

    // Absent or `lz4` is the default and is not sent; any other scheme needs v8.2-pre.
    if (input.attrCodec !== undefined && input.attrCodec !== "lz4") {
        if (!ATTR_CODECS.includes(input.attrCodec)) errors.attrCodec = `unknown attribute codec "${input.attrCodec}" (expected lz4, cold, cold-rgb or all)`;
        else if (input.formatVersion !== "v8.2-pre") errors.attrCodec = `attribute codec ${input.attrCodec} needs format version v8.2-pre`;
        else options.attrCodec = input.attrCodec;
    }

    const fixedFrame = isGlobalFrameFormat(input.formatVersion);

    if (!fixedFrame) {
        const points = integerInRange(input.pointsPerBlock, 1, MAX_POINTS_PER_BLOCK, "Points per block");
        if (points.error) errors.pointsPerBlock = points.error;
        else options.pointsPerBlock = points.value;
    }

    const maxError = parseNumber(input.maxError);
    if (!Number.isFinite(maxError) || maxError < 0) errors.maxError = "Max error must be a number of 0 or more.";
    else options.maxError = maxError;

    if (!fixedFrame) {
        const divisions = Number(input.spatialDivisions);
        if (SPATIAL_DIVISIONS.includes(divisions)) options.spatialDivisions = divisions;
        else errors.spatialDivisions = `Spatial divisions must be ${SPATIAL_DIVISIONS.join(", ")}.`;
    }

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
