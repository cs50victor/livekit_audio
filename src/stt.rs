use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use ezsockets::{
    client::ClientCloseMode, ClientConfig, CloseFrame, RawMessage, SocketConfig, WSError,
};
use log::{error, info};
use serde_json::{json, Value};
use tokio::sync::{
    self,
    mpsc::{self},
};

pub struct STT {
    pub stt_tx: mpsc::UnboundedSender<Vec<u8>>,
}

struct WSClient {
    llm_channel_tx: mpsc::UnboundedSender<String>,
}

#[async_trait]
impl ezsockets::ClientExt for WSClient {
    type Call = ();

    async fn on_text(&mut self, text: String) -> Result<(), ezsockets::Error> {
        let data: Value = serde_json::from_str(&text)?;
        let transcript = data["channel"]["alternatives"][0]["transcript"].clone();

        if transcript != Value::Null {
            if let Err(e) = self.llm_channel_tx.send(transcript.to_string()) {
                error!("Error sending to LLM: {}", e);
            };
        }

        Ok(())
    }

    async fn on_binary(&mut self, bytes: Vec<u8>) -> Result<(), ezsockets::Error> {
        info!("received bytes: {bytes:?}");
        Ok(())
    }

    async fn on_call(&mut self, call: Self::Call) -> Result<(), ezsockets::Error> {
        info!("Deepgram ON CALL: {call:?}");
        let () = call;
        Ok(())
    }

    async fn on_connect(&mut self) -> Result<(), ezsockets::Error> {
        info!("Deepgram CONNECTED");
        Ok(())
    }

    async fn on_connect_fail(&mut self, e: WSError) -> Result<ClientCloseMode, ezsockets::Error> {
        error!("Deepgram CONNECTION FAILED | {e}");
        Ok(ClientCloseMode::Reconnect)
    }

    async fn on_close(
        &mut self,
        frame: Option<CloseFrame>,
    ) -> Result<ClientCloseMode, ezsockets::Error> {
        error!("Deepgram CONNECTION CLOSED | {frame:?}");
        Ok(ClientCloseMode::Reconnect)
    }

    async fn on_disconnect(&mut self) -> Result<ClientCloseMode, ezsockets::Error> {
        error!("Deepgram disconnect");
        Ok(ClientCloseMode::Reconnect)
    }
}

impl STT {
    pub const NUM_OF_CHANNELS: u32 = 1;
    pub const SAMPLE_RATE: u32 = 44100;

    pub async fn new(llm_tx: mpsc::UnboundedSender<String>) -> anyhow::Result<Self> {
        let deepgram_api_key = std::env::var("DEEPGRAM_API_KEY").unwrap();

        let config = ClientConfig::new("wss://api.deepgram.com/v1/listen")
            .socket_config(SocketConfig {
                heartbeat: Duration::from_secs(11),
                timeout: Duration::from_secs(30 * 60), // 30 minutes
                heartbeat_ping_msg_fn: Arc::new(|_t: Duration| {
                    // really important
                    RawMessage::Text(
                        json!({
                            "type": "KeepAlive",
                        })
                        .to_string(),
                    )
                }),
            })
            .header("Authorization", &format!("Token {}", deepgram_api_key))
            .query_parameter("model", "nova-2-conversationalai")
            .query_parameter("smart_format", "true")
            .query_parameter("version", "latest")
            .query_parameter("filler_words", "true");

        let (ws_client, _) =
            ezsockets::connect(|_client| WSClient { llm_channel_tx: llm_tx }, config).await;

        let ws_client = Arc::new(ws_client);

        let (stt_tx, mut rx) = sync::mpsc::unbounded_channel::<Vec<u8>>();

        tokio::spawn(async move {
            while let Some(audio_bytes) = rx.recv().await {
                match ws_client.binary(audio_bytes) {
                    Ok(signal) => {
                        signal.status();
                        // log??
                    },
                    Err(e) => error!("error sending audio to deepgram. reason {e}"),
                };
            }
        });

        Ok(Self { stt_tx })
    }
}
