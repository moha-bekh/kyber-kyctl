// Project Kyber: mod.rs
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

pub(crate) mod requests;

use std::time::Duration;

use log::{info, warn};
use tokio::time;

use crate::error::{Error, Result};
use crate::utils::TypeConvert;
use crate::{Controller, DataPlaneBackend, avservice, controller, inputservice};

pub async fn generate_video_sdp(
    ctrl: &Controller,
    request: requests::GenerateSdpRequest,
) -> Result<String> {
    avservice::Handler::rtp_generate_video_sdp(
        &ctrl.process_factory,
        request.display_id,
        request.yuv444,
        request.intra_refresh,
    )
    .await
}

pub async fn start(
    ctrl: &Controller,
    session_id: uuid::Uuid,
    request: requests::StartRequest,
) -> Result<()> {
    let displays = ctrl.displays.lock().unwrap().convert();

    // Start services first (before accessing session)
    let mut avservice_video = None;
    let mut avservice_audio = None;
    let mut inputservice_handler = None;

    // SDP file will contain some required parameters on client side.
    if let Some(video_request) = request.video {
        info!(
            "Start video. ip: {} - port: {}",
            request.ip, video_request.port
        );

        let avservice = avservice::Handler::rtp_start_video(
            session_id,
            &ctrl.process_factory,
            ctrl.state_machine_proxy.clone(),
            video_request.display_id,
            request.ip.clone(),
            video_request.port,
            request.bitrate,
            video_request.yuv444,
            video_request.intra_refresh,
        )
        .await?;
        avservice_video = Some(avservice);
    }

    if let Some(config) = request.audio {
        info!("Start audio. ip: {} - port: {}", request.ip, config.port);

        let avservice = avservice::Handler::rtp_start_audio(
            session_id,
            &ctrl.process_factory,
            ctrl.state_machine_proxy.clone(),
            request.ip,
            config.port,
        )
        .await?;
        avservice_audio = Some(avservice);
    }

    if let Some(inputs_request) = request.inputs {
        let host_config = kynputservice_types::HostConfig {
            display_list: displays,
        };

        let service_config = kynputservice_types::ServiceConfig {
            cursor_format: inputs_request.cursor_format,
            gamepad_force_xbox360: inputs_request.gamepad_force_xbox360,
            clipboard: inputs_request.clipboard,
        };

        let (inputservice, _) =
            inputservice::Handler::start_tcp(&ctrl.process_factory, host_config, service_config)
                .await?;

        inputservice_handler = Some(inputservice);
    } else {
        warn!("Input server disabled");
    }

    // Store all services
    let services = ctrl.with_session(&session_id, |s| {
        let Some(streaming_session) = s.session.as_mut() else {
            return Err(Error::ControlPlaneNotOpened);
        };

        streaming_session.data_plane = Some(DataPlaneBackend::Rtp);
        Ok(s.services.clone())
    })?;

    *services.lock().await = controller::StreamingServices {
        video: avservice_video.map_or_else(Vec::new, |s| vec![s]),
        audio: avservice_audio,
        input: inputservice_handler,
        clock_sync: None,
        metrics: None,
        tasks: Vec::new(),
        video_subscriptions: Vec::new(),
        audio_subscription: None,
    };

    time::sleep(Duration::from_millis(100)).await;
    Ok(())
}
