import assert from "node:assert/strict";
import test from "node:test";

import {
    FORMAT_VERSIONS, OPTION_DEFAULTS, fixedFrameDisplayValue, fixedFrameHint, isGlobalFrameFormat, progressText, spoolText, timingsText, validateConverterOptions,
} from "../converter-ui.mjs";

test("defaults are the CLI defaults and map to core options", () => {
    const { errors, options } = validateConverterOptions(OPTION_DEFAULTS);
    assert.deepEqual(errors, {});
    assert.deepEqual(options, {
        formatVersion: "v7", maxError: 0.01, maxMemoryMiB: 512, ramBudget: 4e9,
    });
    assert.deepEqual(validateConverterOptions().options, options);
    assert.equal(FORMAT_VERSIONS[0], "v7");
    assert.equal(OPTION_DEFAULTS.formatVersion, "v7");
});

test("v5 and v7.5-pre are accepted; only v5 sends leaf size and divisions", () => {
    const v5 = validateConverterOptions({ formatVersion: "v5" });
    assert.deepEqual(v5.errors, {});
    assert.equal(v5.options.pointsPerBlock, 262144);
    assert.equal(v5.options.spatialDivisions, 2);
    assert.ok(FORMAT_VERSIONS.includes("v7.5-pre"));
    assert.deepEqual(validateConverterOptions({ formatVersion: "v7.5-pre" }).errors, {});
});

test("global-frame formats send neither points per block nor divisions, even if invalid", () => {
    for (const formatVersion of ["v7", "v7.4-pre", "v7.5-pre", "v8.1-pre"]) {
        assert.ok(isGlobalFrameFormat(formatVersion));
        const { errors, options } = validateConverterOptions({ formatVersion, pointsPerBlock: "0", spatialDivisions: "3" });
        assert.deepEqual(errors, {});
        assert.ok(!("pointsPerBlock" in options) && !("spatialDivisions" in options), formatVersion);
    }
    for (const formatVersion of ["v5", "v6.1-pre", "v7.3-pre"]) assert.ok(!isGlobalFrameFormat(formatVersion));
});

test("every listed format version is accepted and unknown ones are not", () => {
    for (const formatVersion of FORMAT_VERSIONS) {
        assert.deepEqual(validateConverterOptions({ formatVersion }).errors, {});
    }
    assert.ok("formatVersion" in validateConverterOptions({ formatVersion: "v6" }).errors);
});

test("out of range, fractional and empty values are rejected per field", () => {
    const cases = [
        ["pointsPerBlock", ["0", "1000001", "1.5", "", "abc"]],
        ["maxError", ["-1", "", "x", "Infinity"]],
        ["spatialDivisions", ["3", "16", ""]],
        ["plannerBudgetMiB", ["0", "4097", "2.5"]],
        ["ramBudgetGB", ["0", "9", "1.5", ""]],
    ];
    for (const [field, values] of cases) {
        for (const value of values) {
            const { errors } = validateConverterOptions({ formatVersion: "v5", [field]: value });
            assert.deepEqual(Object.keys(errors), [field], `${field}=${JSON.stringify(value)}`);
        }
    }
});

test("boundaries are accepted and RAM budget is converted to bytes", () => {
    const { errors, options } = validateConverterOptions({
        formatVersion: "v5", pointsPerBlock: "1000000", maxError: "0", spatialDivisions: "8", plannerBudgetMiB: "4096", ramBudgetGB: "8",
    });
    assert.deepEqual(errors, {});
    assert.equal(options.ramBudget, 8e9);
    assert.equal(options.maxError, 0);
});

test("progress shows stage and percentage", () => {
    assert.equal(progressText("writing", 50, 200), "Writing blocks · 25%");
    assert.equal(progressText("planning", 0, 0), "Planning spatial index");
    assert.equal(progressText("writing", 300, 200), "Writing blocks · 100%");
    assert.equal(progressText("other", 1, 2), "Converting · 50%");
});

test("timings and spool stats render only what is present", () => {
    assert.equal(timingsText({ planningMs: 1500, writingMs: 2500, totalMs: 4200 }), "planning 1.5s · writing 2.5s · total 4.2s");
    assert.equal(timingsText({ totalMs: 900 }), "total 0.9s");
    assert.equal(timingsText(undefined), "");
    assert.equal(spoolText({ spilledBytes: 2.5e9, pagesPeak: 12000 }), "Spool: spilled 2.5 GB · pages peak 12,000");
    assert.equal(spoolText({ spilledBytes: 0, pagesPeak: 0 }), "Spool: spilled 0.0 MB · pages peak 0");
    assert.equal(spoolText(undefined), "");
});

test("fixed-frame fields show the fixed values for global-frame formats and restore the user's otherwise", () => {
    assert.equal(fixedFrameDisplayValue("pointsPerBlock", "v7", "1000"), "64000");
    assert.equal(fixedFrameDisplayValue("spatialDivisions", "v7.5-pre", "4"), "2");
    assert.equal(fixedFrameDisplayValue("pointsPerBlock", "v5", "1000"), "1000");
    assert.equal(fixedFrameDisplayValue("spatialDivisions", "v5", undefined), OPTION_DEFAULTS.spatialDivisions);
    assert.equal(fixedFrameHint("pointsPerBlock"), "Fixed by the selected format (64,000).");
    assert.equal(fixedFrameHint("spatialDivisions"), "Fixed by the selected format (2).");
});
