// Minimum cube size of the demo panel: the pure part (unit conversion and the commands each mode
// sends). The dataset unit comes from the stored CRS (`crsUnit` of the open result, ADR 0049 D6).
const UNIT_METRES = { mm: 1e-3, cm: 1e-2, m: 1 };
export const MIN_CELL_FACTOR = 0.25;

// Value in `unit` -> dataset units (`metresPerUnit` metres each); NaN when the value is not a finite
// positive number, the unit is unknown or `metresPerUnit` is not a finite positive number.
export function lengthToDatasetUnits(value, unit, metresPerUnit = 1) {
    const scale = UNIT_METRES[unit];
    if (scale === undefined || !Number.isFinite(value) || value <= 0) return Number.NaN;
    if (!Number.isFinite(metresPerUnit) || metresPerUnit <= 0) return Number.NaN;
    return value * scale / metresPerUnit;
}

// How the length option reads the CRS unit of the open dataset (`null` before one is open):
// the metres per dataset unit, whether the length option is unusable (geographic CRS) and the note.
export function crsUnitPolicy(crsUnit) {
    if (!crsUnit) return { metresPerUnit: 1, lengthDisabled: false, note: "" };
    if (crsUnit.geographic) {
        return { metresPerUnit: 1, lengthDisabled: true, note: "Coordinates are in degrees (geographic CRS): no length in metres applies." };
    }
    const metresPerUnit = Number.isFinite(crsUnit.metresPerUnit) && crsUnit.metresPerUnit > 0 ? crsUnit.metresPerUnit : 1;
    const note = crsUnit.assumed ? "Metres assumed: the CRS declares no unit." : `Dataset unit: ${crsUnit.unit} (${metresPerUnit} m).`;
    return { metresPerUnit, lengthDisabled: false, note };
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
