export class CoalescedCommandQueue {
    constructor(send) {
        this.send = send;
        this.pending = new Map();
        this.inFlight = new Set();
    }

    enqueue(type, values = {}) {
        this.pending.set(type, values);
        this.flush();
    }

    acknowledge(type) {
        if (!this.inFlight.delete(type)) return false;
        this.flush();
        return true;
    }

    flush() {
        for (const [type, values] of this.pending) {
            if (this.inFlight.has(type)) continue;
            this.pending.delete(type);
            this.inFlight.add(type);
            this.send(type, values);
        }
    }

    reset() {
        this.pending.clear();
        this.inFlight.clear();
    }
}
