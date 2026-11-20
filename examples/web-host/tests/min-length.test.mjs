import assert from "node:assert/strict";
import test from "node:test";
import { FALLBACK_LENGTH, crsUnitPolicy, defaultLength, lengthToDatasetUnits, minLengthCommands, parseSpacingFactor } from "../min-length.mjs";

test("lengths convert to dataset units assuming metres", () => {
    assert.equal(lengthToDatasetUnits(0.1, "mm"), 0.1 * 1e-3);
    assert.equal(lengthToDatasetUnits(2, "cm"), 2 * 1e-2);
    assert.equal(lengthToDatasetUnits(3, "m"), 3);
});

test("a non-positive, non-finite or unit-less length is NaN", () => {
    for (const value of [0, -1, Number.NaN, Infinity]) assert.ok(Number.isNaN(lengthToDatasetUnits(value, "mm")));
    assert.ok(Number.isNaN(lengthToDatasetUnits(1, "km")));
});

test("the spacing factor needs at least 0.25", () => {
    assert.equal(parseSpacingFactor(1), 1);
    assert.equal(parseSpacingFactor(0.25), 0.25);
    for (const value of [0.2, 0, -1, Number.NaN, Infinity]) assert.ok(Number.isNaN(parseSpacingFactor(value)));
});

test("each mode sends its minLength and continuity commands", () => {
    assert.deepEqual(minLengthCommands("length", 1e-4, 1), [["minLength", { side: 1e-4 }], ["continuity", { enabled: false }]]);
    assert.deepEqual(minLengthCommands("spacing", 1e-4, 2), [["minLength", { side: 1e-4 }], ["continuity", { enabled: true, factor: 2 }]]);
    assert.deepEqual(minLengthCommands("off", 1e-4, 1), [["minLength", { side: 0 }], ["continuity", { enabled: false }]]);
    assert.throws(() => minLengthCommands("bogus", 1, 1), TypeError);
});

test("lengths convert with the metres per dataset unit", () => {
    const usFoot = 0.3048006096012192;
    assert.equal(lengthToDatasetUnits(3, "m", usFoot), 3 / usFoot);
    assert.equal(lengthToDatasetUnits(10, "cm", 0.3048), 0.1 / 0.3048);
    assert.equal(lengthToDatasetUnits(1, "m", 1000), 1e-3);
    for (const bad of [0, -1, Number.NaN, null]) assert.ok(Number.isNaN(lengthToDatasetUnits(1, "m", bad)));
});

test("the CRS unit policy disables length only for geographic CRS and notes assumed metres", () => {
    assert.deepEqual(crsUnitPolicy(null), { metresPerUnit: 1, lengthDisabled: false, note: "" });
    const geographic = crsUnitPolicy({ unit: "degree", metresPerUnit: null, geographic: true, assumed: false });
    assert.equal(geographic.lengthDisabled, true);
    assert.match(geographic.note, /degrees/);
    const assumed = crsUnitPolicy({ unit: "metre", metresPerUnit: 1, geographic: false, assumed: true });
    assert.deepEqual([assumed.lengthDisabled, assumed.metresPerUnit], [false, 1]);
    assert.match(assumed.note, /Metres assumed/);
    const unknown = crsUnitPolicy({ unit: "metre", metresPerUnit: 1, geographic: false, assumed: true, unrecognised: true });
    assert.equal(unknown.note, "Unknown CRS unit; metres assumed.");
    assert.equal(unknown.lengthDisabled, false);
    const foot = crsUnitPolicy({ unit: "foot", metresPerUnit: 0.3048, geographic: false, assumed: false });
    assert.deepEqual([foot.lengthDisabled, foot.metresPerUnit], [false, 0.3048]);
    assert.match(foot.note, /foot/);
});

test("the default length is the file's p5 in the unit that reads best, else 0.1 mm (ADR 0059)", () => {
    const metres = { unit: "metre", metresPerUnit: 1, geographic: false };
    assert.deepEqual(defaultLength(0.172, metres), { value: 17.2, unit: "cm" });
    assert.deepEqual(defaultLength(0.001234, metres), { value: 1.23, unit: "mm" });
    assert.deepEqual(defaultLength(2.5, metres), { value: 2.5, unit: "m" });
    assert.deepEqual(defaultLength(0.172, null), { value: 17.2, unit: "cm" });
    // The p5 is in dataset units: a foot (0.3048 m) file converts to metres first.
    assert.deepEqual(defaultLength(1, { unit: "foot", metresPerUnit: 0.3048, geographic: false }), { value: 30.5, unit: "cm" });
    assert.equal(lengthToDatasetUnits(17.2, "cm", 1), 17.2 * 1e-2);
});

// The panel path: control value (shown, rounded) -> side sent to the viewer, as main.js computes it.
function sentSide(recommendedMinSize, crsUnit) {
    const { value, unit } = defaultLength(recommendedMinSize, crsUnit);
    return lengthToDatasetUnits(value, unit, crsUnitPolicy(crsUnit).metresPerUnit);
}

test("the side sent for a default length is p5 within the 3-digit rounding (0.5 %)", () => {
    const metres = { unit: "metre", metresPerUnit: 1, geographic: false };
    const foot = { unit: "foot", metresPerUnit: 0.3048, geographic: false };
    for (const [p5, crs] of [[0.172, metres], [0.001234, metres], [2.5, metres], [0.17, null], [1, foot], [0.0004567, metres]]) {
        const side = sentSide(p5, crs);
        assert.ok(Math.abs(side - p5) / p5 <= 0.005, `p5 ${p5} -> side ${side}`);
    }
    assert.ok(Math.abs(sentSide(0.172, metres) - 0.172) < 1e-12);
    // 0.1 mm when there is no p5 (dataset units are metres): the API default of 1e-4.
    assert.ok(Math.abs(sentSide(null, null) - 1e-4) < 1e-18);
});

test("no usable p5, or a geographic CRS, keeps 0.1 mm", () => {
    for (const p5 of [null, undefined, 0, -1, Number.NaN, Infinity]) assert.deepEqual(defaultLength(p5, null), FALLBACK_LENGTH);
    assert.deepEqual(defaultLength(0.5, { unit: "degree", metresPerUnit: 1, geographic: true }), FALLBACK_LENGTH);
    assert.deepEqual(FALLBACK_LENGTH, { value: 0.1, unit: "mm" });
    assert.ok(Math.abs(sentSide(Number.NaN, null) - 1e-4) < 1e-18);
});
