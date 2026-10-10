import { framebufferSize } from "./rendering-budget.mjs";

const DEFAULT_CACHE_BYTES = 256 * 1024 * 1024;
const MAX_CACHE_BYTES = 1024 * 1024 * 1024;
const MAX_PENDING_REQUESTS = 32;

/** Creates an embeddable viewer on a host-owned canvas. The canvas is transferred to its worker. */
export function createViewer(canvas, options = {}) {
    if (!(canvas instanceof HTMLCanvasElement)) throw new TypeError("canvas must be an HTMLCanvasElement");
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
    let mode = options.mode ?? "observer";
    let resolveReady;
    let rejectReady;
    const readyPromise = new Promise((resolve, reject) => {
        resolveReady = resolve;
        rejectReady = reject;
    });
    const deltas = { orbitX: 0, orbitY: 0, panX: 0, panY: 0, zoom: 0, moveX: 0, moveY: 0, moveZ: 0 };
    if (mode !== "observer" && mode !== "bird") throw new TypeError("mode must be 'observer' or 'bird'");

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
    const request = (operation, values = {}) => new Promise((resolve, reject) => {
        if (disposed) return reject(new Error("viewer is disposed"));
        if (failed) return reject(new Error("viewer worker has failed"));
        if (pending.size >= MAX_PENDING_REQUESTS) return reject(new Error("viewer request queue is full"));
        const requestId = nextRequestId++;
        pending.set(requestId, { resolve, reject });
        worker.postMessage({ type: "api", operation, requestId, ...values });
    });
    const ensureFrame = () => {
        if (!disposed && !failed && ready && !raf && !framePending) raf = requestAnimationFrame(draw);
    };
    const draw = () => {
        raf = 0;
        if (disposed || !ready || framePending) return;
        framePending = true;
        const camera = { ...deltas };
        for (const key of Object.keys(deltas)) deltas[key] = 0;
        worker.postMessage({ type: "frame", camera });
    };
    const resize = () => {
        if (disposed) return;
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
                failed = true;
                loading = false;
                framePending = false;
                if (raf) cancelAnimationFrame(raf);
                raf = 0;
                rejectReady(new Error(data.message));
                rejectPending(new Error(data.message));
            }
            if (!ready) rejectReady(new Error(data.message));
            if (data.requestId && pending.has(data.requestId)) {
                pending.get(data.requestId).reject(new Error(data.message));
                pending.delete(data.requestId);
            }
        } else if (data.type === "frame") {
            framePending = false;
            emit("frame", data);
            if (loading) ensureFrame();
        } else if (data.type === "stats") {
            framePending = false;
            emit("stats", data);
            if (loading) ensureFrame();
        } else if (data.type === "lodProgress") {
            loading = Boolean(data.lod && !data.lod.lodError
                && (data.lod.pendingBlockIds.length || data.lod.lodStagePixels > data.lod.targetVoxelPixels));
            emit("progress", data);
            if (loading) ensureFrame();
        } else if (data.type === "dataset") {
            loading = true;
            emit("dataset", data);
            ensureFrame();
        } else if (data.type === "status") {
            emit("status", data.message);
        }
    };
    worker.addEventListener("message", onMessage);
    worker.addEventListener("error", (event) => {
        failed = true;
        framePending = false;
        const error = new Error(event.message);
        rejectReady(error);
        rejectPending(error);
        emit("error", { message: event.message });
    });
    const resizeObserver = new ResizeObserver(resize);
    resizeObserver.observe(canvas);
    window.addEventListener("resize", resize);
    const offscreen = canvas.transferControlToOffscreen();
    const [width, height] = framebufferSize(canvas.clientWidth, canvas.clientHeight, window.devicePixelRatio || 1);
    worker.postMessage({
        type: "init", canvas: offscreen, width, height,
        colorMode: options.colorMode ?? "rgb", depthShading: options.depthShading ?? true,
    }, [offscreen]);

    function pointerDown(event) {
        if (!ready || (event.button !== 0 && event.button !== 2)) return;
        event.preventDefault();
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
    function keyDown(event) {
        if (!ready) return;
        if (event.key === "Home") {
            void request("fit").then(ensureFrame);
        } else if (mode === "bird" && ["w", "a", "s", "d", "q", "e"].includes(event.key.toLowerCase())) {
            const distance = movementSpeed * 0.05;
            if (event.key.toLowerCase() === "w") deltas.moveY += distance;
            if (event.key.toLowerCase() === "s") deltas.moveY -= distance;
            if (event.key.toLowerCase() === "a") deltas.moveX -= distance;
            if (event.key.toLowerCase() === "d") deltas.moveX += distance;
            if (event.key.toLowerCase() === "q") deltas.moveZ -= distance;
            if (event.key.toLowerCase() === "e") deltas.moveZ += distance;
        } else if (event.key === "+" || event.key === "=") deltas.zoom -= 0.15;
        else if (event.key === "-") deltas.zoom += 0.15;
        else return;
        event.preventDefault();
        ensureFrame();
    }
    canvas.addEventListener("pointerdown", pointerDown);
    canvas.addEventListener("pointermove", pointerMove);
    canvas.addEventListener("pointerup", pointerEnd);
    canvas.addEventListener("pointercancel", pointerEnd);
    canvas.addEventListener("lostpointercapture", pointerEnd);
    canvas.addEventListener("wheel", wheel, { passive: false });
    canvas.addEventListener("contextmenu", contextMenu);
    canvas.addEventListener("keydown", keyDown);

    const api = {
        ready: readyPromise,
        load(file, { voxelPixels = 4, cacheBytes = DEFAULT_CACHE_BYTES } = {}) {
            if (!(file instanceof Blob)) return Promise.reject(new TypeError("load expects a File or Blob"));
            if (loadInFlight) return Promise.reject(new Error("a dataset is already being opened"));
            if (!Number.isFinite(voxelPixels) || voxelPixels < 0.5 || voxelPixels > 16) {
                return Promise.reject(new RangeError("voxelPixels must be between 0.5 and 16"));
            }
            if (!Number.isSafeInteger(cacheBytes) || cacheBytes < 0 || cacheBytes > MAX_CACHE_BYTES) {
                return Promise.reject(new RangeError(`cacheBytes must be an integer from 0 to ${MAX_CACHE_BYTES}`));
            }
            loadInFlight = true;
            loading = true;
            return request("load", { file, targetVoxelPixels: voxelPixels, cacheBytes })
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
            canvas.removeEventListener("pointerdown", pointerDown);
            canvas.removeEventListener("pointermove", pointerMove);
            canvas.removeEventListener("pointerup", pointerEnd);
            canvas.removeEventListener("pointercancel", pointerEnd);
            canvas.removeEventListener("lostpointercapture", pointerEnd);
            canvas.removeEventListener("wheel", wheel);
            canvas.removeEventListener("contextmenu", contextMenu);
            canvas.removeEventListener("keydown", keyDown);
            worker.removeEventListener("message", onMessage);
            worker.terminate();
            rejectPending(new Error("viewer disposed"));
            listeners.clear();
        },
    };
    return api;
}
