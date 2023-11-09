export class AudioRenderer {
    async init(sampleRate, channelCount, sab) {
        let audioContext = new AudioContext({
            sampleRate: sampleRate,
            latencyHint: "interactive"
        });
        audioContext.suspend();

        console.debug(`Audio renderer requested sampleRate: ${sampleRate}. Got: ${audioContext.sampleRate}`)

        await audioContext.audioWorklet.addModule("audio_worklet.js");

        let node = new AudioWorkletNode(audioContext, "AudioRenderer", {
            processorOptions: {
                sab: sab,
            },
            outputChannelCount: [channelCount]
        });

        node.connect(audioContext.destination);

        await audioContext.resume();
    }
}
