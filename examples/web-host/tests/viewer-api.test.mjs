import assert from "node:assert/strict";
import test from "node:test";

import { createViewer } from "../viewer-api.mjs";

class FakeCanvas {
    width = 1280;
    height = 720;
    clientWidth = 640;
    clientHeight = 360;
    listeners = new Map();

    transferControlToOffscreen() { return {}; }
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
    disconnected = false;
    constructor(callback) { this.callback = callback; FakeResizeObserver.instance = this; }
    observe() {}
    disconnect() { this.disconnected = true; }
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
        await assert.rejects(viewer.load(new Blob(), { voxelPixels: 17 }), /between 0.5 and 16/);
        await assert.rejects(viewer.setCamera([Number.NaN, 0, 0], [0, 0, 0]), /finite 3D coordinates/);
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
