// Inspired by <https://blogtitle.github.io/using-javascript-sharedarraybuffers-and-atomics/>

const UNLOCKED = 0;
const LOCKED = 1;

export class JSSpinLock {
    constructor(sabInt32Array) {
        this._data = sabInt32Array;
    }

    lock() {
        while (Atomics.compareExchange(this._data, 0, UNLOCKED, LOCKED) != UNLOCKED) {}
    }

    unlock() {
        if (Atomics.compareExchange(this._data, 0, LOCKED, UNLOCKED) != LOCKED) {
            throw new Error("Inconsistent spinlock state: unlock() unlocked spinlock");
        }
    }
}
