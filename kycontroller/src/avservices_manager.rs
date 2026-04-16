// Project Kyber: avservices_manager.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0-or-later
//
// This file is both under dual license: AGPLv3 and a Commercial one.
//
// ----
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use kyavservice_types::VideoCodec;
use libkypc::process::ProcessFactory;
use log::{error, info, warn};
use uuid::Uuid;

use crate::avservice;
use crate::controller::Controller;
use crate::error::Result;
use crate::kydup;
use crate::utils::Task;
use crate::StateMachineProxy;

/// Backstop on a single reaper teardown batch. Each `avservice.stop()` is
/// individually bounded (it force-kills the child and falls back to aborting
/// its actor on timeout) and [`AVCleanup::run`] fans them out concurrently, so
/// the batch normally finishes in ~one per-stream budget. If it ever elapses
/// anyway we log and move on to the kycom teardown rather than letting one
/// pathological stream freeze the reaper — and every later teardown behind it.
const AV_CLEANUP_TIMEOUT: Duration = Duration::from_secs(30);

/// AV streams (Shared or Exclusive) whose last subscriber just dropped and
/// that need async teardown. The reaper holds the Controller lock only to
/// extract the dropped streams into this struct, then RELEASES the lock
/// before calling `.run().await`; `avservice.stop()` kills a child process
/// and waits on IPC with timeouts, so holding the lock across it would stall
/// every HTTP endpoint (same pattern as `SessionCleanup`).
#[derive(Default)]
pub(crate) struct AVCleanup {
    videos: Vec<VideoStream>,
    audios: Vec<AudioStream>,
}

impl AVCleanup {
    fn push_video(&mut self, stream: Option<VideoStream>) {
        if let Some(stream) = stream {
            self.videos.push(stream);
        }
    }

    fn push_audio(&mut self, stream: Option<AudioStream>) {
        if let Some(stream) = stream {
            self.audios.push(stream);
        }
    }

    pub(crate) async fn run(self) {
        // Stop every stream concurrently. Each `avservice.stop()` is itself
        // bounded, so fanning them out keeps total teardown at ~one per-stream
        // budget regardless of how many streams a burst collapsed into this
        // batch — a sequential loop would be N × that budget, and one wedged
        // stream would delay every stream behind it.
        //
        // Note: the kydup input pump is owned by kycom's Runner, not by us;
        // stopping the avservice closes its IPC TCP, which causes that
        // forwarder to exit naturally. The metrics forwarder is ours and
        // must be aborted explicitly. For `Exclusive` entries there's no
        // kydup; same logic still applies (avservice TCP close ends kycom's
        // forwarder, metrics forwarder if any is ours to abort).
        let mut tasks = Vec::with_capacity(self.videos.len() + self.audios.len());
        for stream in self.videos {
            tasks.push(tokio::spawn(stream.avservice.stop()));
        }
        for stream in self.audios {
            tasks.push(tokio::spawn(stream.avservice.stop()));
        }
        for task in tasks {
            let _ = task.await;
        }
    }
}

/// Per-video metrics forwarder (avserver → first client only).
///
/// The forwarder task starts in drain mode (packets discarded). The
/// [`MetricsClaim`] handed to the first requesting client carries the
/// upgrade channel; once that client provides its send, the task switches
/// to forward mode. If the client disconnects, the task falls back to drain;
/// no other client can claim metrics on this stream after the first one.
pub(crate) struct VideoMetrics {
    /// Forwarder task running [`metrics_forwarder_loop`]. Aborted on stream
    /// teardown by [`AVCleanup`].
    pub(crate) _forwarder_task: Task,
}

/// Handle returned to the first client requesting metrics on a freshly-created
/// shared stream. Consumes the upgrade channel to bind the forwarder to the
/// client's `ProtocolSend<MetricsPacket>` once the kyproto endpoint is ready.
pub(crate) struct MetricsClaim {
    upgrade_tx:
        tokio::sync::oneshot::Sender<kymux::types::ProtocolSend<kymux::types::MetricsPacket>>,
}

impl MetricsClaim {
    /// Hand the client's send to the forwarder. Ignores errors: if the
    /// forwarder task is gone (e.g. avservice already torn down), there's
    /// nothing to do.
    pub(crate) fn claim(self, send: kymux::types::ProtocolSend<kymux::types::MetricsPacket>) {
        let _ = self.upgrade_tx.send(send);
    }
}

/// Drives the per-shared-stream metrics endpoint exposed by the avservice.
///
/// - **Phase 1 (drain)**: until either a client upgrades the destination or
///   `recv` closes. Packets are read and discarded so the avservice's metrics
///   IPC stays drained.
/// - **Phase 2 (forward)**: write each packet to the upgraded destination.
///   If the send fails (e.g. the client disconnected), fall back to draining
///   for the rest of the avservice's lifetime; no new client can take over,
///   metrics for this stream are effectively dropped.
pub(crate) async fn metrics_forwarder_loop(
    mut recv: kymux::types::ProtocolRecv<kymux::types::MetricsPacket>,
    upgrade_rx: tokio::sync::oneshot::Receiver<
        kymux::types::ProtocolSend<kymux::types::MetricsPacket>,
    >,
) {
    // Phase 1: drain while waiting for the upgrade.
    let mut send = {
        tokio::pin!(upgrade_rx);
        loop {
            tokio::select! {
                biased;
                up = &mut upgrade_rx => {
                    // Ok(send) → switch to forward; Err(_) → upgrade_tx dropped
                    // (no client ever claimed; just keep draining).
                    break up.ok();
                }
                packet = recv.recv() => {
                    match packet {
                        Ok(Some(_)) => continue, // discard
                        _ => return,             // recv closed or errored
                    }
                }
            }
        }
    };

    // Phase 2: forward, with fallback to drain if send fails.
    loop {
        let packet = match recv.recv().await {
            Ok(Some(p)) => p,
            _ => return,
        };
        if let Some(s) = send.as_mut() {
            if s.send(packet).await.is_err() {
                send = None; // client gone; drain for the rest of the stream
            }
        }
        // else: drain mode, discard.
    }
}

/// Identifies an entry in the AV registry.
///
/// `Shared` keys are reused across clients (multi-client mode); the same
/// avservice feeds every client on `display_id` via kydup. `Exclusive` keys
/// are unique to a single session (mono-client mode), so each session gets
/// its own avservice regardless of which display it targets; no kydup, the
/// avservice writes directly to that session's client endpoint via kycom.
#[derive(Clone, Hash, PartialEq, Eq, Debug)]
pub(crate) enum VideoStreamKey {
    Shared { display_id: u32 },
    Exclusive { session_id: Uuid, display_id: u32 },
}

/// Audio counterpart of [`VideoStreamKey`]. Audio has no display_id.
#[derive(Clone, Hash, PartialEq, Eq, Debug)]
pub(crate) enum AudioStreamKey {
    Shared,
    Exclusive { session_id: Uuid },
}

/// Routing identity of a single AV stream instance, attached to every event a
/// stream's handler forwards to the StateMachine.
///
/// It simply wraps the registry key. That key is **unique per stream
/// instance by construction**: two streams sharing a key *are* the same stream
/// (that is what decides reuse in `get_or_create_*`). Matching an event's
/// `StreamId` against [`VideoSubscription::key`] / [`AudioSubscription::key`]
/// therefore routes precisely to the subscribed sessions — even when several
/// streams target the same display (mono multi-session, or an exclusive
/// stream alongside a shared one). `display_id` alone could not.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum StreamId {
    Video(VideoStreamKey),
    Audio(AudioStreamKey),
}

/// How the avservice output is plumbed for a given acquire call.
///
/// `Shared`: av_services_manager allocates a kydup, registers its upstream with kycom,
/// and the avservice writes to that URI. Callers attach client outputs to the
/// returned kydup.
///
/// `Exclusive`: the caller has already registered its client endpoint with
/// kycom and got a URI back; av_services_manager just starts the avservice
/// writing to it, with no kydup in the path.
pub(crate) enum VideoTopology {
    Shared {
        display_id: u32,
    },
    Exclusive {
        session_id: Uuid,
        display_id: u32,
        kycom_uri: String,
    },
}

impl VideoTopology {
    fn key(&self) -> VideoStreamKey {
        match self {
            VideoTopology::Shared { display_id } => VideoStreamKey::Shared {
                display_id: *display_id,
            },
            VideoTopology::Exclusive {
                session_id,
                display_id,
                ..
            } => VideoStreamKey::Exclusive {
                session_id: *session_id,
                display_id: *display_id,
            },
        }
    }

    fn display_id(&self) -> u32 {
        match self {
            VideoTopology::Shared { display_id } | VideoTopology::Exclusive { display_id, .. } => {
                *display_id
            }
        }
    }
}

/// Audio counterpart of [`VideoTopology`].
pub(crate) enum AudioTopology {
    Shared,
    Exclusive { session_id: Uuid, kycom_uri: String },
}

impl AudioTopology {
    fn key(&self) -> AudioStreamKey {
        match self {
            AudioTopology::Shared => AudioStreamKey::Shared,
            AudioTopology::Exclusive { session_id, .. } => AudioStreamKey::Exclusive {
                session_id: *session_id,
            },
        }
    }
}

/// Output of [`AVServicesManager::acquire_video`].
pub(crate) struct VideoAcquisition {
    /// `Some` when the entry is `Shared` (caller attaches client outputs to
    /// this kydup). `None` for `Exclusive` (avservice writes directly to the
    /// caller-supplied URI; no fan-out).
    pub(crate) dup: Option<kydup::KydupVideo>,
    /// `Some` only when this call is the first to acquire metrics on a fresh
    /// entry. Caller hands the kyproto metrics send to it via `claim()`.
    pub(crate) metrics_claim: Option<MetricsClaim>,
    pub(crate) subscription: VideoSubscription,
}

/// Output of [`AVServicesManager::acquire_audio`].
pub(crate) struct AudioAcquisition {
    /// `Some` for `Shared`, `None` for `Exclusive`. See [`VideoAcquisition::dup`].
    pub(crate) dup: Option<kydup::KydupAudio>,
    pub(crate) subscription: AudioSubscription,
}

pub(crate) struct VideoStream {
    /// Wrapped in [`avservice::SharedHandler`] so background tasks (IDR
    /// dispatcher, reaper) and HTTP endpoints (set_bitrate) can post
    /// commands via a sync mpsc send instead of acquiring an async mutex.
    /// The actor task inside `SharedHandler` serializes the IPC `.await`s
    /// off the Controller lock; same isolation guarantee, no `lock().await`.
    pub(crate) avservice: avservice::SharedHandler,
    /// `Some` when the entry is `Shared` (kydup fan-out across N clients).
    /// `None` for `Exclusive`: no kydup, the avservice feeds the single
    /// client endpoint directly via kycom.
    pub(crate) dup: Option<kydup::KydupVideo>,
    /// Only present if the first client to create the entry requested
    /// metrics. The choice sticks for the lifetime of the entry; later
    /// clients asking for metrics on the same `Shared` entry will silently
    /// not receive them. TODO: add a runtime toggle on the avservice API.
    pub(crate) _metrics: Option<VideoMetrics>,
    /// Effective config the avservice was started with. Kept around so that
    /// later subscribers can be warned when they ask for something different.
    pub(crate) config: VideoStreamConfig,
    pub(crate) client_count: usize,
}

pub(crate) struct AudioStream {
    /// See `VideoStream::avservice`.
    pub(crate) avservice: avservice::SharedHandler,
    /// See `VideoStream::dup`.
    pub(crate) dup: Option<kydup::KydupAudio>,
    pub(crate) client_count: usize,
}

#[derive(Debug)]
pub(crate) enum AvReaperMsg {
    VideoDropped(VideoStreamKey),
    AudioDropped(AudioStreamKey),
}

/// RAII subscription to a shared video stream. Dropping enqueues a decrement
/// on the AV reaper, which performs the async cleanup when client_count hits 0.
pub(crate) struct VideoSubscription {
    key: VideoStreamKey,
    tx: tokio::sync::mpsc::UnboundedSender<AvReaperMsg>,
}

impl VideoSubscription {
    pub(crate) fn key(&self) -> &VideoStreamKey {
        &self.key
    }
}

impl Drop for VideoSubscription {
    fn drop(&mut self) {
        let _ = self.tx.send(AvReaperMsg::VideoDropped(self.key.clone()));
    }
}

/// RAII subscription to the shared audio stream. See VideoSubscription.
pub(crate) struct AudioSubscription {
    key: AudioStreamKey,
    tx: tokio::sync::mpsc::UnboundedSender<AvReaperMsg>,
}

impl AudioSubscription {
    pub(crate) fn key(&self) -> &AudioStreamKey {
        &self.key
    }
}

impl Drop for AudioSubscription {
    fn drop(&mut self) {
        let _ = self.tx.send(AvReaperMsg::AudioDropped(self.key.clone()));
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct VideoStreamConfig {
    pub(crate) bitrate: u32,
    pub(crate) codec: VideoCodec,
    pub(crate) yuv444: bool,
    pub(crate) intra_refresh: Option<bool>,
    pub(crate) video_stream_count: u32,
}

pub(crate) struct AVServicesManager {
    /// Keyed by [`VideoStreamKey`]: `Shared` entries fan out via kydup;
    /// `Exclusive` entries are per-session, no kydup.
    pub(crate) video_streams: HashMap<VideoStreamKey, VideoStream>,
    /// Same dual nature as `video_streams`. There can be at most one `Shared`
    /// audio entry (audio is not per-display) and one `Exclusive` per active
    /// mono session.
    pub(crate) audio_streams: HashMap<AudioStreamKey, AudioStream>,
    /// Cloned into each kydup's IDR callback. Always valid; messages sent
    /// before the dispatcher task spawns buffer in the bounded channel.
    /// Carries the [`VideoStreamKey`] of the stream that needs a fresh IDR, so
    /// the dispatcher hits exactly that stream (not every stream on the
    /// display).
    idr_tx: tokio::sync::mpsc::Sender<VideoStreamKey>,
    /// Taken by `ensure_idr_dispatcher` on first call to hand over to the task.
    idr_rx: Option<tokio::sync::mpsc::Receiver<VideoStreamKey>>,
    /// Cloned into every VideoSubscription / AudioSubscription. Always valid:
    /// messages sent before `ensure_reaper` has spawned the task are buffered
    /// by the unbounded channel and processed once the task starts.
    reaper_tx: tokio::sync::mpsc::UnboundedSender<AvReaperMsg>,
    /// Taken by `ensure_reaper` on first call to hand over to the spawned task.
    reaper_rx: Option<tokio::sync::mpsc::UnboundedReceiver<AvReaperMsg>>,
    next_endpoint_id: u16,
}

impl Default for AVServicesManager {
    fn default() -> Self {
        let (idr_tx, idr_rx) = tokio::sync::mpsc::channel::<VideoStreamKey>(32);
        let (reaper_tx, reaper_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            video_streams: HashMap::new(),
            audio_streams: HashMap::new(),
            idr_tx,
            idr_rx: Some(idr_rx),
            reaper_tx,
            reaper_rx: Some(reaper_rx),
            next_endpoint_id: 0xC000,
        }
    }
}

/// Handle returned by [`AVServicesManager::init`]. Dropping (or calling
/// [`Self::stop`]) cancels the IDR dispatcher and AV reaper tasks.
///
/// The `web::Data<Mutex<Controller>>` wrapper holds Arc clones for the
/// process lifetime, so relying on `Weak::upgrade` to fail as an exit signal
/// doesn't work; we cancel explicitly here instead.
pub(crate) struct AvServicesTasksHandle {
    cancel_tx: tokio::sync::watch::Sender<bool>,
}

impl AvServicesTasksHandle {
    pub(crate) fn stop(&self) {
        let _ = self.cancel_tx.send(true);
    }
}

impl Drop for AvServicesTasksHandle {
    fn drop(&mut self) {
        let _ = self.cancel_tx.send(true);
    }
}

impl AVServicesManager {
    /// Spawn the IDR dispatcher and AV reaper background tasks.
    ///
    /// Must be called exactly once, from `main.rs`, right after the Controller
    /// has been wrapped in `Arc<Mutex<_>>`. The tasks hold a Weak reference
    /// to the Controller and exit on the explicit cancel signal carried by the
    /// returned handle (drop or `stop()`).
    ///
    /// Subsequent calls return a handle whose cancel signal is unused.
    #[must_use = "drop the handle (or call stop) to cancel the spawned tasks"]
    pub(crate) fn init(&mut self, weak: std::sync::Weak<Controller>) -> AvServicesTasksHandle {
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        self.spawn_idr_dispatcher(weak.clone(), cancel_rx.clone());
        self.spawn_reaper(weak, cancel_rx);
        AvServicesTasksHandle { cancel_tx }
    }

    fn spawn_idr_dispatcher(
        &mut self,
        weak: std::sync::Weak<Controller>,
        mut cancel_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        let Some(mut idr_rx) = self.idr_rx.take() else {
            return;
        };
        tokio::spawn(async move {
            loop {
                let key = tokio::select! {
                    _ = cancel_rx.changed() => break,
                    msg = idr_rx.recv() => match msg {
                        Some(k) => k,
                        None => break,
                    },
                };
                // Drain duplicates already queued; multiple IDR requests for
                // the same stream are redundant, and batching reduces the time
                // we hold the Controller lock.
                let mut pending = std::collections::HashSet::new();
                pending.insert(key);
                while let Ok(k) = idr_rx.try_recv() {
                    pending.insert(k);
                }
                let Some(ctrl) = weak.upgrade() else {
                    break;
                };
                // Each SharedHandler exposes a sync mpsc send for IDR, so
                // we can issue the request directly under the Controller
                // lock; no IPC `.await` is held here, the avservice's
                // actor task drains the queue asynchronously off-lock.
                //
                // Requests carry the exact `VideoStreamKey`, so a direct map
                // lookup forces an IDR on precisely the targeted stream —
                // never on the other streams that happen to share its display.
                let av_services_manager = ctrl.av_services_manager.lock().await;
                for key in pending {
                    if let Some(stream) = av_services_manager.video_streams.get(&key) {
                        if let Err(e) = stream.avservice.video_force_idr() {
                            warn!("Force IDR failed for {key:?}: {e}");
                        }
                    }
                }
            }
        });
    }

    fn spawn_reaper(
        &mut self,
        weak: std::sync::Weak<Controller>,
        mut cancel_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        let Some(mut rx) = self.reaper_rx.take() else {
            return;
        };
        tokio::spawn(async move {
            loop {
                let first = tokio::select! {
                    _ = cancel_rx.changed() => break,
                    msg = rx.recv() => match msg {
                        Some(m) => m,
                        None => break,
                    },
                };
                // Drain under one lock acquisition: under a burst (e.g., all
                // clients disconnecting at once), this collapses N round-trips
                // into a single lock+unlock, same as the IDR dispatcher.
                let mut batch = vec![first];
                while let Ok(msg) = rx.try_recv() {
                    batch.push(msg);
                }

                let Some(ctrl) = weak.upgrade() else {
                    break;
                };
                // Extract the streams whose last subscriber just left, then
                // release the lock BEFORE calling avservice.stop(), which
                // can take seconds (child.kill + commander stop, both
                // timeout-guarded) and would otherwise stall every HTTP
                // endpoint waiting on the Controller lock.
                let cleanup = {
                    let mut cleanup = AVCleanup::default();
                    let mut av_services_manager = ctrl.av_services_manager.lock().await;
                    for msg in batch {
                        match msg {
                            AvReaperMsg::VideoDropped(key) => {
                                cleanup.push_video(av_services_manager.take_video_if_last(&key));
                            }
                            AvReaperMsg::AudioDropped(key) => {
                                cleanup.push_audio(av_services_manager.take_audio_if_last(&key));
                            }
                        }
                    }
                    cleanup
                };
                if tokio::time::timeout(AV_CLEANUP_TIMEOUT, cleanup.run())
                    .await
                    .is_err()
                {
                    // The per-stream stops are still running detached (dropping
                    // their join handles only detaches them) and will finish
                    // killing the children; we just stop waiting so the kycom
                    // teardown and later batches aren't blocked.
                    warn!(
                        "AV cleanup exceeded {AV_CLEANUP_TIMEOUT:?}; \
                         proceeding with kycom teardown"
                    );
                }

                // Now that the last avservers have been stopped via libkypc,
                // it is safe to tear down the shared kycom; closing its IPC
                // TCP sockets before the avservers received their Stop would
                // have produced spurious broken-pipe errors on the host.
                let kycom_to_stop = ctrl.try_take_idle_kycom().await;
                if kycom_to_stop.is_some() {
                    info!("Last AV stream torn down, stopping shared kycom");
                }
                drop(kycom_to_stop);
            }
        });
    }

    /// `config` is only honored when a new entry is created. If an entry
    /// already exists (only possible for `Shared` topology since `Exclusive`
    /// keys are unique per session), the existing avservice is reused as-is
    /// and the caller's `config` is ignored; a mismatch is logged as a
    /// warning so it can be diagnosed. Reconfiguring a running shared
    /// avservice would affect every subscribed client, so we accept the
    /// "first client wins" trade-off for now.
    async fn get_or_create_video(
        &mut self,
        kycom: &mut kymux::kycom::KyCom,
        topology: VideoTopology,
        config: VideoStreamConfig,
        metrics_requested: bool,
        process_factory: &Arc<ProcessFactory>,
        statemachine_proxy: StateMachineProxy,
    ) -> Result<(
        VideoStreamKey,
        Option<kydup::KydupVideo>,
        Option<MetricsClaim>,
    )> {
        let key = topology.key();
        let display_id = topology.display_id();

        // Resolve the kydup + kycom URI, dispatching once on topology:
        //  - Shared: reuse the existing entry if there is one; otherwise
        //    allocate a kydup and register its upstream with kycom. The URI we
        //    get back is what the avservice writes to; kydup fans the packets
        //    out to N client outputs.
        //  - Exclusive: the caller already registered its client endpoint with
        //    kycom and got an URI back; no kydup in the path, and the entry must
        //    not already exist.
        let (dup, kycom_uri) = match topology {
            VideoTopology::Shared { .. } => {
                if let Some(stream) = self.video_streams.get_mut(&key) {
                    if stream.config != config {
                        warn!(
                            "Shared video stream for display {display_id} already running with \
                             {:?}; new subscriber requested {:?}; existing config is kept.",
                            stream.config, config,
                        );
                    }
                    // Metrics are bound to the first client to request them when
                    // the stream was created. A later subscriber cannot claim,
                    // even if the first client is already gone (one-shot slot).
                    return Ok((key, stream.dup.clone(), None));
                }

                let endpoint_id = self.alloc_endpoint_id();
                // Lag handler: continue (re-sync on lag) AND force an IDR so the
                // laggy output can resync on the next forced keyframe. This is
                // required, not just an optimization: under intra-refresh (the
                // default) the encoder emits no periodic keyframes, so a
                // re-syncing output would otherwise wait forever for a keyframe
                // that never comes. The dispatcher dedupes concurrent requests,
                // so an N-client lag burst still produces a single IDR; non-laggy
                // clients receive one extra keyframe (a bitrate blip, but
                // functionally harmless).
                let idr_tx = self.idr_tx.clone();
                let idr_key = key.clone();
                let (dup, upstream) = kydup::KydupVideo::new(endpoint_id, move || {
                    if let Err(e) = idr_tx.try_send(idr_key.clone()) {
                        warn!("IDR request on lag dropped for {idr_key:?}: {e}");
                    }
                    kydup::LagAction::Continue
                });
                // kycom drives the upstream pump: it bridges incoming IPC TCP
                // from the avservice into kydup's broadcast send driver. No
                // task to track on our side; kycom's Runner owns it and
                // aborts on stop().
                let uri = kycom.register_and_forward(upstream)?.url();
                (Some(dup), uri)
            }
            VideoTopology::Exclusive { kycom_uri, .. } => {
                // Exclusive entries are keyed by `(session_id, display_id)` and
                // a single session cannot start streaming twice concurrently
                // (`has_data_plane` gate). So we should never find a pre-existing
                // entry; if we do, it's a leak.
                debug_assert!(
                    !self.video_streams.contains_key(&key),
                    "Exclusive video stream already exists for {key:?}",
                );
                (None, kycom_uri)
            }
        };

        // Metrics infra is only allocated if the caller requested it; kymedia
        // receives a metrics URI only in that case, so it won't emit metrics
        // packets at all otherwise. The forwarder task drains the IPC until
        // the requesting client provides its `ProtocolSend` via the
        // `MetricsClaim` returned below, then forwards. The claim is one-shot:
        // for `Shared` entries, later subscribers never get metrics; for
        // `Exclusive` the single client is always the claimant, so this is
        // trivially "1st client only" in both modes.
        let (_metrics, metrics_claim, metrics_uri) = if metrics_requested {
            let metrics_kycom_channel = kycom.register_channel()?;
            let metrics_uri = metrics_kycom_channel.addr().url();

            let (upgrade_tx, upgrade_rx) = tokio::sync::oneshot::channel();
            let _forwarder_task = Task::spawn(async move {
                let recv = match metrics_kycom_channel
                    .into_metrics_client_endpoint()
                    .ready()
                    .await
                {
                    Ok(ipc) => ipc.recv,
                    Err(e) => {
                        error!("Metrics IPC ready failed: {e}");
                        return;
                    }
                };
                metrics_forwarder_loop(recv, upgrade_rx).await;
            });

            (
                Some(VideoMetrics { _forwarder_task }),
                Some(MetricsClaim { upgrade_tx }),
                Some(metrics_uri),
            )
        } else {
            (None, None, None)
        };

        info!("Start Video avservice for display {display_id} ({key:?})");
        // session_id: None; the StateMachine routes events by `stream_id` to
        // every session whose `video_subscriptions` hold this exact key. For
        // `Shared` that's the multi-client fan-out; for `Exclusive` only the
        // owning session subscribes — and two streams on the same display no
        // longer cross-talk.
        let avservice = avservice::Handler::kymux_start_video(
            StreamId::Video(key.clone()),
            process_factory,
            statemachine_proxy,
            display_id,
            kycom_uri,
            metrics_uri,
            config.bitrate / config.video_stream_count,
            config.codec.clone(),
            config.yuv444,
            config.intra_refresh,
        )
        .await?;

        let dup_for_return = dup.clone();
        self.video_streams.insert(
            key.clone(),
            VideoStream {
                avservice: avservice::SharedHandler::spawn(avservice),
                dup,
                _metrics,
                config,
                client_count: 0,
            },
        );

        Ok((key, dup_for_return, metrics_claim))
    }

    fn subscribe_video(&mut self, key: &VideoStreamKey) {
        if let Some(stream) = self.video_streams.get_mut(key) {
            stream.client_count += 1;
        }
    }

    async fn get_or_create_audio(
        &mut self,
        kycom: &mut kymux::kycom::KyCom,
        topology: AudioTopology,
        process_factory: &Arc<ProcessFactory>,
        statemachine_proxy: StateMachineProxy,
    ) -> Result<(AudioStreamKey, Option<kydup::KydupAudio>)> {
        let key = topology.key();

        let (dup, kycom_uri) = match topology {
            AudioTopology::Shared => {
                if let Some(stream) = self.audio_streams.get_mut(&key) {
                    return Ok((key, stream.dup.clone()));
                }
                let endpoint_id = self.alloc_endpoint_id();
                let (dup, upstream) =
                    kydup::KydupAudio::new(endpoint_id, || kydup::LagAction::Continue);
                let uri = kycom.register_and_forward(upstream)?.url();
                (Some(dup), uri)
            }
            AudioTopology::Exclusive { kycom_uri, .. } => {
                debug_assert!(
                    !self.audio_streams.contains_key(&key),
                    "Exclusive audio stream already exists for {key:?}",
                );
                (None, kycom_uri)
            }
        };

        info!("Start Audio avservice ({key:?})");
        // session_id: None; routed by `stream_id` (the AudioStreamKey). Shared
        // fans out to every audio subscriber; Exclusive reaches only the owning
        // session instead of every audio subscriber as display-less routing did.
        let avservice = avservice::Handler::kymux_start_audio(
            StreamId::Audio(key.clone()),
            process_factory,
            statemachine_proxy,
            kycom_uri,
        )
        .await?;

        let dup_for_return = dup.clone();
        self.audio_streams.insert(
            key.clone(),
            AudioStream {
                avservice: avservice::SharedHandler::spawn(avservice),
                dup,
                client_count: 0,
            },
        );

        Ok((key, dup_for_return))
    }

    fn subscribe_audio(&mut self, key: &AudioStreamKey) {
        if let Some(stream) = self.audio_streams.get_mut(key) {
            stream.client_count += 1;
        }
    }

    /// Acquire a video subscription atomically: get-or-create the entry,
    /// bump its client_count, and return a VideoSubscription whose Drop
    /// signals the reaper to decrement (and stop the avservice when count
    /// hits 0).
    ///
    /// Callers must keep the returned VideoSubscription alive for as long as
    /// they consume the stream; dropping it during partial-failure rollback
    /// cleans up automatically.
    pub(crate) async fn acquire_video(
        &mut self,
        kycom: &mut kymux::kycom::KyCom,
        topology: VideoTopology,
        config: VideoStreamConfig,
        metrics_requested: bool,
        process_factory: &Arc<ProcessFactory>,
        statemachine_proxy: StateMachineProxy,
    ) -> Result<VideoAcquisition> {
        debug_assert!(
            self.reaper_rx.is_none(),
            "AVServicesManager::init must be called before acquire_video"
        );
        let tx = self.reaper_tx.clone();
        let (key, dup, metrics_claim) = self
            .get_or_create_video(
                kycom,
                topology,
                config,
                metrics_requested,
                process_factory,
                statemachine_proxy,
            )
            .await?;
        self.subscribe_video(&key);
        Ok(VideoAcquisition {
            dup,
            metrics_claim,
            subscription: VideoSubscription { key, tx },
        })
    }

    /// Acquire an audio subscription atomically. See `acquire_video`.
    pub(crate) async fn acquire_audio(
        &mut self,
        kycom: &mut kymux::kycom::KyCom,
        topology: AudioTopology,
        process_factory: &Arc<ProcessFactory>,
        statemachine_proxy: StateMachineProxy,
    ) -> Result<AudioAcquisition> {
        debug_assert!(
            self.reaper_rx.is_none(),
            "AVServicesManager::init must be called before acquire_audio"
        );
        let tx = self.reaper_tx.clone();
        let (key, dup) = self
            .get_or_create_audio(kycom, topology, process_factory, statemachine_proxy)
            .await?;
        self.subscribe_audio(&key);
        Ok(AudioAcquisition {
            dup,
            subscription: AudioSubscription { key, tx },
        })
    }

    /// Decrement a video client count, and if it hits zero, remove and
    /// return the stream so the caller can stop it OUTSIDE the Controller
    /// lock via `AVCleanup::run`.
    fn take_video_if_last(&mut self, key: &VideoStreamKey) -> Option<VideoStream> {
        let stream = self.video_streams.get_mut(key)?;
        stream.client_count -= 1;
        if stream.client_count != 0 {
            return None;
        }
        info!("Last client left video {key:?}, stopping avservice");
        self.video_streams.remove(key)
    }

    /// Audio counterpart of `take_video_if_last`.
    fn take_audio_if_last(&mut self, key: &AudioStreamKey) -> Option<AudioStream> {
        let stream = self.audio_streams.get_mut(key)?;
        stream.client_count -= 1;
        if stream.client_count != 0 {
            return None;
        }
        info!("Last client left audio {key:?}, stopping avservice");
        self.audio_streams.remove(key)
    }

    fn alloc_endpoint_id(&mut self) -> u16 {
        let id = self.next_endpoint_id;
        self.next_endpoint_id = self.next_endpoint_id.wrapping_add(1);
        id
    }

    /// Request a fresh keyframe (IDR) on the single video stream identified by
    /// `key`. Unlike the previous display-scoped request, this targets exactly
    /// one stream, so the other streams sharing its display are untouched.
    ///
    /// Used by backends right after wiring a fresh output to give the joining
    /// client a sync point. Under intra-refresh (the default) the encoder emits
    /// no periodic keyframes, so this forced IDR is the only frame a client in
    /// keyframe-sync mode can sync on; with intra-refresh off it merely avoids
    /// waiting for the next periodic keyframe.
    ///
    /// The dispatcher batches requests and forwards them to the avservice
    /// out-of-band, so this method never awaits IPC and is safe to call
    /// while holding the Controller lock. Duplicate requests for the same
    /// stream are collapsed by the dispatcher.
    pub(crate) fn request_idr(&self, key: VideoStreamKey) {
        if let Err(e) = self.idr_tx.try_send(key.clone()) {
            warn!("IDR request dropped for {key:?}: {e}");
        }
    }

    /// An owned handle that requests a fresh IDR for `key` when invoked, usable
    /// from a spawned task (e.g. a kydup post-subscribe hook). Same semantics as
    /// [`request_idr`](Self::request_idr); cloning the dispatcher sender keeps it
    /// cheap and lock-free, and duplicate requests are collapsed downstream.
    pub(crate) fn idr_requester(&self, key: VideoStreamKey) -> impl FnOnce() + Send + 'static {
        let idr_tx = self.idr_tx.clone();
        move || {
            if let Err(e) = idr_tx.try_send(key.clone()) {
                warn!("IDR request dropped for {key:?}: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::{error::TryRecvError, unbounded_channel};

    fn shared_key(display_id: u32) -> VideoStreamKey {
        VideoStreamKey::Shared { display_id }
    }

    #[test]
    fn video_subscription_signals_reaper_on_drop() {
        let (tx, mut rx) = unbounded_channel::<AvReaperMsg>();
        let sub = VideoSubscription {
            key: shared_key(42),
            tx,
        };
        assert_eq!(rx.try_recv().unwrap_err(), TryRecvError::Empty);
        drop(sub);
        match rx.try_recv() {
            Ok(AvReaperMsg::VideoDropped(k)) if k == shared_key(42) => (),
            other => panic!("expected VideoDropped(Shared{{42}}), got {other:?}"),
        }
    }

    #[test]
    fn audio_subscription_signals_reaper_on_drop() {
        let (tx, mut rx) = unbounded_channel::<AvReaperMsg>();
        let sub = AudioSubscription {
            key: AudioStreamKey::Shared,
            tx,
        };
        drop(sub);
        match rx.try_recv() {
            Ok(AvReaperMsg::AudioDropped(AudioStreamKey::Shared)) => (),
            other => panic!("expected AudioDropped(Shared), got {other:?}"),
        }
    }

    /// Each subscription fires its own decrement; they are !Clone, so the
    /// only way to get N signals is to have constructed N handles. This is
    /// the symmetry with acquire_video that keeps client_count honest.
    #[test]
    fn two_video_subscriptions_signal_twice_independently() {
        let (tx, mut rx) = unbounded_channel::<AvReaperMsg>();
        let sub1 = VideoSubscription {
            key: shared_key(1),
            tx: tx.clone(),
        };
        let sub2 = VideoSubscription {
            key: shared_key(2),
            tx,
        };
        drop(sub1);
        drop(sub2);

        let mut seen = vec![];
        while let Ok(msg) = rx.try_recv() {
            match msg {
                AvReaperMsg::VideoDropped(k) => seen.push(k),
                AvReaperMsg::AudioDropped(_) => panic!("unexpected AudioDropped"),
            }
        }
        assert_eq!(seen, vec![shared_key(1), shared_key(2)]);
    }

    /// The main guarantee of the migration: if start_session fails midway
    /// (or panics), in-flight subscription handles drop as stacks unwind and
    /// reach the reaper. No manual rollback needed.
    #[test]
    fn video_subscription_drop_fires_after_panic_unwind() {
        let (tx, mut rx) = unbounded_channel::<AvReaperMsg>();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _sub = VideoSubscription {
                key: shared_key(7),
                tx,
            };
            panic!("simulated partial-failure mid start_session");
        }));
        assert!(result.is_err(), "panic should have been caught");

        match rx.try_recv() {
            Ok(AvReaperMsg::VideoDropped(k)) if k == shared_key(7) => (),
            other => panic!("expected VideoDropped(Shared{{7}}) after unwind, got {other:?}"),
        }
    }

    /// A VideoSubscription whose reaper receiver has already been dropped
    /// must not panic on Drop; the unbounded_channel send returns an error
    /// that is silently swallowed. Guards against ordering issues during
    /// Controller teardown.
    #[test]
    fn video_subscription_drop_is_silent_when_receiver_gone() {
        let (tx, rx) = unbounded_channel::<AvReaperMsg>();
        drop(rx);
        let sub = VideoSubscription {
            key: shared_key(99),
            tx,
        };
        drop(sub); // must not panic
    }

    /// Exclusive keys with different session_ids are distinct entries:
    /// two mono sessions on the same display never collide in the map.
    #[test]
    fn exclusive_keys_with_distinct_sessions_are_distinct() {
        let s1 = Uuid::new_v4();
        let s2 = Uuid::new_v4();
        let k1 = VideoStreamKey::Exclusive {
            session_id: s1,
            display_id: 0,
        };
        let k2 = VideoStreamKey::Exclusive {
            session_id: s2,
            display_id: 0,
        };
        assert_ne!(k1, k2);
        assert!(matches!(
            k1,
            VideoStreamKey::Exclusive { display_id: 0, .. }
        ));
        assert!(matches!(
            k2,
            VideoStreamKey::Exclusive { display_id: 0, .. }
        ));
    }

    /// The crux of the stream-id routing fix: several streams can target the
    /// same display, and their `StreamId`s must stay distinct so an event
    /// tagged for one never matches another's subscription. `display_id` alone
    /// could not tell these apart.
    #[test]
    fn stream_ids_on_same_display_are_distinct() {
        let s1 = Uuid::new_v4();
        let s2 = Uuid::new_v4();
        let shared = StreamId::Video(VideoStreamKey::Shared { display_id: 0 });
        let excl1 = StreamId::Video(VideoStreamKey::Exclusive {
            session_id: s1,
            display_id: 0,
        });
        let excl2 = StreamId::Video(VideoStreamKey::Exclusive {
            session_id: s2,
            display_id: 0,
        });
        // Two exclusive streams on the same display: distinct.
        assert_ne!(excl1, excl2);
        // A shared stream alongside them on the same display: distinct from both.
        assert_ne!(shared, excl1);
        assert_ne!(shared, excl2);

        // Audio: shared vs per-session exclusive are distinct too.
        let a_shared = StreamId::Audio(AudioStreamKey::Shared);
        let a_excl = StreamId::Audio(AudioStreamKey::Exclusive { session_id: s1 });
        assert_ne!(a_shared, a_excl);
    }
}
