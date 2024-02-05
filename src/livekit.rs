// livekit utils
use anyhow::Result;
use futures::StreamExt as _;
use livekit::{
    options::TrackPublishOptions,
    publication::LocalTrackPublication,
    track::{LocalAudioTrack, LocalTrack, RemoteTrack, TrackSource},
    webrtc::{audio_source::{native::NativeAudioSource, AudioSourceOptions, RtcAudioSource}, audio_stream::native::NativeAudioStream},
    DataPacketKind, Room, RoomError, RoomEvent,
};
use livekit_api::access_token::{AccessToken, VideoGrants};
use log::{error, info, warn};
use rodio::cpal::Sample as _;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::{
    llm::{run_llm, LLM}, stt::STT, tts::TTS, BOT_NAME, LIVEKIT_API_KEY, LIVEKIT_API_SECRET, LIVEKIT_WS_URL,
};

pub const CHAT_PREFIX: &str = "[chat]";

#[derive(Serialize, Deserialize)]
struct RoomText {
    message: String,
    timestamp: i64,
}

pub struct RoomData {
    room: std::sync::Arc<Room>,
    room_events: mpsc::UnboundedReceiver<RoomEvent>,
    audio_src: NativeAudioSource,
    audio_pub: LocalTrackPublication,
}

fn create_bot_token(room_name: String, ai_name: &str) -> Result<String> {
    let api_key = std::env::var(LIVEKIT_API_KEY).unwrap();
    let api_secret = std::env::var(LIVEKIT_API_SECRET).unwrap();

    let ttl = std::time::Duration::from_secs(60 * 5); // 10 minutes (in sync with frontend)
    Ok(AccessToken::with_api_key(api_key.as_str(), api_secret.as_str())
        .with_ttl(ttl)
        .with_identity(ai_name)
        .with_name(ai_name)
        .with_grants(VideoGrants {
            room: room_name,
            room_list: true,
            room_join: true,
            room_admin: true,
            can_publish: true,
            room_record: true,
            can_subscribe: true,
            can_publish_data: true,
            can_update_own_metadata: true,
            ..Default::default()
        })
        .to_jwt()?)
}

async fn publish_audio_tracks(
    room: std::sync::Arc<Room>,
    bot_name: &str,
) -> Result<(NativeAudioSource, LocalTrackPublication), RoomError> {
    let audio_src = NativeAudioSource::new(
        AudioSourceOptions::default(),
        STT::SAMPLE_RATE,
        STT::NUM_OF_CHANNELS,
    );

    let audio_track =
        LocalAudioTrack::create_audio_track(bot_name, RtcAudioSource::Native(audio_src.clone()));

    let audio_publication = room
        .local_participant()
        .publish_track(
            LocalTrack::Audio(audio_track),
            TrackPublishOptions { source: TrackSource::Microphone, ..Default::default() },
        )
        .await;

    let audio_pub = audio_publication?;
    Ok((audio_src, audio_pub))
}

async fn connect_to_livekit_room(room_name: String) -> Result<RoomData> {
    let lvkt_url = std::env::var(LIVEKIT_WS_URL).unwrap();
    let bot_name = BOT_NAME;
    let lvkt_token = create_bot_token(room_name, bot_name)?;

    let room_options = livekit::RoomOptions { ..Default::default() };

    let (room, room_events) = livekit::Room::connect(&lvkt_url, &lvkt_token, room_options).await?;
    let room = std::sync::Arc::new(room);

    info!("Established connection with livekit room. ID -> [{}]", room.name());

    let (audio_src, audio_pub) = publish_audio_tracks(room.clone(), bot_name).await?;

    Ok(RoomData { room, room_events, audio_src, audio_pub })

    /*
    commands.init_resource::<LLMChannel>();
    commands.init_resource::<AudioInputChannel>();
    commands.init_resource::<STT>();
    commands.insert_resource(tts);
    commands.insert_resource(livekit_room);

    */
}

pub async fn handle_room_events(
    mut room_events: mpsc::UnboundedReceiver<RoomEvent>,
    llm_tx: mpsc::UnboundedSender<String>,
    stt_tx: mpsc::UnboundedSender<Vec<u8>>,
) {
    while let Some(event) = room_events.recv().await {
        match event {
            RoomEvent::TrackSubscribed { track, publication:_, participant:_ } => {
                if let RemoteTrack::Audio(audio_track) = track{
                    let audio_rtc_track = audio_track.rtc_track();
                        let mut audio_stream = NativeAudioStream::new(audio_rtc_track);
                        // let audio_should_stop = audio_syncer.should_stop.clone();
                        let stt_tx = stt_tx.clone();
                        tokio::spawn(async move {
                            while let Some(frame) = audio_stream.next().await {
                                // if audio_should_stop.load(Ordering::Relaxed) {
                                //     continue;
                                // }
                                
                                let audio_buffer = frame.data.into_iter().map(|sample| sample.to_sample::<u8>()).collect::<Vec<u8>>();

                                if let Err(e) = stt_tx.send(audio_buffer) {
                                    error!("Couldn't send audio frame to stt {e}");
                                };
                            }
                        });
                }
            },
            // RoomEvent::TrackUnsubscribed { track, publication, participant } => todo!(),
            // RoomEvent::LocalTrackPublished { publication, track, participant } => todo!(),
            // RoomEvent::LocalTrackUnpublished { publication, participant } => todo!(),
            // RoomEvent::Disconnected { reason } => todo!(),
            // RoomEvent::ParticipantConnected(_) => todo!(),
            // RoomEvent::ParticipantDisconnected(_) => todo!(),
            // RoomEvent::TrackPublished { publication, participant } => todo!(),
            // RoomEvent::TrackUnpublished { publication, participant } => todo!(),
            // RoomEvent::TrackMuted { participant, publication } => todo!(),
            // RoomEvent::TrackUnmuted { participant, publication } => todo!(),
            RoomEvent::DataReceived { payload, topic: _, kind, participant : _} => {
                if kind == DataPacketKind::Reliable {
                    if let Some(payload) = payload.as_ascii() {
                        let room_text: serde_json::Result<RoomText> =
                            serde_json::from_str(payload.as_str());
                        match room_text {
                            Ok(room_text) => {
                                if let Err(e) = llm_tx.send(format!("[chat]{} ", room_text.message))
                                {
                                    error!("Couldn't send the text to gpt {e}")
                                };
                            },
                            Err(e) => {
                                warn!("Couldn't deserialize room text. {e:#?}");
                            },
                        }

                        info!("text from room {:#?}", payload.as_str());
                    }
                }
            },
            _ => {},
        }
    }
}

pub async fn join_room_with_ai(room_name: String) -> Result<()> {
    let RoomData { audio_src, audio_pub: _, room:_, room_events } =
        connect_to_livekit_room(room_name).await?;

    let LLM {tx:llm_tx, rx:llm_rx, client} = LLM::new();
    let tts = TTS::new(audio_src).await?;
    let STT{stt_tx} = STT::new(llm_tx.clone()).await?;

    tokio::spawn(run_llm(llm_rx,client,tts));
    tokio::spawn(handle_room_events(room_events,llm_tx,stt_tx));
    Ok(())
}
