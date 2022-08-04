mod enums;
mod error;
pub mod event_manager;
mod host_service;
pub mod input_service;
mod player;
pub mod window_handle;

#[cfg(any(cargo_c, feature = "capi"))]
pub mod capi;

pub use enums::Event;

use std::sync::{Arc, Mutex};
use std::thread;

use log::{info, warn};
use tokio::runtime;
use tokio::sync::mpsc::{self, Sender};

use libkynput_rs::Pipeline;

use crate::error::{Error, Result};
use enums::Message;
use event_manager::EventManager;
use window_handle::WindowHandle;

const RTP_PORT: u16 = 30000;

#[derive(Clone)]
pub struct MessageProxy {
    sender: Sender<Message>,
}

impl MessageProxy {
    pub fn send(&self, msg: Message) -> Result<()> {
        self.sender.blocking_send(msg)?;
        Ok(())
    }

    pub async fn send_async(&self, msg: Message) -> Result<()> {
        self.sender.send(msg).await?;
        Ok(())
    }
}

struct Handler {
    config: ClientConfig,
    event_manager: Arc<Mutex<EventManager>>,
    sender: Sender<Message>,

    player: Option<player::Player>,
    input_service: Option<input_service::Client>,
}

impl Handler {
    fn new(
        config: ClientConfig,
        event_manager: Arc<Mutex<EventManager>>,
        sender: Sender<Message>,
    ) -> Self {
        Self {
            config,
            event_manager,
            sender,
            player: None,
            input_service: None,
        }
    }

    fn create_message_proxy(&self) -> MessageProxy {
        MessageProxy {
            sender: self.sender.clone(),
        }
    }

    async fn start(&mut self) -> Result<()> {
        let local_ip = host_service::hello(&self.config.streamer_ip).await?;

        let sdp_param =
            host_service::generate_sdp(&self.config.streamer_ip, self.config.yuv444).await?;

        let mut player =
            player::Player::new(self.config.window_handle, self.create_message_proxy())?;
        player.play(&local_ip, RTP_PORT, sdp_param)?;

        host_service::start_stream(
            &self.config.streamer_ip,
            &local_ip,
            RTP_PORT,
            self.config.yuv444,
            self.config.inputs,
        )
        .await?;

        self.player = Some(player);

        if self.config.inputs {
            let input_service = input_service::Client::new(
                &self.config.streamer_ip,
                self.create_message_proxy(),
                self.config.input_pipeline.get_input_sink(),
            )?;
            self.input_service = Some(input_service);
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(player) = self.player.as_mut() {
            player.stop_async().await?;
        }

        if let Some(input_service) = self.input_service.as_mut() {
            input_service.stop().await?;
        }

        let ret = host_service::stop_stream(&self.config.streamer_ip).await;
        if let Err(err) = ret {
            warn!(
                "Stop stream rejected by controller ({:?}). Stop client anyway.",
                err
            );
        }

        let mut event_manager = self.event_manager.lock()?;
        event_manager.send_event(Event::Stopped);

        Ok(())
    }

    async fn handle_message(&mut self, msg: &Message) -> Result<()> {
        match msg {
            Message::Start => self.start().await,
            Message::Stop => self.stop().await,
            Message::PlayerStarted => {
                info!("Player started");
                Ok(())
            }
            Message::PlayerStopped => {
                info!("Player stopped");
                Ok(())
            }
            Message::InputServiceConnected => {
                info!("Input service connected");

                let input_service = self.input_service.as_mut().ok_or(Error::NoInputService)?;
                self.config
                    .input_pipeline
                    .plug_network(&input_service.get_tx_consumer())?;

                self.config.input_pipeline.start()?;

                Ok(())
            }
            Message::InputServiceDisconnected => {
                info!("Input service disconnected");

                self.config.input_pipeline.stop()?;
                self.input_service = None;

                Ok(())
            }
        }
    }
}

#[derive(Clone)]
pub struct ClientConfig {
    pub streamer_ip: String,
    pub window_handle: WindowHandle,
    pub input_pipeline: Pipeline,
    pub yuv444: bool,
    pub inputs: bool,
}

pub struct Client {
    event_manager: Arc<Mutex<EventManager>>,
    sender: Sender<Message>,
}

impl Client {
    pub fn new(config: ClientConfig) -> Self {
        let event_manager = Arc::new(Mutex::new(EventManager::new()));

        let (sender, mut receiver) = mpsc::channel(32);
        let event_manager_handler = event_manager.clone();
        let sender_handler = sender.clone();

        thread::spawn(move || {
            let rt = runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("EventLoop")
                .enable_io()
                .enable_time()
                .build()
                .unwrap();

            rt.block_on(async move {
                let mut handler = Handler::new(config, event_manager_handler, sender_handler);

                loop {
                    let msg = receiver.recv().await;
                    if let Some(msg) = msg.as_ref() {
                        let ret = handler.handle_message(msg).await;
                        if let Err(err) = ret {
                            info!(
                                "Processing of message {:?} failed with error {:?}",
                                msg, err
                            );
                        }
                    } else {
                        break;
                    }
                }
            });
        });

        Self {
            event_manager,
            sender,
        }
    }

    pub fn start(&self) -> Result<()> {
        self.sender.blocking_send(Message::Start)?;
        Ok(())
    }

    pub fn stop(&self) -> Result<()> {
        self.sender.blocking_send(Message::Stop)?;
        Ok(())
    }

    pub fn get_event_manager(&self) -> Arc<Mutex<EventManager>> {
        self.event_manager.clone()
    }
}
