import init, { VideoDemuxerWorkerCtx } from "./libclient_wasm.js";

let ctx;
let initialized_promise = init().then(() => {
    ctx = VideoDemuxerWorkerCtx.new();
});

// The onmessage callback must be set immediately to avoid a race condition (if
// a message is posted while the callback is not set, it will be missed), but
// the actual callback execution must wait until Wasm is initialized.
self.onmessage = async (event) => {
    await initialized_promise;
    ctx.on_message(event);
}
