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
    let note = `Dataset unit: ${crsUnit.unit} (${metresPerUnit} m).`;
    if (crsUnit.unrecognised) note = "Unknown CRS unit; metres assumed.";
    else if (crsUnit.assumed) note = "Metres assumed: the CRS declares no unit.";
    return { metresPerUnit, lengthDisabled: false, note };
}

// Default panel length of an opened dataset (ADR 0059): the file's p5 (`recommendedMinSize`, dataset
// units) shown in the unit that reads best, else 0.1 mm (ADR 0054). The value is rounded to 3
// significant digits, so the control and the viewer apply the same number. A geographic CRS has no
// length in metres: it keeps 0.1 mm (the Length option is disabled there anyway).
export const FALLBACK_LENGTH = Object.freeze({ value: 0.1, unit: "mm" });
export function defaultLength(recommendedMinSize, crsUnit) {
    if (!Number.isFinite(recommendedMinSize) || recommendedMinSize <= 0) return FALLBACK_LENGTH;
    const policy = crsUnitPolicy(crsUnit);
    if (policy.lengthDisabled) return FALLBACK_LENGTH;
    const metres = recommendedMinSize * policy.metresPerUnit;
    const unit = metres >= 1 ? "m" : metres >= 1e-2 ? "cm" : "mm";
    const value = Number((metres / UNIT_METRES[unit]).toPrecision(3));
    return Number.isFinite(value) && value > 0 ? { value, unit } : FALLBACK_LENGTH;
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
