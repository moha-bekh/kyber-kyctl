export class AudioRenderer {
    async init(sampleRate, channelCount, sab) {
        this.audioContext = new AudioContext({
            sampleRate: sampleRate,
            latencyHint: "interactive"
        });
        this.audioContext.suspend();

        console.debug(`Audio renderer requested sampleRate: ${sampleRate}. Got: ${this.audioContext.sampleRate}`)

        await this.audioContext.audioWorklet.addModule("audio_worklet.js");

        this.node = new AudioWorkletNode(this.audioContext, "AudioRenderer", {
            processorOptions: {
                sab: sab,
            },
            outputChannelCount: [channelCount]
        });

        this.node.connect(this.audioContext.destination);

        await this.audioContext.resume();
    }

    async stop() {
        this.audioContext.suspend();
        await this.audioContext.close()
        this.node.disconnect(this.audioContext.destination)

        this.audioContext = null
        this.node = null
    }
}
