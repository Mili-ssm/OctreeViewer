import assert from "node:assert/strict";
import test from "node:test";

import {
    OPTION_DEFAULTS, progressText, spoolText, timingsText, validateConverterOptions,
} from "../converter-ui.mjs";

test("defaults are the CLI defaults and map to core options", () => {
    const { errors, options } = validateConverterOptions(OPTION_DEFAULTS);
    assert.deepEqual(errors, {});
    assert.deepEqual(options, {
        maxError: 0.01, maxMemoryMiB: 512, ramBudget: 4e9,
    });
    assert.deepEqual(validateConverterOptions().options, options);
    assert.deepEqual(Object.keys(OPTION_DEFAULTS), ["maxError", "plannerBudgetMiB", "ramBudgetGB"]);
});

test("the options V8 fixes are refused with a message that names them", () => {
    for (const name of ["formatVersion", "rowOrder", "pointsPerBlock", "spatialDivisions"]) {
        for (const value of ["v8", "v5", 64000, "", 0]) {
            const { errors, options } = validateConverterOptions({ [name]: value });
            assert.deepEqual(Object.keys(errors), [name], `${name}=${JSON.stringify(value)}`);
            assert.equal(errors[name], `option ${name} was removed: V8 is the only format and fixes it`);
            assert.ok(!(name in options));
        }
    }
});

test("out of range, fractional and empty values are rejected per field", () => {
    const cases = [
        ["maxError", ["-1", "", "x", "Infinity"]],
        ["plannerBudgetMiB", ["0", "4097", "2.5"]],
        ["ramBudgetGB", ["0", "9", "1.5", ""]],
    ];
    for (const [field, values] of cases) {
        for (const value of values) {
            const { errors } = validateConverterOptions({ [field]: value });
            assert.deepEqual(Object.keys(errors), [field], `${field}=${JSON.stringify(value)}`);
        }
    }
});

test("boundaries are accepted and RAM budget is converted to bytes", () => {
    const { errors, options } = validateConverterOptions({
        maxError: "0", plannerBudgetMiB: "4096", ramBudgetGB: "8",
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

test("attr codec was removed (ADR 0060): any value is an error that names it", () => {
    assert.ok(!("attrCodec" in validateConverterOptions(OPTION_DEFAULTS).options));
    for (const attrCodec of ["lz4", "cold", "cold-rgb", "all", "zstd"]) {
        const { errors, options } = validateConverterOptions({ attrCodec });
        assert.deepEqual(Object.keys(errors), ["attrCodec"]);
        assert.match(errors.attrCodec, /attrCodec was removed \(ADR 0060\)/);
        assert.ok(!("attrCodec" in options));
    }
});
