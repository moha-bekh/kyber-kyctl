import { JSMutex } from './mutex.js';

const FIELD_MUTEX = 0;
const FIELD_HEAD = 1;
const FIELD_TAIL = 2;
const FIELD_UNDERFLOW = 3;
const FIELD_RECEIVED = 4;
const FIELD_PLAYED = 5;

const FIELDS = 6;
const FIELDS_SIZE = FIELDS * 4;

class AudioPlayerSharedMemory {
    constructor(sab, sampleSize, capacity) {
        this.sab = sab;

        this.fields = new Int32Array(sab, 0, FIELDS);
        this.mutex = new JSMutex(this.fields.subarray(0, 1))

        this.sampleSize = sampleSize;
        this.allocSize = capacity + 1; // in samples

        // offset is in bytes, but length is in number of items
        let nbChannels = 2;
        this.data = new Float32Array(sab, FIELDS_SIZE, this.allocSize * nbChannels);
    }

    // getXXX() could theoretically be replaced by a getter (get XXX()), but
    // keep a method to make it explicit that it is not a raw assignment

    getHead() {
        return Atomics.load(this.fields, FIELD_HEAD);
    }

    setHead(head) {
        Atomics.store(this.fields, FIELD_HEAD, head);
    }

    getTail() {
        return Atomics.load(this.fields, FIELD_TAIL);
    }

    setTail(tail) {
        Atomics.store(this.fields, FIELD_TAIL, tail);
    }

    addUnderflow(underflow) {
        return Atomics.add(this.fields, FIELD_UNDERFLOW, underflow);
    }

    getReceived() {
        return Atomics.load(this.fields, FIELD_RECEIVED) != 0;
    }

    setReceived(received) {
        Atomics.store(this.fields, FIELD_RECEIVED, received ? 1 : 0);
    }

    getPlayed() {
        return Atomics.load(this.fields, FIELD_PLAYED) != 0;
    }

    setPlayed(played) {
        Atomics.store(this.fields, FIELD_PLAYED, played ? 1 : 0);
    }
}

class AudioBuffer {
    constructor(memory) {
        this.memory = memory;
    }

    read(output) {
        let allocSize = this.memory.allocSize;
        let nbChannels = output.length;
        let nbFrames = output[0].length;

        let tail = this.memory.getTail();
        let head = this.memory.getHead();

        let canRead = (allocSize + head - tail) % allocSize;
        let samplesCount = Math.min(nbFrames, canRead);

        for (let frameIdx = 0; frameIdx < samplesCount; ++frameIdx) {
            let idx = ((tail + frameIdx) % allocSize) * nbChannels;
            for (let channelIdx = 0; channelIdx < nbChannels; ++channelIdx) {
                output[channelIdx][frameIdx] = this.memory.data[idx + channelIdx];
            }
        }

        let newTail = (tail + samplesCount) % allocSize;
        this.memory.setTail(newTail);

        return samplesCount;
    }

    canRead() {
        let allocSize = this.memory.allocSize;
        let head = this.memory.getHead();
        let tail = this.memory.getTail();
        return (allocSize + head - tail) % allocSize;
    }

    // no need to write from the javascript AudioWorklet
}

class KyberAudioWorklet extends AudioWorkletProcessor {
    constructor(options) {
        super();

        let { sab, sampleRate, sampleSize, capacity, targetBuffering } = options.processorOptions;
        this.sampleRate = sampleRate;
        this.targetBuffering = targetBuffering;
        this.memory = new AudioPlayerSharedMemory(sab, sampleSize, capacity);
        this.audioBuf = new AudioBuffer(this.memory);
    }

    process(inputs, outputs) {
        const output = outputs[0];
        const nbChannels = output.length;
        const nbFrames = output[0].length;

        this.memory.mutex.lock();
        try {

            let played = this.memory.getPlayed();
            if (!played) {
                let bufferedSamples = this.audioBuf.canRead();
                // Wait until the buffer is filled up to at least targetBuffering
                // before playing
                if (bufferedSamples < this.targetBuffering) {
                    console.log("[Audio] Inserting initial buffering silence: " + nbFrames + " samples");
                    // Output buffer is already initialized with zeros
                    return true;
                }
            }

            let read = this.audioBuf.read(output);

            if (read < nbFrames) {
                // The remaining of the output buffer is already initialized with zeros
                let silence = nbFrames - read;
                let received = this.memory.getReceived();
                if (received) {
                    this.memory.addUnderflow(silence);
                }
            }

            this.memory.setPlayed(true);

        } finally {
            this.memory.mutex.unlock();
        }

        return true;
    }
}

registerProcessor("AudioRenderer", KyberAudioWorklet);
