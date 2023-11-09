class AudioRenderer extends AudioWorkletProcessor {
    constructor(options) {
        super();

        this.sab = options.processorOptions.sab;
        this.read_ptr = new Int32Array(this.sab, 0, 1);
        this.write_ptr = new Int32Array(this.sab, 4, 1);

        this.capacity = (this.sab.byteLength - 8) / 4;
        this.data = new Float32Array(this.sab, 8, this._capacity);
    }

    process(inputs, outputs, parameters) {
        const output = outputs[0];
        const nbChannels = output.length;
        const nbFrames = output[0].length;

        let read_ptr = Atomics.load(this.read_ptr, 0);
        let write_ptr = Atomics.load(this.write_ptr, 0);
        if (read_ptr == write_ptr) {
            return true;
        }

        // Get the number of frames to read
        let max_read_size = (write_ptr + this.capacity - read_ptr) % this.capacity;
        let frames_to_read = Math.min(max_read_size, nbFrames * nbChannels);

        // Get the ringbuffer parts to read
        let part_one_size = Math.min(this.capacity - read_ptr, frames_to_read);
        let part_two_size = frames_to_read - part_one_size;

        // Prepare for reading from the ringbuffer
        let read_areas = [
            [read_ptr, part_one_size],
            [0, part_two_size],
        ];

        let area_idx = 0;
        let read_offset, read_remaining;

        [read_offset, read_remaining] = read_areas[area_idx];
        area_idx++;

        let process = true;

        for (let frameIdx = 0; frameIdx < frames_to_read; frameIdx++) {
            for (let channel = 0; channel < nbChannels; channel++) {
                output[channel][frameIdx] = this.data[read_offset];

                read_remaining--;
                if (read_remaining == 0) {
                    if (area_idx >= read_areas.length) {
                        process = false;
                        break;
                    } else {
                        [read_offset, read_remaining] = read_areas[area_idx];
                        area_idx++;
                    }
                } else {
                    read_offset++;
                }
            }
        }

        let new_read_ptr = (read_ptr + frames_to_read) % this.capacity;
        Atomics.store(this.read_ptr, 0, new_read_ptr);

        return true;
    }
};

registerProcessor("AudioRenderer", AudioRenderer);
