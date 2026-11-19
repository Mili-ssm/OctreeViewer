/* @ts-self-types="./viewer_wasm.d.ts" */

/**
 * JS result shape: `{kind, values, pointCount, components?, name?, dataType?}`.
 * Positions are `Float32Array` xyz; geometry is `Float32Array` xyz + RGB bytes; colors are RGB bytes.
 * Attributes preserve their declared integer/float type in a JS typed array.
 */
export class WasmAsyncFormatReader {
    static __wrap(ptr) {
        const obj = Object.create(WasmAsyncFormatReader.prototype);
        obj.__wbg_ptr = ptr;
        WasmAsyncFormatReaderFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmAsyncFormatReaderFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmasyncformatreader_free(ptr, 0);
    }
    /**
     * Fetches, validates and registers the hierarchy page hanging from `anchor` (a decimal node
     * id). The result is grafted with `graftPage` on the viewer or builder that owns the
     * hierarchy. Page reads have no `ReadBudget`: the dataset's page budget bounds them.
     * @param {string} anchor
     * @returns {Promise<WasmPageGraft>}
     */
    loadPage(anchor) {
        const ptr0 = passStringToWasm0(anchor, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmasyncformatreader_loadPage(this.__wbg_ptr, ptr0, len0);
        return ret;
    }
    /**
     * Returns metadata in the descriptor shape accepted by `WasmVoxelViewer.openLodDataset`.
     * @param {number} cache_bytes
     * @returns {any}
     */
    lodDatasetDescriptor(cache_bytes) {
        const ret = wasm.wasmasyncformatreader_lodDatasetDescriptor(this.__wbg_ptr, cache_bytes);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Opens split primary and optional attribute Blobs. Reads header/footer/index by ranges only.
     * @param {Blob} primary
     * @param {Blob | null} [attributes]
     * @param {WasmAsyncReadBudget | null} [budget]
     * @returns {Promise<WasmAsyncFormatReader>}
     */
    static open(primary, attributes, budget) {
        let ptr0 = 0;
        if (!isLikeNone(budget)) {
            _assertClass(budget, WasmAsyncReadBudget);
            ptr0 = budget.__destroy_into_raw();
        }
        const ret = wasm.wasmasyncformatreader_open(primary, isLikeNone(attributes) ? 0 : addToExternrefTable0(attributes), ptr0);
        return ret;
    }
    /**
     * Plans a block result with one shared reservation, ready for independent async execution.
     * `positionsAttribute` returns XYZ and one typed column as a single quota-owned result.
     * `geometryAttributes` (the cells path) returns XYZ and RGB, the optional `schemaIndex`
     * column as `attribute` and every `carriedIndices` column in `attributes`.
     * @param {bigint} block_id
     * @param {string} selection
     * @param {number | null} [schema_index]
     * @param {Uint32Array | null} [carried_indices]
     * @returns {WasmAsyncLodReadRequest | undefined}
     */
    planLodRead(block_id, selection, schema_index, carried_indices) {
        const ptr0 = passStringToWasm0(selection, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        var ptr1 = isLikeNone(carried_indices) ? 0 : passArray32ToWasm0(carried_indices, wasm.__wbindgen_malloc);
        var len1 = WASM_VECTOR_LEN;
        const ret = wasm.wasmasyncformatreader_planLodRead(this.__wbg_ptr, block_id, ptr0, len0, isLikeNone(schema_index) ? Number.MAX_SAFE_INTEGER : (schema_index) >>> 0, ptr1, len1);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] === 0 ? undefined : WasmAsyncLodReadRequest.__wrap(ret[0]);
    }
    /**
     * Plans one supported selection and returns the exact ranges to fetch. The plan remains
     * private to this reader and must be consumed by `materialize` before another plan is made.
     * @param {bigint} block_id
     * @param {string} selection
     * @param {number | null} [schema_index]
     * @returns {any}
     */
    planRead(block_id, selection, schema_index) {
        const ptr0 = passStringToWasm0(selection, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmasyncformatreader_planRead(this.__wbg_ptr, block_id, ptr0, len0, isLikeNone(schema_index) ? Number.MAX_SAFE_INTEGER : (schema_index) >>> 0);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Fetches the ranges from the plan and materializes its validated selection.
     * @returns {Promise<any>}
     */
    read() {
        const ret = wasm.wasmasyncformatreader_read(this.__wbg_ptr);
        return ret;
    }
}
if (Symbol.dispose) WasmAsyncFormatReader.prototype[Symbol.dispose] = WasmAsyncFormatReader.prototype.free;

/**
 * An independently owned LOD block request. Separate requests share the same immutable index
 * and read budget, so their Blob promises can overlap without borrowing the facade mutably.
 */
export class WasmAsyncLodReadRequest {
    static __wrap(ptr) {
        const obj = Object.create(WasmAsyncLodReadRequest.prototype);
        obj.__wbg_ptr = ptr;
        WasmAsyncLodReadRequestFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmAsyncLodReadRequestFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmasynclodreadrequest_free(ptr, 0);
    }
    /**
     * @returns {WasmAsyncReadCancelToken}
     */
    cancelToken() {
        const ret = wasm.wasmasynclodreadrequest_cancelToken(this.__wbg_ptr);
        return WasmAsyncReadCancelToken.__wrap(ret);
    }
    /**
     * @returns {Promise<WasmAsyncLodReadResult>}
     */
    read() {
        const ret = wasm.wasmasynclodreadrequest_read(this.__wbg_ptr);
        return ret;
    }
    /**
     * Replaces the shared Blobs for this request only (the default stays when not called). The
     * replacements must expose the same bytes; they let a source scope its reads to this request.
     * @param {Blob} primary
     * @param {Blob | null} [attributes]
     */
    setSource(primary, attributes) {
        wasm.wasmasynclodreadrequest_setSource(this.__wbg_ptr, primary, isLikeNone(attributes) ? 0 : addToExternrefTable0(attributes));
    }
}
if (Symbol.dispose) WasmAsyncLodReadRequest.prototype[Symbol.dispose] = WasmAsyncLodReadRequest.prototype.free;

/**
 * Owns typed arrays and the request's quota until the worker inserts or discards the result.
 */
export class WasmAsyncLodReadResult {
    static __wrap(ptr) {
        const obj = Object.create(WasmAsyncLodReadResult.prototype);
        obj.__wbg_ptr = ptr;
        WasmAsyncLodReadResultFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmAsyncLodReadResultFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmasynclodreadresult_free(ptr, 0);
    }
    /**
     * Time spent decoding payloads and building typed arrays, for benchmarks.
     * @returns {number}
     */
    get decodeMs() {
        const ret = wasm.wasmasynclodreadresult_decodeMs(this.__wbg_ptr);
        return ret;
    }
    /**
     * Wall time awaiting Blob range reads (includes event-loop waits), for benchmarks.
     * @returns {number}
     */
    get readMs() {
        const ret = wasm.wasmasynclodreadresult_readMs(this.__wbg_ptr);
        return ret;
    }
    /**
     * Releases the typed arrays and shared quota after insertion or logical discard.
     */
    release() {
        wasm.wasmasynclodreadresult_release(this.__wbg_ptr);
    }
    /**
     * @returns {any}
     */
    get value() {
        const ret = wasm.wasmasynclodreadresult_value(this.__wbg_ptr);
        return ret;
    }
}
if (Symbol.dispose) WasmAsyncLodReadResult.prototype[Symbol.dispose] = WasmAsyncLodReadResult.prototype.free;

export class WasmAsyncReadBudget {
    static __wrap(ptr) {
        const obj = Object.create(WasmAsyncReadBudget.prototype);
        obj.__wbg_ptr = ptr;
        WasmAsyncReadBudgetFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmAsyncReadBudgetFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmasyncreadbudget_free(ptr, 0);
    }
    /**
     * @returns {WasmAsyncReadBudget}
     */
    clone() {
        const ret = wasm.wasmasyncreadbudget_clone(this.__wbg_ptr);
        return WasmAsyncReadBudget.__wrap(ret);
    }
    /**
     * @returns {string}
     */
    limitBytes() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.wasmasyncreadbudget_limitBytes(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * @param {number} limit_bytes
     */
    constructor(limit_bytes) {
        const ret = wasm.wasmasyncreadbudget_new(limit_bytes);
        this.__wbg_ptr = ret;
        WasmAsyncReadBudgetFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * @returns {string}
     */
    reservedBytes() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.wasmasyncreadbudget_reservedBytes(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
}
if (Symbol.dispose) WasmAsyncReadBudget.prototype[Symbol.dispose] = WasmAsyncReadBudget.prototype.free;

/**
 * Cancellation is logical: an active Blob promise resolves before its reservation is released.
 */
export class WasmAsyncReadCancelToken {
    static __wrap(ptr) {
        const obj = Object.create(WasmAsyncReadCancelToken.prototype);
        obj.__wbg_ptr = ptr;
        WasmAsyncReadCancelTokenFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmAsyncReadCancelTokenFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmasyncreadcanceltoken_free(ptr, 0);
    }
    cancel() {
        wasm.wasmasyncreadcanceltoken_cancel(this.__wbg_ptr);
    }
}
if (Symbol.dispose) WasmAsyncReadCancelToken.prototype[Symbol.dispose] = WasmAsyncReadCancelToken.prototype.free;

/**
 * WASM façade over the core block scheduler used by the Blob-backed LOD worker.
 */
export class WasmBlockRequestScheduler {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmBlockRequestSchedulerFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmblockrequestscheduler_free(ptr, 0);
    }
    /**
     * @returns {Array<any>}
     */
    cancelAll() {
        const ret = wasm.wasmblockrequestscheduler_cancelAll(this.__wbg_ptr);
        return ret;
    }
    /**
     * @param {string} ticket_id
     * @param {string} actual_block_id
     * @param {boolean} succeeded
     * @returns {string}
     */
    complete(ticket_id, actual_block_id, succeeded) {
        let deferred4_0;
        let deferred4_1;
        try {
            const ptr0 = passStringToWasm0(ticket_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(actual_block_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const ret = wasm.wasmblockrequestscheduler_complete(this.__wbg_ptr, ptr0, len0, ptr1, len1, succeeded);
            var ptr3 = ret[0];
            var len3 = ret[1];
            if (ret[3]) {
                ptr3 = 0; len3 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred4_0 = ptr3;
            deferred4_1 = len3;
            return getStringFromWasm0(ptr3, len3);
        } finally {
            wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
        }
    }
    /**
     * @param {string} block_id
     * @returns {boolean}
     */
    fail(block_id) {
        const ptr0 = passStringToWasm0(block_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmblockrequestscheduler_fail(this.__wbg_ptr, ptr0, len0);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] !== 0;
    }
    /**
     * @returns {number}
     */
    inFlightCount() {
        const ret = wasm.wasmblockrequestscheduler_inFlightCount(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * @param {string} ticket_id
     * @returns {boolean}
     */
    isCurrent(ticket_id) {
        const ptr0 = passStringToWasm0(ticket_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmblockrequestscheduler_isCurrent(this.__wbg_ptr, ptr0, len0);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] !== 0;
    }
    /**
     * @param {number} max_in_flight
     */
    constructor(max_in_flight) {
        const ret = wasm.wasmblockrequestscheduler_new(max_in_flight);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        this.__wbg_ptr = ret[0];
        WasmBlockRequestSchedulerFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * @returns {Array<any>}
     */
    pendingBlocks() {
        const ret = wasm.wasmblockrequestscheduler_pendingBlocks(this.__wbg_ptr);
        return ret;
    }
    /**
     * @param {string} block_id
     * @returns {boolean}
     */
    retry(block_id) {
        const ptr0 = passStringToWasm0(block_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmblockrequestscheduler_retry(this.__wbg_ptr, ptr0, len0);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] !== 0;
    }
    /**
     * Admits the best-priority pending request among the IDs whose byte reservation succeeded.
     * @param {Array<any>} admitted_blocks
     * @returns {any}
     */
    takeNextAdmitted(admitted_blocks) {
        const ret = wasm.wasmblockrequestscheduler_takeNextAdmitted(this.__wbg_ptr, admitted_blocks);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Reconciles the existing LOD priority order; the first ID has highest priority.
     * @param {Array<any>} block_ids
     * @returns {Array<any>}
     */
    updateBlocks(block_ids) {
        const ret = wasm.wasmblockrequestscheduler_updateBlocks(this.__wbg_ptr, block_ids);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
}
if (Symbol.dispose) WasmBlockRequestScheduler.prototype[Symbol.dispose] = WasmBlockRequestScheduler.prototype.free;

/**
 * Converts a LAS/LAZ Blob into the `.oct` / `.octattrs` pair behind two OPFS sync access handles.
 */
export class WasmCoreConverter {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmCoreConverterFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmcoreconverter_free(ptr, 0);
    }
    /**
     * Converts, flushes and validates by reading the handles back. Returns
     * `{ pointCount, blockCount, spool }` (point count as a string: it may exceed 2^53; `spool`
     * holds the `HybridStats` counters). A full output or spill zone throws an `Error` named
     * `QuotaExceededError`.
     * @returns {any}
     */
    convert() {
        const ret = wasm.wasmcoreconverter_convert(this.__wbg_ptr);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Reads the LAS/LAZ header of `input` and returns the spool estimate for `options` as
     * `{ sourceSpoolBytes, livePeakBytes, nodes, spilledPeakBytes, zones, outputBytes,
     * ramOnlyBytes }`, so the host can open `zones` spill zones before converting.
     * @param {Blob} input
     * @param {any} options
     * @returns {any}
     */
    static estimate(input, options) {
        const ret = wasm.wasmcoreconverter_estimate(input, options);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * `options` is a plain object (see `parse_options`) plus `spoolBudgetBytes`, the RAM the
     * spools may retain before spilling to the zones of `spool_host`. `on_progress` receives
     * `{ stage, processedPoints, totalPoints }` and may return `false` to stop the conversion.
     * @param {Blob} input
     * @param {any} geometry_handle
     * @param {any} attributes_handle
     * @param {any} options
     * @param {Function | null | undefined} on_progress
     * @param {any} spool_host
     */
    constructor(input, geometry_handle, attributes_handle, options, on_progress, spool_host) {
        const ret = wasm.wasmcoreconverter_new(input, geometry_handle, attributes_handle, options, isLikeNone(on_progress) ? 0 : addToExternrefTable0(on_progress), spool_host);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        this.__wbg_ptr = ret[0];
        WasmCoreConverterFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
}
if (Symbol.dispose) WasmCoreConverter.prototype[Symbol.dispose] = WasmCoreConverter.prototype.free;

/**
 * A validated hierarchy page in flight between the reader and the dataset that grafts it.
 */
export class WasmPageGraft {
    static __wrap(ptr) {
        const obj = Object.create(WasmPageGraft.prototype);
        obj.__wbg_ptr = ptr;
        WasmPageGraftFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmPageGraftFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmpagegraft_free(ptr, 0);
    }
}
if (Symbol.dispose) WasmPageGraft.prototype[Symbol.dispose] = WasmPageGraft.prototype.free;

/**
 * LOD scene builder without a GPU, canvas or renderer, for a dedicated build worker.
 *
 * It owns the dataset, block cache, planner and voxel builder. The host sends the camera with
 * every `update` and receives the instance bytes as a transferable `ArrayBuffer`; a separate
 * render worker uploads and draws them.
 */
export class WasmSceneBuilder {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmSceneBuilderFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmscenebuilder_free(ptr, 0);
    }
    /**
     * `evict` from the render worker: the nodes its driver forgot (`BigUint64Array`).
     * @param {any} nodes
     */
    applyEvict(nodes) {
        const ret = wasm.wasmscenebuilder_applyEvict(this.__wbg_ptr, nodes);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * `levelRequests` from the render worker: the whole set of levels its driver wants built on
     * demand, as node ids with a parallel array of depths. It replaces the previous set.
     * @param {any} nodes
     * @param {Uint8Array} depths
     */
    applyLevelRequests(nodes, depths) {
        const ret = wasm.wasmscenebuilder_applyLevelRequests(this.__wbg_ptr, nodes, depths);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * `poolAck` from the render worker: `{ cutEpoch, drawn, missing }`. The last one wins.
     * @param {any} ack
     */
    applyPoolAck(ack) {
        const ret = wasm.wasmscenebuilder_applyPoolAck(this.__wbg_ptr, ack);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * `[min, max]` the selected attribute is normalised with, or `null` when no attribute (or
     * no range for it) is selected. Lets the render worker show the same legend as the viewer.
     * @returns {any}
     */
    attributeRange() {
        const ret = wasm.wasmscenebuilder_attributeRange(this.__wbg_ptr);
        return ret;
    }
    /**
     * `"off"`, `"enabled"` or `"CellPathUnsupported"` for the open dataset.
     * @returns {string}
     */
    cellPath() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.wasmscenebuilder_cellPath(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Stats of the build side of the cells path (`cellsBusy` is "another update is pending"),
     * or `null` while the cells path is off.
     * @returns {any}
     */
    cellStats() {
        const ret = wasm.wasmscenebuilder_cellStats(this.__wbg_ptr);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Dataset grid and origin for the render worker's `beginRemoteCells`, or `null`.
     * @returns {any}
     */
    cellsInfo() {
        const ret = wasm.wasmscenebuilder_cellsInfo(this.__wbg_ptr);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * `driverReplaced`: the render worker has a new driver; every node is sent again.
     */
    driverReplaced() {
        const ret = wasm.wasmscenebuilder_driverReplaced(this.__wbg_ptr);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * Opt in to the cells path; call before opening a dataset. A format the cells cannot draw
     * (no global frame, not replacement) keeps the CPU path and reports `cellPath:
     * "CellPathUnsupported"` on the open result.
     * @param {boolean} enabled
     */
    enableCells(enabled) {
        wasm.wasmscenebuilder_enableCells(this.__wbg_ptr, enabled);
    }
    /**
     * Grafts a page returned by `WasmAsyncFormatReader.loadPage`.
     * @param {WasmPageGraft} page
     */
    graftPage(page) {
        _assertClass(page, WasmPageGraft);
        var ptr0 = page.__destroy_into_raw();
        const ret = wasm.wasmscenebuilder_graftPage(this.__wbg_ptr, ptr0);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * Inserts one block delivered by the host; same contract as `WasmVoxelViewer`, plus
     * `carried`: columns (intensity, classification) that travel with the block unpainted so
     * the cells keep them in every colour mode (R3-Q32).
     * @param {string} block_id
     * @param {Float32Array} positions
     * @param {Uint8Array | null} [colors]
     * @param {any | null} [attribute]
     * @param {Array<any> | null} [carried]
     * @returns {number}
     */
    insertAsyncLodBlock(block_id, positions, colors, attribute, carried) {
        const ptr0 = passStringToWasm0(block_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmscenebuilder_insertAsyncLodBlock(this.__wbg_ptr, ptr0, len0, positions, isLikeNone(colors) ? 0 : addToExternrefTable0(colors), isLikeNone(attribute) ? 0 : addToExternrefTable0(attribute), isLikeNone(carried) ? 0 : addToExternrefTable0(carried));
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] >>> 0;
    }
    /**
     * The limits bound estimated construction memory and voxel count, not process RSS. Pass
     * the render worker's effective limits, already capped by its GPU instance budget.
     * @param {number} max_voxels
     * @param {number} max_estimated_bytes
     */
    constructor(max_voxels, max_estimated_bytes) {
        const ret = wasm.wasmscenebuilder_new(max_voxels, max_estimated_bytes);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        this.__wbg_ptr = ret[0];
        WasmSceneBuilderFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * Opens the random-access format from a Blob; the builder reads blocks itself.
     * @param {Blob} file
     * @param {number} cache_bytes
     * @param {Blob | null} [attributes]
     * @returns {any}
     */
    openFormatDataset(file, cache_bytes, attributes) {
        const ret = wasm.wasmscenebuilder_openFormatDataset(this.__wbg_ptr, file, cache_bytes, isLikeNone(attributes) ? 0 : addToExternrefTable0(attributes));
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Opens hierarchy metadata whose blocks the host delivers with `insertAsyncLodBlock`.
     * @param {any} descriptor
     * @returns {any}
     */
    openLodDataset(descriptor) {
        const ret = wasm.wasmscenebuilder_openLodDataset(this.__wbg_ptr, descriptor);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Opens a paged format: the reader's root page is resident, other pages arrive with
     * `graftPage` (see `lod.pendingPageAnchors` in `update`).
     * @param {WasmAsyncFormatReader} reader
     * @param {number} cache_bytes
     * @returns {any}
     */
    openPagedLodDataset(reader, cache_bytes) {
        _assertClass(reader, WasmAsyncFormatReader);
        const ret = wasm.wasmscenebuilder_openPagedLodDataset(this.__wbg_ptr, reader.__wbg_ptr, cache_bytes);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Missing blocks in the order the LOD plan wants them, as decimal id strings.
     * @returns {Array<any>}
     */
    pendingAsyncLodBlocks() {
        const ret = wasm.wasmscenebuilder_pendingAsyncLodBlocks(this.__wbg_ptr);
        return ret;
    }
    /**
     * `poolReset`: the render worker's pool was emptied; its driver keeps the nodes.
     */
    poolReset() {
        const ret = wasm.wasmscenebuilder_poolReset(this.__wbg_ptr);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * The pages of the render worker's GPU pool (`cellPoolPages` of the viewer); call before
     * opening a dataset. Without it the cut is not fitted to the pool.
     * @param {number} pages
     */
    setCellPoolPages(pages) {
        wasm.wasmscenebuilder_setCellPoolPages(this.__wbg_ptr, pages);
    }
    /**
     * Selects `rgb`, `intensity`, `elevation`, `classification` or `attribute:index:component`.
     * @param {string} mode
     */
    setColorMode(mode) {
        const ptr0 = passStringToWasm0(mode, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmscenebuilder_setColorMode(this.__wbg_ptr, ptr0, len0);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * Smallest cell side the cut may reach (RQ-09), in dataset units: the render side's
     * `continuityMinSide` (`null` or 0 for none). A node is not refined into children whose
     * coarsest cells would be smaller. A change plans again on the next `update`.
     * @param {number | null} [side]
     */
    setMinSide(side) {
        const ret = wasm.wasmscenebuilder_setMinSide(this.__wbg_ptr, !isLikeNone(side), isLikeNone(side) ? 0 : side);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * Epoch of the view of the next `update` (the viewer's `viewEpoch()` when it read the
     * camera sent with it); echoed as `viewEpoch` in the update result.
     * @param {number} epoch
     */
    setViewEpoch(epoch) {
        const ret = wasm.wasmscenebuilder_setViewEpoch(this.__wbg_ptr, epoch);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * Like `update`, but errors unless the cells path is active.
     * @param {Float64Array} camera
     * @param {number} viewport_width
     * @param {number} viewport_height
     * @param {number} visual_scale
     * @param {number} target_voxel_pixels
     * @returns {any}
     */
    updateCells(camera, viewport_width, viewport_height, visual_scale, target_voxel_pixels) {
        const ret = wasm.wasmscenebuilder_updateCells(this.__wbg_ptr, camera, viewport_width, viewport_height, visual_scale, target_voxel_pixels);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Selects the cut for a camera and rebuilds the scene when it changed.
     *
     * `camera` is `OrbitCamera.to_f64_array()` (11 numbers). Returns
     * `{ epoch, sceneBuffer, instanceCount, blockIds, digest, needsRefinement, lod }` where
     * `sceneBuffer` holds the `InstanceRaw` bytes in a fresh, transferable `ArrayBuffer`, or is
     * `null` when the previous scene still applies.
     * @param {Float64Array} camera
     * @param {number} viewport_width
     * @param {number} viewport_height
     * @param {number} visual_scale
     * @param {number} target_voxel_pixels
     * @returns {any}
     */
    update(camera, viewport_width, viewport_height, visual_scale, target_voxel_pixels) {
        const ret = wasm.wasmscenebuilder_update(this.__wbg_ptr, camera, viewport_width, viewport_height, visual_scale, target_voxel_pixels);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
}
if (Symbol.dispose) WasmSceneBuilder.prototype[Symbol.dispose] = WasmSceneBuilder.prototype.free;

export class WasmVoxelViewer {
    static __wrap(ptr) {
        const obj = Object.create(WasmVoxelViewer.prototype);
        obj.__wbg_ptr = ptr;
        WasmVoxelViewerFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        WasmVoxelViewerFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_wasmvoxelviewer_free(ptr, 0);
    }
    /**
     * One entry of `nodeLevels`: the codec bytes of the levels built on demand for a node,
     * queued behind the nodes sent before it and applied in order under the same byte cap.
     * @param {number} generation
     * @param {Uint8Array} bytes
     */
    addCellLevels(generation, bytes) {
        const ret = wasm.wasmvoxelviewer_addCellLevels(this.__wbg_ptr, generation, bytes);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * One `nodeCells` entry: the codec bytes of a node, queued and applied in order under the
     * byte cap.
     * @param {number} generation
     * @param {Uint8Array} bytes
     */
    addCellNode(generation, bytes) {
        const ret = wasm.wasmvoxelviewer_addCellNode(this.__wbg_ptr, generation, bytes);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * @param {Float32Array} positions
     * @param {Uint8Array} colors
     */
    appendPoints(positions, colors) {
        const ret = wasm.wasmvoxelviewer_appendPoints(this.__wbg_ptr, positions, colors);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * @param {Float32Array} positions
     * @param {Uint8Array} colors
     */
    appendVoxelizedPoints(positions, colors) {
        const ret = wasm.wasmvoxelviewer_appendVoxelizedPoints(this.__wbg_ptr, positions, colors);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * Render worker of a build-worker setup: creates the driver for the dataset described by
     * the builder's `cellsInfo` (`origin`, `rootMin`, `rootSide`). Returns the `cellPath`. The
     * first `takeCellMessages` carries `driverReplaced`. Follow it with `setCellElevationRange`.
     * @param {Float64Array} origin
     * @param {Float64Array} root_min
     * @param {number} root_side
     * @param {number | null} [reference_spacing]
     * @param {Float64Array | null} [data_min]
     * @param {Float64Array | null} [data_max]
     * @returns {string}
     */
    beginRemoteCells(origin, root_min, root_side, reference_spacing, data_min, data_max) {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.wasmvoxelviewer_beginRemoteCells(this.__wbg_ptr, origin, root_min, root_side, !isLikeNone(reference_spacing), isLikeNone(reference_spacing) ? 0 : reference_spacing, isLikeNone(data_min) ? 0 : addToExternrefTable0(data_min), isLikeNone(data_max) ? 0 : addToExternrefTable0(data_max));
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * Prepares the renderer for a dataset whose hierarchy and blocks live in a build worker:
     * drops the previous scene, adopts the dataset's visual scale and frames its root bounds.
     * @param {number} visual_scale
     * @param {Float64Array} origin
     * @param {Float64Array} bounds_min
     * @param {Float64Array} bounds_max
     * @returns {boolean}
     */
    beginRemoteDataset(visual_scale, origin, bounds_min, bounds_max) {
        const ret = wasm.wasmvoxelviewer_beginRemoteDataset(this.__wbg_ptr, visual_scale, origin, bounds_min, bounds_max);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] !== 0;
    }
    /**
     * Returns the camera pose in source dataset coordinates.
     * @returns {any}
     */
    cameraPose() {
        const ret = wasm.wasmvoxelviewer_cameraPose(this.__wbg_ptr);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * The camera in the lossless 11-number form `WasmSceneBuilder.update` takes.
     * @returns {Float64Array}
     */
    cameraTransport() {
        const ret = wasm.wasmvoxelviewer_cameraTransport(this.__wbg_ptr);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * `"off"`, `"enabled"` or `"CellPathUnsupported"`.
     * @returns {string}
     */
    cellPath() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.wasmvoxelviewer_cellPath(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Pages of the GPU cell pool this device runs (65,536 for 512 MiB, fewer when its limits
     * admit less, never below 32,768); 0 when only the CPU path is available.
     * @returns {number}
     */
    cellPoolPages() {
        const ret = wasm.wasmvoxelviewer_cellPoolPages(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Stats of the cells path of this viewer, or `null` while it is off.
     * @returns {any}
     */
    cellStats() {
        const ret = wasm.wasmvoxelviewer_cellStats(this.__wbg_ptr);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    clear() {
        wasm.wasmvoxelviewer_clear(this.__wbg_ptr);
    }
    /**
     * Creates a worker renderer with explicit CPU voxel-construction limits.
     * The limits bound estimated construction memory and voxel count, not process RSS or VRAM.
     * @param {OffscreenCanvas} canvas
     * @param {number} voxel_size
     * @param {number} max_voxels
     * @param {number} max_estimated_bytes
     * @returns {Promise<WasmVoxelViewer>}
     */
    static createOffscreenWithVoxelLimits(canvas, voxel_size, max_voxels, max_estimated_bytes) {
        const ret = wasm.wasmvoxelviewer_createOffscreenWithVoxelLimits(canvas, voxel_size, max_voxels, max_estimated_bytes);
        return ret;
    }
    /**
     * Creates a renderer for a host-transferred canvas in a dedicated Web Worker.
     * @param {OffscreenCanvas} canvas
     * @param {number} voxel_size
     * @returns {Promise<WasmVoxelViewer>}
     */
    static createOffscreen(canvas, voxel_size) {
        const ret = wasm.wasmvoxelviewer_createOffscreen(canvas, voxel_size);
        return ret;
    }
    /**
     * Creates a viewer that renders directly to the host-owned canvas.
     * @param {HTMLCanvasElement} canvas
     * @param {number} voxel_size
     * @returns {Promise<WasmVoxelViewer>}
     */
    static create(canvas, voxel_size) {
        const ret = wasm.wasmvoxelviewer_create(canvas, voxel_size);
        return ret;
    }
    /**
     * `{ reason: "unknown" | "destroyed", message }` once the GPU device was lost, else `null`.
     * Cheap: the host polls it to start the recovery of R3-Q26.
     * @returns {any}
     */
    deviceLost() {
        const ret = wasm.wasmvoxelviewer_deviceLost(this.__wbg_ptr);
        return ret;
    }
    /**
     * Opt in to the cells path; call before opening a dataset (or `beginRemoteCells`).
     * @param {boolean} enabled
     */
    enableCells(enabled) {
        wasm.wasmvoxelviewer_enableCells(this.__wbg_ptr, enabled);
    }
    /**
     * @returns {boolean}
     */
    fitScene() {
        const ret = wasm.wasmvoxelviewer_fitScene(this.__wbg_ptr);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] !== 0;
    }
    /**
     * Flies the camera relative to the view: `forward` on the horizontal plane, `right` along the
     * screen's right and `up` along dataset Z, all in units of the orbit distance.
     * @param {number} forward
     * @param {number} right
     * @param {number} up
     * @returns {boolean}
     */
    flyCamera(forward, right, up) {
        const ret = wasm.wasmvoxelviewer_flyCamera(this.__wbg_ptr, forward, right, up);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] !== 0;
    }
    /**
     * @returns {any}
     */
    gpuStats() {
        const ret = wasm.wasmvoxelviewer_gpuStats(this.__wbg_ptr);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Grafts a page returned by `WasmAsyncFormatReader.loadPage`.
     * @param {WasmPageGraft} page
     */
    graftPage(page) {
        _assertClass(page, WasmPageGraft);
        var ptr0 = page.__destroy_into_raw();
        const ret = wasm.wasmvoxelviewer_graftPage(this.__wbg_ptr, ptr0);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * Inserts one block received from an async format reader, preserving an optional typed column.
     * @param {string} block_id
     * @param {Float32Array} positions
     * @param {Uint8Array | null} [colors]
     * @param {any | null} [attribute]
     * @returns {number}
     */
    insertAsyncLodBlock(block_id, positions, colors, attribute) {
        const ptr0 = passStringToWasm0(block_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmvoxelviewer_insertAsyncLodBlock(this.__wbg_ptr, ptr0, len0, positions, isLikeNone(colors) ? 0 : addToExternrefTable0(colors), isLikeNone(attribute) ? 0 : addToExternrefTable0(attribute));
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] >>> 0;
    }
    /**
     * Inserts one decoded RGB block. Position coordinates are local to coordinateOrigin.
     * @param {string} block_id
     * @param {Float32Array} positions
     * @param {Uint8Array} colors
     * @returns {number}
     */
    insertLodBlock(block_id, positions, colors) {
        const ptr0 = passStringToWasm0(block_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmvoxelviewer_insertLodBlock(this.__wbg_ptr, ptr0, len0, positions, colors);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] >>> 0;
    }
    /**
     * @returns {any}
     */
    lodStats() {
        const ret = wasm.wasmvoxelviewer_lodStats(this.__wbg_ptr);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Translates the camera in source dataset coordinates.
     * @param {number} x
     * @param {number} y
     * @param {number} z
     * @returns {boolean}
     */
    moveCamera(x, y, z) {
        const ret = wasm.wasmvoxelviewer_moveCamera(this.__wbg_ptr, x, y, z);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] !== 0;
    }
    /**
     * @param {number} voxel_size
     */
    constructor(voxel_size) {
        const ret = wasm.wasmvoxelviewer_new(voxel_size);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        this.__wbg_ptr = ret[0];
        WasmVoxelViewerFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * Highest-priority missing block selected by the most recent async LOD update.
     * @returns {any}
     */
    nextAsyncLodBlock() {
        const ret = wasm.wasmvoxelviewer_nextAsyncLodBlock(this.__wbg_ptr);
        return ret;
    }
    /**
     * Opens the project's random-access format from a Blob; call from a dedicated worker.
     * @param {Blob} file
     * @param {number} cache_bytes
     * @param {Blob | null} [attributes]
     * @returns {any}
     */
    openFormatDataset(file, cache_bytes, attributes) {
        const ret = wasm.wasmvoxelviewer_openFormatDataset(this.__wbg_ptr, file, cache_bytes, isLikeNone(attributes) ? 0 : addToExternrefTable0(attributes));
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Opens resident hierarchy metadata for decoded block streaming.
     * @param {any} descriptor
     * @returns {any}
     */
    openLodDataset(descriptor) {
        const ret = wasm.wasmvoxelviewer_openLodDataset(this.__wbg_ptr, descriptor);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Opens a paged format: the reader's root page is resident, other pages arrive with
     * `graftPage` as the LOD asks for them (`pendingPageAnchors`).
     * @param {WasmAsyncFormatReader} reader
     * @param {number} cache_bytes
     * @returns {any}
     */
    openPagedLodDataset(reader, cache_bytes) {
        _assertClass(reader, WasmAsyncFormatReader);
        const ret = wasm.wasmvoxelviewer_openPagedLodDataset(this.__wbg_ptr, reader.__wbg_ptr, cache_bytes);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * @param {number} delta_x
     * @param {number} delta_y
     * @returns {boolean}
     */
    orbit(delta_x, delta_y) {
        const ret = wasm.wasmvoxelviewer_orbit(this.__wbg_ptr, delta_x, delta_y);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] !== 0;
    }
    /**
     * @param {number} delta_x
     * @param {number} delta_y
     * @returns {boolean}
     */
    pan(delta_x, delta_y) {
        const ret = wasm.wasmvoxelviewer_pan(this.__wbg_ptr, delta_x, delta_y);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] !== 0;
    }
    /**
     * Current missing blocks in the projected-error order chosen by the LOD plan.
     * @returns {Array<any>}
     */
    pendingAsyncLodBlocks() {
        const ret = wasm.wasmvoxelviewer_pendingAsyncLodBlocks(this.__wbg_ptr);
        return ret;
    }
    /**
     * Pages the last `updateLod` needed and lacked, highest projected error first.
     * @returns {Array<any>}
     */
    pendingPageAnchors() {
        const ret = wasm.wasmvoxelviewer_pendingPageAnchors(this.__wbg_ptr);
        return ret;
    }
    /**
     * Picks the frontmost rendered voxel at a framebuffer pixel. Coordinates and voxel data are
     * returned in the source dataset's coordinate system; `None` means that the ray hit no voxel.
     * @param {number} x
     * @param {number} y
     * @returns {Promise<any>}
     */
    pick(x, y) {
        const ret = wasm.wasmvoxelviewer_pick(this.__wbg_ptr, x, y);
        return ret;
    }
    /**
     * One `levelsRefused` message: depths the build side cannot serve, as node ids with a
     * parallel array of depths. The driver stops wanting them.
     * @param {any} nodes
     * @param {Uint8Array} depths
     */
    refuseCellLevels(nodes, depths) {
        const ret = wasm.wasmvoxelviewer_refuseCellLevels(this.__wbg_ptr, nodes, depths);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * @returns {string}
     */
    renderFrame() {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.wasmvoxelviewer_renderFrame(this.__wbg_ptr);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * Returns a bounded diagnostic page of the current CPU render scene.
     * @param {number} start
     * @param {number} count
     * @returns {any}
     */
    renderSceneBatch(start, count) {
        const ret = wasm.wasmvoxelviewer_renderSceneBatch(this.__wbg_ptr, start, count);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * @param {Float32Array} positions
     * @param {Uint8Array} colors
     */
    replacePoints(positions, colors) {
        const ret = wasm.wasmvoxelviewer_replacePoints(this.__wbg_ptr, positions, colors);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * @param {Float32Array} positions
     * @param {Uint8Array} colors
     */
    replaceVoxelizedPoints(positions, colors) {
        const ret = wasm.wasmvoxelviewer_replaceVoxelizedPoints(this.__wbg_ptr, positions, colors);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * Starts a new window for the `gpuFrameMs` percentiles.
     */
    resetGpuFrameStats() {
        wasm.wasmvoxelviewer_resetGpuFrameStats(this.__wbg_ptr);
    }
    /**
     * @param {number} width
     * @param {number} height
     */
    resize(width, height) {
        wasm.wasmvoxelviewer_resize(this.__wbg_ptr, width, height);
    }
    /**
     * Points the camera down along the dataset's positive Z up axis.
     */
    setBirdView() {
        const ret = wasm.wasmvoxelviewer_setBirdView(this.__wbg_ptr);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * Sets camera position and orbit target in source dataset coordinates.
     * @param {Float64Array} position
     * @param {Float64Array} target
     * @returns {boolean}
     */
    setCameraPose(position, target) {
        const ret = wasm.wasmvoxelviewer_setCameraPose(this.__wbg_ptr, position, target);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] !== 0;
    }
    /**
     * One `cellCut` message: `{ cutEpoch, viewEpoch, cut, support, idle, targetPixels }`. The
     * last wins.
     * @param {any} message
     */
    setCellCut(message) {
        const ret = wasm.wasmvoxelviewer_setCellCut(this.__wbg_ptr, message);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * Fixes the elevation colour range of the remote cells path to `[min, max]` (absolute
     * dataset Z, `cellsInfo.elevationRange`), so the colour does not depend on the view. Without
     * it the range follows the stored levels. No-op when the path is off.
     * @param {number} min
     * @param {number} max
     */
    setCellElevationRange(min, max) {
        const ret = wasm.wasmvoxelviewer_setCellElevationRange(this.__wbg_ptr, min, max);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * Bytes of `nodeCells` applied per frame (at least one node); takes effect on the next
     * `beginRemoteCells`.
     * @param {number} bytes
     */
    setCellNodeBytesPerFrame(bytes) {
        wasm.wasmvoxelviewer_setCellNodeBytesPerFrame(this.__wbg_ptr, bytes);
    }
    /**
     * @param {string} mode
     */
    setColorMode(mode) {
        const ptr0 = passStringToWasm0(mode, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmvoxelviewer_setColorMode(this.__wbg_ptr, ptr0, len0);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * Continuity of the open cloud (RF-05): with `enabled`, no cell is drawn smaller than
     * `factor` (default 1) x the reference spacing of the cloud; off (the default) leaves only the
     * length minimum (`setMinLength`). It survives `beginRemoteCells` and dataset changes, takes effect on the
     * next frame and rebuilds nothing: the cells hold every level.
     * @param {boolean} enabled
     * @param {number | null} [factor]
     */
    setContinuity(enabled, factor) {
        const ret = wasm.wasmvoxelviewer_setContinuity(this.__wbg_ptr, enabled, !isLikeNone(factor), isLikeNone(factor) ? 0 : factor);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * @param {boolean} enabled
     */
    setDepthShading(enabled) {
        const ret = wasm.wasmvoxelviewer_setDepthShading(this.__wbg_ptr, enabled);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * Turns per-frame GPU timing on or off (off by default; needs `timestamp-query`).
     * @param {boolean} enabled
     */
    setGpuFrameTiming(enabled) {
        wasm.wasmvoxelviewer_setGpuFrameTiming(this.__wbg_ptr, enabled);
    }
    /**
     * Length minimum of the drawn cubes, in dataset units (ADR 0049 revision of 16-11-2026,
     * ADR 0054): no cell is drawn smaller than it, and the larger of it and the continuity side
     * wins. `undefined` restores the default (1e-4, 0.1 mm if the unit is metres); 0 turns the
     * length filter off; a negative, NaN or infinite value is an error and leaves the state as
     * it was. It survives `beginRemoteCells` and dataset changes like the continuity, takes
     * effect on the next frame and rebuilds nothing.
     * @param {number | null} [side]
     */
    setMinLength(side) {
        const ret = wasm.wasmvoxelviewer_setMinLength(this.__wbg_ptr, !isLikeNone(side), isLikeNone(side) ? 0 : side);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * Moves the orbit center while preserving the camera position.
     * @param {Float64Array} target
     * @returns {boolean}
     */
    setOrbitTarget(target) {
        const ret = wasm.wasmvoxelviewer_setOrbitTarget(this.__wbg_ptr, target);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] !== 0;
    }
    /**
     * Selects how the renderer shades the displayed scene when the LOD hierarchy lives in a
     * build worker. `attribute:*` modes are painted into the colours by the builder, so they
     * render as `rgb`.
     * @param {string} mode
     */
    setPresentationColorMode(mode) {
        const ptr0 = passStringToWasm0(mode, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmvoxelviewer_setPresentationColorMode(this.__wbg_ptr, ptr0, len0);
        if (ret[1]) {
            throw takeFromExternrefTable0(ret[0]);
        }
    }
    /**
     * @returns {any}
     */
    stats() {
        const ret = wasm.wasmvoxelviewer_stats(this.__wbg_ptr);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * What the render worker owes the build worker since the last call.
     * @returns {any}
     */
    takeCellMessages() {
        const ret = wasm.wasmvoxelviewer_takeCellMessages(this.__wbg_ptr);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Selects a camera-aware voxel cut using the target projected voxel edge in pixels.
     * @param {number} target_voxel_pixels
     * @returns {any}
     */
    updateLod(target_voxel_pixels) {
        const ret = wasm.wasmvoxelviewer_updateLod(this.__wbg_ptr, target_voxel_pixels);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Uploads the `InstanceRaw` bytes a `WasmSceneBuilder` produced as the displayed scene.
     * `block_ids` are decimal strings, kept for diagnostics. Returns the bytes written.
     * @param {Uint8Array} instances
     * @param {string[]} block_ids
     * @returns {number}
     */
    uploadSceneInstances(instances, block_ids) {
        const ptr0 = passArrayJsValueToWasm0(block_ids, wasm.__wbindgen_malloc);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.wasmvoxelviewer_uploadSceneInstances(this.__wbg_ptr, instances, ptr0, len0);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0];
    }
    /**
     * Epoch of the view `cameraTransport` returns now: bumped by every camera move and resize.
     * Send it to the builder (`setViewEpoch`) with that camera.
     * @returns {number}
     */
    viewEpoch() {
        const ret = wasm.wasmvoxelviewer_viewEpoch(this.__wbg_ptr);
        return ret;
    }
    /**
     * @returns {Uint32Array}
     */
    viewportSize() {
        const ret = wasm.wasmvoxelviewer_viewportSize(this.__wbg_ptr);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * Voxel construction limits a `WasmSceneBuilder` must use: `[maxVoxels, maxEstimatedBytes]`,
     * already capped by this renderer's GPU instance budget.
     * @returns {Float64Array}
     */
    voxelBuildLimits() {
        const ret = wasm.wasmvoxelviewer_voxelBuildLimits(this.__wbg_ptr);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return takeFromExternrefTable0(ret[0]);
    }
    /**
     * @param {number} delta
     * @returns {boolean}
     */
    zoom(delta) {
        const ret = wasm.wasmvoxelviewer_zoom(this.__wbg_ptr, delta);
        if (ret[2]) {
            throw takeFromExternrefTable0(ret[1]);
        }
        return ret[0] !== 0;
    }
}
if (Symbol.dispose) WasmVoxelViewer.prototype[Symbol.dispose] = WasmVoxelViewer.prototype.free;
function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg_Error_30c8987f7c2ed4e2: function(arg0, arg1) {
            const ret = Error(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_Number_14af1003b8dd5ead: function(arg0) {
            const ret = Number(arg0);
            return ret;
        },
        __wbg_String_8564e559799eccda: function(arg0, arg1) {
            const ret = String(arg1);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_Window_a2a6c4d665047b14: function(arg0) {
            const ret = arg0.Window;
            return ret;
        },
        __wbg_WorkerGlobalScope_2664448a7c667d67: function(arg0) {
            const ret = arg0.WorkerGlobalScope;
            return ret;
        },
        __wbg___wbindgen_bigint_get_as_i64_a2383202b9353e4c: function(arg0, arg1) {
            const v = arg1;
            const ret = typeof(v) === 'bigint' ? v : undefined;
            getDataViewMemory0().setBigInt64(arg0 + 8 * 1, isLikeNone(ret) ? BigInt(0) : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg___wbindgen_boolean_get_5b446f51afd21013: function(arg0) {
            const v = arg0;
            const ret = typeof(v) === 'boolean' ? v : undefined;
            return isLikeNone(ret) ? 0xFFFFFF : ret ? 1 : 0;
        },
        __wbg___wbindgen_debug_string_4687d8d8c2017d52: function(arg0, arg1) {
            const ret = debugString(arg1);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_in_92f62ee1427d9e49: function(arg0, arg1) {
            const ret = arg0 in arg1;
            return ret;
        },
        __wbg___wbindgen_is_bigint_b123553bed3bb382: function(arg0) {
            const ret = typeof(arg0) === 'bigint';
            return ret;
        },
        __wbg___wbindgen_is_falsy_16bd49b68658263e: function(arg0) {
            const ret = !arg0;
            return ret;
        },
        __wbg___wbindgen_is_function_1f9d30630b8b1d3d: function(arg0) {
            const ret = typeof(arg0) === 'function';
            return ret;
        },
        __wbg___wbindgen_is_null_e343b7d08827ba72: function(arg0) {
            const ret = arg0 === null;
            return ret;
        },
        __wbg___wbindgen_is_object_3c45d4f2dde4e749: function(arg0) {
            const val = arg0;
            const ret = typeof(val) === 'object' && val !== null;
            return ret;
        },
        __wbg___wbindgen_is_string_90b56bc79aad6f6c: function(arg0) {
            const ret = typeof(arg0) === 'string';
            return ret;
        },
        __wbg___wbindgen_is_undefined_8865fb403f8fe9d8: function(arg0) {
            const ret = arg0 === undefined;
            return ret;
        },
        __wbg___wbindgen_jsval_eq_02babf21faa37971: function(arg0, arg1) {
            const ret = arg0 === arg1;
            return ret;
        },
        __wbg___wbindgen_jsval_loose_eq_677f21e468d6b461: function(arg0, arg1) {
            const ret = arg0 == arg1;
            return ret;
        },
        __wbg___wbindgen_number_get_2e0e7dee9f701a71: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'number' ? obj : undefined;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg___wbindgen_string_get_0380ccaa2f57f0d9: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'string' ? obj : undefined;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_throw_41e9ee4f547fc59a: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg__wbg_cb_unref_dcc1a90847f04c41: function(arg0) {
            arg0._wbg_cb_unref();
        },
        __wbg_arrayBuffer_0fe6e1300abcf908: function(arg0) {
            const ret = arg0.arrayBuffer();
            return ret;
        },
        __wbg_beginComputePass_b9a325184985e6ff: function(arg0, arg1) {
            const ret = arg0.beginComputePass(arg1);
            return ret;
        },
        __wbg_beginRenderPass_3c53642423af50dc: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.beginRenderPass(arg1);
            return ret;
        }, arguments); },
        __wbg_buffer_56ec2905a66f58b9: function(arg0) {
            const ret = arg0.buffer;
            return ret;
        },
        __wbg_call_187d372bd5fdd4aa: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.call(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_call_6137034ef55c9d0f: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.call(arg1);
            return ret;
        }, arguments); },
        __wbg_clearBuffer_3d5a3057c3c8a9bf: function(arg0, arg1, arg2, arg3) {
            arg0.clearBuffer(arg1, arg2, arg3);
        },
        __wbg_clearBuffer_82ba999c294f42fb: function(arg0, arg1, arg2) {
            arg0.clearBuffer(arg1, arg2);
        },
        __wbg_configure_1e2c1c9edad07d26: function() { return handleError(function (arg0, arg1) {
            arg0.configure(arg1);
        }, arguments); },
        __wbg_copyBufferToBuffer_01766818654a9868: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.copyBufferToBuffer(arg1, arg2, arg3, arg4);
        }, arguments); },
        __wbg_copyBufferToBuffer_9c174b96fb08d551: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.copyBufferToBuffer(arg1, arg2, arg3, arg4, arg5);
        }, arguments); },
        __wbg_createBindGroupLayout_b1bd63b4e88459d8: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.createBindGroupLayout(arg1);
            return ret;
        }, arguments); },
        __wbg_createBindGroup_f539b26ca341308f: function(arg0, arg1) {
            const ret = arg0.createBindGroup(arg1);
            return ret;
        },
        __wbg_createBuffer_d800e9b1d41b2ee5: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.createBuffer(arg1);
            return ret;
        }, arguments); },
        __wbg_createCommandEncoder_3352d1ffc36c6fc0: function(arg0, arg1) {
            const ret = arg0.createCommandEncoder(arg1);
            return ret;
        },
        __wbg_createComputePipeline_224fa2618d9948a0: function(arg0, arg1) {
            const ret = arg0.createComputePipeline(arg1);
            return ret;
        },
        __wbg_createPipelineLayout_6eab52c327118937: function(arg0, arg1) {
            const ret = arg0.createPipelineLayout(arg1);
            return ret;
        },
        __wbg_createQuerySet_2dc8cde53df9849d: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.createQuerySet(arg1);
            return ret;
        }, arguments); },
        __wbg_createRenderPipeline_0ebb7ebc653e9207: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.createRenderPipeline(arg1);
            return ret;
        }, arguments); },
        __wbg_createShaderModule_cefa51336cb288ae: function(arg0, arg1) {
            const ret = arg0.createShaderModule(arg1);
            return ret;
        },
        __wbg_createTexture_ed7e9fc04dd54d84: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.createTexture(arg1);
            return ret;
        }, arguments); },
        __wbg_createView_da41c2d2cb212715: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.createView(arg1);
            return ret;
        }, arguments); },
        __wbg_description_83b8a393160021b9: function(arg0, arg1) {
            const ret = arg1.description;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_dispatchWorkgroupsIndirect_efe17f2da3a39a3f: function(arg0, arg1, arg2) {
            arg0.dispatchWorkgroupsIndirect(arg1, arg2);
        },
        __wbg_dispatchWorkgroups_56b943172790add0: function(arg0, arg1, arg2, arg3) {
            arg0.dispatchWorkgroups(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0);
        },
        __wbg_document_9854e03c05fc8834: function(arg0) {
            const ret = arg0.document;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_done_b41a1d26cdb37fb6: function(arg0) {
            const ret = arg0.done;
            return ret;
        },
        __wbg_drawIndexedIndirect_d01f2181b905a0b5: function(arg0, arg1, arg2) {
            arg0.drawIndexedIndirect(arg1, arg2);
        },
        __wbg_drawIndexed_638959aae942557c: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.drawIndexed(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0, arg4, arg5 >>> 0);
        },
        __wbg_end_82fd0c12ba185009: function(arg0) {
            arg0.end();
        },
        __wbg_end_b57473834b877409: function(arg0) {
            arg0.end();
        },
        __wbg_features_5cac120c28ba0475: function(arg0) {
            const ret = arg0.features;
            return ret;
        },
        __wbg_features_dec7bd2fd3d91bd6: function(arg0) {
            const ret = arg0.features;
            return ret;
        },
        __wbg_finish_09ec094c10f41e7b: function(arg0) {
            const ret = arg0.finish();
            return ret;
        },
        __wbg_finish_ec1c191f66a895b1: function(arg0, arg1) {
            const ret = arg0.finish(arg1);
            return ret;
        },
        __wbg_flush_cc7a0c869c969595: function() { return handleError(function (arg0) {
            arg0.flush();
        }, arguments); },
        __wbg_getBindGroupLayout_349f9101ad6d4d76: function(arg0, arg1) {
            const ret = arg0.getBindGroupLayout(arg1 >>> 0);
            return ret;
        },
        __wbg_getBindGroupLayout_fb40c3b93d37f466: function(arg0, arg1) {
            const ret = arg0.getBindGroupLayout(arg1 >>> 0);
            return ret;
        },
        __wbg_getContext_635e36719cad2623: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.getContext(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_getContext_e567868594d2e0c6: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.getContext(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_getCurrentTexture_9f3b84d0eaa6cd95: function() { return handleError(function (arg0) {
            const ret = arg0.getCurrentTexture();
            return ret;
        }, arguments); },
        __wbg_getMappedRange_fb54c6327b2d8d20: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.getMappedRange(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_getPreferredCanvasFormat_0ef5034c8902201b: function(arg0) {
            const ret = arg0.getPreferredCanvasFormat();
            return (__wbindgen_enum_GpuTextureFormat.indexOf(ret) + 1 || 102) - 1;
        },
        __wbg_getSize_b3c9adeccff7e656: function() { return handleError(function (arg0) {
            const ret = arg0.getSize();
            return ret;
        }, arguments); },
        __wbg_get_31af05bd4842a84f: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_get_658f6698067d9515: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(arg0, arg1);
            return ret;
        }, arguments); },
        __wbg_get_fd12a9100976c296: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_get_unchecked_288889d017702237: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_with_ref_key_6412cf3094599694: function(arg0, arg1) {
            const ret = arg0[arg1];
            return ret;
        },
        __wbg_gpu_afdd4387c7afe5f9: function(arg0) {
            const ret = arg0.gpu;
            return ret;
        },
        __wbg_has_eafa12e457ea88fb: function(arg0, arg1, arg2) {
            const ret = arg0.has(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_height_aa756cf29b6d629c: function(arg0) {
            const ret = arg0.height;
            return ret;
        },
        __wbg_height_fc97e1a0c2e7331f: function(arg0) {
            const ret = arg0.height;
            return ret;
        },
        __wbg_info_971d8b9db3dae69f: function(arg0) {
            const ret = arg0.info;
            return ret;
        },
        __wbg_instanceof_ArrayBuffer_a99f175873e5d9b8: function(arg0) {
            let result;
            try {
                result = arg0 instanceof ArrayBuffer;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_BigInt64Array_1a1234d945c1625f: function(arg0) {
            let result;
            try {
                result = arg0 instanceof BigInt64Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_BigUint64Array_0362060ffc2f1e06: function(arg0) {
            let result;
            try {
                result = arg0 instanceof BigUint64Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Float32Array_595afb0f28b828c9: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Float32Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Float64Array_8a59377f9eec1b8b: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Float64Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_GpuOutOfMemoryError_5c3b8a2499f59adb: function(arg0) {
            let result;
            try {
                result = arg0 instanceof GPUOutOfMemoryError;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_GpuValidationError_7659e07b12d4e184: function(arg0) {
            let result;
            try {
                result = arg0 instanceof GPUValidationError;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Int16Array_dc79388a406d1748: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Int16Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Int32Array_4b9a86620726269b: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Int32Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Int8Array_dead59f83656e45a: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Int8Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Performance_cbec0dbedaa91be1: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Performance;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Uint16Array_eff85aa31096af44: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Uint16Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Uint32Array_53a4facaf04b8a8b: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Uint32Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Uint8Array_828cef2aaacafc31: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Uint8Array;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Window_82d71df4eddf88bc: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Window;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_isArray_e15a2ff68ffdbef2: function(arg0) {
            const ret = Array.isArray(arg0);
            return ret;
        },
        __wbg_isFallbackAdapter_4c8cc3b18677460a: function(arg0) {
            const ret = arg0.isFallbackAdapter;
            return ret;
        },
        __wbg_isSafeInteger_717808ad6a54bd9e: function(arg0) {
            const ret = Number.isSafeInteger(arg0);
            return ret;
        },
        __wbg_iterator_e3c31c892080e444: function() {
            const ret = Symbol.iterator;
            return ret;
        },
        __wbg_label_7add8cb37a6ef98f: function(arg0, arg1) {
            const ret = arg1.label;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_length_0acba72356589f9c: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_1e9d4dcab09bd28f: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_3035e53132b7a991: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_511b1f84719d3462: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_58572db4c38f3c3e: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_7f3c00c40364105e: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_961efae5d4679ef5: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_b5f0008bbf60cf59: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_beff923aab722a55: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_d4bdea10311bd9cf: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_e5fdd54468b9e6ed: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_limits_06bcb36c8409843b: function(arg0) {
            const ret = arg0.limits;
            return ret;
        },
        __wbg_limits_601ad2e086ef8141: function(arg0) {
            const ret = arg0.limits;
            return ret;
        },
        __wbg_lost_bb454eb62101af1a: function(arg0) {
            const ret = arg0.lost;
            return ret;
        },
        __wbg_mapAsync_b0597127f5037286: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.mapAsync(arg1 >>> 0, arg2, arg3);
            return ret;
        },
        __wbg_maxBindGroupsPlusVertexBuffers_52369f089736ef9d: function(arg0) {
            const ret = arg0.maxBindGroupsPlusVertexBuffers;
            return ret;
        },
        __wbg_maxBindGroups_4e424afe6ce86ca2: function(arg0) {
            const ret = arg0.maxBindGroups;
            return ret;
        },
        __wbg_maxBindingsPerBindGroup_7d035da36821c44f: function(arg0) {
            const ret = arg0.maxBindingsPerBindGroup;
            return ret;
        },
        __wbg_maxBufferSize_423f4a084e32a195: function(arg0) {
            const ret = arg0.maxBufferSize;
            return ret;
        },
        __wbg_maxColorAttachmentBytesPerSample_c4cd9126f6d287c6: function(arg0) {
            const ret = arg0.maxColorAttachmentBytesPerSample;
            return ret;
        },
        __wbg_maxColorAttachments_d924670762b9e250: function(arg0) {
            const ret = arg0.maxColorAttachments;
            return ret;
        },
        __wbg_maxComputeInvocationsPerWorkgroup_707a3868f7cebb59: function(arg0) {
            const ret = arg0.maxComputeInvocationsPerWorkgroup;
            return ret;
        },
        __wbg_maxComputeWorkgroupSizeX_0a4d99463cbd6e5e: function(arg0) {
            const ret = arg0.maxComputeWorkgroupSizeX;
            return ret;
        },
        __wbg_maxComputeWorkgroupSizeY_85123ea0587f7558: function(arg0) {
            const ret = arg0.maxComputeWorkgroupSizeY;
            return ret;
        },
        __wbg_maxComputeWorkgroupSizeZ_a3186b4c5267d44f: function(arg0) {
            const ret = arg0.maxComputeWorkgroupSizeZ;
            return ret;
        },
        __wbg_maxComputeWorkgroupStorageSize_57b297355cfb6204: function(arg0) {
            const ret = arg0.maxComputeWorkgroupStorageSize;
            return ret;
        },
        __wbg_maxComputeWorkgroupsPerDimension_4158f95e673d54c4: function(arg0) {
            const ret = arg0.maxComputeWorkgroupsPerDimension;
            return ret;
        },
        __wbg_maxDynamicStorageBuffersPerPipelineLayout_226b0b70910aa16c: function(arg0) {
            const ret = arg0.maxDynamicStorageBuffersPerPipelineLayout;
            return ret;
        },
        __wbg_maxDynamicUniformBuffersPerPipelineLayout_0e835fda711fc7e6: function(arg0) {
            const ret = arg0.maxDynamicUniformBuffersPerPipelineLayout;
            return ret;
        },
        __wbg_maxInterStageShaderVariables_8c4a1d727e2aa35a: function(arg0) {
            const ret = arg0.maxInterStageShaderVariables;
            return ret;
        },
        __wbg_maxSampledTexturesPerShaderStage_6675f5e91d9a728a: function(arg0) {
            const ret = arg0.maxSampledTexturesPerShaderStage;
            return ret;
        },
        __wbg_maxSamplersPerShaderStage_1910fa38a6ed1e1f: function(arg0) {
            const ret = arg0.maxSamplersPerShaderStage;
            return ret;
        },
        __wbg_maxStorageBufferBindingSize_2e244bded070b18d: function(arg0) {
            const ret = arg0.maxStorageBufferBindingSize;
            return ret;
        },
        __wbg_maxStorageBuffersPerShaderStage_a285f3ebca51ca0d: function(arg0) {
            const ret = arg0.maxStorageBuffersPerShaderStage;
            return ret;
        },
        __wbg_maxStorageTexturesPerShaderStage_7aa946f0fc322a2b: function(arg0) {
            const ret = arg0.maxStorageTexturesPerShaderStage;
            return ret;
        },
        __wbg_maxTextureArrayLayers_0e699147ad00502d: function(arg0) {
            const ret = arg0.maxTextureArrayLayers;
            return ret;
        },
        __wbg_maxTextureDimension1D_aabf6add54decfe2: function(arg0) {
            const ret = arg0.maxTextureDimension1D;
            return ret;
        },
        __wbg_maxTextureDimension2D_dd598b27e9c0c1c4: function(arg0) {
            const ret = arg0.maxTextureDimension2D;
            return ret;
        },
        __wbg_maxTextureDimension3D_f944266c65dfd1a9: function(arg0) {
            const ret = arg0.maxTextureDimension3D;
            return ret;
        },
        __wbg_maxUniformBufferBindingSize_59fa6be7cfbeeb53: function(arg0) {
            const ret = arg0.maxUniformBufferBindingSize;
            return ret;
        },
        __wbg_maxUniformBuffersPerShaderStage_bee5f00a4d706c7f: function(arg0) {
            const ret = arg0.maxUniformBuffersPerShaderStage;
            return ret;
        },
        __wbg_maxVertexAttributes_5cf6392c4e9033fe: function(arg0) {
            const ret = arg0.maxVertexAttributes;
            return ret;
        },
        __wbg_maxVertexBufferArrayStride_548baa887375d865: function(arg0) {
            const ret = arg0.maxVertexBufferArrayStride;
            return ret;
        },
        __wbg_maxVertexBuffers_75d881156591f5da: function(arg0) {
            const ret = arg0.maxVertexBuffers;
            return ret;
        },
        __wbg_message_2aad50368e15e8f1: function(arg0, arg1) {
            const ret = arg1.message;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_message_ab4cad9eeb386607: function(arg0, arg1) {
            const ret = arg1.message;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_minStorageBufferOffsetAlignment_5ba9b77792bdadb3: function(arg0) {
            const ret = arg0.minStorageBufferOffsetAlignment;
            return ret;
        },
        __wbg_minUniformBufferOffsetAlignment_ab7d52a5293b22bd: function(arg0) {
            const ret = arg0.minUniformBufferOffsetAlignment;
            return ret;
        },
        __wbg_navigator_2156486643462a87: function(arg0) {
            const ret = arg0.navigator;
            return ret;
        },
        __wbg_navigator_c9bce48d4b578c9c: function(arg0) {
            const ret = arg0.navigator;
            return ret;
        },
        __wbg_new_0207353e062d9613: function(arg0) {
            const ret = new Int16Array(arg0);
            return ret;
        },
        __wbg_new_17966584ea16a1c2: function(arg0) {
            const ret = new Uint16Array(arg0);
            return ret;
        },
        __wbg_new_1dbf7428bba60a42: function(arg0) {
            const ret = new Uint8Array(arg0);
            return ret;
        },
        __wbg_new_1de155c52d932668: function(arg0) {
            const ret = new BigUint64Array(arg0);
            return ret;
        },
        __wbg_new_320e2e208176b20f: function(arg0) {
            const ret = new Int32Array(arg0);
            return ret;
        },
        __wbg_new_343a093a3c2ffb4e: function(arg0, arg1) {
            const ret = new Error(getStringFromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_51964c8aa9e1daa2: function(arg0) {
            const ret = new BigInt64Array(arg0);
            return ret;
        },
        __wbg_new_5502aad30c185fc8: function(arg0, arg1) {
            try {
                var state0 = {a: arg0, b: arg1};
                var cb0 = (arg0, arg1) => {
                    const a = state0.a;
                    state0.a = 0;
                    try {
                        return wasm_bindgen_540468089f657b51___convert__closures_____invoke___js_sys_47f7c93dc3ab9b71___Function_fn_wasm_bindgen_540468089f657b51___JsValue_____wasm_bindgen_540468089f657b51___sys__Undefined___js_sys_47f7c93dc3ab9b71___Function_fn_wasm_bindgen_540468089f657b51___JsValue_____wasm_bindgen_540468089f657b51___sys__Undefined_______true_(a, state0.b, arg0, arg1);
                    } finally {
                        state0.a = a;
                    }
                };
                const ret = new Promise(cb0);
                return ret;
            } finally {
                state0.a = 0;
            }
        },
        __wbg_new_58861e4871588832: function(arg0) {
            const ret = new Int8Array(arg0);
            return ret;
        },
        __wbg_new_617a8cdb8bb1130e: function() {
            const ret = new Object();
            return ret;
        },
        __wbg_new_63c4c65d2534e5d3: function(arg0) {
            const ret = new Float32Array(arg0);
            return ret;
        },
        __wbg_new_74cd12440f4b8e4e: function(arg0) {
            const ret = new Uint32Array(arg0);
            return ret;
        },
        __wbg_new_9ffe4dece67ce9fc: function() { return handleError(function () {
            const ret = new FileReaderSync();
            return ret;
        }, arguments); },
        __wbg_new_a217df4351db4b9b: function() { return handleError(function (arg0, arg1) {
            const ret = new OffscreenCanvas(arg0 >>> 0, arg1 >>> 0);
            return ret;
        }, arguments); },
        __wbg_new_c0cfdc72bf7dee4d: function(arg0) {
            const ret = new Float64Array(arg0);
            return ret;
        },
        __wbg_new_ee2291f50781bf1d: function() {
            const ret = new Array();
            return ret;
        },
        __wbg_new_from_slice_23f60f47cde8d664: function(arg0, arg1) {
            const ret = new Uint16Array(getArrayU16FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_slice_2443ca2017fac0fd: function(arg0, arg1) {
            const ret = new Int16Array(getArrayI16FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_slice_36f6b4e63daa7f59: function(arg0, arg1) {
            const ret = new Int8Array(getArrayI8FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_slice_37a3c3a194266cb8: function(arg0, arg1) {
            const ret = new Uint32Array(getArrayU32FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_slice_911f717853b222c7: function(arg0, arg1) {
            const ret = new Int32Array(getArrayI32FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_slice_9a868026ffa4208a: function(arg0, arg1) {
            const ret = new Uint8Array(getArrayU8FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_slice_aa8f37bd5e72dcc7: function(arg0, arg1) {
            const ret = new BigInt64Array(getArrayI64FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_slice_ca6ad97db1f4779a: function(arg0, arg1) {
            const ret = new Float32Array(getArrayF32FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_slice_f545fd22ddc142b8: function(arg0, arg1) {
            const ret = new Float64Array(getArrayF64FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_from_slice_f6cd96ab370a57b3: function(arg0, arg1) {
            const ret = new BigUint64Array(getArrayU64FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_typed_689a3a281a8d4da6: function() {
            const ret = new Object();
            return ret;
        },
        __wbg_new_typed_b01cb72a8af741a3: function(arg0, arg1) {
            try {
                var state0 = {a: arg0, b: arg1};
                var cb0 = (arg0, arg1) => {
                    const a = state0.a;
                    state0.a = 0;
                    try {
                        return wasm_bindgen_540468089f657b51___convert__closures_____invoke___js_sys_47f7c93dc3ab9b71___Function_fn_wasm_bindgen_540468089f657b51___JsValue_____wasm_bindgen_540468089f657b51___sys__Undefined___js_sys_47f7c93dc3ab9b71___Function_fn_wasm_bindgen_540468089f657b51___JsValue_____wasm_bindgen_540468089f657b51___sys__Undefined_______true_(a, state0.b, arg0, arg1);
                    } finally {
                        state0.a = a;
                    }
                };
                const ret = new Promise(cb0);
                return ret;
            } finally {
                state0.a = 0;
            }
        },
        __wbg_new_with_byte_offset_and_length_2f5d7fc2a828b74d: function(arg0, arg1, arg2) {
            const ret = new Uint8Array(arg0, arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_new_with_length_3da0ad195f6f63ba: function(arg0) {
            const ret = new Uint8Array(arg0 >>> 0);
            return ret;
        },
        __wbg_next_33784799010f1bbe: function(arg0) {
            const ret = arg0.next;
            return ret;
        },
        __wbg_next_f4aac29c42af995c: function() { return handleError(function (arg0) {
            const ret = arg0.next();
            return ret;
        }, arguments); },
        __wbg_now_aa4ccb83129e9e55: function() {
            const ret = Date.now();
            return ret;
        },
        __wbg_now_d0b7f4bea9f38490: function(arg0) {
            const ret = arg0.now();
            return ret;
        },
        __wbg_of_20798cb14708764f: function(arg0, arg1) {
            const ret = Array.of(arg0, arg1);
            return ret;
        },
        __wbg_onSubmittedWorkDone_1190213cee1ecf7e: function(arg0) {
            const ret = arg0.onSubmittedWorkDone();
            return ret;
        },
        __wbg_pageRead_0eb99c7a95d41fff: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.pageRead(arg1 >>> 0, arg2, getArrayU8FromWasm0(arg3, arg4));
            return ret;
        }, arguments); },
        __wbg_pageWrite_a48ba4bf4ab5556e: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            const ret = arg0.pageWrite(arg1 >>> 0, arg2, getArrayU8FromWasm0(arg3, arg4));
            return ret;
        }, arguments); },
        __wbg_popErrorScope_182b8e03671d81ef: function(arg0) {
            const ret = arg0.popErrorScope();
            return ret;
        },
        __wbg_postMessage_7dd4fec24fe9919c: function() { return handleError(function (arg0, arg1) {
            arg0.postMessage(arg1);
        }, arguments); },
        __wbg_prototypesetcall_06eb15da165dee8f: function(arg0, arg1, arg2) {
            Uint32Array.prototype.set.call(getArrayU32FromWasm0(arg0, arg1), arg2);
        },
        __wbg_prototypesetcall_13adbfb002dee200: function(arg0, arg1, arg2) {
            Int32Array.prototype.set.call(getArrayI32FromWasm0(arg0, arg1), arg2);
        },
        __wbg_prototypesetcall_158adb600d119d3c: function(arg0, arg1, arg2) {
            BigUint64Array.prototype.set.call(getArrayU64FromWasm0(arg0, arg1), arg2);
        },
        __wbg_prototypesetcall_2ee09b855e324444: function(arg0, arg1, arg2) {
            Uint16Array.prototype.set.call(getArrayU16FromWasm0(arg0, arg1), arg2);
        },
        __wbg_prototypesetcall_546a6fcbab18189a: function(arg0, arg1, arg2) {
            Int8Array.prototype.set.call(getArrayI8FromWasm0(arg0, arg1), arg2);
        },
        __wbg_prototypesetcall_8b03c01e2c0f8066: function(arg0, arg1, arg2) {
            Float32Array.prototype.set.call(getArrayF32FromWasm0(arg0, arg1), arg2);
        },
        __wbg_prototypesetcall_b275ad9519a0a737: function(arg0, arg1, arg2) {
            BigInt64Array.prototype.set.call(getArrayI64FromWasm0(arg0, arg1), arg2);
        },
        __wbg_prototypesetcall_bc27214492979395: function(arg0, arg1, arg2) {
            Uint8Array.prototype.set.call(getArrayU8FromWasm0(arg0, arg1), arg2);
        },
        __wbg_prototypesetcall_d49a4fab5ca427bc: function(arg0, arg1, arg2) {
            Float64Array.prototype.set.call(getArrayF64FromWasm0(arg0, arg1), arg2);
        },
        __wbg_prototypesetcall_f8d32a400a944612: function(arg0, arg1, arg2) {
            Int16Array.prototype.set.call(getArrayI16FromWasm0(arg0, arg1), arg2);
        },
        __wbg_pushErrorScope_fa7df784c43bdb51: function(arg0, arg1) {
            arg0.pushErrorScope(__wbindgen_enum_GpuErrorFilter[arg1]);
        },
        __wbg_push_2baf45db356cf468: function(arg0, arg1) {
            const ret = arg0.push(arg1);
            return ret;
        },
        __wbg_querySelectorAll_6aebfe3df1fb2014: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.querySelectorAll(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_queueMicrotask_9833f9a49df95a49: function(arg0) {
            const ret = arg0.queueMicrotask;
            return ret;
        },
        __wbg_queueMicrotask_a72f977e97f23c5f: function(arg0) {
            queueMicrotask(arg0);
        },
        __wbg_queue_7b62c28143d44293: function(arg0) {
            const ret = arg0.queue;
            return ret;
        },
        __wbg_ramAlloc_2151ed6b4d85255f: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.ramAlloc(arg1 >>> 0, arg2 >>> 0);
        }, arguments); },
        __wbg_ramFree_93be18630462a1d6: function() { return handleError(function (arg0, arg1) {
            arg0.ramFree(arg1 >>> 0);
        }, arguments); },
        __wbg_ramRead_90c9a5e158213a8e: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.ramRead(arg1 >>> 0, arg2 >>> 0, getArrayU8FromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_ramShrink_8e5b0edb2b200fa7: function() { return handleError(function (arg0, arg1, arg2) {
            arg0.ramShrink(arg1 >>> 0, arg2 >>> 0);
        }, arguments); },
        __wbg_ramWrite_a7a2cd4d34dc6d38: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4) {
            arg0.ramWrite(arg1 >>> 0, arg2 >>> 0, getArrayU8FromWasm0(arg3, arg4));
        }, arguments); },
        __wbg_readAsArrayBuffer_14bc98cc9cdf7c0b: function() { return handleError(function (arg0, arg1) {
            const ret = arg0.readAsArrayBuffer(arg1);
            return ret;
        }, arguments); },
        __wbg_read_fcd52999af006e79: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.read(getArrayU8FromWasm0(arg1, arg2), arg3);
            return ret;
        }, arguments); },
        __wbg_reason_b4fa3af867fc4430: function(arg0) {
            const ret = arg0.reason;
            return (__wbindgen_enum_GpuDeviceLostReason.indexOf(ret) + 1 || 3) - 1;
        },
        __wbg_requestAdapter_a539af006419f2e9: function(arg0, arg1) {
            const ret = arg0.requestAdapter(arg1);
            return ret;
        },
        __wbg_requestDevice_5cb8a582e55d08cb: function(arg0, arg1) {
            const ret = arg0.requestDevice(arg1);
            return ret;
        },
        __wbg_resolveQuerySet_770f23fabac49845: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.resolveQuerySet(arg1, arg2 >>> 0, arg3 >>> 0, arg4, arg5 >>> 0);
        },
        __wbg_resolve_0076e10020304ede: function(arg0) {
            const ret = Promise.resolve(arg0);
            return ret;
        },
        __wbg_setBindGroup_0b7c2a055ef1f314: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.setBindGroup(arg1 >>> 0, arg2, getArrayU32FromWasm0(arg3, arg4), arg5, arg6 >>> 0);
        }, arguments); },
        __wbg_setBindGroup_11bdbb60cc8b54b9: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.setBindGroup(arg1 >>> 0, arg2, getArrayU32FromWasm0(arg3, arg4), arg5, arg6 >>> 0);
        }, arguments); },
        __wbg_setBindGroup_418c3e0eb6943ce0: function(arg0, arg1, arg2) {
            arg0.setBindGroup(arg1 >>> 0, arg2);
        },
        __wbg_setBindGroup_c83391351ce68826: function(arg0, arg1, arg2) {
            arg0.setBindGroup(arg1 >>> 0, arg2);
        },
        __wbg_setIndexBuffer_241097e303986c14: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.setIndexBuffer(arg1, __wbindgen_enum_GpuIndexFormat[arg2], arg3, arg4);
        },
        __wbg_setPipeline_5146f7b8d2b4af5c: function(arg0, arg1) {
            arg0.setPipeline(arg1);
        },
        __wbg_setPipeline_b6f981027e02cd16: function(arg0, arg1) {
            arg0.setPipeline(arg1);
        },
        __wbg_setVertexBuffer_6db3b60e99280744: function(arg0, arg1, arg2, arg3) {
            arg0.setVertexBuffer(arg1 >>> 0, arg2, arg3);
        },
        __wbg_setVertexBuffer_cbf4ca1627c02f4c: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.setVertexBuffer(arg1 >>> 0, arg2, arg3, arg4);
        },
        __wbg_set_145a351398b48c65: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Reflect.set(arg0, arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_set_34d08fd992d43d61: function(arg0, arg1, arg2) {
            arg0.set(arg1, arg2 >>> 0);
        },
        __wbg_set_6be42768c690e380: function(arg0, arg1, arg2) {
            arg0[arg1] = arg2;
        },
        __wbg_set_a_82818effc94f6256: function(arg0, arg1) {
            arg0.a = arg1;
        },
        __wbg_set_access_a099cfbbeec9b96f: function(arg0, arg1) {
            arg0.access = __wbindgen_enum_GpuStorageTextureAccess[arg1];
        },
        __wbg_set_alpha_106f21a936a85eba: function(arg0, arg1) {
            arg0.alpha = arg1;
        },
        __wbg_set_alpha_mode_5544568dbac50280: function(arg0, arg1) {
            arg0.alphaMode = __wbindgen_enum_GpuCanvasAlphaMode[arg1];
        },
        __wbg_set_alpha_to_coverage_enabled_3372ce329447b8f1: function(arg0, arg1) {
            arg0.alphaToCoverageEnabled = arg1 !== 0;
        },
        __wbg_set_array_layer_count_22afa0a979e4ad55: function(arg0, arg1) {
            arg0.arrayLayerCount = arg1 >>> 0;
        },
        __wbg_set_array_stride_f64_6816040e5e7598c3: function(arg0, arg1) {
            arg0.arrayStride = arg1;
        },
        __wbg_set_aspect_a48d046965270281: function(arg0, arg1) {
            arg0.aspect = __wbindgen_enum_GpuTextureAspect[arg1];
        },
        __wbg_set_at_a00f11a442de8f74: function(arg0, arg1) {
            arg0.at = arg1;
        },
        __wbg_set_attributes_9e38cb1dde387a5b: function(arg0, arg1, arg2) {
            arg0.attributes = getArrayJsValueViewFromWasm0(arg1, arg2);
        },
        __wbg_set_b_a3297ee7e7cac3a8: function(arg0, arg1) {
            arg0.b = arg1;
        },
        __wbg_set_base_array_layer_2435ba92c80346ae: function(arg0, arg1) {
            arg0.baseArrayLayer = arg1 >>> 0;
        },
        __wbg_set_base_mip_level_8b6093e875e7c65d: function(arg0, arg1) {
            arg0.baseMipLevel = arg1 >>> 0;
        },
        __wbg_set_bea140a88be9b277: function(arg0, arg1, arg2) {
            arg0[arg1 >>> 0] = arg2;
        },
        __wbg_set_beginning_of_pass_write_index_e552c5e8b8bbf52f: function(arg0, arg1) {
            arg0.beginningOfPassWriteIndex = arg1 >>> 0;
        },
        __wbg_set_beginning_of_pass_write_index_fa9ae10d5d2804ce: function(arg0, arg1) {
            arg0.beginningOfPassWriteIndex = arg1 >>> 0;
        },
        __wbg_set_bind_group_layouts_458c44ba55100b82: function(arg0, arg1, arg2) {
            arg0.bindGroupLayouts = getArrayJsValueViewFromWasm0(arg1, arg2);
        },
        __wbg_set_binding_81b3fac7f7acaf8d: function(arg0, arg1) {
            arg0.binding = arg1 >>> 0;
        },
        __wbg_set_binding_b6cee57f35ac5190: function(arg0, arg1) {
            arg0.binding = arg1 >>> 0;
        },
        __wbg_set_blend_1a801617945f7945: function(arg0, arg1) {
            arg0.blend = arg1;
        },
        __wbg_set_buffer_1548ae88a9188037: function(arg0, arg1) {
            arg0.buffer = arg1;
        },
        __wbg_set_buffer_8d0ac64ad20dfc84: function(arg0, arg1) {
            arg0.buffer = arg1;
        },
        __wbg_set_buffers_5d0e0c50791f710e: function(arg0, arg1, arg2) {
            arg0.buffers = getArrayJsValueViewFromWasm0(arg1, arg2);
        },
        __wbg_set_clear_value_gpu_color_dict_a9f763e8372ac1de: function(arg0, arg1) {
            arg0.clearValue = arg1;
        },
        __wbg_set_code_5d5b0b9e2fd0dca7: function(arg0, arg1, arg2) {
            arg0.code = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_color_8ecace4011f47d2e: function(arg0, arg1) {
            arg0.color = arg1;
        },
        __wbg_set_color_attachments_622fe2d5997fda7a: function(arg0, arg1, arg2) {
            arg0.colorAttachments = getArrayJsValueViewFromWasm0(arg1, arg2);
        },
        __wbg_set_compare_080c9e492ff36990: function(arg0, arg1) {
            arg0.compare = __wbindgen_enum_GpuCompareFunction[arg1];
        },
        __wbg_set_compute_bc754d9e37f6e4c9: function(arg0, arg1) {
            arg0.compute = arg1;
        },
        __wbg_set_count_8ff0c9474e39a849: function(arg0, arg1) {
            arg0.count = arg1 >>> 0;
        },
        __wbg_set_count_d9dc88156fd05bc7: function(arg0, arg1) {
            arg0.count = arg1 >>> 0;
        },
        __wbg_set_cull_mode_85d2b4ab0ce3a564: function(arg0, arg1) {
            arg0.cullMode = __wbindgen_enum_GpuCullMode[arg1];
        },
        __wbg_set_depth_bias_95abf479cae3f3cd: function(arg0, arg1) {
            arg0.depthBias = arg1;
        },
        __wbg_set_depth_bias_clamp_ba3d0b8348151350: function(arg0, arg1) {
            arg0.depthBiasClamp = arg1;
        },
        __wbg_set_depth_bias_slope_scale_6b2584d93f5b9cd2: function(arg0, arg1) {
            arg0.depthBiasSlopeScale = arg1;
        },
        __wbg_set_depth_clear_value_e30a4c754c6b3b26: function(arg0, arg1) {
            arg0.depthClearValue = arg1;
        },
        __wbg_set_depth_compare_a90de4e3714397ab: function(arg0, arg1) {
            arg0.depthCompare = __wbindgen_enum_GpuCompareFunction[arg1];
        },
        __wbg_set_depth_fail_op_b5c64541d1b6b482: function(arg0, arg1) {
            arg0.depthFailOp = __wbindgen_enum_GpuStencilOperation[arg1];
        },
        __wbg_set_depth_load_op_932888016d762d3e: function(arg0, arg1) {
            arg0.depthLoadOp = __wbindgen_enum_GpuLoadOp[arg1];
        },
        __wbg_set_depth_or_array_layers_e2f074a0284e4806: function(arg0, arg1) {
            arg0.depthOrArrayLayers = arg1 >>> 0;
        },
        __wbg_set_depth_read_only_be790175a1c2db9a: function(arg0, arg1) {
            arg0.depthReadOnly = arg1 !== 0;
        },
        __wbg_set_depth_stencil_attachment_54a8922f5fbe08bf: function(arg0, arg1) {
            arg0.depthStencilAttachment = arg1;
        },
        __wbg_set_depth_stencil_b7cffc59ad4da529: function(arg0, arg1) {
            arg0.depthStencil = arg1;
        },
        __wbg_set_depth_store_op_9054814f164ab55d: function(arg0, arg1) {
            arg0.depthStoreOp = __wbindgen_enum_GpuStoreOp[arg1];
        },
        __wbg_set_depth_write_enabled_31a821ee1fb3b0b3: function(arg0, arg1) {
            arg0.depthWriteEnabled = arg1 !== 0;
        },
        __wbg_set_device_210484a77b675c9c: function(arg0, arg1) {
            arg0.device = arg1;
        },
        __wbg_set_dimension_3da9d03131a9f446: function(arg0, arg1) {
            arg0.dimension = __wbindgen_enum_GpuTextureDimension[arg1];
        },
        __wbg_set_dimension_56332450afa3e0c0: function(arg0, arg1) {
            arg0.dimension = __wbindgen_enum_GpuTextureViewDimension[arg1];
        },
        __wbg_set_dst_factor_865ba9aaf187890c: function(arg0, arg1) {
            arg0.dstFactor = __wbindgen_enum_GpuBlendFactor[arg1];
        },
        __wbg_set_end_of_pass_write_index_8f164f9e60d4ad16: function(arg0, arg1) {
            arg0.endOfPassWriteIndex = arg1 >>> 0;
        },
        __wbg_set_end_of_pass_write_index_b54a8e3802b9ae0d: function(arg0, arg1) {
            arg0.endOfPassWriteIndex = arg1 >>> 0;
        },
        __wbg_set_entries_6f866302103b81e9: function(arg0, arg1, arg2) {
            arg0.entries = getArrayJsValueViewFromWasm0(arg1, arg2);
        },
        __wbg_set_entries_f26b77ab9548e906: function(arg0, arg1, arg2) {
            arg0.entries = getArrayJsValueViewFromWasm0(arg1, arg2);
        },
        __wbg_set_entry_point_71cef95c137b5774: function(arg0, arg1, arg2) {
            arg0.entryPoint = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_entry_point_ac8db535971761fe: function(arg0, arg1, arg2) {
            arg0.entryPoint = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_entry_point_b70f98f5025a114d: function(arg0, arg1, arg2) {
            arg0.entryPoint = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_external_texture_7f966c604c4f8098: function(arg0, arg1) {
            arg0.externalTexture = arg1;
        },
        __wbg_set_fail_op_d59d0187e4111dfe: function(arg0, arg1) {
            arg0.failOp = __wbindgen_enum_GpuStencilOperation[arg1];
        },
        __wbg_set_format_23f7f32549751d43: function(arg0, arg1) {
            arg0.format = __wbindgen_enum_GpuTextureFormat[arg1];
        },
        __wbg_set_format_283dca56552f07a3: function(arg0, arg1) {
            arg0.format = __wbindgen_enum_GpuTextureFormat[arg1];
        },
        __wbg_set_format_5080a858117ad2c1: function(arg0, arg1) {
            arg0.format = __wbindgen_enum_GpuVertexFormat[arg1];
        },
        __wbg_set_format_66735b94bd868ba2: function(arg0, arg1) {
            arg0.format = __wbindgen_enum_GpuTextureFormat[arg1];
        },
        __wbg_set_format_7f2bdbfb101b1ae1: function(arg0, arg1) {
            arg0.format = __wbindgen_enum_GpuTextureFormat[arg1];
        },
        __wbg_set_format_92732ea75d3b79f5: function(arg0, arg1) {
            arg0.format = __wbindgen_enum_GpuTextureFormat[arg1];
        },
        __wbg_set_format_f009e603f7d4c28e: function(arg0, arg1) {
            arg0.format = __wbindgen_enum_GpuTextureFormat[arg1];
        },
        __wbg_set_fragment_d2b0ec97d7cf8d47: function(arg0, arg1) {
            arg0.fragment = arg1;
        },
        __wbg_set_front_face_d3f8a2e07e7b25dd: function(arg0, arg1) {
            arg0.frontFace = __wbindgen_enum_GpuFrontFace[arg1];
        },
        __wbg_set_g_b527ee8a9bed553d: function(arg0, arg1) {
            arg0.g = arg1;
        },
        __wbg_set_has_dynamic_offset_0c72ffa900c5a269: function(arg0, arg1) {
            arg0.hasDynamicOffset = arg1 !== 0;
        },
        __wbg_set_height_c9789c1c77eaedff: function(arg0, arg1) {
            arg0.height = arg1 >>> 0;
        },
        __wbg_set_height_f6619158e5735877: function(arg0, arg1) {
            arg0.height = arg1 >>> 0;
        },
        __wbg_set_height_fde391767df1ce27: function(arg0, arg1) {
            arg0.height = arg1 >>> 0;
        },
        __wbg_set_index_1e17e9f8455f4ec8: function(arg0, arg1, arg2) {
            arg0[arg1 >>> 0] = arg2;
        },
        __wbg_set_label_09e2da5f8522dbe8: function(arg0, arg1, arg2) {
            arg0.label = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_label_17202740051e9722: function(arg0, arg1, arg2) {
            arg0.label = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_label_17b4858e1eef23dd: function(arg0, arg1, arg2) {
            arg0.label = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_label_2fefb39c0e0dbbe8: function(arg0, arg1, arg2) {
            arg0.label = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_label_3f2ccaafef5ff7c9: function(arg0, arg1, arg2) {
            arg0.label = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_label_612add98a4398f92: function(arg0, arg1, arg2) {
            arg0.label = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_label_6f69e25822616a1b: function(arg0, arg1, arg2) {
            arg0.label = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_label_70a09ee68d6b1b26: function(arg0, arg1, arg2) {
            arg0.label = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_label_92cd3811e96b487c: function(arg0, arg1, arg2) {
            arg0.label = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_label_9c2a186152427ee0: function(arg0, arg1, arg2) {
            arg0.label = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_label_c3eaf136aa464cba: function(arg0, arg1, arg2) {
            arg0.label = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_label_c7987704d29f284b: function(arg0, arg1, arg2) {
            arg0.label = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_label_cfe64bca8945ee30: function(arg0, arg1, arg2) {
            arg0.label = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_label_e02179cf97e95763: function(arg0, arg1, arg2) {
            arg0.label = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_label_ee172cd5f6a96961: function(arg0, arg1, arg2) {
            arg0.label = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_layout_454e3a091b390cd4: function(arg0, arg1) {
            arg0.layout = arg1;
        },
        __wbg_set_layout_4b5d5b12fbb72a76: function(arg0, arg1) {
            arg0.layout = arg1;
        },
        __wbg_set_layout_75dc1ca3f2421cff: function(arg0, arg1) {
            arg0.layout = arg1;
        },
        __wbg_set_layout_gpu_auto_layout_mode_06a2b95af1043098: function(arg0, arg1) {
            arg0.layout = __wbindgen_enum_GpuAutoLayoutMode[arg1];
        },
        __wbg_set_layout_gpu_auto_layout_mode_ee432e515c357fa8: function(arg0, arg1) {
            arg0.layout = __wbindgen_enum_GpuAutoLayoutMode[arg1];
        },
        __wbg_set_load_op_c56b1269acc2d51f: function(arg0, arg1) {
            arg0.loadOp = __wbindgen_enum_GpuLoadOp[arg1];
        },
        __wbg_set_mapped_at_creation_3f320fef6761b02c: function(arg0, arg1) {
            arg0.mappedAtCreation = arg1 !== 0;
        },
        __wbg_set_mask_c1079e551ec360dc: function(arg0, arg1) {
            arg0.mask = arg1 >>> 0;
        },
        __wbg_set_min_binding_size_f64_897e3cd4496ddec9: function(arg0, arg1) {
            arg0.minBindingSize = arg1;
        },
        __wbg_set_mip_level_count_047936c630acee7b: function(arg0, arg1) {
            arg0.mipLevelCount = arg1 >>> 0;
        },
        __wbg_set_mip_level_count_44bc46a1ae6f6daa: function(arg0, arg1) {
            arg0.mipLevelCount = arg1 >>> 0;
        },
        __wbg_set_mode_7edfbc344ef9c650: function(arg0, arg1) {
            arg0.mode = __wbindgen_enum_GpuCanvasToneMappingMode[arg1];
        },
        __wbg_set_module_392eeaa269f203b0: function(arg0, arg1) {
            arg0.module = arg1;
        },
        __wbg_set_module_715d37652c4998ec: function(arg0, arg1) {
            arg0.module = arg1;
        },
        __wbg_set_module_c9946af218d23b53: function(arg0, arg1) {
            arg0.module = arg1;
        },
        __wbg_set_multisample_ff72a7a5456cbeb7: function(arg0, arg1) {
            arg0.multisample = arg1;
        },
        __wbg_set_multisampled_039f032dc4b67367: function(arg0, arg1) {
            arg0.multisampled = arg1 !== 0;
        },
        __wbg_set_name_2c630595dc90a7aa: function(arg0, arg1, arg2) {
            arg0.name = getStringFromWasm0(arg1, arg2);
        },
        __wbg_set_offset_f64_127e8a0aa5c5485a: function(arg0, arg1) {
            arg0.offset = arg1;
        },
        __wbg_set_offset_f64_457756429ede426d: function(arg0, arg1) {
            arg0.offset = arg1;
        },
        __wbg_set_operation_00a77386523b88f9: function(arg0, arg1) {
            arg0.operation = __wbindgen_enum_GpuBlendOperation[arg1];
        },
        __wbg_set_pass_op_3cf10feb3d76ab97: function(arg0, arg1) {
            arg0.passOp = __wbindgen_enum_GpuStencilOperation[arg1];
        },
        __wbg_set_power_preference_b42d00a8facfbade: function(arg0, arg1) {
            arg0.powerPreference = __wbindgen_enum_GpuPowerPreference[arg1];
        },
        __wbg_set_primitive_e796cf76f0ff89f3: function(arg0, arg1) {
            arg0.primitive = arg1;
        },
        __wbg_set_query_set_f030702f1b69199f: function(arg0, arg1) {
            arg0.querySet = arg1;
        },
        __wbg_set_query_set_f9a94586851fdba2: function(arg0, arg1) {
            arg0.querySet = arg1;
        },
        __wbg_set_r_6ece4d74af63364f: function(arg0, arg1) {
            arg0.r = arg1;
        },
        __wbg_set_required_features_bbab71414c45e621: function(arg0, arg1, arg2) {
            arg0.requiredFeatures = getArrayJsValueViewFromWasm0(arg1, arg2);
        },
        __wbg_set_required_limits_837f62d865e7cfac: function(arg0, arg1) {
            arg0.requiredLimits = arg1;
        },
        __wbg_set_resolve_target_gpu_texture_view_e4c1e3bbb8c27d87: function(arg0, arg1) {
            arg0.resolveTarget = arg1;
        },
        __wbg_set_resource_8fd8658b30d86ecf: function(arg0, arg1) {
            arg0.resource = arg1;
        },
        __wbg_set_resource_gpu_buffer_binding_33099b25da65b610: function(arg0, arg1) {
            arg0.resource = arg1;
        },
        __wbg_set_resource_gpu_texture_view_4cffe7bc7c8e5cbe: function(arg0, arg1) {
            arg0.resource = arg1;
        },
        __wbg_set_sample_count_481c255a12054e1d: function(arg0, arg1) {
            arg0.sampleCount = arg1 >>> 0;
        },
        __wbg_set_sample_type_ebc5fcd029513bda: function(arg0, arg1) {
            arg0.sampleType = __wbindgen_enum_GpuTextureSampleType[arg1];
        },
        __wbg_set_sampler_89cb4a7efcfc6005: function(arg0, arg1) {
            arg0.sampler = arg1;
        },
        __wbg_set_shader_location_3fb9f6a012eba494: function(arg0, arg1) {
            arg0.shaderLocation = arg1 >>> 0;
        },
        __wbg_set_size_f64_2f591b0654540477: function(arg0, arg1) {
            arg0.size = arg1;
        },
        __wbg_set_size_f64_e844c985b8f95261: function(arg0, arg1) {
            arg0.size = arg1;
        },
        __wbg_set_size_gpu_extent_3d_dict_adf57388ab1d4f18: function(arg0, arg1) {
            arg0.size = arg1;
        },
        __wbg_set_src_factor_6f2c9ec8e4d3d979: function(arg0, arg1) {
            arg0.srcFactor = __wbindgen_enum_GpuBlendFactor[arg1];
        },
        __wbg_set_stencil_back_c54d0443b8b6a957: function(arg0, arg1) {
            arg0.stencilBack = arg1;
        },
        __wbg_set_stencil_clear_value_a321b0e045bfd8c2: function(arg0, arg1) {
            arg0.stencilClearValue = arg1 >>> 0;
        },
        __wbg_set_stencil_front_3ff3f8385852efff: function(arg0, arg1) {
            arg0.stencilFront = arg1;
        },
        __wbg_set_stencil_load_op_37d20deccb26a0f1: function(arg0, arg1) {
            arg0.stencilLoadOp = __wbindgen_enum_GpuLoadOp[arg1];
        },
        __wbg_set_stencil_read_mask_021ef4271b24352c: function(arg0, arg1) {
            arg0.stencilReadMask = arg1 >>> 0;
        },
        __wbg_set_stencil_read_only_75fe66a2356d6e92: function(arg0, arg1) {
            arg0.stencilReadOnly = arg1 !== 0;
        },
        __wbg_set_stencil_store_op_501f91638dd386e6: function(arg0, arg1) {
            arg0.stencilStoreOp = __wbindgen_enum_GpuStoreOp[arg1];
        },
        __wbg_set_stencil_write_mask_ec1c12237e094bdd: function(arg0, arg1) {
            arg0.stencilWriteMask = arg1 >>> 0;
        },
        __wbg_set_step_mode_3cbbdeba1e5dfd62: function(arg0, arg1) {
            arg0.stepMode = __wbindgen_enum_GpuVertexStepMode[arg1];
        },
        __wbg_set_storage_texture_786aea7c5773b6c1: function(arg0, arg1) {
            arg0.storageTexture = arg1;
        },
        __wbg_set_store_op_678f33376d741711: function(arg0, arg1) {
            arg0.storeOp = __wbindgen_enum_GpuStoreOp[arg1];
        },
        __wbg_set_strip_index_format_70313df755145d5e: function(arg0, arg1) {
            arg0.stripIndexFormat = __wbindgen_enum_GpuIndexFormat[arg1];
        },
        __wbg_set_targets_674b33931e512fb1: function(arg0, arg1, arg2) {
            arg0.targets = getArrayJsValueViewFromWasm0(arg1, arg2);
        },
        __wbg_set_texture_a33be3fe02ac6264: function(arg0, arg1) {
            arg0.texture = arg1;
        },
        __wbg_set_timestamp_writes_5f240bbaad97fcaf: function(arg0, arg1) {
            arg0.timestampWrites = arg1;
        },
        __wbg_set_timestamp_writes_de6a09f299b71b76: function(arg0, arg1) {
            arg0.timestampWrites = arg1;
        },
        __wbg_set_tone_mapping_320c1aad31db2e7f: function(arg0, arg1) {
            arg0.toneMapping = arg1;
        },
        __wbg_set_topology_b92cfe523bd9653b: function(arg0, arg1) {
            arg0.topology = __wbindgen_enum_GpuPrimitiveTopology[arg1];
        },
        __wbg_set_type_43e0092f16775979: function(arg0, arg1) {
            arg0.type = __wbindgen_enum_GpuSamplerBindingType[arg1];
        },
        __wbg_set_type_79cec55caf4cdb6d: function(arg0, arg1) {
            arg0.type = __wbindgen_enum_GpuBufferBindingType[arg1];
        },
        __wbg_set_type_7f7e54057b801caa: function(arg0, arg1) {
            arg0.type = __wbindgen_enum_GpuQueryType[arg1];
        },
        __wbg_set_unclipped_depth_32b7caf29fa5633d: function(arg0, arg1) {
            arg0.unclippedDepth = arg1 !== 0;
        },
        __wbg_set_usage_1ee33d98267e787d: function(arg0, arg1) {
            arg0.usage = arg1 >>> 0;
        },
        __wbg_set_usage_2365e2704b1fdb10: function(arg0, arg1) {
            arg0.usage = arg1 >>> 0;
        },
        __wbg_set_usage_d53ee6f0c7aedbfa: function(arg0, arg1) {
            arg0.usage = arg1 >>> 0;
        },
        __wbg_set_usage_f3e34822998d2147: function(arg0, arg1) {
            arg0.usage = arg1 >>> 0;
        },
        __wbg_set_vertex_77ed7a1229239b5a: function(arg0, arg1) {
            arg0.vertex = arg1;
        },
        __wbg_set_view_dimension_893e2d16561e56e8: function(arg0, arg1) {
            arg0.viewDimension = __wbindgen_enum_GpuTextureViewDimension[arg1];
        },
        __wbg_set_view_dimension_f2c5fe4bf927c3fe: function(arg0, arg1) {
            arg0.viewDimension = __wbindgen_enum_GpuTextureViewDimension[arg1];
        },
        __wbg_set_view_formats_427069064d8b7139: function(arg0, arg1, arg2) {
            arg0.viewFormats = getArrayJsValueViewFromWasm0(arg1, arg2);
        },
        __wbg_set_view_formats_9c2f01a6f3b365c7: function(arg0, arg1, arg2) {
            arg0.viewFormats = getArrayJsValueViewFromWasm0(arg1, arg2);
        },
        __wbg_set_view_gpu_texture_view_35f4655788535c4d: function(arg0, arg1) {
            arg0.view = arg1;
        },
        __wbg_set_view_gpu_texture_view_a532c825c52042c0: function(arg0, arg1) {
            arg0.view = arg1;
        },
        __wbg_set_visibility_d8a6821789538c25: function(arg0, arg1) {
            arg0.visibility = arg1 >>> 0;
        },
        __wbg_set_width_0ca908d167690992: function(arg0, arg1) {
            arg0.width = arg1 >>> 0;
        },
        __wbg_set_width_b0e1267db4b196b5: function(arg0, arg1) {
            arg0.width = arg1 >>> 0;
        },
        __wbg_set_width_b20525f5f4df4eb8: function(arg0, arg1) {
            arg0.width = arg1 >>> 0;
        },
        __wbg_set_write_mask_42d89f182ade6b2d: function(arg0, arg1) {
            arg0.writeMask = arg1 >>> 0;
        },
        __wbg_size_8f1c0b1a1fbb3810: function(arg0) {
            const ret = arg0.size;
            return ret;
        },
        __wbg_slice_afd8274aed4149c0: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.slice(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_static_accessor_GLOBAL_266715b9d96ba635: function() {
            const ret = typeof global === 'undefined' ? null : global;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_GLOBAL_THIS_10fb7dc1ae063179: function() {
            const ret = typeof globalThis === 'undefined' ? null : globalThis;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_SELF_0b583911f537483a: function() {
            const ret = typeof self === 'undefined' ? null : self;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_WINDOW_d7f903d1508cbdc4: function() {
            const ret = typeof window === 'undefined' ? null : window;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_subarray_002b94d5e13d1411: function(arg0, arg1, arg2) {
            const ret = arg0.subarray(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_subarray_8a56c92643c071e0: function(arg0, arg1, arg2) {
            const ret = arg0.subarray(arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_subgroupMaxSize_b43be0aa16182403: function(arg0) {
            const ret = arg0.subgroupMaxSize;
            return ret;
        },
        __wbg_subgroupMinSize_03feb6ee0cda6775: function(arg0) {
            const ret = arg0.subgroupMinSize;
            return ret;
        },
        __wbg_submit_077c85cc28e36892: function(arg0, arg1, arg2) {
            arg0.submit(getArrayJsValueViewFromWasm0(arg1, arg2));
        },
        __wbg_then_c8a35d4ad59c6b8e: function(arg0, arg1) {
            const ret = arg0.then(arg1);
            return ret;
        },
        __wbg_then_c949d5a25a4e78f8: function(arg0, arg1, arg2) {
            const ret = arg0.then(arg1, arg2);
            return ret;
        },
        __wbg_then_e71170d78fcf8954: function(arg0, arg1) {
            const ret = arg0.then(arg1);
            return ret;
        },
        __wbg_unconfigure_835307f58dc68d80: function(arg0) {
            arg0.unconfigure();
        },
        __wbg_unmap_6a96b14c9ef5f7f5: function(arg0) {
            arg0.unmap();
        },
        __wbg_value_f3c585ee8f5ba40c: function(arg0) {
            const ret = arg0.value;
            return ret;
        },
        __wbg_wasmasyncformatreader_new: function(arg0) {
            const ret = WasmAsyncFormatReader.__wrap(arg0);
            return ret;
        },
        __wbg_wasmasynclodreadresult_new: function(arg0) {
            const ret = WasmAsyncLodReadResult.__wrap(arg0);
            return ret;
        },
        __wbg_wasmpagegraft_new: function(arg0) {
            const ret = WasmPageGraft.__wrap(arg0);
            return ret;
        },
        __wbg_wasmvoxelviewer_new: function(arg0) {
            const ret = WasmVoxelViewer.__wrap(arg0);
            return ret;
        },
        __wbg_width_3d0dce3d9892e35e: function(arg0) {
            const ret = arg0.width;
            return ret;
        },
        __wbg_width_7a1b335e4a553dc3: function(arg0) {
            const ret = arg0.width;
            return ret;
        },
        __wbg_writeBuffer_f4bb3f54adfe1330: function() { return handleError(function (arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.writeBuffer(arg1, arg2, getArrayU8FromWasm0(arg3, arg4), arg5, arg6);
        }, arguments); },
        __wbg_write_9e2910049de41b0a: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = arg0.write(getArrayU8FromWasm0(arg1, arg2), arg3);
            return ret;
        }, arguments); },
        __wbg_zoneCount_6ed431757001b958: function(arg0) {
            const ret = arg0.zoneCount;
            return ret;
        },
        __wbg_zoneRelease_040089f4675a52a4: function() { return handleError(function (arg0, arg1) {
            arg0.zoneRelease(arg1 >>> 0);
        }, arguments); },
        __wbindgen_generic_0000000000000001: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [Externref], shim_idx: 608, ret: Result(Unit), inner_ret: Some(Result(Unit)) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen_540468089f657b51___convert__closures_____invoke___wasm_bindgen_540468089f657b51___JsValue__core_d134427b58e45605___result__Result_____wasm_bindgen_540468089f657b51___JsError___true_);
            return ret;
        },
        __wbindgen_generic_0000000000000002: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [NamedExternref("GPUDevice")], shim_idx: 555, ret: Result(Unit), inner_ret: Some(Result(Unit)) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen_540468089f657b51___convert__closures_____invoke___wasm_bindgen_540468089f657b51___sys__JsNullable_wgpu_8448f4460114fe7c___backend__webgpu__webgpu_sys__gen_GpuError__GpuError___core_d134427b58e45605___result__Result_____wasm_bindgen_540468089f657b51___JsError___true_);
            return ret;
        },
        __wbindgen_generic_0000000000000003: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [NamedExternref("GPUDeviceLostInfo")], shim_idx: 559, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen_540468089f657b51___convert__closures_____invoke___wgpu_8448f4460114fe7c___backend__webgpu__webgpu_sys__gen_GpuDeviceLostInfo__GpuDeviceLostInfo______true_);
            return ret;
        },
        __wbindgen_generic_0000000000000004: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [NamedExternref("any")], shim_idx: 555, ret: Result(Unit), inner_ret: Some(Result(Unit)) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen_540468089f657b51___convert__closures_____invoke___wasm_bindgen_540468089f657b51___sys__JsNullable_wgpu_8448f4460114fe7c___backend__webgpu__webgpu_sys__gen_GpuError__GpuError___core_d134427b58e45605___result__Result_____wasm_bindgen_540468089f657b51___JsError___true__120);
            return ret;
        },
        __wbindgen_generic_0000000000000005: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [NamedExternref("undefined")], shim_idx: 555, ret: Result(Unit), inner_ret: Some(Result(Unit)) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen_540468089f657b51___convert__closures_____invoke___wasm_bindgen_540468089f657b51___sys__JsNullable_wgpu_8448f4460114fe7c___backend__webgpu__webgpu_sys__gen_GpuError__GpuError___core_d134427b58e45605___result__Result_____wasm_bindgen_540468089f657b51___JsError___true__121);
            return ret;
        },
        __wbindgen_generic_0000000000000006: function(arg0) {
            // Cast intrinsic for `F64 -> Externref`.
            const ret = arg0;
            return ret;
        },
        __wbindgen_generic_0000000000000007: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(U8)) -> NamedExternref("Uint8Array")`.
            const ret = getArrayU8FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_generic_0000000000000008: function(arg0, arg1) {
            // Cast intrinsic for `Ref(String) -> Externref`.
            const ret = getStringFromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_generic_0000000000000009: function(arg0) {
            // Cast intrinsic for `U64 -> Externref`.
            const ret = BigInt.asUintN(64, arg0);
            return ret;
        },
        __wbindgen_init_externref_table: function() {
            const table = wasm.__wbindgen_externrefs;
            const offset = table.grow(4);
            table.set(0, undefined);
            table.set(offset + 0, undefined);
            table.set(offset + 1, null);
            table.set(offset + 2, true);
            table.set(offset + 3, false);
        },
    };
    return {
        __proto__: null,
        "./viewer_wasm_bg.js": import0,
    };
}

function wasm_bindgen_540468089f657b51___convert__closures_____invoke___wgpu_8448f4460114fe7c___backend__webgpu__webgpu_sys__gen_GpuDeviceLostInfo__GpuDeviceLostInfo______true_(arg0, arg1, arg2) {
    wasm.wasm_bindgen_540468089f657b51___convert__closures_____invoke___wgpu_8448f4460114fe7c___backend__webgpu__webgpu_sys__gen_GpuDeviceLostInfo__GpuDeviceLostInfo______true_(arg0, arg1, arg2);
}

function wasm_bindgen_540468089f657b51___convert__closures_____invoke___wasm_bindgen_540468089f657b51___JsValue__core_d134427b58e45605___result__Result_____wasm_bindgen_540468089f657b51___JsError___true_(arg0, arg1, arg2) {
    const ret = wasm.wasm_bindgen_540468089f657b51___convert__closures_____invoke___wasm_bindgen_540468089f657b51___JsValue__core_d134427b58e45605___result__Result_____wasm_bindgen_540468089f657b51___JsError___true_(arg0, arg1, arg2);
    if (ret[1]) {
        throw takeFromExternrefTable0(ret[0]);
    }
}

function wasm_bindgen_540468089f657b51___convert__closures_____invoke___wasm_bindgen_540468089f657b51___sys__JsNullable_wgpu_8448f4460114fe7c___backend__webgpu__webgpu_sys__gen_GpuError__GpuError___core_d134427b58e45605___result__Result_____wasm_bindgen_540468089f657b51___JsError___true_(arg0, arg1, arg2) {
    const ret = wasm.wasm_bindgen_540468089f657b51___convert__closures_____invoke___wasm_bindgen_540468089f657b51___sys__JsNullable_wgpu_8448f4460114fe7c___backend__webgpu__webgpu_sys__gen_GpuError__GpuError___core_d134427b58e45605___result__Result_____wasm_bindgen_540468089f657b51___JsError___true_(arg0, arg1, arg2);
    if (ret[1]) {
        throw takeFromExternrefTable0(ret[0]);
    }
}

function wasm_bindgen_540468089f657b51___convert__closures_____invoke___wasm_bindgen_540468089f657b51___sys__JsNullable_wgpu_8448f4460114fe7c___backend__webgpu__webgpu_sys__gen_GpuError__GpuError___core_d134427b58e45605___result__Result_____wasm_bindgen_540468089f657b51___JsError___true__120(arg0, arg1, arg2) {
    const ret = wasm.wasm_bindgen_540468089f657b51___convert__closures_____invoke___wasm_bindgen_540468089f657b51___sys__JsNullable_wgpu_8448f4460114fe7c___backend__webgpu__webgpu_sys__gen_GpuError__GpuError___core_d134427b58e45605___result__Result_____wasm_bindgen_540468089f657b51___JsError___true__120(arg0, arg1, arg2);
    if (ret[1]) {
        throw takeFromExternrefTable0(ret[0]);
    }
}

function wasm_bindgen_540468089f657b51___convert__closures_____invoke___wasm_bindgen_540468089f657b51___sys__JsNullable_wgpu_8448f4460114fe7c___backend__webgpu__webgpu_sys__gen_GpuError__GpuError___core_d134427b58e45605___result__Result_____wasm_bindgen_540468089f657b51___JsError___true__121(arg0, arg1, arg2) {
    const ret = wasm.wasm_bindgen_540468089f657b51___convert__closures_____invoke___wasm_bindgen_540468089f657b51___sys__JsNullable_wgpu_8448f4460114fe7c___backend__webgpu__webgpu_sys__gen_GpuError__GpuError___core_d134427b58e45605___result__Result_____wasm_bindgen_540468089f657b51___JsError___true__121(arg0, arg1, arg2);
    if (ret[1]) {
        throw takeFromExternrefTable0(ret[0]);
    }
}

function wasm_bindgen_540468089f657b51___convert__closures_____invoke___js_sys_47f7c93dc3ab9b71___Function_fn_wasm_bindgen_540468089f657b51___JsValue_____wasm_bindgen_540468089f657b51___sys__Undefined___js_sys_47f7c93dc3ab9b71___Function_fn_wasm_bindgen_540468089f657b51___JsValue_____wasm_bindgen_540468089f657b51___sys__Undefined_______true_(arg0, arg1, arg2, arg3) {
    wasm.wasm_bindgen_540468089f657b51___convert__closures_____invoke___js_sys_47f7c93dc3ab9b71___Function_fn_wasm_bindgen_540468089f657b51___JsValue_____wasm_bindgen_540468089f657b51___sys__Undefined___js_sys_47f7c93dc3ab9b71___Function_fn_wasm_bindgen_540468089f657b51___JsValue_____wasm_bindgen_540468089f657b51___sys__Undefined_______true_(arg0, arg1, arg2, arg3);
}


const __wbindgen_enum_GpuAutoLayoutMode = ["auto"];


const __wbindgen_enum_GpuBlendFactor = ["zero", "one", "src", "one-minus-src", "src-alpha", "one-minus-src-alpha", "dst", "one-minus-dst", "dst-alpha", "one-minus-dst-alpha", "src-alpha-saturated", "constant", "one-minus-constant", "src1", "one-minus-src1", "src1-alpha", "one-minus-src1-alpha"];


const __wbindgen_enum_GpuBlendOperation = ["add", "subtract", "reverse-subtract", "min", "max"];


const __wbindgen_enum_GpuBufferBindingType = ["uniform", "storage", "read-only-storage"];


const __wbindgen_enum_GpuCanvasAlphaMode = ["opaque", "premultiplied"];


const __wbindgen_enum_GpuCanvasToneMappingMode = ["standard", "extended"];


const __wbindgen_enum_GpuCompareFunction = ["never", "less", "equal", "less-equal", "greater", "not-equal", "greater-equal", "always"];


const __wbindgen_enum_GpuCullMode = ["none", "front", "back"];


const __wbindgen_enum_GpuDeviceLostReason = ["unknown", "destroyed"];


const __wbindgen_enum_GpuErrorFilter = ["validation", "out-of-memory", "internal"];


const __wbindgen_enum_GpuFrontFace = ["ccw", "cw"];


const __wbindgen_enum_GpuIndexFormat = ["uint16", "uint32"];


const __wbindgen_enum_GpuLoadOp = ["load", "clear"];


const __wbindgen_enum_GpuPowerPreference = ["low-power", "high-performance"];


const __wbindgen_enum_GpuPrimitiveTopology = ["point-list", "line-list", "line-strip", "triangle-list", "triangle-strip"];


const __wbindgen_enum_GpuQueryType = ["occlusion", "timestamp"];


const __wbindgen_enum_GpuSamplerBindingType = ["filtering", "non-filtering", "comparison"];


const __wbindgen_enum_GpuStencilOperation = ["keep", "zero", "replace", "invert", "increment-clamp", "decrement-clamp", "increment-wrap", "decrement-wrap"];


const __wbindgen_enum_GpuStorageTextureAccess = ["write-only", "read-only", "read-write"];


const __wbindgen_enum_GpuStoreOp = ["store", "discard"];


const __wbindgen_enum_GpuTextureAspect = ["all", "stencil-only", "depth-only"];


const __wbindgen_enum_GpuTextureDimension = ["1d", "2d", "3d"];


const __wbindgen_enum_GpuTextureFormat = ["r8unorm", "r8snorm", "r8uint", "r8sint", "r16unorm", "r16snorm", "r16uint", "r16sint", "r16float", "rg8unorm", "rg8snorm", "rg8uint", "rg8sint", "r32uint", "r32sint", "r32float", "rg16unorm", "rg16snorm", "rg16uint", "rg16sint", "rg16float", "rgba8unorm", "rgba8unorm-srgb", "rgba8snorm", "rgba8uint", "rgba8sint", "bgra8unorm", "bgra8unorm-srgb", "rgb9e5ufloat", "rgb10a2uint", "rgb10a2unorm", "rg11b10ufloat", "rg32uint", "rg32sint", "rg32float", "rgba16unorm", "rgba16snorm", "rgba16uint", "rgba16sint", "rgba16float", "rgba32uint", "rgba32sint", "rgba32float", "stencil8", "depth16unorm", "depth24plus", "depth24plus-stencil8", "depth32float", "depth32float-stencil8", "bc1-rgba-unorm", "bc1-rgba-unorm-srgb", "bc2-rgba-unorm", "bc2-rgba-unorm-srgb", "bc3-rgba-unorm", "bc3-rgba-unorm-srgb", "bc4-r-unorm", "bc4-r-snorm", "bc5-rg-unorm", "bc5-rg-snorm", "bc6h-rgb-ufloat", "bc6h-rgb-float", "bc7-rgba-unorm", "bc7-rgba-unorm-srgb", "etc2-rgb8unorm", "etc2-rgb8unorm-srgb", "etc2-rgb8a1unorm", "etc2-rgb8a1unorm-srgb", "etc2-rgba8unorm", "etc2-rgba8unorm-srgb", "eac-r11unorm", "eac-r11snorm", "eac-rg11unorm", "eac-rg11snorm", "astc-4x4-unorm", "astc-4x4-unorm-srgb", "astc-5x4-unorm", "astc-5x4-unorm-srgb", "astc-5x5-unorm", "astc-5x5-unorm-srgb", "astc-6x5-unorm", "astc-6x5-unorm-srgb", "astc-6x6-unorm", "astc-6x6-unorm-srgb", "astc-8x5-unorm", "astc-8x5-unorm-srgb", "astc-8x6-unorm", "astc-8x6-unorm-srgb", "astc-8x8-unorm", "astc-8x8-unorm-srgb", "astc-10x5-unorm", "astc-10x5-unorm-srgb", "astc-10x6-unorm", "astc-10x6-unorm-srgb", "astc-10x8-unorm", "astc-10x8-unorm-srgb", "astc-10x10-unorm", "astc-10x10-unorm-srgb", "astc-12x10-unorm", "astc-12x10-unorm-srgb", "astc-12x12-unorm", "astc-12x12-unorm-srgb"];


const __wbindgen_enum_GpuTextureSampleType = ["float", "unfilterable-float", "depth", "sint", "uint"];


const __wbindgen_enum_GpuTextureViewDimension = ["1d", "2d", "2d-array", "cube", "cube-array", "3d"];


const __wbindgen_enum_GpuVertexFormat = ["uint8", "uint8x2", "uint8x4", "sint8", "sint8x2", "sint8x4", "unorm8", "unorm8x2", "unorm8x4", "snorm8", "snorm8x2", "snorm8x4", "uint16", "uint16x2", "uint16x4", "sint16", "sint16x2", "sint16x4", "unorm16", "unorm16x2", "unorm16x4", "snorm16", "snorm16x2", "snorm16x4", "float16", "float16x2", "float16x4", "float32", "float32x2", "float32x3", "float32x4", "uint32", "uint32x2", "uint32x3", "uint32x4", "sint32", "sint32x2", "sint32x3", "sint32x4", "unorm10-10-10-2", "unorm8x4-bgra"];


const __wbindgen_enum_GpuVertexStepMode = ["vertex", "instance"];
const WasmAsyncFormatReaderFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmasyncformatreader_free(ptr, 1));
const WasmAsyncLodReadRequestFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmasynclodreadrequest_free(ptr, 1));
const WasmAsyncLodReadResultFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmasynclodreadresult_free(ptr, 1));
const WasmAsyncReadBudgetFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmasyncreadbudget_free(ptr, 1));
const WasmAsyncReadCancelTokenFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmasyncreadcanceltoken_free(ptr, 1));
const WasmBlockRequestSchedulerFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmblockrequestscheduler_free(ptr, 1));
const WasmCoreConverterFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmcoreconverter_free(ptr, 1));
const WasmPageGraftFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmpagegraft_free(ptr, 1));
const WasmSceneBuilderFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmscenebuilder_free(ptr, 1));
const WasmVoxelViewerFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_wasmvoxelviewer_free(ptr, 1));

function addToExternrefTable0(obj) {
    const idx = wasm.__externref_table_alloc();
    wasm.__wbindgen_externrefs.set(idx, obj);
    return idx;
}

function _assertClass(instance, klass) {
    if (!(instance instanceof klass)) {
        throw new Error(`expected instance of ${klass.name}`);
    }
}

const CLOSURE_DTORS = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(state => wasm.__wbindgen_destroy_closure(state.a, state.b));

function debugString(val) {
    // primitive types
    const type = typeof val;
    if (type == 'number' || type == 'boolean' || val == null) {
        return  `${val}`;
    }
    if (type == 'string') {
        return `"${val}"`;
    }
    if (type == 'symbol') {
        const description = val.description;
        if (description == null) {
            return 'Symbol';
        } else {
            return `Symbol(${description})`;
        }
    }
    if (type == 'function') {
        const name = val.name;
        if (typeof name == 'string' && name.length > 0) {
            return `Function(${name})`;
        } else {
            return 'Function';
        }
    }
    // objects
    if (Array.isArray(val)) {
        const length = val.length;
        let debug = '[';
        if (length > 0) {
            debug += debugString(val[0]);
        }
        for(let i = 1; i < length; i++) {
            debug += ', ' + debugString(val[i]);
        }
        debug += ']';
        return debug;
    }
    // Test for built-in
    const builtInMatches = /\[object ([^\]]+)\]/.exec(toString.call(val));
    let className;
    if (builtInMatches && builtInMatches.length > 1) {
        className = builtInMatches[1];
    } else {
        // Failed to match the standard '[object ClassName]'
        return toString.call(val);
    }
    if (className == 'Object') {
        // we're a user defined class or Object
        // JSON.stringify avoids problems with cycles, and is generally much
        // easier than looping through ownProperties of `val`.
        try {
            return 'Object(' + JSON.stringify(val) + ')';
        } catch (_) {
            return 'Object';
        }
    }
    // errors
    if (val instanceof Error) {
        return `${val.name}: ${val.message}\n${val.stack}`;
    }
    // TODO we could test for more things here, like `Set`s and `Map`s.
    return className;
}

function getArrayF32FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getFloat32ArrayMemory0().subarray(ptr / 4, ptr / 4 + len);
}

function getArrayF64FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getFloat64ArrayMemory0().subarray(ptr / 8, ptr / 8 + len);
}

function getArrayI16FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getInt16ArrayMemory0().subarray(ptr / 2, ptr / 2 + len);
}

function getArrayI32FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getInt32ArrayMemory0().subarray(ptr / 4, ptr / 4 + len);
}

function getArrayI64FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getBigInt64ArrayMemory0().subarray(ptr / 8, ptr / 8 + len);
}

function getArrayI8FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getInt8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
}

function getArrayJsValueViewFromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    const mem = getDataViewMemory0();
    const result = [];
    for (let i = ptr; i < ptr + 4 * len; i += 4) {
        result.push(wasm.__wbindgen_externrefs.get(mem.getUint32(i, true)));
    }
    return result;
}

function getArrayU16FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint16ArrayMemory0().subarray(ptr / 2, ptr / 2 + len);
}

function getArrayU32FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint32ArrayMemory0().subarray(ptr / 4, ptr / 4 + len);
}

function getArrayU64FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getBigUint64ArrayMemory0().subarray(ptr / 8, ptr / 8 + len);
}

function getArrayU8FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
}

let cachedBigInt64ArrayMemory0 = null;
function getBigInt64ArrayMemory0() {
    if (cachedBigInt64ArrayMemory0 === null || cachedBigInt64ArrayMemory0.byteLength === 0) {
        cachedBigInt64ArrayMemory0 = new BigInt64Array(wasm.memory.buffer);
    }
    return cachedBigInt64ArrayMemory0;
}

let cachedBigUint64ArrayMemory0 = null;
function getBigUint64ArrayMemory0() {
    if (cachedBigUint64ArrayMemory0 === null || cachedBigUint64ArrayMemory0.byteLength === 0) {
        cachedBigUint64ArrayMemory0 = new BigUint64Array(wasm.memory.buffer);
    }
    return cachedBigUint64ArrayMemory0;
}

let cachedDataViewMemory0 = null;
function getDataViewMemory0() {
    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
        cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
    }
    return cachedDataViewMemory0;
}

let cachedFloat32ArrayMemory0 = null;
function getFloat32ArrayMemory0() {
    if (cachedFloat32ArrayMemory0 === null || cachedFloat32ArrayMemory0.byteLength === 0) {
        cachedFloat32ArrayMemory0 = new Float32Array(wasm.memory.buffer);
    }
    return cachedFloat32ArrayMemory0;
}

let cachedFloat64ArrayMemory0 = null;
function getFloat64ArrayMemory0() {
    if (cachedFloat64ArrayMemory0 === null || cachedFloat64ArrayMemory0.byteLength === 0) {
        cachedFloat64ArrayMemory0 = new Float64Array(wasm.memory.buffer);
    }
    return cachedFloat64ArrayMemory0;
}

let cachedInt16ArrayMemory0 = null;
function getInt16ArrayMemory0() {
    if (cachedInt16ArrayMemory0 === null || cachedInt16ArrayMemory0.byteLength === 0) {
        cachedInt16ArrayMemory0 = new Int16Array(wasm.memory.buffer);
    }
    return cachedInt16ArrayMemory0;
}

let cachedInt32ArrayMemory0 = null;
function getInt32ArrayMemory0() {
    if (cachedInt32ArrayMemory0 === null || cachedInt32ArrayMemory0.byteLength === 0) {
        cachedInt32ArrayMemory0 = new Int32Array(wasm.memory.buffer);
    }
    return cachedInt32ArrayMemory0;
}

let cachedInt8ArrayMemory0 = null;
function getInt8ArrayMemory0() {
    if (cachedInt8ArrayMemory0 === null || cachedInt8ArrayMemory0.byteLength === 0) {
        cachedInt8ArrayMemory0 = new Int8Array(wasm.memory.buffer);
    }
    return cachedInt8ArrayMemory0;
}

function getStringFromWasm0(ptr, len) {
    return decodeText(ptr >>> 0, len);
}

let cachedUint16ArrayMemory0 = null;
function getUint16ArrayMemory0() {
    if (cachedUint16ArrayMemory0 === null || cachedUint16ArrayMemory0.byteLength === 0) {
        cachedUint16ArrayMemory0 = new Uint16Array(wasm.memory.buffer);
    }
    return cachedUint16ArrayMemory0;
}

let cachedUint32ArrayMemory0 = null;
function getUint32ArrayMemory0() {
    if (cachedUint32ArrayMemory0 === null || cachedUint32ArrayMemory0.byteLength === 0) {
        cachedUint32ArrayMemory0 = new Uint32Array(wasm.memory.buffer);
    }
    return cachedUint32ArrayMemory0;
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

function handleError(f, args) {
    try {
        return f.apply(this, args);
    } catch (e) {
        const idx = addToExternrefTable0(e);
        wasm.__wbindgen_exn_store(idx);
    }
}

function isLikeNone(x) {
    return x === undefined || x === null;
}

function makeMutClosure(arg0, arg1, f) {
    const state = { a: arg0, b: arg1, cnt: 1 };
    const real = (...args) => {

        // First up with a closure we increment the internal reference
        // count. This ensures that the Rust closure environment won't
        // be deallocated while we're invoking it.
        state.cnt++;
        const a = state.a;
        state.a = 0;
        try {
            return f(a, state.b, ...args);
        } finally {
            state.a = a;
            real._wbg_cb_unref();
        }
    };
    real._wbg_cb_unref = () => {
        if (--state.cnt === 0) {
            wasm.__wbindgen_destroy_closure(state.a, state.b);
            state.a = 0;
            CLOSURE_DTORS.unregister(state);
        }
    };
    CLOSURE_DTORS.register(real, state, state);
    return real;
}

function passArray32ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 4, 4) >>> 0;
    getUint32ArrayMemory0().set(arg, ptr / 4);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passArrayJsValueToWasm0(array, malloc) {
    const ptr = malloc(array.length * 4, 4) >>> 0;
    for (let i = 0; i < array.length; i++) {
        const add = addToExternrefTable0(array[i]);
        getDataViewMemory0().setUint32(ptr + 4 * i, add, true);
    }
    WASM_VECTOR_LEN = array.length;
    return ptr;
}

function passStringToWasm0(arg, malloc, realloc) {
    if (realloc === undefined) {
        const buf = cachedTextEncoder.encode(arg);
        const ptr = malloc(buf.length, 1) >>> 0;
        getUint8ArrayMemory0().subarray(ptr, ptr + buf.length).set(buf);
        WASM_VECTOR_LEN = buf.length;
        return ptr;
    }

    let len = arg.length;
    let ptr = malloc(len, 1) >>> 0;

    const mem = getUint8ArrayMemory0();

    let offset = 0;

    for (; offset < len; offset++) {
        const code = arg.charCodeAt(offset);
        if (code > 0x7F) break;
        mem[ptr + offset] = code;
    }
    if (offset !== len) {
        if (offset !== 0) {
            arg = arg.slice(offset);
        }
        ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
        const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
        const ret = cachedTextEncoder.encodeInto(arg, view);

        offset += ret.written;
        ptr = realloc(ptr, len, offset, 1) >>> 0;
    }

    WASM_VECTOR_LEN = offset;
    return ptr;
}

function takeFromExternrefTable0(idx) {
    const value = wasm.__wbindgen_externrefs.get(idx);
    wasm.__externref_table_dealloc(idx);
    return value;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
cachedTextDecoder.decode();
const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
        cachedTextDecoder.decode();
        numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

const cachedTextEncoder = new TextEncoder();

if (!('encodeInto' in cachedTextEncoder)) {
    cachedTextEncoder.encodeInto = function (arg, view) {
        const buf = cachedTextEncoder.encode(arg);
        view.set(buf);
        return {
            read: arg.length,
            written: buf.length
        };
    };
}

let WASM_VECTOR_LEN = 0;

let wasmModule, wasmInstance, wasm;
function __wbg_finalize_init(instance, module) {
    wasmInstance = instance;
    wasm = instance.exports;
    wasmModule = module;
    cachedBigInt64ArrayMemory0 = null;
    cachedBigUint64ArrayMemory0 = null;
    cachedDataViewMemory0 = null;
    cachedFloat32ArrayMemory0 = null;
    cachedFloat64ArrayMemory0 = null;
    cachedInt16ArrayMemory0 = null;
    cachedInt32ArrayMemory0 = null;
    cachedInt8ArrayMemory0 = null;
    cachedUint16ArrayMemory0 = null;
    cachedUint32ArrayMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    wasm.__wbindgen_start();
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (!module.ok) {
            throw new Error(`failed to fetch Wasm: ${module.status} ${module.statusText} fetching '${module.url}'`);
        }

        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = expectedResponseType(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else { throw e; }
            }
        }

        const bytes = await module.arrayBuffer();
        return await WebAssembly.instantiate(bytes, imports);
    } else {
        const instance = await WebAssembly.instantiate(module, imports);

        if (instance instanceof WebAssembly.Instance) {
            return { instance, module };
        } else {
            return instance;
        }
    }

    function expectedResponseType(type) {
        switch (type) {
            case 'basic': case 'cors': case 'default': return true;
        }
        return false;
    }
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (module !== undefined) {
        if (Object.getPrototypeOf(module) === Object.prototype) {
            ({module} = module)
        } else {
            console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
        }
    }

    const imports = __wbg_get_imports();
    if (!(module instanceof WebAssembly.Module)) {
        module = new WebAssembly.Module(module);
    }
    const instance = new WebAssembly.Instance(module, imports);
    return __wbg_finalize_init(instance, module);
}

async function __wbg_init(module_or_path) {
    if (wasm !== undefined) return wasm;


    if (module_or_path !== undefined) {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (module_or_path === undefined) {
        module_or_path = new URL('viewer_wasm_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync, __wbg_init as default };
