import assert from "node:assert/strict";
import test from "node:test";

import { createViewer } from "../viewer-api.mjs";

class FakeCanvas {
    width = 1280;
    height = 720;
    clientWidth = 640;
    clientHeight = 360;
    listeners = new Map();
    transferError = null;

    transferControlToOffscreen() {
        if (this.transferError) throw this.transferError;
        return {};
    }
    getBoundingClientRect() { return { left: 10, top: 20, width: 640, height: 360 }; }
    addEventListener(type, callback) {
        this.listeners.set(type, [...(this.listeners.get(type) ?? []), callback]);
    }
    removeEventListener(type, callback) {
        this.listeners.set(type, (this.listeners.get(type) ?? []).filter((item) => item !== callback));
    }
    dispatchEvent(event) {
        for (const callback of this.listeners.get(event.type) ?? []) callback(event);
        return true;
    }
}

class FakeWorker {
    static instance;
    static failInitMessage = false;
    messages = [];
    listeners = new Map();
    terminated = false;
    holdApi = false;
    events = new Map();

    constructor() { FakeWorker.instance = this; }
    addEventListener(type, callback) {
        const target = type === "message" ? this.listeners : this.events;
        target.set(type, [...(target.get(type) ?? []), callback]);
    }
    removeEventListener(type, callback) {
        const target = type === "message" ? this.listeners : this.events;
        target.set(type, (target.get(type) ?? []).filter((item) => item !== callback));
    }
    postMessage(message) {
        if (message.type === "init" && FakeWorker.failInitMessage) {
            throw new Error("init postMessage failed");
        }
        this.messages.push(message);
        if (message.type === "init") queueMicrotask(() => this.send({ type: "ready" }));
        if (message.type === "api") {
            if (this.holdApi) return;
            const value = message.operation === "pick"
                ? { hit: { voxelSize: 0.5 }, sceneGeneration: 9 }
                : message.operation === "getCamera"
                    ? { position: new Float64Array([1, 2, 3]), target: new Float64Array([0, 0, 0]) }
                    : true;
            queueMicrotask(() => this.send({ type: "apiResult", requestId: message.requestId, value }));
        }
    }
    send(data) {
        for (const callback of this.listeners.get("message") ?? []) callback({ data });
    }
    sendEvent(type, event = {}) {
        for (const callback of this.events.get(type) ?? []) callback(event);
    }
    terminate() { this.terminated = true; }
}

class FakeResizeObserver {
    static instance;
    static failObserve = false;
    disconnected = false;
    constructor(callback) { this.callback = callback; FakeResizeObserver.instance = this; }
    observe() {
        if (FakeResizeObserver.failObserve) throw new Error("resize observer failed");
    }
    unobserve() {}
    disconnect() { this.disconnected = true; }
}

test("invalid camera mode is rejected before creating a worker", () => {
    const previous = {
        HTMLCanvasElement: globalThis.HTMLCanvasElement,
        Worker: globalThis.Worker,
        ResizeObserver: globalThis.ResizeObserver,
        window: globalThis.window,
    };
    globalThis.HTMLCanvasElement = FakeCanvas;
    globalThis.Worker = FakeWorker;
    globalThis.ResizeObserver = FakeResizeObserver;
    globalThis.window = { Worker: FakeWorker };
    FakeWorker.instance = undefined;
    try {
        assert.throws(() => createViewer(new FakeCanvas(), { mode: "invalid" }), /mode must be/);
        assert.equal(FakeWorker.instance, undefined);
    } finally {
        Object.assign(globalThis, previous);
    }
});

for (const failure of [
    {
        name: "canvas transfer throws",
        configure(canvas) { canvas.transferError = new Error("already transferred"); },
    },
    {
        name: "worker initialization postMessage throws",
        configure() { FakeWorker.failInitMessage = true; },
    },
    {
        name: "resize observer setup throws",
        expectsObserver: true,
        configure() { FakeResizeObserver.failObserve = true; },
    },
    {
        name: "window resize listener setup throws",
        expectsObserver: true,
        configure() {},
        failResizeRegistration: true,
    },
]) {
    test(`${failure.name} releases resources acquired during initialization`, () => {
        const previous = {
            HTMLCanvasElement: globalThis.HTMLCanvasElement,
            Worker: globalThis.Worker,
            ResizeObserver: globalThis.ResizeObserver,
            window: globalThis.window,
        };
        globalThis.HTMLCanvasElement = FakeCanvas;
        globalThis.Worker = FakeWorker;
        globalThis.ResizeObserver = FakeResizeObserver;
        const windowListeners = new Map();
        globalThis.window = {
            Worker: FakeWorker,
            devicePixelRatio: 1,
            addEventListener(type, callback) {
                if (failure.failResizeRegistration) throw new Error("resize listener failed");
                windowListeners.set(type, [...(windowListeners.get(type) ?? []), callback]);
            },
            removeEventListener(type, callback) {
                windowListeners.set(type, (windowListeners.get(type) ?? []).filter((item) => item !== callback));
            },
        };
        FakeWorker.instance = undefined;
        FakeWorker.failInitMessage = false;
        FakeResizeObserver.instance = undefined;
        FakeResizeObserver.failObserve = false;
        try {
            const canvas = new FakeCanvas();
            failure.configure(canvas);
            assert.throws(
                () => createViewer(canvas),
                /already transferred|init postMessage failed|resize observer failed|resize listener failed/,
            );
            assert.equal(FakeWorker.instance.terminated, true);
            assert.ok([...FakeWorker.instance.listeners.values()].every((callbacks) => callbacks.length === 0));
            assert.ok([...FakeWorker.instance.events.values()].every((callbacks) => callbacks.length === 0));
            if (failure.expectsObserver) assert.equal(FakeResizeObserver.instance.disconnected, true);
            else assert.equal(FakeResizeObserver.instance, undefined);
            assert.equal(windowListeners.get("resize")?.length ?? 0, 0);
        } finally {
            FakeWorker.failInitMessage = false;
            FakeResizeObserver.failObserve = false;
            Object.assign(globalThis, previous);
        }
    });
}

test("embeddable API converts CSS pick coordinates and disposes its worker resources", async () => {
    const previous = {
        HTMLCanvasElement: globalThis.HTMLCanvasElement,
        Worker: globalThis.Worker,
        ResizeObserver: globalThis.ResizeObserver,
        window: globalThis.window,
        requestAnimationFrame: globalThis.requestAnimationFrame,
        cancelAnimationFrame: globalThis.cancelAnimationFrame,
        CustomEvent: globalThis.CustomEvent,
    };
    globalThis.HTMLCanvasElement = FakeCanvas;
    globalThis.Worker = FakeWorker;
    globalThis.ResizeObserver = FakeResizeObserver;
    globalThis.window = { Worker: FakeWorker, devicePixelRatio: 2, addEventListener() {}, removeEventListener() {} };
    globalThis.requestAnimationFrame = () => 1;
    globalThis.cancelAnimationFrame = () => {};
    globalThis.CustomEvent = class { constructor(type, init) { this.type = type; this.detail = init.detail; } };
    try {
        const canvas = new FakeCanvas();
        const viewer = createViewer(canvas);
        await viewer.ready;
        const attributes = new Blob(["sidecar"]);
        await viewer.load(new Blob(), { attributes });
        const loadMessage = FakeWorker.instance.messages.find((message) => message.operation === "load");
        assert.equal(loadMessage.attributes, attributes);
        await assert.rejects(viewer.load(new Blob(), { attributes: "bad" }), /attributes must be a Blob/);
        await assert.rejects(viewer.load(new Blob(), { voxelPixels: 17 }), /between 0.5 and 16/);
        await assert.rejects(viewer.load(new Blob(), { cacheBytes: 1024 ** 3 + 1 }), /cacheBytes/);
        await assert.rejects(viewer.setCamera([Number.NaN, 0, 0], [0, 0, 0]), /finite 3D coordinates/);
        const pickCount = FakeWorker.instance.messages.filter((message) => message.operation === "pick").length;
        assert.equal(await viewer.pick(-1, 10), null);
        assert.equal(FakeWorker.instance.messages.filter((message) => message.operation === "pick").length, pickCount);
        const hit = await viewer.pick(100, 80);
        assert.deepEqual(hit, { voxelSize: 0.5, sceneGeneration: 9 });
        assert.deepEqual(await viewer.getCamera(), {
            position: [1, 2, 3], target: [0, 0, 0], mode: "observer", up: [0, 0, 1], movementSpeed: 1,
        });
        const pickMessage = FakeWorker.instance.messages.find((message) => message.operation === "pick");
        assert.equal(pickMessage.cssX, 100);
        assert.equal(pickMessage.cssY, 80);
        assert.equal(pickMessage.cssWidth, 640);
        assert.equal(pickMessage.cssHeight, 360);
        await viewer.setMode("bird");
        assert.ok(FakeWorker.instance.messages.some((message) => message.operation === "birdView"));
        await viewer.setMinLength(0.5);
        assert.ok(FakeWorker.instance.messages.some((message) => message.operation === "setMinLength" && message.side === 0.5));
        await viewer.setMinLength(0);
        await viewer.setMinLength();
        await viewer.setContinuity(true, 2);
        assert.ok(FakeWorker.instance.messages.some((message) => message.operation === "setContinuity" && message.enabled === true && message.factor === 2));
        const postedLength = FakeWorker.instance.messages.length;
        for (const bad of [-1, Number.NaN, Infinity, "1"]) await assert.rejects(viewer.setMinLength(bad), /minimum length/);
        await assert.rejects(viewer.setContinuity("yes"), /boolean/);
        await assert.rejects(viewer.setContinuity(true, 0), /factor/);
        assert.equal(FakeWorker.instance.messages.length, postedLength, "invalid values are not sent");
        FakeWorker.instance.holdApi = true;
        const pending = viewer.getStats();
        const loading = viewer.load(new Blob());
        await assert.rejects(viewer.load(new Blob()), /already being opened/);
        viewer.dispose();
        await assert.rejects(pending, /disposed/);
        await assert.rejects(loading, /disposed/);
        assert.equal(FakeWorker.instance.terminated, true);
        assert.equal(FakeResizeObserver.instance.disconnected, true);
    } finally {
        Object.assign(globalThis, previous);
    }
});

test("embeddable API caps the number of outstanding worker requests", async () => {
    const previous = {
        HTMLCanvasElement: globalThis.HTMLCanvasElement,
        Worker: globalThis.Worker,
        ResizeObserver: globalThis.ResizeObserver,
        window: globalThis.window,
        requestAnimationFrame: globalThis.requestAnimationFrame,
        cancelAnimationFrame: globalThis.cancelAnimationFrame,
        CustomEvent: globalThis.CustomEvent,
    };
    globalThis.HTMLCanvasElement = FakeCanvas;
    globalThis.Worker = FakeWorker;
    globalThis.ResizeObserver = FakeResizeObserver;
    globalThis.window = { Worker: FakeWorker, devicePixelRatio: 2, addEventListener() {}, removeEventListener() {} };
    globalThis.requestAnimationFrame = () => 1;
    globalThis.cancelAnimationFrame = () => {};
    globalThis.CustomEvent = class { constructor(type, init) { this.type = type; this.detail = init.detail; } };
    try {
        const viewer = createViewer(new FakeCanvas());
        await viewer.ready;
        FakeWorker.instance.holdApi = true;
        const pending = Array.from({ length: 32 }, () => viewer.getStats());
        await assert.rejects(viewer.getStats(), /request queue is full/);
        viewer.dispose();
        const results = await Promise.allSettled(pending);
        assert.ok(results.every((result) => result.status === "rejected"));
    } finally {
        Object.assign(globalThis, previous);
    }
});

for (const failure of [
    {
        name: "fatal frame error message",
        emit(worker) { worker.send({ type: "error", operation: "frame", message: "frame failed" }); },
    },
    {
        name: "worker error event",
        emit(worker) { worker.sendEvent("error", { message: "worker crashed" }); },
    },
    {
        name: "worker messageerror event",
        emit(worker) { worker.sendEvent("messageerror", { message: "message decode failed" }); },
    },
]) {
    test(`${failure.name} terminates worker and rejects pending requests without later resize`, async () => {
        const previous = {
            HTMLCanvasElement: globalThis.HTMLCanvasElement,
            Worker: globalThis.Worker,
            ResizeObserver: globalThis.ResizeObserver,
            window: globalThis.window,
            requestAnimationFrame: globalThis.requestAnimationFrame,
            cancelAnimationFrame: globalThis.cancelAnimationFrame,
            CustomEvent: globalThis.CustomEvent,
        };
        globalThis.HTMLCanvasElement = FakeCanvas;
        globalThis.Worker = FakeWorker;
        globalThis.ResizeObserver = FakeResizeObserver;
        globalThis.window = { Worker: FakeWorker, devicePixelRatio: 2, addEventListener() {}, removeEventListener() {} };
        globalThis.requestAnimationFrame = () => 1;
        globalThis.cancelAnimationFrame = () => {};
        globalThis.CustomEvent = class { constructor(type, init) { this.type = type; this.detail = init.detail; } };
        try {
            const canvas = new FakeCanvas();
            const viewer = createViewer(canvas);
            await viewer.ready;
            const worker = FakeWorker.instance;
            worker.holdApi = true;
            const pending = viewer.getStats();
            failure.emit(worker);

            await assert.rejects(pending, /frame failed|worker crashed|message decode failed/);
            assert.equal(worker.terminated, true);
            const messageCount = worker.messages.length;
            FakeResizeObserver.instance.callback();
            assert.equal(worker.messages.length, messageCount);
            assert.equal(worker.messages.some((message) => message.type === "resize"), false);
        } finally {
            Object.assign(globalThis, previous);
        }
    });
}

for (const scenario of [
    { name: "by default", options: {}, expected: { url: /\/build-worker\.js$/, type: "module" } },
    { name: "with a custom buildWorkerUrl", options: { buildWorkerUrl: "blob:custom" }, expected: { url: /^blob:custom$/, type: "module" } },
    { name: "unless buildWorker is false", options: { buildWorker: false }, expected: null },
]) {
    test(`init message carries the build worker options ${scenario.name}`, async () => {
        const previous = {
            HTMLCanvasElement: globalThis.HTMLCanvasElement,
            Worker: globalThis.Worker,
            ResizeObserver: globalThis.ResizeObserver,
            window: globalThis.window,
            requestAnimationFrame: globalThis.requestAnimationFrame,
            cancelAnimationFrame: globalThis.cancelAnimationFrame,
        };
        globalThis.HTMLCanvasElement = FakeCanvas;
        globalThis.Worker = FakeWorker;
        globalThis.ResizeObserver = FakeResizeObserver;
        globalThis.window = { Worker: FakeWorker, devicePixelRatio: 1, addEventListener() {}, removeEventListener() {} };
        globalThis.requestAnimationFrame = () => 1;
        globalThis.cancelAnimationFrame = () => {};
        try {
            const viewer = createViewer(new FakeCanvas(), scenario.options);
            await viewer.ready;
            const init = FakeWorker.instance.messages.find((message) => message.type === "init");
            if (scenario.expected === null) {
                assert.equal(init.buildWorker, undefined);
            } else {
                assert.match(init.buildWorker.url, scenario.expected.url);
                assert.equal(init.buildWorker.type, scenario.expected.type);
            }
            viewer.dispose();
        } finally {
            Object.assign(globalThis, previous);
        }
    });
}

class DomCanvas extends FakeCanvas {
    isConnected = true;
    parentNode = {};
    replacedBy = null;
    cloneNode() { return new DomCanvas(); }
    replaceWith(other) { this.replacedBy = other; this.isConnected = false; this.parentNode = null; }
    transferControlToOffscreen() { return { owner: this }; }
}

async function withDeviceLossViewer(canvas, run) {
    const previous = {
        HTMLCanvasElement: globalThis.HTMLCanvasElement,
        Worker: globalThis.Worker,
        ResizeObserver: globalThis.ResizeObserver,
        window: globalThis.window,
        requestAnimationFrame: globalThis.requestAnimationFrame,
    };
    globalThis.HTMLCanvasElement = FakeCanvas;
    globalThis.Worker = FakeWorker;
    globalThis.ResizeObserver = FakeResizeObserver;
    globalThis.requestAnimationFrame = () => 1;
    globalThis.window = { Worker: FakeWorker, devicePixelRatio: 1, addEventListener() {}, removeEventListener() {} };
    try {
        const viewer = createViewer(canvas);
        await viewer.ready;
        await run(viewer, FakeWorker.instance);
    } finally {
        Object.assign(globalThis, previous);
    }
}

test("a device loss swaps the host canvas, hands the worker the new OffscreenCanvas and announces it", async () => {
    const canvas = new DomCanvas();
    await withDeviceLossViewer(canvas, async (viewer, worker) => {
        const events = [];
        viewer.on("deviceLost", detail => events.push(detail));
        let announced = null;
        // The event goes to the new element, which the host finds through the old one's replacement.
        worker.send({ type: "deviceLost", reason: "unknown", message: "gpu crash" });
        const replacement = canvas.replacedBy;
        assert.ok(replacement instanceof DomCanvas, "the element was replaced in the DOM");
        const message = worker.messages.find(m => m.type === "replaceCanvas");
        assert.equal(message.canvas.owner, replacement, "the worker gets the replacement's OffscreenCanvas");
        assert.equal(events[0].canvasReplaced, true);
        assert.equal(canvas.listeners.get("pointerdown").length, 0, "listeners left the old element");
        assert.equal(replacement.listeners.get("pointerdown").length, 1, "and are on the new one");
        replacement.addEventListener("voxelviewer:status", () => { announced = true; });
        worker.send({ type: "status", message: "later" });
        assert.equal(announced, true, "later events go to the new element");
    });
});

test("canvasReplaced is dispatched on the new element", async () => {
    const canvas = new DomCanvas();
    const seen = [];
    const original = DomCanvas.prototype.cloneNode;
    DomCanvas.prototype.cloneNode = function () {
        const clone = original.call(this);
        clone.addEventListener("voxelviewer:canvasReplaced", event => seen.push(event.detail.canvas === clone));
        return clone;
    };
    try {
        await withDeviceLossViewer(canvas, async (_viewer, worker) => {
            worker.send({ type: "deviceLost", reason: "unknown", message: "x" });
        });
    } finally {
        DomCanvas.prototype.cloneNode = original;
    }
    assert.deepEqual(seen, [true]);
});

test("a destroyed device does not replace the canvas", async () => {
    const canvas = new DomCanvas();
    await withDeviceLossViewer(canvas, async (_viewer, worker) => {
        worker.send({ type: "deviceLost", reason: "destroyed", message: "destroy()" });
        assert.equal(canvas.replacedBy, null);
        assert.equal(worker.messages.some(m => m.type === "replaceCanvas"), false);
    });
});

test("a canvas outside the DOM is not replaced: the worker is told to recover on its own and canvasReplaced is false", async () => {
    const canvas = new DomCanvas();
    canvas.isConnected = false;
    await withDeviceLossViewer(canvas, async (viewer, worker) => {
        const events = [];
        viewer.on("deviceLost", detail => events.push(detail));
        worker.send({ type: "deviceLost", reason: "unknown", message: "x" });
        assert.equal(canvas.replacedBy, null);
        assert.equal(worker.messages.find(m => m.type === "replaceCanvas").canvas, null);
        assert.equal(events[0].canvasReplaced, false);
    });
});

test("the clone takes the focus the old element had", async () => {
    const canvas = new DomCanvas();
    const focused = [];
    canvas.ownerDocument = { activeElement: canvas };
    const original = DomCanvas.prototype.cloneNode;
    DomCanvas.prototype.cloneNode = function () {
        const clone = original.call(this);
        clone.focus = (options) => focused.push(options);
        return clone;
    };
    try {
        await withDeviceLossViewer(canvas, async (_viewer, worker) => {
            worker.send({ type: "deviceLost", reason: "unknown", message: "x" });
        });
        assert.deepEqual(focused, [{ preventScroll: true }]);
        const plain = new DomCanvas();
        plain.ownerDocument = { activeElement: {} };
        const before = focused.length;
        await withDeviceLossViewer(plain, async (_viewer, worker) => {
            worker.send({ type: "deviceLost", reason: "unknown", message: "x" });
        });
        assert.equal(focused.length, before, "no focus change when the old element had none");
    } finally {
        DomCanvas.prototype.cloneNode = original;
    }
});

test("init asks for the cells path unless the option is false (R3-Q35)", async () => {
    const previous = {
        HTMLCanvasElement: globalThis.HTMLCanvasElement,
        Worker: globalThis.Worker,
        ResizeObserver: globalThis.ResizeObserver,
        window: globalThis.window,
        requestAnimationFrame: globalThis.requestAnimationFrame,
        cancelAnimationFrame: globalThis.cancelAnimationFrame,
    };
    globalThis.HTMLCanvasElement = FakeCanvas;
    globalThis.Worker = FakeWorker;
    globalThis.ResizeObserver = FakeResizeObserver;
    globalThis.window = { Worker: FakeWorker, devicePixelRatio: 1, addEventListener() {}, removeEventListener() {} };
    globalThis.requestAnimationFrame = () => 1;
    globalThis.cancelAnimationFrame = () => {};
    try {
        const initCells = async (options) => {
            const viewer = createViewer(new FakeCanvas(), options);
            await viewer.ready;
            const cells = FakeWorker.instance.messages.find((message) => message.type === "init").cells;
            viewer.dispose();
            return cells;
        };
        assert.equal(await initCells(undefined), true);
        assert.equal(await initCells({ cells: false }), false);
        assert.equal(await initCells({ cells: true }), true);
    } finally {
        Object.assign(globalThis, previous);
    }
});

test("held WASD/QE keys fly the camera each frame, scaled by dt, Shift and the speed multiplier", async () => {
    const previous = {
        HTMLCanvasElement: globalThis.HTMLCanvasElement,
        Worker: globalThis.Worker,
        ResizeObserver: globalThis.ResizeObserver,
        window: globalThis.window,
        requestAnimationFrame: globalThis.requestAnimationFrame,
        cancelAnimationFrame: globalThis.cancelAnimationFrame,
        CustomEvent: globalThis.CustomEvent,
    };
    const realNow = performance.now.bind(performance);
    let now = 1000;
    performance.now = () => now;
    let frameCallback = null;
    globalThis.HTMLCanvasElement = FakeCanvas;
    globalThis.Worker = FakeWorker;
    globalThis.ResizeObserver = FakeResizeObserver;
    const windowListeners = new Map();
    globalThis.window = {
        Worker: FakeWorker,
        devicePixelRatio: 1,
        addEventListener(type, callback) { windowListeners.set(type, callback); },
        removeEventListener() {},
    };
    globalThis.requestAnimationFrame = (callback) => { frameCallback = callback; return 1; };
    globalThis.cancelAnimationFrame = () => {};
    globalThis.CustomEvent = class { constructor(type, init) { this.type = type; this.detail = init.detail; } };
    try {
        const canvas = new FakeCanvas();
        const viewer = createViewer(canvas);
        await viewer.ready;
        const worker = FakeWorker.instance;
        const key = (type, event) => canvas.dispatchEvent({ type, key: "", preventDefault() {}, ...event });
        // Draws one animation frame after `ms` and returns its camera message.
        const frame = (ms) => {
            now += ms;
            const callback = frameCallback;
            frameCallback = null;
            assert.ok(callback, "a frame is scheduled");
            const before = worker.messages.length;
            callback();
            const message = worker.messages.slice(before).find((item) => item.type === "frame");
            worker.send({ type: "frame" });
            return message.camera;
        };
        key("keydown", { key: "w" });
        key("keydown", { key: "w", repeat: true });
        key("keydown", { key: "d" });
        let camera = frame(50);
        assert.ok(Math.abs(camera.flyForward - 0.05) < 1e-12);
        assert.ok(Math.abs(camera.flyRight - 0.05) < 1e-12);
        assert.equal(camera.flyUp, 0);
        assert.equal(camera.moveY, 0, "no world-axis motion");
        camera = frame(1000);
        assert.ok(Math.abs(camera.flyForward - 0.1) < 1e-12, "dt is clamped to 0.1 s");
        key("keyup", { key: "d" });
        key("keydown", { key: "Q", shiftKey: true });
        camera = frame(100);
        assert.ok(Math.abs(camera.flyForward - 0.4) < 1e-12, "Shift multiplies by 4");
        assert.equal(camera.flyRight, 0);
        assert.ok(Math.abs(camera.flyUp + 0.4) < 1e-12);
        key("keyup", { key: "q" });
        key("keyup", { key: "w" });
        camera = frame(100);
        assert.deepEqual([camera.flyForward, camera.flyRight, camera.flyUp], [0, 0, 0]);
        assert.equal(frameCallback, null, "no frame is scheduled once nothing is held");
        // Blur and other modifiers never start or keep motion.
        key("keydown", { key: "e" });
        windowListeners.get("blur")();
        for (const modifier of ["ctrlKey", "metaKey", "altKey"]) key("keydown", { key: "a", [modifier]: true });
        key("keydown", { key: "+" });
        camera = frame(100);
        assert.ok(camera.zoom < 0);
        assert.deepEqual([camera.flyForward, camera.flyRight, camera.flyUp], [0, 0, 0]);
        viewer.dispose();
    } finally {
        performance.now = realNow;
        Object.assign(globalThis, previous);
    }
});
