use std::time::Duration;

use async_openai::{
    config::OpenAIConfig,
    types::{
        ChatCompletionRequestUserMessageArgs, ChatCompletionRequestUserMessageContent,
        CreateChatCompletionRequestArgs,
    },
    Client,
};
use futures::StreamExt as _;
use log::{error, warn};
use tokio::{
    sync::{self, mpsc},
    time::Instant,
};

use crate::{tts::TTS, OPENAI_ORG_ID};

pub struct LLM {
    pub tx: sync::mpsc::UnboundedSender<String>,
    pub rx: sync::mpsc::UnboundedReceiver<String>,
    pub client: Client<OpenAIConfig>,
}

impl LLM {
    pub fn new() -> Self {
        let open_ai_org_id = std::env::var(OPENAI_ORG_ID).unwrap();

        let (tx, rx) = sync::mpsc::unbounded_channel::<String>();

        let openai_client = async_openai::Client::with_config(
            async_openai::config::OpenAIConfig::new().with_org_id(open_ai_org_id),
        );

        Self { tx, rx, client: openai_client }
    }
}

pub async fn run_llm(
    mut text_input_rx: mpsc::UnboundedReceiver<String>,
    openai_client: Client<OpenAIConfig>,
    mut tts_client: TTS,
) -> anyhow::Result<()> {
    let splitters = ['.', ',', '?', '!', ';', ':', '—', '-', '(', ')', '[', ']', '}', ' '];
    let mut txt_buffer = String::new();
    let mut tts_buffer = String::new();
    let mut last_text_send_time = Instant::now();
    let text_latency = Duration::from_millis(500);

    let mut req_args = CreateChatCompletionRequestArgs::default();
    let openai_req = req_args.model("gpt-4-1106-preview").max_tokens(512u16);

    // let text_latency = Duration::from_millis(500);
    while let Some(chunk) = text_input_rx.recv().await {
        txt_buffer.push_str(&chunk);

        if ends_with_splitter(&splitters, &txt_buffer)
            && last_text_send_time.elapsed() >= text_latency
        {
            warn!("GPT ABOUT TO RECEIVE - {txt_buffer}");
            let request = openai_req
                .messages([ChatCompletionRequestUserMessageArgs::default()
                    .content(ChatCompletionRequestUserMessageContent::Text(txt_buffer.clone()))
                    .build()?
                    .into()])
                .build()?;

            let mut gpt_resp_stream = openai_client.chat().create_stream(request).await?;
            while let Some(result) = gpt_resp_stream.next().await {
                match result {
                    Ok(response) => {
                        for chat_choice in response.choices {
                            if let Some(content) = chat_choice.delta.content {
                                tts_buffer.push_str(&content);
                                if ends_with_splitter(&splitters, &tts_buffer) {
                                    if let Err(e) = tts_client.send(tts_buffer.clone()) {
                                        error!("Coudln't send gpt text chunk to tts channel - {e}");
                                    } else {
                                        tts_buffer.clear();
                                    };
                                }
                            };
                        }
                    },
                    Err(err) => {
                        warn!("chunk error: {err:#?}");
                    },
                }
            }
            txt_buffer.clear();
            last_text_send_time = Instant::now();
        } else if !txt_buffer.ends_with(' ') {
            txt_buffer.push(' ');
        }
    }
    Ok(())
}

fn ends_with_splitter(splitters: &[char], chunk: &str) -> bool {
    !chunk.is_empty() && chunk != " " && splitters.iter().any(|&splitter| chunk.ends_with(splitter))
}
