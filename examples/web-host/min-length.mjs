// Minimum cube size of the demo panel: the pure part (unit conversion and the commands each mode
// sends). Metres are assumed because the CRS unit is not read yet (ADR 0049).
const UNIT_METRES = { mm: 1e-3, cm: 1e-2, m: 1 };
export const MIN_CELL_FACTOR = 0.25;

// Value in `unit` -> dataset units; NaN when the value is not a finite positive number or the unit is unknown.
export function lengthToDatasetUnits(value, unit) {
    const scale = UNIT_METRES[unit];
    if (scale === undefined || !Number.isFinite(value) || value <= 0) return Number.NaN;
    return value * scale;
}

// Cloud-spacing factor; NaN below the demo's minimum or when not finite.
export function parseSpacingFactor(value) {
    return Number.isFinite(value) && value >= MIN_CELL_FACTOR ? value : Number.NaN;
}

// Worker commands for a mode ("length" | "spacing" | "off"): `side` in dataset units (kept when
// the cloud-spacing side is larger: the larger wins, ADR 0054), `factor` of the spacing mode.
export function minLengthCommands(mode, side, factor) {
    switch (mode) {
        case "length":
            return [["minLength", { side }], ["continuity", { enabled: false }]];
        case "spacing":
            return [["minLength", { side }], ["continuity", { enabled: true, factor }]];
        case "off":
            return [["minLength", { side: 0 }], ["continuity", { enabled: false }]];
        default:
            throw new TypeError(`unknown minimum cube size mode ${mode}`);
    }
}
