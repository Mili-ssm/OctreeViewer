// Async split-Blob reader probe; no GPU adapter or viewer renderer is required.
import init, {
    WasmAsyncFormatReader, WasmAsyncReadBudget, WasmBlockRequestScheduler, WasmVoxelViewer,
} from "../../../pkg/viewer_wasm.js";

function assert(condition, message) {
    if (!condition) throw new Error(message);
}

self.onmessage = async ({ data = {} }) => {
    try {
        await init();
        const primary = data.primary
            ?? await fetch(new URL("../sample-small.oct", import.meta.url)).then(response => response.blob());
        const attributes = data.attributes
            ?? await fetch(new URL("../sample-small.octattrs", import.meta.url)).then(response => response.blob());
        const primaryBytes = new Uint8Array(await primary.arrayBuffer());

        const scheduler = new WasmBlockRequestScheduler(2);
        scheduler.updateBlocks(["3", "2", "1"]);
        const secondPriority = scheduler.takeNextAdmitted(["2", "1"]);
        const highest = scheduler.takeNextAdmitted(["3", "1"]);
        assert(secondPriority.blockId === "2" && highest.blockId === "3",
            "scheduler did not preserve priority among byte-admitted requests");
        assert(scheduler.takeNextAdmitted(["1"]) === null,
            "scheduler exceeded its in-flight limit");
        const cancelledTickets = scheduler.updateBlocks(["2", "1"]);
        assert(cancelledTickets.length === 1 && cancelledTickets[0] === highest.ticket,
            "scheduler did not report a selection-obsolete ticket");
        assert(scheduler.complete(highest.ticket, "3", false) === "discarded",
            "cancelled ticket was not discarded on completion");
        assert(scheduler.complete(secondPriority.ticket, "2", false) === "failed",
            "selected ticket failure was not retained for explicit retry");
        assert(scheduler.pendingBlocks().length === 1 && scheduler.pendingBlocks()[0] === "1",
            "scheduler retried a failed ticket without an explicit retry");
        assert(scheduler.retry("2") && scheduler.pendingBlocks()[0] === "2",
            "scheduler could not explicitly retry a failed block");

        const originalArrayBuffer = Blob.prototype.arrayBuffer;
        const originalSlice = Blob.prototype.slice;
        const ranges = [];
        let arrayBufferCalls = 0;
        Blob.prototype.arrayBuffer = function () {
            arrayBufferCalls += 1;
            return originalArrayBuffer.call(this);
        };
        Blob.prototype.slice = function (start, end, type) {
            ranges.push({ size: this.size, start, end });
            return originalSlice.call(this, start, end, type);
        };
        try {
            const readBudget = new WasmAsyncReadBudget(64 * 1024 * 1024);
            const reader = await WasmAsyncFormatReader.open(
                primary, attributes, readBudget.clone(),
            );
            const descriptor = reader.lodDatasetDescriptor(64 * 1024 * 1024);
            assert(descriptor.semantics === "replacement" && descriptor.paged && descriptor.nodeCount > 0
                && descriptor.attributeSchema[0].name === "intensity"
                && descriptor.attributeRanges[0][0][0] === 0
                && descriptor.visualScale > 0 && descriptor.hasAttributes,
            "async reader did not export complete LOD metadata");
            const metadataViewer = new WasmVoxelViewer(0.6);
            assert(metadataViewer.openPagedLodDataset(reader, 64 * 1024 * 1024).status === "metadata_ready",
                "viewer rejected async reader metadata");
            metadataViewer.free();
            const geometryOnlyReader = await WasmAsyncFormatReader.open(primary);
            const geometryOnlyViewer = new WasmVoxelViewer(0.6);
            assert(!geometryOnlyReader.lodDatasetDescriptor(64 * 1024 * 1024).hasAttributes,
                "metadata-only reader claimed a missing companion");
            geometryOnlyViewer.openPagedLodDataset(geometryOnlyReader, 64 * 1024 * 1024);
            geometryOnlyViewer.free();
            geometryOnlyReader.free();

            const geometryPlan = reader.planRead(0n, "geometry");
            const geometryRanges = geometryPlan.ranges;
            assert(geometryRanges.length === 2, "geometry did not plan XYZ and RGB ranges");
            assert(BigInt(geometryPlan.peak_bytes) > 0n, "read plan omitted its memory estimate");
            let pendingPlanProtected = false;
            try {
                reader.planRead(0n, "positions");
            } catch {
                pendingPlanProtected = true;
            }
            assert(pendingPlanProtected, "reader replaced a plan before its buffers were consumed");
            const geometry = await reader.read();
            assert(geometry.kind === "geometry" && geometry.pointCount === 4096,
                "geometry result shape is invalid");
            assert(geometry.values instanceof Float32Array && geometry.values.length === 4096 * 3,
                "positions are not transferred as xyz float values");
            assert(geometry.colors instanceof Uint8Array && geometry.colors.length === 4096 * 3,
                "display colors are not transferred as packed RGB bytes");

            const corruptBytes = primaryBytes.slice();
            corruptBytes[Number(geometryRanges[0].offset)] ^= 1;
            const corruptReader = await WasmAsyncFormatReader.open(new Blob([corruptBytes]), attributes);
            corruptReader.planRead(0n, "geometry");
            let checksumRejected = false;
            try {
                await corruptReader.read();
            } catch (error) {
                checksumRejected = String(error).toLowerCase().includes("checksum");
            } finally {
                corruptReader.free();
            }
            assert(checksumRejected, "corrupt payload was not rejected by its checksum");

            const attrPlan = reader.planRead(0n, "attribute", 0);
            const attrRanges = attrPlan.ranges;
            assert(attrRanges.length === 1 && attrRanges[0].source === "attributes",
                "attribute selection did not plan its companion range");
            const column = await reader.read();
            assert(column.kind === "attribute" && column.name === "intensity"
                && column.dataType === "U16" && column.values instanceof Uint16Array
                && column.values.length === 4096 && column.pointCount === 4096,
            "typed attribute result is invalid");

            const positionPlan = reader.planRead(0n, "positions");
            const positionRanges = positionPlan.ranges;
            assert(positionRanges.length === 1 && positionRanges[0].source === "primary",
                "position selection did not plan its primary range");
            const positions = await reader.read();
            assert(positions.kind === "positions" && positions.values instanceof Float32Array
                && positions.values.length === 4096 * 3, "positions result is invalid");

            const colorPlan = reader.planRead(0n, "displayColors");
            const colorRanges = colorPlan.ranges;
            assert(colorRanges.length === 1 && colorRanges[0].source === "attributes",
                "RGB selection did not plan its companion range");
            const colors = await reader.read();
            assert(colors.kind === "displayColors" && colors.values instanceof Uint8Array
                && colors.values.length === 4096 * 3, "RGB result is invalid");

            const metadataReservation = BigInt(readBudget.reservedBytes());
            const delayedArrayBuffer = Blob.prototype.arrayBuffer;
            let activeArrayBuffers = 0;
            let maxActiveArrayBuffers = 0;
            Blob.prototype.arrayBuffer = function () {
                activeArrayBuffers += 1;
                maxActiveArrayBuffers = Math.max(maxActiveArrayBuffers, activeArrayBuffers);
                return new Promise((resolve, reject) => {
                    setTimeout(() => delayedArrayBuffer.call(this).then(resolve, reject)
                        .finally(() => { activeArrayBuffers -= 1; }), 5);
                });
            };
            try {
                const geometryRequest = reader.planLodRead(0n, "geometry");
                const attributeRequest = reader.planLodRead(0n, "positionsAttribute", 0);
                assert(geometryRequest && attributeRequest,
                    "shared read quota rejected small concurrent requests");
                assert(BigInt(readBudget.reservedBytes()) > metadataReservation,
                    "concurrent requests did not reserve shared read quota");
                const [geometryResult, attributeResult] = await Promise.all([
                    geometryRequest.read(), attributeRequest.read(),
                ]);
                let geometryPayload = geometryResult.value;
                let attributePayload = attributeResult.value;
                assert(maxActiveArrayBuffers > 1,
                    "independent requests from one reader did not overlap Blob reads");
                assert(geometryPayload.positions instanceof Float32Array
                    && geometryPayload.colors instanceof Uint8Array,
                "concurrent geometry result lost XYZ/RGB arrays");
                assert(attributePayload.positions instanceof Float32Array
                    && attributePayload.attribute.name === "intensity"
                    && attributePayload.attribute.values instanceof Uint16Array,
                "concurrent typed attribute result is invalid");
                geometryPayload = null;
                attributePayload = null;
                geometryResult.release();
                attributeResult.release();
                assert(BigInt(readBudget.reservedBytes()) === metadataReservation,
                    "result release did not return quota to the shared reader budget");
            } finally {
                Blob.prototype.arrayBuffer = delayedArrayBuffer;
            }

            let wrongCompanionRejected = false;
            try {
                await WasmAsyncFormatReader.open(primary, primary);
            } catch {
                wrongCompanionRejected = true;
            }
            assert(wrongCompanionRejected, "primary Blob was accepted as its own companion");

            assert(ranges.length > 0 && ranges.every(({ size, start, end }) => (
                Number.isSafeInteger(start) && Number.isSafeInteger(end)
                && start >= 0 && end > start && end <= size
            )), "reader issued an invalid Blob range");
            // `open` reads one tail window of up to 1 MiB (`TAIL_READ_BYTES`), which is the whole Blob
            // for the small fixture. The default sample is larger, so it proves the payload reads
            // stay ranged.
            const tailWindowBytes = 1024 * 1024;
            const largeOpened = ranges.length;
            const largePrimary = await fetch(new URL("../sample.oct", import.meta.url)).then(r => r.blob());
            const largeAttributes = await fetch(new URL("../sample.octattrs", import.meta.url)).then(r => r.blob());
            assert(largePrimary.size > tailWindowBytes, "default sample no longer exceeds the tail window");
            const largeReader = await WasmAsyncFormatReader.open(largePrimary, largeAttributes);
            largeReader.planRead(0n, "geometry");
            await largeReader.read();
            largeReader.free();
            assert(ranges.length > largeOpened, "default sample read no ranges");
            assert(ranges.every(({ size, start, end }) => start !== 0 || end !== size || size <= tailWindowBytes),
                "reader requested an entire Blob beyond the open-time tail window");
            assert(arrayBufferCalls === ranges.length, "each range must resolve through Blob.arrayBuffer");
            self.postMessage({
                type: "probeResult",
                passed: true,
                geometryRangeCount: geometryRanges.length,
                geometryPeakBytes: geometryPlan.peak_bytes,
                attributeRangeCount: attrRanges.length,
                positionRangeCount: positionRanges.length,
                colorRangeCount: colorRanges.length,
                blobRangeReads: ranges.length,
                arrayBufferCalls,
                checksumRejected,
                pointCount: geometry.pointCount,
                attributeType: column.dataType,
                schedulerTicketRetryPassed: true,
            });
            reader.free();
        } finally {
            Blob.prototype.arrayBuffer = originalArrayBuffer;
            Blob.prototype.slice = originalSlice;
        }
    } catch (error) {
        self.postMessage({ type: "probeResult", passed: false, error: String(error) });
    }
};
