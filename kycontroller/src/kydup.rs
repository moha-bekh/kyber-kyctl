// Project Kyber: kydup.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0
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

//! Kydup: AV stream duplicator for multi-client fan-out.
//!
//! Kydup exposes itself as a kyproto-style endpoint that absorbs an AV stream
//! and broadcasts it to N outputs. Two flavors: [`KydupVideo`] (keyframe sync,
//! used for video) and [`KydupAudio`] (immediate sync, used for audio).
//!
//! The API mirrors `kycom`:
//!
//! ```ignore
//! // Mono (no fan-out, no kydup): the producer writes to the client
//! // endpoint directly via kycom.
//! let (_id, endpoint) = connection.register_video_endpoint(proto).await?;
//! let uri = kycom.register_and_forward(endpoint)?.url();
//!
//! // Multi (with kydup fan-out): the producer writes to a kydup upstream
//! // endpoint via kycom; kydup broadcasts to N client endpoints.
//! let (kydup, upstream) = KydupVideo::new(|| LagAction::Continue);
//! let uri = kycom.register_and_forward(upstream)?.url();
//! for client_endpoint in clients {
//!     kydup.register_and_forward(client_endpoint, || { /* runs once subscribed */ });
//! }
//! ```

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use kymux::types::{
    self, AVPacket, AudioServerEndpoint, AudioServerProtocol, ProtocolEndpoint,
    ProtocolEndpointDriver, ProtocolError, ProtocolSend, ProtocolSendDriver, VideoServerEndpoint,
    VideoServerProtocol,
};
use log::{debug, error, warn};
use tokio::sync::{Notify, broadcast};

use crate::utils::Task;

/// Broadcast ring capacity, in retained packets: the hard backlog at which an
/// output is dropped by the broadcast (a `Lagged` error and a re-sync). Kept
/// above [`BACKPRESSURE_THRESHOLD`] so the gate always acts first, and small
/// so a transient stall is ridden out without unbounded memory growth.
const DEFAULT_CAPACITY: usize = 32;

/// Backpressure threshold: the slowest output's backlog, in packets, at which
/// the upstream `send()` blocks until it drains, pausing the whole shared
/// stream. Kept below [`DEFAULT_CAPACITY`] so active outputs never reach the
/// broadcast's lag point.
const BACKPRESSURE_THRESHOLD: usize = 20;

/// Fallback wakeup for the backpressure gate: it is normally woken by
/// [`GatedReceiver`], this only guards against a missed wakeup.
const BACKPRESSURE_GATE_FALLBACK: Duration = Duration::from_secs(1);

/// Time budget for an output's endpoint readiness handshake.
const OUTPUT_READY_TIMEOUT: Duration = Duration::from_secs(5);

// ===========================================================================
// Public types
// ===========================================================================

/// What an output should do when the broadcast lags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LagAction {
    /// Re-sync: skip the backlog and wait for the next keyframe (kydup never
    /// requests one itself).
    Continue,
    /// Terminate the output.
    //
    // Unused today; kept so both policies stay expressible.
    #[allow(dead_code)]
    Stop,
}

/// Whether a fresh output must wait for a keyframe before forwarding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncMode {
    /// Wait for a keyframe before forwarding (video). Kydup never requests
    /// one itself.
    Keyframe,
    /// Send cached headers then forward immediately (audio).
    Immediate,
}

// ===========================================================================
// KydupVideo
// ===========================================================================

/// Kydup variant for video: keyframe-synced fan-out.
#[derive(Clone)]
pub struct KydupVideo {
    tx: broadcast::Sender<AVBroadcast>,
    lag_handler: Arc<dyn Fn() -> LagAction + Send + Sync>,
    /// Shared with the upstream send driver and every output; wakes the
    /// backpressure gate.
    notify: Arc<Notify>,
}

impl KydupVideo {
    /// Returns the kydup handle and the **upstream endpoint** that absorbs the
    /// stream to broadcast.
    ///
    /// `lag_handler`: called when an output lags, never at first sync; see
    /// [`LagAction`].
    pub fn new(
        lag_handler: impl Fn() -> LagAction + Send + Sync + 'static,
    ) -> (Self, VideoServerEndpoint) {
        let (tx, _) = broadcast::channel(DEFAULT_CAPACITY);
        let notify = Arc::new(Notify::new());
        let kydup = Self {
            tx: tx.clone(),
            lag_handler: Arc::new(lag_handler),
            notify: Arc::clone(&notify),
        };
        let endpoint = ProtocolEndpoint::new(KydupVideoUpstreamDriver {
            tx: Some(tx),
            notify,
        });
        (kydup, endpoint)
    }

    /// Wire a client endpoint as an output of this kydup.
    ///
    /// Subscribes to the broadcast only after the endpoint's readiness
    /// handshake, so a connecting output never holds the gate (see
    /// [`spawn_output_lazy`]). The returned [`Task`] aborts the output on
    /// drop: store it for as long as the output should live.
    ///
    /// `on_subscribed` runs right after the subscribe: from that point this
    /// output sees every broadcast packet, so a keyframe triggered from there
    /// cannot be missed.
    pub fn register_and_forward(
        &self,
        endpoint: VideoServerEndpoint,
        on_subscribed: impl FnOnce() + Send + 'static,
    ) -> Task {
        spawn_output_lazy(
            self.tx.clone(),
            Arc::clone(&self.notify),
            SyncMode::Keyframe,
            Arc::clone(&self.lag_handler),
            ready_with_timeout(endpoint),
            on_subscribed,
        )
    }

    /// Add an output from an already-ready `ProtocolSend`: subscribes (and
    /// counts toward the gate) immediately.
    #[cfg(test)]
    pub fn add_output(&self, send: ProtocolSend<AVPacket>) -> Task {
        spawn_output(
            self.tx.subscribe(),
            Arc::clone(&self.notify),
            SyncMode::Keyframe,
            Arc::clone(&self.lag_handler),
            async move { Ok(send) },
        )
    }

    #[cfg(test)]
    pub fn output_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

struct KydupVideoUpstreamDriver {
    /// `take`n by `ready_boxed`. Still `Some` in [`Drop`] means the endpoint
    /// was never ready'd, so `Shutdown` must be sent from there or subscribers
    /// would wait forever.
    tx: Option<broadcast::Sender<AVBroadcast>>,
    notify: Arc<Notify>,
}

#[async_trait]
impl ProtocolEndpointDriver for KydupVideoUpstreamDriver {
    type Protocol = VideoServerProtocol;

    async fn ready_boxed(mut self: Box<Self>) -> Result<Self::Protocol, ProtocolError> {
        let tx = self
            .tx
            .take()
            .expect("KydupVideoUpstreamDriver::ready_boxed called twice");
        Ok(ProtocolSend::new(KydupBroadcastSendDriver::new(
            tx,
            Arc::clone(&self.notify),
        )))
    }
}

impl Drop for KydupVideoUpstreamDriver {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(AVBroadcast::Shutdown);
        }
    }
}

// ===========================================================================
// KydupAudio
// ===========================================================================

/// Kydup variant for audio: immediate fan-out (no keyframe wait).
#[derive(Clone)]
pub struct KydupAudio {
    tx: broadcast::Sender<AVBroadcast>,
    lag_handler: Arc<dyn Fn() -> LagAction + Send + Sync>,
    /// See [`KydupVideo::notify`].
    notify: Arc<Notify>,
}

impl KydupAudio {
    /// See [`KydupVideo::new`].
    pub fn new(
        lag_handler: impl Fn() -> LagAction + Send + Sync + 'static,
    ) -> (Self, AudioServerEndpoint) {
        let (tx, _) = broadcast::channel(DEFAULT_CAPACITY);
        let notify = Arc::new(Notify::new());
        let kydup = Self {
            tx: tx.clone(),
            lag_handler: Arc::new(lag_handler),
            notify: Arc::clone(&notify),
        };
        let endpoint = ProtocolEndpoint::new(KydupAudioUpstreamDriver {
            tx: Some(tx),
            notify,
        });
        (kydup, endpoint)
    }

    /// See [`KydupVideo::register_and_forward`]; audio needs no post-subscribe
    /// hook.
    pub fn register_and_forward(&self, endpoint: AudioServerEndpoint) -> Task {
        spawn_output_lazy(
            self.tx.clone(),
            Arc::clone(&self.notify),
            SyncMode::Immediate,
            Arc::clone(&self.lag_handler),
            ready_with_timeout(endpoint),
            || {},
        )
    }

    /// See [`KydupVideo::add_output`].
    #[cfg(test)]
    pub fn add_output(&self, send: ProtocolSend<AVPacket>) -> Task {
        spawn_output(
            self.tx.subscribe(),
            Arc::clone(&self.notify),
            SyncMode::Immediate,
            Arc::clone(&self.lag_handler),
            async move { Ok(send) },
        )
    }

    #[cfg(test)]
    pub fn output_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

struct KydupAudioUpstreamDriver {
    /// See [`KydupVideoUpstreamDriver::tx`].
    tx: Option<broadcast::Sender<AVBroadcast>>,
    notify: Arc<Notify>,
}

#[async_trait]
impl ProtocolEndpointDriver for KydupAudioUpstreamDriver {
    type Protocol = AudioServerProtocol;

    async fn ready_boxed(mut self: Box<Self>) -> Result<Self::Protocol, ProtocolError> {
        let tx = self
            .tx
            .take()
            .expect("KydupAudioUpstreamDriver::ready_boxed called twice");
        Ok(ProtocolSend::new(KydupBroadcastSendDriver::new(
            tx,
            Arc::clone(&self.notify),
        )))
    }
}

impl Drop for KydupAudioUpstreamDriver {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(AVBroadcast::Shutdown);
        }
    }
}

// ===========================================================================
// Upstream send driver: receives packets, caches headers, broadcasts
// ===========================================================================

/// Receives upstream packets, caches header packets, and broadcasts each
/// packet with a snapshot of the headers active at that moment. The snapshot
/// (instead of a shared mutable cache) prevents an output from pairing a
/// packet with headers updated after it was produced.
struct KydupBroadcastSendDriver {
    tx: broadcast::Sender<AVBroadcast>,
    headers: Arc<HeaderPackets>,
    notify: Arc<Notify>,
}

impl KydupBroadcastSendDriver {
    fn new(tx: broadcast::Sender<AVBroadcast>, notify: Arc<Notify>) -> Self {
        Self {
            tx,
            headers: Arc::new(HeaderPackets::default()),
            notify,
        }
    }

    /// Block the upstream push until the slowest output's backlog
    /// (`broadcast::Sender::len()`) drops below [`BACKPRESSURE_THRESHOLD`]: a
    /// lagging output throttles the whole shared stream.
    async fn await_backpressure(&self) {
        loop {
            if self.tx.len() < BACKPRESSURE_THRESHOLD {
                return;
            }
            let notified = self.notify.notified();
            tokio::pin!(notified);
            // Arm before re-reading so a wakeup racing in right now is
            // captured rather than lost between the check and the await.
            notified.as_mut().enable();
            if self.tx.len() < BACKPRESSURE_THRESHOLD {
                return;
            }
            tokio::select! {
                _ = notified.as_mut() => {}
                _ = tokio::time::sleep(BACKPRESSURE_GATE_FALLBACK) => {}
            }
        }
    }
}

#[async_trait]
impl ProtocolSendDriver for KydupBroadcastSendDriver {
    type Packet = AVPacket;

    async fn send(&mut self, packet: AVPacket) -> Result<(), ProtocolError> {
        self.await_backpressure().await;
        if is_header(&packet) {
            let mut updated = (*self.headers).clone();
            cache_header(&mut updated, &packet);
            self.headers = Arc::new(updated);
        }
        let _ = self
            .tx
            .send(AVBroadcast::Data(packet, Arc::clone(&self.headers)));
        Ok(())
    }
}

impl Drop for KydupBroadcastSendDriver {
    fn drop(&mut self) {
        // End-of-stream signal; without it outputs would idle until the kydup
        // itself drops.
        let _ = self.tx.send(AVBroadcast::Shutdown);
    }
}

// ===========================================================================
// AV internals: shared between video and audio outputs
// ===========================================================================

/// Broadcast item: packet + snapshot of the headers cached when it was sent.
/// See [`KydupBroadcastSendDriver`] for the race the snapshot avoids.
#[derive(Clone)]
enum AVBroadcast {
    Data(AVPacket, Arc<HeaderPackets>),
    Shutdown,
}

#[derive(Default, Clone)]
struct HeaderPackets {
    codec: Option<AVPacket>,
    config: Option<AVPacket>,
    version: u64,
}

fn is_header(packet: &AVPacket) -> bool {
    matches!(packet, AVPacket::Codec(_))
        || matches!(packet, AVPacket::Media(m) if m.header.is_config)
}

fn cache_header(headers: &mut HeaderPackets, packet: &AVPacket) {
    match packet {
        AVPacket::Codec(_) => {
            headers.codec = Some(packet.clone());
            headers.config = None;
            headers.version += 1;
        }
        AVPacket::Media(media) if media.header.is_config => {
            headers.config = Some(packet.clone());
            headers.version += 1;
        }
        _ => {}
    }
}

async fn ready_with_timeout<P: Send + 'static>(
    endpoint: ProtocolEndpoint<P>,
) -> Result<P, ProtocolError> {
    match tokio::time::timeout(OUTPUT_READY_TIMEOUT, endpoint.ready()).await {
        Ok(res) => res,
        Err(_) => Err(types::ProtocolError::new(
            "Kydup output endpoint ready timed out",
        )),
    }
}

/// Broadcast receiver wrapper: every `recv()` wakes the gated upstream
/// `send()`; `Drop` wakes it too, so a departed output no longer holds the
/// stream back.
struct GatedReceiver {
    rx: broadcast::Receiver<AVBroadcast>,
    notify: Arc<Notify>,
}

impl GatedReceiver {
    fn new(rx: broadcast::Receiver<AVBroadcast>, notify: Arc<Notify>) -> Self {
        Self { rx, notify }
    }

    async fn recv(&mut self) -> Result<AVBroadcast, broadcast::error::RecvError> {
        let res = self.rx.recv().await;
        self.notify.notify_one();
        res
    }
}

impl Drop for GatedReceiver {
    fn drop(&mut self) {
        self.notify.notify_one();
    }
}

/// Drive one **already-subscribed** output: await `ready_send`, then run the
/// sync/forward/lag loop until shutdown, send error, or [`LagAction::Stop`].
/// Handshaking outputs must use [`spawn_output_lazy`] instead.
#[cfg(test)]
fn spawn_output(
    rx: broadcast::Receiver<AVBroadcast>,
    notify: Arc<Notify>,
    sync_mode: SyncMode,
    lag_handler: Arc<dyn Fn() -> LagAction + Send + Sync>,
    ready_send: impl Future<Output = Result<ProtocolSend<AVPacket>, ProtocolError>> + Send + 'static,
) -> Task {
    Task::spawn(async move {
        let mut rx = GatedReceiver::new(rx, notify);
        let send = match ready_send.await {
            Ok(s) => s,
            Err(e) => {
                error!("Kydup AV output ready failed: {e}");
                return;
            }
        };
        if let Err(e) = run_av_output(&mut rx, send, sync_mode, lag_handler).await {
            error!("Kydup AV output error: {e}");
        }
    })
}

/// Like [`spawn_output`], but subscribes only **after** the readiness
/// handshake: a still-connecting output would otherwise count toward the gate
/// and could freeze the shared stream for the whole handshake.
///
/// `on_subscribed` runs right after the subscribe: from that point the output
/// sees every broadcast packet.
fn spawn_output_lazy(
    tx: broadcast::Sender<AVBroadcast>,
    notify: Arc<Notify>,
    sync_mode: SyncMode,
    lag_handler: Arc<dyn Fn() -> LagAction + Send + Sync>,
    ready_send: impl Future<Output = Result<ProtocolSend<AVPacket>, ProtocolError>> + Send + 'static,
    on_subscribed: impl FnOnce() + Send + 'static,
) -> Task {
    Task::spawn(async move {
        let send = match ready_send.await {
            Ok(s) => s,
            Err(e) => {
                error!("Kydup AV output ready failed: {e}");
                return;
            }
        };
        // Dropping our `Sender` clone is hygiene only: outputs terminate on
        // the explicit `Shutdown` message, not on channel closure.
        let mut rx = GatedReceiver::new(tx.subscribe(), notify);
        drop(tx);
        on_subscribed();
        if let Err(e) = run_av_output(&mut rx, send, sync_mode, lag_handler).await {
            error!("Kydup AV output error: {e}");
        }
    })
}

enum SyncOutcome {
    Synced,
    Shutdown,
}

enum DesyncReason {
    Lagged(u64),
    Closed,
    Error,
    Shutdown,
}

/// Drive an output: initial sync per `sync_mode`, then forward until desync;
/// on lag, `lag_handler` picks the [`LagAction`]. Keyframe sync re-sends the
/// cached headers before every keyframe; Immediate skips them when unchanged.
async fn run_av_output(
    rx: &mut GatedReceiver,
    mut send: ProtocolSend<AVPacket>,
    sync_mode: SyncMode,
    lag_handler: Arc<dyn Fn() -> LagAction + Send + Sync>,
) -> Result<(), ProtocolError> {
    let mut last_headers_version = 0u64;

    loop {
        match wait_for_sync(rx, &mut send, sync_mode, &mut last_headers_version).await? {
            SyncOutcome::Synced => {}
            SyncOutcome::Shutdown => break,
        }

        match forward_until_desync(rx, &mut send).await {
            DesyncReason::Lagged(skipped) => {
                warn!("Kydup AV output lagged, skipped {skipped} packets");
                match lag_handler() {
                    LagAction::Continue => {
                        // fall through to wait_for_sync again
                    }
                    LagAction::Stop => break,
                }
            }
            DesyncReason::Closed | DesyncReason::Error | DesyncReason::Shutdown => break,
        }
    }

    Ok(())
}

async fn wait_for_sync(
    rx: &mut GatedReceiver,
    send: &mut ProtocolSend<AVPacket>,
    sync_mode: SyncMode,
    last_headers_version: &mut u64,
) -> Result<SyncOutcome, ProtocolError> {
    match sync_mode {
        SyncMode::Keyframe => wait_for_keyframe(rx, send, last_headers_version).await,
        SyncMode::Immediate => wait_for_first_packet(rx, send, last_headers_version).await,
    }
}

async fn wait_for_keyframe(
    rx: &mut GatedReceiver,
    send: &mut ProtocolSend<AVPacket>,
    last_headers_version: &mut u64,
) -> Result<SyncOutcome, ProtocolError> {
    loop {
        match rx.recv().await {
            Ok(AVBroadcast::Shutdown) => return Ok(SyncOutcome::Shutdown),
            Ok(AVBroadcast::Data(AVPacket::Media(media), headers)) if media.header.is_key => {
                send_headers(send, &headers, last_headers_version).await?;
                send.send(AVPacket::Media(media)).await?;
                return Ok(SyncOutcome::Synced);
            }
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => return Ok(SyncOutcome::Shutdown),
        }
    }
}

/// Immediate-mode sync: emit the first data packet, preceded by its attached
/// headers if they changed.
async fn wait_for_first_packet(
    rx: &mut GatedReceiver,
    send: &mut ProtocolSend<AVPacket>,
    last_headers_version: &mut u64,
) -> Result<SyncOutcome, ProtocolError> {
    loop {
        match rx.recv().await {
            Ok(AVBroadcast::Shutdown) => return Ok(SyncOutcome::Shutdown),
            Ok(AVBroadcast::Data(packet, headers)) => {
                send_headers_if_changed(send, &headers, last_headers_version).await?;
                send.send(packet).await?;
                return Ok(SyncOutcome::Synced);
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => return Ok(SyncOutcome::Shutdown),
        }
    }
}

async fn send_headers(
    send: &mut ProtocolSend<AVPacket>,
    headers: &HeaderPackets,
    last_version: &mut u64,
) -> Result<(), ProtocolError> {
    if let Some(codec) = &headers.codec {
        send.send(codec.clone()).await?;
    }
    if let Some(config) = &headers.config {
        send.send(config.clone()).await?;
    }
    *last_version = headers.version;
    Ok(())
}

async fn send_headers_if_changed(
    send: &mut ProtocolSend<AVPacket>,
    headers: &HeaderPackets,
    last_version: &mut u64,
) -> Result<(), ProtocolError> {
    if headers.version == *last_version {
        return Ok(());
    }
    send_headers(send, headers, last_version).await
}

async fn forward_until_desync(
    rx: &mut GatedReceiver,
    send: &mut ProtocolSend<AVPacket>,
) -> DesyncReason {
    loop {
        match rx.recv().await {
            Ok(AVBroadcast::Data(packet, _)) => {
                if let Err(e) = send.send(packet).await {
                    debug!("Kydup AV output send error: {e}");
                    return DesyncReason::Error;
                }
            }
            Ok(AVBroadcast::Shutdown) => return DesyncReason::Shutdown,
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                return DesyncReason::Lagged(skipped);
            }
            Err(broadcast::error::RecvError::Closed) => return DesyncReason::Closed,
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use kymux::types::{
        CodecPacket, CodecPacketHeader, MediaPacket, MediaPacketHeader, ProtocolSendDriver,
    };
    use std::sync::{Arc, Mutex};

    /// Drives the upstream of a kydup as if it were behind a kycom forwarder:
    /// awaits `ready` on the upstream endpoint, then `send`s each packet from
    /// `pkts` into the resulting `ProtocolSend`.
    async fn pump(
        endpoint: ProtocolEndpoint<ProtocolSend<AVPacket>>,
        pkts: Vec<AVPacket>,
    ) -> Result<(), ProtocolError> {
        let mut send = endpoint.ready().await?;
        for p in pkts {
            send.send(p).await?;
        }
        Ok(())
    }

    struct CollectSend(Arc<Mutex<Vec<AVPacket>>>);
    #[async_trait]
    impl ProtocolSendDriver for CollectSend {
        type Packet = AVPacket;
        async fn send(&mut self, packet: AVPacket) -> Result<(), ProtocolError> {
            self.0.lock().unwrap().push(packet);
            Ok(())
        }
    }

    fn collect_output() -> (ProtocolSend<AVPacket>, Arc<Mutex<Vec<AVPacket>>>) {
        let pkts = Arc::new(Mutex::new(Vec::new()));
        (ProtocolSend::new(CollectSend(Arc::clone(&pkts))), pkts)
    }

    fn codec(id: u32) -> AVPacket {
        AVPacket::Codec(CodecPacket {
            header: CodecPacketHeader {
                codec: id,
                rotation: 0,
                frame_size: 0,
            },
        })
    }

    fn config(data: &[u8]) -> AVPacket {
        AVPacket::Media(MediaPacket {
            header: MediaPacketHeader {
                is_config: true,
                is_key: false,
                pts: 0,
                size: data.len() as u32,
            },
            payload: Bytes::copy_from_slice(data),
        })
    }

    fn keyframe(pts: u64, data: &[u8]) -> AVPacket {
        AVPacket::Media(MediaPacket {
            header: MediaPacketHeader {
                is_config: false,
                is_key: true,
                pts,
                size: data.len() as u32,
            },
            payload: Bytes::copy_from_slice(data),
        })
    }

    fn media(pts: u64, data: &[u8]) -> AVPacket {
        AVPacket::Media(MediaPacket {
            header: MediaPacketHeader {
                is_config: false,
                is_key: false,
                pts,
                size: data.len() as u32,
            },
            payload: Bytes::copy_from_slice(data),
        })
    }

    #[tokio::test]
    async fn av_basic_forwarding() {
        let (kydup, upstream) = KydupVideo::new(|| LagAction::Continue);
        let (send, pkts) = collect_output();
        let oh = kydup.add_output(send);

        pump(
            upstream,
            vec![
                codec(1),
                config(b"sps"),
                keyframe(0, b"idr"),
                media(33, b"p1"),
            ],
        )
        .await
        .unwrap();
        oh.await;

        let p = pkts.lock().unwrap();
        assert_eq!(p.len(), 4);
        assert!(matches!(&p[0], AVPacket::Codec(c) if c.header.codec == 1));
        assert!(matches!(&p[1], AVPacket::Media(m) if m.header.is_config));
        assert!(matches!(&p[2], AVPacket::Media(m) if m.header.is_key));
        assert!(matches!(&p[3], AVPacket::Media(m) if m.header.pts == 33));
    }

    /// A client attaching after the stream has started must wait for the next
    /// keyframe, then receive cached headers + that keyframe + subsequent
    /// media. We pump enough packets to drain the broadcast before adding the
    /// output, then keep pumping; only post-keyframe packets land.
    #[tokio::test]
    async fn av_late_joiner() {
        let (kydup, upstream) = KydupVideo::new(|| LagAction::Continue);

        // First batch (pre-output): headers + early keyframe + early media,
        // all consumed by the broadcast and dropped because nothing is
        // subscribed yet.
        let mut proto = upstream.ready().await.unwrap();
        proto.send(codec(1)).await.unwrap();
        proto.send(config(b"sps")).await.unwrap();
        proto.send(keyframe(0, b"idr0")).await.unwrap();
        proto.send(media(33, b"p1")).await.unwrap();

        let (collect, pkts) = collect_output();
        let oh = kydup.add_output(collect);

        // Second batch: new keyframe; output picks up from there.
        proto.send(keyframe(100, b"idr1")).await.unwrap();
        proto.send(media(133, b"p2")).await.unwrap();
        drop(proto);
        oh.await;

        let p = pkts.lock().unwrap();
        assert_eq!(p.len(), 4); // codec, config, keyframe(100), media(133)
        assert!(matches!(&p[2], AVPacket::Media(m) if m.header.is_key && m.header.pts == 100));
    }

    /// Audio (Immediate mode): a fresh output starts on the next packet,
    /// emitting cached headers if any.
    #[tokio::test]
    async fn av_immediate_mode() {
        let (kydup, upstream) = KydupAudio::new(|| LagAction::Continue);

        let mut proto = upstream.ready().await.unwrap();
        proto.send(codec(1)).await.unwrap();
        proto.send(config(b"cfg")).await.unwrap();

        let (collect, pkts) = collect_output();
        let oh = kydup.add_output(collect);

        proto.send(media(10, b"a1")).await.unwrap();
        drop(proto);
        oh.await;

        let p = pkts.lock().unwrap();
        assert_eq!(p.len(), 3); // codec, config, media
    }

    #[tokio::test]
    async fn av_multiple_outputs() {
        let (kydup, upstream) = KydupVideo::new(|| LagAction::Continue);

        let (s1, p1) = collect_output();
        let (s2, p2) = collect_output();
        let h1 = kydup.add_output(s1);
        let h2 = kydup.add_output(s2);

        pump(
            upstream,
            vec![
                codec(1),
                config(b"sps"),
                keyframe(0, b"idr"),
                media(33, b"p"),
            ],
        )
        .await
        .unwrap();
        h1.await;
        h2.await;

        assert_eq!(p1.lock().unwrap().len(), 4);
        assert_eq!(p2.lock().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn av_output_count() {
        let (kydup, upstream) = KydupAudio::new(|| LagAction::Continue);
        assert_eq!(kydup.output_count(), 0);

        let (s1, _) = collect_output();
        let h1 = kydup.add_output(s1);
        assert_eq!(kydup.output_count(), 1);

        let (s2, _) = collect_output();
        let h2 = kydup.add_output(s2);
        assert_eq!(kydup.output_count(), 2);

        // Drop upstream to end the stream; outputs see Shutdown via the
        // broadcast Drop on KydupBroadcastSendDriver.
        drop(upstream);
        h1.await;
        h2.await;
        assert_eq!(kydup.output_count(), 0);
    }

    #[tokio::test]
    async fn av_codec_change_resets_config() {
        let mut headers = HeaderPackets::default();
        cache_header(&mut headers, &codec(1));
        cache_header(&mut headers, &config(b"sps1"));
        assert!(headers.config.is_some());

        cache_header(&mut headers, &codec(2));
        assert!(headers.config.is_none());
        assert!(matches!(&headers.codec, Some(AVPacket::Codec(c)) if c.header.codec == 2));
    }

    /// A sink whose `send` blocks until `gate` releases a permit, modelling a
    /// client that stops consuming. With zero permits the output stalls, its
    /// broadcast backlog grows, and the upstream backpressure gate must engage.
    struct BlockingSink {
        gate: Arc<tokio::sync::Semaphore>,
        received: Arc<Mutex<Vec<AVPacket>>>,
    }
    #[async_trait]
    impl ProtocolSendDriver for BlockingSink {
        type Packet = AVPacket;
        async fn send(&mut self, packet: AVPacket) -> Result<(), ProtocolError> {
            self.gate.acquire().await.unwrap().forget();
            self.received.lock().unwrap().push(packet);
            Ok(())
        }
    }

    /// Pumps `codec, config, keyframe` then `total_media` media packets into an
    /// upstream, returning the join handle. Kept alive `VideoServerProtocol` is
    /// returned so the caller controls when the broadcast shuts down.
    fn spawn_pump(
        mut proto: VideoServerProtocol,
        total_media: usize,
    ) -> tokio::task::JoinHandle<VideoServerProtocol> {
        tokio::spawn(async move {
            proto.send(codec(1)).await.unwrap();
            proto.send(config(b"sps")).await.unwrap();
            proto.send(keyframe(0, b"idr")).await.unwrap();
            for i in 0..total_media {
                proto.send(media(i as u64, b"p")).await.unwrap();
            }
            proto
        })
    }

    /// A stalled client must block the upstream push for everyone once its
    /// backlog reaches the threshold, then release it once it drains.
    #[tokio::test]
    async fn av_backpressure_blocks_until_slow_drains() {
        let (kydup, upstream) = KydupVideo::new(|| LagAction::Continue);

        // Fast output consumes immediately.
        let (fast, fast_pkts) = collect_output();
        let fast_h = kydup.add_output(fast);

        // Slow output blocks on its first send (zero permits).
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let slow_pkts = Arc::new(Mutex::new(Vec::new()));
        let slow = ProtocolSend::new(BlockingSink {
            gate: Arc::clone(&gate),
            received: Arc::clone(&slow_pkts),
        });
        let slow_h = kydup.add_output(slow);

        let total_media = BACKPRESSURE_THRESHOLD + 10;
        let proto = upstream.ready().await.unwrap();
        let pump = spawn_pump(proto, total_media);

        // While the slow client is stalled, the gate must stop the push before
        // the whole stream is sent: neither the pump nor the fast client finish.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!pump.is_finished(), "push must block while a client lags");
        assert!(
            fast_pkts.lock().unwrap().len() < 3 + total_media,
            "fast client must be throttled to the slow client's pace"
        );

        // Let the slow client drain: the gate reopens and the push completes.
        gate.add_permits(3 + total_media);
        let proto = tokio::time::timeout(std::time::Duration::from_secs(5), pump)
            .await
            .expect("push must finish once the slow client drains")
            .unwrap();
        drop(proto);
        fast_h.await;
        slow_h.await;

        assert_eq!(fast_pkts.lock().unwrap().len(), 3 + total_media);
        assert_eq!(slow_pkts.lock().unwrap().len(), 3 + total_media);
    }

    /// Dropping a lagging output must release the global backpressure so the
    /// remaining clients keep streaming.
    #[tokio::test]
    async fn av_backpressure_releases_when_slow_output_dropped() {
        let (kydup, upstream) = KydupVideo::new(|| LagAction::Continue);

        let (fast, fast_pkts) = collect_output();
        let fast_h = kydup.add_output(fast);

        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let slow = ProtocolSend::new(BlockingSink {
            gate: Arc::clone(&gate),
            received: Arc::new(Mutex::new(Vec::new())),
        });
        let slow_h = kydup.add_output(slow);

        let total_media = BACKPRESSURE_THRESHOLD + 10;
        let proto = upstream.ready().await.unwrap();
        let pump = spawn_pump(proto, total_media);

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!pump.is_finished(), "push must block while a client lags");

        // Dropping the stalled output removes its receiver; the gate reopens.
        drop(slow_h);

        let proto = tokio::time::timeout(std::time::Duration::from_secs(5), pump)
            .await
            .expect("push must resume once the lagging client is dropped")
            .unwrap();
        drop(proto);
        fast_h.await;

        assert_eq!(fast_pkts.lock().unwrap().len(), 3 + total_media);
    }
}
