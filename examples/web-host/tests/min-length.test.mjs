import assert from "node:assert/strict";
import test from "node:test";
import { crsUnitPolicy, lengthToDatasetUnits, minLengthCommands, parseSpacingFactor } from "../min-length.mjs";

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
    const foot = crsUnitPolicy({ unit: "foot", metresPerUnit: 0.3048, geographic: false, assumed: false });
    assert.deepEqual([foot.lengthDisabled, foot.metresPerUnit], [false, 0.3048]);
    assert.match(foot.note, /foot/);
});
