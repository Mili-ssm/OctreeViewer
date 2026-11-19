import { replaceCanvasElement } from "./canvas-replacement.mjs";
import { framebufferSize } from "./rendering-budget.mjs";

const DEFAULT_CACHE_BYTES = 256 * 1024 * 1024;
const MAX_CACHE_BYTES = 1024 * 1024 * 1024;
const MAX_PENDING_REQUESTS = 32;

// Remote source `{ url, maxGap?, remoteReads? }` (HTTP Range, D4). Only these keys travel to the worker:
// `resolveSource` spreads the descriptor over the `HttpRangeSource` options, so any other key would
// become public API by accident. The URL is resolved here because a worker resolves against its own URL.
const DESCRIPTOR_KEYS = new Set(["url", "maxGap", "remoteReads"]);
function remoteDescriptor(value, label) {
    if (value === null || typeof value !== "object") throw new TypeError(`${label} must be a Blob or a { url } descriptor`);
    for (const key of Object.keys(value)) {
        if (!DESCRIPTOR_KEYS.has(key)) throw new TypeError(`${label} descriptor has an unknown field: ${key}`);
    }
    if (typeof value.url !== "string" && !(value.url instanceof URL)) {
        throw new TypeError(`${label} url must be a string or a URL`);
    }
    const base = globalThis.document?.baseURI ?? globalThis.location?.href;
    let url;
    try {
        url = new URL(value.url instanceof URL ? value.url.href : value.url, base);
    } catch {
        throw new TypeError(`${label} url must be an absolute or page-relative http(s) URL`);
    }
    if (url.protocol !== "http:" && url.protocol !== "https:") throw new TypeError(`${label} url must use http: or https:`);
    if (url.username || url.password) throw new TypeError(`${label} url must not embed credentials`);
    const descriptor = { url: url.href };
    if (value.maxGap !== undefined) {
        if (!Number.isSafeInteger(value.maxGap) || value.maxGap < 0) throw new TypeError(`${label} maxGap must be an integer >= 0`);
        descriptor.maxGap = value.maxGap;
    }
    if (value.remoteReads !== undefined) {
        if (!Number.isSafeInteger(value.remoteReads) || value.remoteReads < 1) {
            throw new TypeError(`${label} remoteReads must be an integer >= 1`);
        }
        descriptor.remoteReads = value.remoteReads;
    }
    return descriptor;
}

// Last path segment of a resolved URL, decoded when it is valid percent-encoding.
function remoteName(href) {
    const segment = new URL(href).pathname.split("/").pop();
    try {
        return decodeURIComponent(segment);
    } catch {
        return segment;
    }
}

// Held-key flight (WASD/QE). Amounts are in units of the camera's orbit distance, so the feel is
// independent of the scene size. Same logic in main.js and viewer-api.mjs: the offline packager
// inlines or copies each file on its own, so they cannot share a module.
const FLIGHT_KEYS = { w: [1, 0, 0], s: [-1, 0, 0], d: [0, 1, 0], a: [0, -1, 0], e: [0, 0, 1], q: [0, 0, -1] };
// Orbit radii per second at movement speed 1: a 1 s hold of W covers about a third of the view.
const FLIGHT_BASE_SPEED = 0.3;
const FLIGHT_SHIFT_FACTOR = 4;
const FLIGHT_MAX_DT = 0.1;
function createFlight() {
    const held = new Set();
    let shift = false;
    let last = 0;
    return {
        active: () => held.size > 0,
        /** Returns whether the key is a flight key the page should consume. */
        press(event, now) {
            // Any key refreshes Shift, so pressing it while W is held speeds up at once.
            shift = Boolean(event.shiftKey);
            const key = String(event.key).toLowerCase();
            if (!(key in FLIGHT_KEYS) || event.ctrlKey || event.altKey || event.metaKey) return false;
            if (held.size === 0) last = now;
            held.add(key);
            return true;
        },
        release(event) {
            shift = Boolean(event.shiftKey);
            held.delete(String(event.key).toLowerCase());
        },
        clear() { held.clear(); shift = false; },
        /** Per-frame `{ flyForward, flyRight, flyUp }` for the time since the last step, or null. */
        step(now, speed) {
            if (held.size === 0) return null;
            const dt = Math.min(FLIGHT_MAX_DT, Math.max(0, (now - last) / 1000));
            last = now;
            const amount = FLIGHT_BASE_SPEED * speed * dt * (shift ? FLIGHT_SHIFT_FACTOR : 1);
            const total = [0, 0, 0];
            for (const key of held) for (let axis = 0; axis < 3; axis += 1) total[axis] += FLIGHT_KEYS[key][axis] * amount;
            return { flyForward: total[0], flyRight: total[1], flyUp: total[2] };
        },
    };
}

/** Creates an embeddable viewer on a host-owned canvas. The canvas is transferred to its worker. */
export function createViewer(hostCanvas, options = {}) {
    let canvas = hostCanvas;
    if (!(canvas instanceof HTMLCanvasElement)) throw new TypeError("canvas must be an HTMLCanvasElement");
    let mode = options.mode ?? "observer";
    if (mode !== "observer" && mode !== "bird") throw new TypeError("mode must be 'observer' or 'bird'");
    if (!window.Worker || !canvas.transferControlToOffscreen) {
        throw new Error("Web Workers and OffscreenCanvas are required");
    }

    const worker = new Worker(options.workerUrl ?? new URL("./viewer-worker.js", import.meta.url), {
        type: "module",
    });
    const listeners = new Map();
    const pending = new Map();
    let nextRequestId = 1;
    let disposed = false;
    let failed = false;
    let workerTerminated = false;
    let ready = false;
    let loading = false;
    let loadInFlight = false;
    let framePending = false;
    let raf = 0;
    let lastPointer = null;
    let pointerGesture = null;
    let pointerStart = null;
    let pointerMoved = false;
    let pickPending = false;
    let movementSpeed = 1;
    const flight = createFlight();
    let resolveReady;
    let rejectReady;
    const readyPromise = new Promise((resolve, reject) => {
        resolveReady = resolve;
        rejectReady = reject;
    });
    const deltas = { orbitX: 0, orbitY: 0, panX: 0, panY: 0, zoom: 0, moveX: 0, moveY: 0, moveZ: 0, flyForward: 0, flyRight: 0, flyUp: 0 };
    const emit = (type, detail) => {
        for (const callback of listeners.get(type) ?? []) {
            try {
                callback(detail);
            } catch (error) {
                window.reportError?.(error);
            }
        }
        canvas.dispatchEvent(new CustomEvent(`voxelviewer:${type}`, { detail }));
    };
    const rejectPending = (error) => {
        for (const entry of pending.values()) entry.reject(error);
        pending.clear();
    };
    const terminateWorker = () => {
        if (workerTerminated) return;
        workerTerminated = true;
        worker.terminate();
    };
    const failWorker = (error, detail = {}, emitError = true) => {
        if (failed || disposed) return;
        failed = true;
        loading = false;
        framePending = false;
        if (raf) cancelAnimationFrame(raf);
        raf = 0;
        const failure = error instanceof Error ? error : new Error(String(error));
        if (emitError) emit("error", { operation: "worker", message: failure.message, ...detail });
        rejectReady(failure);
        rejectPending(failure);
        terminateWorker();
    };
    const request = (operation, values = {}) => new Promise((resolve, reject) => {
        if (disposed) return reject(new Error("viewer is disposed"));
        if (failed) return reject(new Error("viewer worker has failed"));
        if (pending.size >= MAX_PENDING_REQUESTS) return reject(new Error("viewer request queue is full"));
        const requestId = nextRequestId++;
        pending.set(requestId, { resolve, reject });
        try {
            worker.postMessage({ type: "api", operation, requestId, ...values });
        } catch (error) {
            pending.delete(requestId);
            reject(error);
        }
    });
    const ensureFrame = () => {
        if (!disposed && !failed && ready && !raf && !framePending) raf = requestAnimationFrame(draw);
    };
    const draw = () => {
        raf = 0;
        if (disposed || !ready || framePending) return;
        framePending = true;
        const fly = flight.step(performance.now(), movementSpeed);
        if (fly) for (const [key, value] of Object.entries(fly)) deltas[key] += value;
        const camera = { ...deltas };
        for (const key of Object.keys(deltas)) deltas[key] = 0;
        worker.postMessage({ type: "frame", camera });
    };
    const resize = () => {
        if (disposed || failed) return;
        const [width, height] = framebufferSize(canvas.clientWidth, canvas.clientHeight, window.devicePixelRatio || 1);
        worker.postMessage({ type: "resize", width, height });
    };
    const onMessage = ({ data }) => {
        if (data.type === "ready") {
            ready = true;
            resolveReady(api);
            emit("ready", null);
            if (mode === "bird") void request("birdView").then(ensureFrame);
            ensureFrame();
        } else if (data.type === "apiResult") {
            const entry = pending.get(data.requestId);
            if (!entry) return;
            pending.delete(data.requestId);
            entry.resolve(data.value);
        } else if (data.type === "error") {
            emit("error", data);
            const fatal = data.operation === "frame" || data.operation === "init";
            if (fatal) {
                failWorker(new Error(data.message), data, false);
                return;
            }
            if (!ready) rejectReady(new Error(data.message));
            if (data.requestId && pending.has(data.requestId)) {
                pending.get(data.requestId).reject(new Error(data.message));
                pending.delete(data.requestId);
            }
        } else if (data.type === "frame") {
            framePending = false;
            emit("frame", data);
            if (loading || flight.active()) ensureFrame();
        } else if (data.type === "stats") {
            framePending = false;
            emit("stats", data);
            if (loading) ensureFrame();
        } else if (data.type === "lodProgress") {
            loading = Boolean(data.lod && !data.lod.lodError && !data.lod.pageBudgetExhausted
                && (data.lod.pendingBlockIds.length || data.lod.pendingPageAnchors?.length
                    || data.lod.lodStagePixels > data.lod.targetVoxelPixels));
            emit("progress", data);
            if (loading) ensureFrame();
        } else if (data.type === "dataset") {
            loading = true;
            emit("dataset", data);
            ensureFrame();
        } else if (data.type === "status") {
            emit("status", data.message);
        } else if (data.type === "deviceLost") {
            const canvasReplaced = data.reason !== "destroyed" && replaceCanvas();
            emit("deviceLost", { ...data, canvasReplaced });
        } else if (data.type === "deviceRecovery") {
            emit("deviceRecovery", data);
            if (data.state === "recovered") {
                loading = true;
                framePending = false;
                ensureFrame();
            }
        }
    };
    const onWorkerError = (event) => {
        event.preventDefault?.();
        failWorker(event.message || "viewer worker failed", { operation: "worker" });
    };
    let resizeObserver = null;
    let resizeListenerAttached = false;
    const cleanupInitialization = () => {
        resizeObserver?.disconnect();
        if (resizeListenerAttached) window.removeEventListener("resize", resize);
        worker.removeEventListener("message", onMessage);
        worker.removeEventListener("error", onWorkerError);
        worker.removeEventListener("messageerror", onWorkerError);
        terminateWorker();
    };
    try {
        worker.addEventListener("message", onMessage);
        worker.addEventListener("error", onWorkerError);
        worker.addEventListener("messageerror", onWorkerError);
        const offscreen = canvas.transferControlToOffscreen();
        const [width, height] = framebufferSize(
            canvas.clientWidth,
            canvas.clientHeight,
            window.devicePixelRatio || 1,
        );
        worker.postMessage({
            type: "init", canvas: offscreen, width, height,
            colorMode: options.colorMode ?? "rgb", depthShading: options.depthShading ?? true,
            cells: options.cells !== false,
            canvasReplacement: true,
            cellNodeBytesPerFrame: options.cellNodeBytesPerFrame,
            buildWorker: options.buildWorker !== false ? {
                url: String(options.buildWorkerUrl ?? new URL("./build-worker.js", import.meta.url)),
                type: "module",
            } : undefined,
        }, [offscreen]);
        resizeObserver = new ResizeObserver(resize);
        resizeObserver.observe(canvas);
        resizeListenerAttached = true;
        window.addEventListener("resize", resize);
    } catch (error) {
        cleanupInitialization();
        throw error;
    }

    function pointerDown(event) {
        if (!ready || (event.button !== 0 && event.button !== 2)) return;
        event.preventDefault();
        // preventDefault also stops the browser from focusing the canvas: focus it for the keys.
        canvas.focus?.({ preventScroll: true });
        lastPointer = [event.clientX, event.clientY];
        pointerStart = lastPointer;
        pointerMoved = false;
        pointerGesture = event.button === 2 ? "orbit" : "pan";
        canvas.setPointerCapture(event.pointerId);
    }
    function pointerMove(event) {
        if (!lastPointer || !canvas.hasPointerCapture(event.pointerId)) return;
        const dx = (event.clientX - lastPointer[0]) / Math.max(canvas.clientWidth, 1);
        const dy = (event.clientY - lastPointer[1]) / Math.max(canvas.clientHeight, 1);
        if (Math.hypot(event.clientX - pointerStart[0], event.clientY - pointerStart[1]) >= 4) {
            pointerMoved = true;
        }
        if (pointerGesture === "orbit") {
            deltas.orbitX += dx;
            deltas.orbitY += dy;
        } else {
            deltas.panX += dx;
            deltas.panY += dy;
        }
        lastPointer = [event.clientX, event.clientY];
        ensureFrame();
    }
    function pointerEnd(event) {
        if (event?.type === "pointerup" && event.button === 0 && mode === "observer" && !pointerMoved) {
            const rect = canvas.getBoundingClientRect();
            void pickAt(event.clientX - rect.left, event.clientY - rect.top, true).catch(() => {});
        }
        lastPointer = null;
        pointerStart = null;
        pointerGesture = null;
    }
    async function pickAt(cssX, cssY, recenter = false) {
        if (pickPending) return null;
        const rect = canvas.getBoundingClientRect();
        if (!rect.width || !rect.height || !canvas.width || !canvas.height) return null;
        if (cssX < 0 || cssY < 0 || cssX >= rect.width || cssY >= rect.height) return null;
        pickPending = true;
        try {
            const result = await request("pick", {
                cssX, cssY, cssWidth: rect.width, cssHeight: rect.height, recenter,
            });
            const hit = result.hit ? { ...result.hit, sceneGeneration: result.sceneGeneration } : null;
            emit("pick", hit);
            if (recenter && hit) ensureFrame();
            return hit;
        } catch (error) {
            emit("error", { operation: "pick", message: String(error) });
            throw error;
        } finally {
            pickPending = false;
        }
    }
    function wheel(event) {
        if (!ready) return;
        event.preventDefault();
        const unit = event.deltaMode === WheelEvent.DOM_DELTA_LINE ? 16
            : event.deltaMode === WheelEvent.DOM_DELTA_PAGE ? Math.max(canvas.clientHeight, 1) : 1;
        const delta = event.deltaY * unit;
        if (mode === "bird") movementSpeed = Math.min(1e6, Math.max(1e-3, movementSpeed * Math.exp(-delta * 0.001)));
        else deltas.zoom += delta * 0.001;
        ensureFrame();
    }
    function contextMenu(event) { event.preventDefault(); }
    function keyUp(event) { flight.release(event); }
    function releaseKeys() { flight.clear(); }
    function keyDown(event) {
        if (!ready) return;
        if (event.key === "Home") {
            void request("fit").then(ensureFrame);
        } else if (flight.press(event, performance.now())) {
            // Motion is applied per animation frame while the key is held.
        } else if (event.key === "+" || event.key === "=") deltas.zoom -= 0.15;
        else if (event.key === "-") deltas.zoom += 0.15;
        else return;
        event.preventDefault();
        ensureFrame();
    }
    const bindings = [
        ["pointerdown", pointerDown], ["pointermove", pointerMove], ["pointerup", pointerEnd],
        ["pointercancel", pointerEnd], ["lostpointercapture", pointerEnd],
        ["wheel", wheel, { passive: false }], ["contextmenu", contextMenu], ["keydown", keyDown],
        ["keyup", keyUp], ["blur", releaseKeys],
    ];
    const bindCanvas = () => { for (const [type, handler, flags] of bindings) canvas.addEventListener(type, handler, flags); };
    const unbindCanvas = () => { for (const [type, handler] of bindings) canvas.removeEventListener(type, handler); };
    bindCanvas();
    window.addEventListener("blur", releaseKeys);
    globalThis.document?.addEventListener("visibilitychange", releaseKeys);

    // R3-Q34: swaps the host's canvas element for a clone and gives the worker its OffscreenCanvas.
    // Returns whether it did; without a DOM element the worker recovers on its current canvas.
    function replaceCanvas() {
        const replaced = replaceCanvasElement(canvas);
        if (!replaced) {
            worker.postMessage({ type: "replaceCanvas", canvas: null });
            return false;
        }
        resizeObserver.unobserve(canvas);
        unbindCanvas();
        canvas = replaced.canvas;
        bindCanvas();
        resizeObserver.observe(canvas);
        worker.postMessage({ type: "replaceCanvas", canvas: replaced.offscreen }, [replaced.offscreen]);
        canvas.dispatchEvent(new CustomEvent("voxelviewer:canvasReplaced", { detail: { canvas } }));
        return true;
    }

    const api = {
        ready: readyPromise,
        load(file, { voxelPixels = 4, cacheBytes = DEFAULT_CACHE_BYTES, attributes = undefined } = {}) {
            let source = file;
            let sidecar = attributes;
            let name;
            try {
                if (!(file instanceof Blob)) {
                    if (file === null || typeof file !== "object") throw new TypeError("load expects a File, a Blob or a { url } descriptor");
                    source = remoteDescriptor(file, "source");
                    name = remoteName(source.url);
                }
                if (attributes !== undefined && !(attributes instanceof Blob)) sidecar = remoteDescriptor(attributes, "attributes");
            } catch (error) {
                return Promise.reject(error);
            }
            if (loadInFlight) return Promise.reject(new Error("a dataset is already being opened"));
            if (!Number.isFinite(voxelPixels) || voxelPixels < 0.5 || voxelPixels > 16) {
                return Promise.reject(new RangeError("voxelPixels must be between 0.5 and 16"));
            }
            if (!Number.isSafeInteger(cacheBytes) || cacheBytes < 0 || cacheBytes > MAX_CACHE_BYTES) {
                return Promise.reject(new RangeError(`cacheBytes must be an integer from 0 to ${MAX_CACHE_BYTES}`));
            }
            loadInFlight = true;
            loading = true;
            return request("load", { file: source, attributes: sidecar, name, targetVoxelPixels: voxelPixels, cacheBytes })
                .catch((error) => {
                    loading = false;
                    throw error;
                }).finally(() => {
                    loadInFlight = false;
                });
        },
        fit: () => request("fit").then((value) => request("renderOnce").then(() => {
            ensureFrame();
            return value;
        })),
        async getCamera() {
            const pose = await request("getCamera");
            return {
                position: Array.from(pose.position),
                target: Array.from(pose.target),
                mode,
                up: [0, 0, 1],
                movementSpeed,
            };
        },
        setCamera(position, target) {
            const coordinates = (value) => Array.from(value ?? []);
            const cameraPosition = coordinates(position);
            const cameraTarget = coordinates(target);
            if (cameraPosition.length !== 3 || cameraTarget.length !== 3
                || [...cameraPosition, ...cameraTarget].some((value) => !Number.isFinite(value))) {
                return Promise.reject(new TypeError("camera position and target must be finite 3D coordinates"));
            }
            return request("setCamera", { position: cameraPosition, target: cameraTarget })
                .then((value) => request("renderOnce").then(() => {
                    ensureFrame();
                    return value;
                }));
        },
        getMetadata: () => request("getMetadata"),
        getStats: () => request("getStats"),
        // RF-06/RF-07: "fixed" keeps the load's voxelPixels, "dynamic" lets the worker pick 1, 2, 4 or 8 px
        // from frame cost. Resolves with the px stats ({ mode, targetPx, signalMs, signalSource }).
        setPxMode(pxMode) {
            if (pxMode !== "fixed" && pxMode !== "dynamic") {
                return Promise.reject(new TypeError("pxMode must be 'fixed' or 'dynamic'"));
            }
            return request("setPxMode", { mode: pxMode });
        },
        // Length minimum of the drawn cubes in dataset units (ADR 0049/0054): `undefined` restores the
        // default (1e-4), 0 turns it off; a negative or non-finite side rejects. The larger of it and the
        // continuity side wins. Resolves with the side sent.
        setMinLength(side) {
            if (side !== undefined && side !== null && !(Number.isFinite(side) && side >= 0)) {
                return Promise.reject(new TypeError("minimum length must be a finite number >= 0, or undefined for the default"));
            }
            return request("setMinLength", { side: side ?? undefined });
        },
        // Cloud-spacing minimum (RF-05): with `enabled`, no cube is drawn smaller than `factor` (default 1)
        // x the cloud's reference spacing. Independent of the length minimum.
        setContinuity(enabled, factor) {
            if (typeof enabled !== "boolean") {
                return Promise.reject(new TypeError("continuity enabled must be a boolean"));
            }
            if (factor !== undefined && !(Number.isFinite(factor) && factor > 0)) {
                return Promise.reject(new TypeError("continuity factor must be a finite number > 0"));
            }
            return request("setContinuity", { enabled, factor });
        },
        pick(cssX, cssY) {
            if (!Number.isFinite(cssX) || !Number.isFinite(cssY)) {
                return Promise.reject(new TypeError("pick coordinates must be finite CSS pixels"));
            }
            return pickAt(cssX, cssY);
        },
        setMode(value) {
            if (value !== "bird" && value !== "observer") throw new TypeError("mode must be 'bird' or 'observer'");
            mode = value;
            emit("mode", mode);
            if (mode === "bird") return request("birdView").then(() => request("renderOnce")).then((value) => {
                ensureFrame();
                return value;
            });
            return Promise.resolve();
        },
        on(type, callback) {
            const callbacks = listeners.get(type) ?? [];
            callbacks.push(callback);
            listeners.set(type, callbacks);
            return () => listeners.set(type, callbacks.filter((item) => item !== callback));
        },
        dispose() {
            if (disposed) return;
            disposed = true;
            if (!ready) rejectReady(new Error("viewer disposed before initialization"));
            if (raf) cancelAnimationFrame(raf);
            resizeObserver.disconnect();
            window.removeEventListener("resize", resize);
            window.removeEventListener("blur", releaseKeys);
            globalThis.document?.removeEventListener("visibilitychange", releaseKeys);
            unbindCanvas();
            worker.removeEventListener("message", onMessage);
            worker.removeEventListener("error", onWorkerError);
            worker.removeEventListener("messageerror", onWorkerError);
            terminateWorker();
            rejectPending(new Error("viewer disposed"));
            listeners.clear();
        },
    };
    return api;
}
