// Inspired by <https://blogtitle.github.io/using-javascript-sharedarraybuffers-and-atomics/>

const UNLOCKED = 0;
const LOCKED = 1;

export class JSMutex {
    constructor(sabInt32Array) {
        this._mutex = sabInt32Array;
    }

    lock() {
        for (;;) {
            if (Atomics.compareExchange(this._mutex, 0, UNLOCKED, LOCKED) == UNLOCKED) {
                return;
            }
        }
    }

    unlock() {
        if (Atomics.compareExchange(this._mutex, 0, LOCKED, UNLOCKED) != LOCKED) {
            throw new Error("Inconsistent mutex state: unlock() unlocked mutex");
        }
    }
}
