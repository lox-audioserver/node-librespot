use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use librespot_core::{
  authentication::Credentials,
  cache::Cache,
  config::SessionConfig,
  session::Session,
  spotify_id::SpotifyId,
};
use librespot_connect::spirc::Spirc;
use librespot_discovery::{DeviceType, Discovery};
use librespot_metadata::{Album, Artist, AudioItem, Metadata, Track};
use librespot_playback::{
  audio_backend::{Sink, SinkResult},
  config::{AudioFormat, Bitrate, PlayerConfig},
  convert::Converter,
  decoder::AudioPacket,
  mixer::{softmixer::SoftMixer, Mixer, MixerConfig, NoOpVolume, VolumeGetter},
  player::{Player, PlayerEvent},
};
use serde_json;
use napi::bindgen_prelude::{Error, Result};
use napi::threadsafe_function::{
  ErrorStrategy, ThreadSafeCallContext, ThreadsafeFunction, ThreadsafeFunctionCallMode,
};
use napi::JsFunction;
use napi_derive::napi;
use tokio::sync::mpsc;
use tokio::time::timeout;
use std::time::Instant;
use std::thread::sleep;
use tokio_stream::StreamExt;

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn runtime() -> &'static tokio::runtime::Runtime {
  RUNTIME.get_or_init(|| {
    tokio::runtime::Runtime::new()
      .expect("failed to create tokio runtime for librespot addon")
  })
}

/// Options used to create a librespot session.
#[napi(object)]
pub struct CreateSessionOpts {
  pub cache_dir: Option<String>,
  pub credentials_path: Option<String>,
  pub credentials_json: Option<String>,
  pub username: Option<String>,
  pub password: Option<String>,
  pub device_name: Option<String>,
}

/// Options for streaming a track.
#[napi(object)]
pub struct StreamTrackOpts {
  pub uri: String,
  pub start_position_ms: Option<u32>,
  pub bitrate: Option<u32>,
  pub output: Option<String>,
  pub emit_events: Option<bool>,
}

/// Result of a credentials login flow.
#[napi(object)]
pub struct CredentialsResult {
  pub username: String,
  pub credentials_json: String,
}

/// Event payload emitted by the connect host.
#[napi(object)]
pub struct ConnectEvent {
  pub r#type: String,
  pub track_id: Option<String>,
  pub uri: Option<String>,
  pub title: Option<String>,
  pub artist: Option<String>,
  pub album: Option<String>,
  pub duration_ms: Option<u32>,
  pub position_ms: Option<u32>,
  pub volume: Option<u16>,
}

/// Handle returned when hosting a connect device (placeholder).
#[napi]
pub struct ConnectHandle {
  spirc: Spirc,
  stop_tx: Option<mpsc::Sender<()>>,
  #[allow(dead_code)]
  tsfn: ThreadsafeFunction<Bytes, ErrorStrategy::Fatal>,
  sample_rate: u32,
  channels: u16,
  task: tokio::task::JoinHandle<()>,
}

#[napi]
impl ConnectHandle {
  #[napi]
  pub fn stop(&mut self) {
    if let Some(tx) = self.stop_tx.take() {
      let _ = tx.try_send(());
    }
    self.spirc.shutdown();
    self.task.abort();
  }

  #[napi]
  pub fn play(&self) {
    self.spirc.play();
  }

  #[napi]
  pub fn pause(&self) {
    self.spirc.pause();
  }

  #[napi]
  pub fn next(&self) {
    self.spirc.next();
  }

  #[napi]
  pub fn prev(&self) {
    self.spirc.prev();
  }

  #[napi(getter)]
  pub fn sample_rate(&self) -> u32 {
    self.sample_rate
  }

  #[napi(getter)]
  pub fn channels(&self) -> u16 {
    self.channels
  }
}

/// Handle to a librespot session.
#[napi]
pub struct LibrespotSession {
  session: Session,
  player_config: PlayerConfig,
  _cache: Arc<Cache>,
}

#[napi]
pub struct StreamHandle {
  stop_tx: Option<mpsc::Sender<()>>,
  #[allow(dead_code)]
  tsfn: ThreadsafeFunction<Bytes, ErrorStrategy::Fatal>,
  sample_rate: u32,
  channels: u16,
  #[allow(dead_code)]
  event_tsfn: Option<ThreadsafeFunction<ConnectEvent, ErrorStrategy::Fatal>>,
}

#[napi]
impl StreamHandle {
  #[napi]
  pub fn stop(&mut self) {
    if let Some(tx) = self.stop_tx.take() {
      let _ = tx.try_send(());
    }
  }

  #[napi(getter)]
  pub fn sample_rate(&self) -> u32 {
    self.sample_rate
  }

  #[napi(getter)]
  pub fn channels(&self) -> u16 {
    self.channels
  }
}

struct ChannelSink {
  tx: mpsc::Sender<Bytes>,
  format: AudioFormat,
  sample_rate: u32,
  channels: u16,
  start: Option<Instant>,
  expected_elapsed: Duration,
}

impl ChannelSink {
  fn new(tx: mpsc::Sender<Bytes>, format: AudioFormat, sample_rate: u32, channels: u16) -> Self {
    Self {
      tx,
      format,
      sample_rate,
      channels,
      start: None,
      expected_elapsed: Duration::from_millis(0),
    }
  }
}

impl Sink for ChannelSink {
  fn start(&mut self) -> SinkResult<()> {
    Ok(())
  }

  fn stop(&mut self) -> SinkResult<()> {
    Ok(())
  }

  fn write(&mut self, packet: AudioPacket, converter: &mut Converter) -> SinkResult<()> {
    let bytes: Bytes = match packet {
      AudioPacket::Samples(samples) => {
        match self.format {
          AudioFormat::S16 => {
            let s16: &[i16] = &converter.f64_to_s16(&samples);
            Bytes::copy_from_slice(bytemuck::cast_slice(s16))
          }
          AudioFormat::F32 => {
            let s32: &[f32] = &converter.f64_to_f32(&samples);
            Bytes::copy_from_slice(bytemuck::cast_slice(s32))
          }
          AudioFormat::S24 => {
            let s24: &[i32] = &converter.f64_to_s24(&samples);
            Bytes::copy_from_slice(bytemuck::cast_slice(s24))
          }
          _ => {
            let s16: &[i16] = &converter.f64_to_s16(&samples);
            Bytes::copy_from_slice(bytemuck::cast_slice(s16))
          }
        }
      }
      AudioPacket::OggData(data) => Bytes::from(data),
    };
    // Pacing: throttle to approximate realtime based on sample count.
    let bytes_per_sample = match self.format {
      AudioFormat::S24 => 3,
      AudioFormat::S32 | AudioFormat::F32 => 4,
      _ => 2,
    };
    let samples = bytes.len() / (bytes_per_sample * self.channels as usize);
    let duration = Duration::from_secs_f64(samples as f64 / self.sample_rate as f64);
    let start = self.start.get_or_insert_with(Instant::now);
    self.expected_elapsed += duration;
    let target = *start + self.expected_elapsed;
    let now = Instant::now();
    let sleep_dur = target.saturating_duration_since(now);

    if !sleep_dur.is_zero() {
      sleep(sleep_dur);
    }

    let _ = self.tx.blocking_send(bytes);
    Ok(())
  }
}

#[napi]
impl LibrespotSession {
  /// Stream a Spotify track or episode as PCM s16le and deliver chunks via a JS callback.
  /// The callback receives Buffer chunks. Returns a handle with a `stop()` method.
  #[napi]
  pub fn stream_track(
    &self,
    opts: StreamTrackOpts,
    on_chunk: JsFunction,
    on_event: Option<JsFunction>,
  ) -> Result<StreamHandle> {
    let uri = opts.uri;
    if uri.is_empty() {
      return Err(Error::from_reason("uri is required"));
    }
    let spotify_id = SpotifyId::from_uri(&uri)
      .map_err(|e| Error::from_reason(format!("invalid spotify uri: {:?}", e)))?;

    let (tx, mut rx) = mpsc::channel::<Bytes>(16);
    let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);

    // Threadsafe function to deliver chunks to JS.
    let tsfn: ThreadsafeFunction<Bytes, ErrorStrategy::Fatal> = on_chunk
      .create_threadsafe_function(0, |ctx: ThreadSafeCallContext<Bytes>| {
        let env = ctx.env;
        let buffer = ctx.value;
        let js_buffer = env.create_buffer_with_data(buffer.to_vec())?;
        Ok(vec![js_buffer.into_unknown()])
      })?;

    // Optional threadsafe function to deliver playback events.
    let event_tsfn: Option<ThreadsafeFunction<ConnectEvent, ErrorStrategy::Fatal>> = on_event
      .map(|f| {
        f.create_threadsafe_function(0, |ctx: ThreadSafeCallContext<ConnectEvent>| {
          let env = ctx.env;
          let val = ctx.value;
          let mut obj = env.create_object()?;
          obj.set_named_property("type", env.create_string(&val.r#type)?)?;
          if let Some(tid) = val.track_id {
            obj.set_named_property("trackId", env.create_string(&tid)?)?;
          }
          if let Some(uri) = val.uri {
            obj.set_named_property("uri", env.create_string(&uri)?)?;
          }
          if let Some(pos) = val.position_ms {
            obj.set_named_property("positionMs", env.create_uint32(pos))?;
          }
          if let Some(dur) = val.duration_ms {
            obj.set_named_property("durationMs", env.create_uint32(dur))?;
          }
          if let Some(vol) = val.volume {
            obj.set_named_property("volume", env.create_uint32(vol as u32))?;
          }
          Ok(vec![obj.into_unknown()])
        })
      })
      .transpose()?;

    // Spawn forwarder task to drain mpsc into TSFN.
    runtime().spawn({
      let tsfn = tsfn.clone();
      async move {
        while let Some(chunk) = rx.recv().await {
          let _ = tsfn.call(chunk, ThreadsafeFunctionCallMode::NonBlocking);
        }
      }
    });

    let mut player_config = self.player_config.clone();
    let session = self.session.clone();

    // Build a playback backend that writes to our channel.
    let backend_factory = {
      let tx_clone = tx.clone();
      move || {
        let sink = ChannelSink::new(tx_clone.clone(), AudioFormat::S16, 44100, 2);
        Box::new(sink) as Box<dyn Sink>
      }
    };

    let start_position_ms = opts.start_position_ms.unwrap_or(0);
    player_config.bitrate = match opts.bitrate {
      Some(96) => Bitrate::Bitrate96,
      Some(160) => Bitrate::Bitrate160,
      _ => Bitrate::Bitrate320,
    };

    // Volume getter (no-op).
    let volume_getter: Box<dyn VolumeGetter + Send> = Box::new(NoOpVolume);

    // Spawn playback task.
    let event_tsfn_clone = event_tsfn.clone();
    runtime().spawn(async move {
      let (mut player, mut event_rx) = Player::new(player_config, session, volume_getter, backend_factory);
      let _ = player.load(spotify_id, true, start_position_ms);

      loop {
        tokio::select! {
          _ = stop_rx.recv() => {
            player.stop();
            break;
          }
          Some(ev) = event_rx.recv(), if opts.emit_events.unwrap_or(true) => {
            if let Some(tsfn_ev) = &event_tsfn_clone {
              let payload = match ev {
                PlayerEvent::Playing { track_id, position_ms, duration_ms, .. } => ConnectEvent {
                  r#type: "playing".into(),
                  track_id: Some(track_id.to_base62().unwrap_or_default()),
                  uri: Some(format!("spotify:track:{}", track_id.to_base62().unwrap_or_default())),
                  title: None,
                  artist: None,
                  album: None,
                  position_ms: Some(position_ms),
                  duration_ms: Some(duration_ms),
                  volume: None,
                },
                PlayerEvent::Paused { track_id, position_ms, duration_ms, .. } => ConnectEvent {
                  r#type: "paused".into(),
                  track_id: Some(track_id.to_base62().unwrap_or_default()),
                  uri: Some(format!("spotify:track:{}", track_id.to_base62().unwrap_or_default())),
                  title: None,
                  artist: None,
                  album: None,
                  position_ms: Some(position_ms),
                  duration_ms: Some(duration_ms),
                  volume: None,
                },
                PlayerEvent::Loading { track_id, position_ms, .. } => ConnectEvent {
                  r#type: "loading".into(),
                  track_id: Some(track_id.to_base62().unwrap_or_default()),
                  uri: Some(format!("spotify:track:{}", track_id.to_base62().unwrap_or_default())),
                  title: None,
                  artist: None,
                  album: None,
                  position_ms: Some(position_ms),
                  duration_ms: None,
                  volume: None,
                },
                PlayerEvent::Started { track_id, position_ms, .. } => ConnectEvent {
                  r#type: "started".into(),
                  track_id: Some(track_id.to_base62().unwrap_or_default()),
                  uri: Some(format!("spotify:track:{}", track_id.to_base62().unwrap_or_default())),
                  title: None,
                  artist: None,
                  album: None,
                  position_ms: Some(position_ms),
                  duration_ms: None,
                  volume: None,
                },
                PlayerEvent::Stopped { track_id, .. } => ConnectEvent {
                  r#type: "stopped".into(),
                  track_id: Some(track_id.to_base62().unwrap_or_default()),
                  uri: Some(format!("spotify:track:{}", track_id.to_base62().unwrap_or_default())),
                  title: None,
                  artist: None,
                  album: None,
                  position_ms: None,
                  duration_ms: None,
                  volume: None,
                },
                PlayerEvent::EndOfTrack { track_id, .. } => ConnectEvent {
                  r#type: "end_of_track".into(),
                  track_id: Some(track_id.to_base62().unwrap_or_default()),
                  uri: Some(format!("spotify:track:{}", track_id.to_base62().unwrap_or_default())),
                  title: None,
                  artist: None,
                  album: None,
                  position_ms: None,
                  duration_ms: None,
                  volume: None,
                },
                PlayerEvent::VolumeSet { volume } => ConnectEvent {
                  r#type: "volume".into(),
                  track_id: None,
                  uri: None,
                  title: None,
                  artist: None,
                  album: None,
                  position_ms: None,
                  duration_ms: None,
                  volume: Some(volume),
                },
                _ => continue,
              };
              let _ = tsfn_ev.call(payload, ThreadsafeFunctionCallMode::NonBlocking);
            }
          }
        }
      }
    });

    Ok(StreamHandle {
      stop_tx: Some(stop_tx),
      tsfn,
      sample_rate: 44100,
      channels: 2,
      event_tsfn,
    })
  }

  #[napi]
  pub async fn close(&self) -> Result<()> {
    Ok(())
  }
}

/// Create a session from stored credentials or username/password.
#[napi]
pub async fn create_session(opts: CreateSessionOpts) -> Result<LibrespotSession> {
  if opts.username.is_none() && opts.credentials_path.is_none() && opts.credentials_json.is_none() {
    return Err(Error::from_reason(
      "provide credentials_path or username/password",
    ));
  }

  let credentials = if let Some(raw) = opts.credentials_json.clone() {
    serde_json::from_str::<Credentials>(&raw)
      .map_err(|e| Error::from_reason(format!("invalid credentials json: {e}")))?
  } else if let Some(path) = opts.credentials_path.clone() {
    if !Path::new(&path).exists() {
      return Err(Error::from_reason(format!(
        "credentials file not found: {}",
        path
      )));
    }
    let mut file = File::open(path).map_err(|e| Error::from_reason(format!("{e}")))?;
    let mut buf = String::new();
    file
      .read_to_string(&mut buf)
      .map_err(|e| Error::from_reason(format!("{e}")))?;
    serde_json::from_str::<Credentials>(&buf)
      .map_err(|e| Error::from_reason(format!("invalid credentials json: {e}")))?
  } else {
    Credentials::with_password(
      opts.username.unwrap_or_default(),
      opts.password.unwrap_or_default(),
    )
  };

  let cache = Arc::new(
    Cache::new::<std::path::PathBuf>(None, None, None, None)
      .map_err(|e| Error::from_reason(format!("cache init failed: {e}")))?,
  );
  let mut session_config = SessionConfig::default();
  if let Some(name) = opts.device_name {
    session_config.device_id = name;
  }

  let (session, _) = Session::connect(session_config, credentials, Some((*cache).clone()), false)
    .await
    .map_err(|e| Error::from_reason(format!("session connect failed: {e}")))?;

  let player_config = PlayerConfig::default();

  Ok(LibrespotSession {
    session,
    player_config,
    _cache: cache,
  })
}

/// Start a Spotify Connect device. Placeholder implementation that returns an error until
/// full connect hosting is wired.
#[napi]
pub fn start_connect_device(
  _cache_dir: Option<String>,
  credentials_path: String,
  name: String,
  device_id: String,
  on_chunk: JsFunction,
  on_event: Option<JsFunction>,
) -> Result<ConnectHandle> {
  if credentials_path.trim().is_empty() {
    return Err(Error::from_reason("credentials payload is required"));
  }

  let tsfn: ThreadsafeFunction<Bytes, ErrorStrategy::Fatal> = on_chunk
    .create_threadsafe_function(0, |ctx: ThreadSafeCallContext<Bytes>| {
      let env = ctx.env;
      let buffer = ctx.value;
      let js_buffer = env.create_buffer_with_data(buffer.to_vec())?;
      Ok(vec![js_buffer.into_unknown()])
    })?;
  let event_tsfn: Option<ThreadsafeFunction<ConnectEvent, ErrorStrategy::Fatal>> = on_event
    .map(|f| {
      f.create_threadsafe_function(0, |ctx: ThreadSafeCallContext<ConnectEvent>| {
        let env = ctx.env;
        let val = ctx.value;
        // Serialize ConnectEvent into a JS object.
        let mut obj = env.create_object()?;
        obj.set_named_property("type", env.create_string(&val.r#type)?)?;
        if let Some(tid) = val.track_id {
          obj.set_named_property("trackId", env.create_string(&tid)?)?;
        }
        if let Some(uri) = val.uri {
          obj.set_named_property("uri", env.create_string(&uri)?)?;
        }
        if let Some(title) = val.title {
          obj.set_named_property("title", env.create_string(&title)?)?;
        }
        if let Some(artist) = val.artist {
          obj.set_named_property("artist", env.create_string(&artist)?)?;
        }
        if let Some(album) = val.album {
          obj.set_named_property("album", env.create_string(&album)?)?;
        }
        if let Some(pos) = val.position_ms {
          obj.set_named_property("positionMs", env.create_uint32(pos))?;
        }
        if let Some(dur) = val.duration_ms {
          obj.set_named_property("durationMs", env.create_uint32(dur))?;
        }
        if let Some(vol) = val.volume {
          obj.set_named_property("volume", env.create_uint32(vol as u32))?;
        }
        Ok(vec![obj.into_unknown()])
      })
    })
    .transpose()?;

  runtime().block_on(async move {
    // Treat credentials_path as either a file path or an inline JSON blob.
    let credentials: Credentials = if Path::new(&credentials_path).exists() {
      let mut file = File::open(&credentials_path).map_err(|e| Error::from_reason(format!("{e}")))?;
      let mut buf = String::new();
      file
        .read_to_string(&mut buf)
        .map_err(|e| Error::from_reason(format!("{e}")))?;
      serde_json::from_str(&buf)
        .map_err(|e| Error::from_reason(format!("invalid credentials json: {e}")))?
    } else {
      serde_json::from_str(&credentials_path)
        .map_err(|e| Error::from_reason(format!("invalid credentials json: {e}")))?  
    };

    let cache = Arc::new(
      Cache::new::<std::path::PathBuf>(None, None, None, None)
        .map_err(|e| Error::from_reason(format!("cache init failed: {e}")))?,
    );
    let mut session_config = SessionConfig::default();
    session_config.device_id = device_id.clone();

    let (session, _) =
      Session::connect(session_config, credentials, Some(cache.as_ref().clone()), true)
        .await
        .map_err(|e| Error::from_reason(format!("session connect failed: {e}")))?;

    let connect_config = librespot_core::config::ConnectConfig {
      name: name.clone(),
      device_type: DeviceType::Speaker,
      // Start with full volume so we rely on zone-side volume control; we do not sync Spotify volume.
      // Spotify volume scale is 0..65535; use max to avoid muted start.
      initial_volume: Some(u16::MAX),
      has_volume_ctrl: true,
      autoplay: false,
    };

    let player_config = PlayerConfig::default();
    let mixer = SoftMixer::open(MixerConfig::default());
    let volume_getter = mixer.get_soft_volume();

    let (tx, mut rx) = mpsc::channel::<Bytes>(32);

    runtime().spawn({
      let tsfn = tsfn.clone();
      async move {
        while let Some(chunk) = rx.recv().await {
          let _ = tsfn.call(chunk, ThreadsafeFunctionCallMode::NonBlocking);
        }
      }
    });

    let sink_builder = {
      let tx_clone = tx.clone();
      move || {
        let sink = ChannelSink::new(tx_clone.clone(), AudioFormat::S16, 44100, 2);
        Box::new(sink) as Box<dyn Sink>
      }
    };

    let (player, _rx) = Player::new(player_config, session.clone(), volume_getter, sink_builder);
    // Forward player events to JS if requested.
    if let Some(tsfn_ev) = event_tsfn.clone() {
      let mut ev_rx = player.get_player_event_channel();
      let session_for_events = session.clone();
      runtime().spawn(async move {
        while let Some(ev) = ev_rx.recv().await {
          let mut payload = match ev {
            PlayerEvent::Playing {
              track_id,
              position_ms,
              duration_ms,
              ..
            } => ConnectEvent {
              r#type: "playing".into(),
              track_id: Some(track_id.to_base62().unwrap_or_default()),
              uri: Some(format!("spotify:track:{}", track_id.to_base62().unwrap_or_default())),
              title: None,
              artist: None,
              album: None,
              position_ms: Some(position_ms),
              duration_ms: Some(duration_ms),
              volume: None,
            },
            PlayerEvent::Paused {
              track_id,
              position_ms,
              duration_ms,
              ..
            } => ConnectEvent {
              r#type: "paused".into(),
              track_id: Some(track_id.to_base62().unwrap_or_default()),
              uri: Some(format!("spotify:track:{}", track_id.to_base62().unwrap_or_default())),
              title: None,
              artist: None,
              album: None,
              position_ms: Some(position_ms),
              duration_ms: Some(duration_ms),
              volume: None,
            },
            PlayerEvent::Loading { track_id, position_ms, .. } => ConnectEvent {
              r#type: "loading".into(),
              track_id: Some(track_id.to_base62().unwrap_or_default()),
              uri: Some(format!("spotify:track:{}", track_id.to_base62().unwrap_or_default())),
              title: None,
              artist: None,
              album: None,
              position_ms: Some(position_ms),
              duration_ms: None,
              volume: None,
            },
            PlayerEvent::Started { track_id, position_ms, .. } => ConnectEvent {
              r#type: "started".into(),
              track_id: Some(track_id.to_base62().unwrap_or_default()),
              uri: Some(format!("spotify:track:{}", track_id.to_base62().unwrap_or_default())),
              title: None,
              artist: None,
              album: None,
              position_ms: Some(position_ms),
              duration_ms: None,
              volume: None,
            },
            PlayerEvent::Stopped { track_id, .. } => ConnectEvent {
              r#type: "stopped".into(),
              track_id: Some(track_id.to_base62().unwrap_or_default()),
              uri: Some(format!("spotify:track:{}", track_id.to_base62().unwrap_or_default())),
              title: None,
              artist: None,
              album: None,
              position_ms: None,
              duration_ms: None,
              volume: None,
            },
            PlayerEvent::EndOfTrack { track_id, .. } => ConnectEvent {
              r#type: "end_of_track".into(),
              track_id: Some(track_id.to_base62().unwrap_or_default()),
              uri: Some(format!("spotify:track:{}", track_id.to_base62().unwrap_or_default())),
              title: None,
              artist: None,
              album: None,
              position_ms: None,
              duration_ms: None,
              volume: None,
            },
            PlayerEvent::VolumeSet { volume } => ConnectEvent {
              r#type: "volume".into(),
              track_id: None,
              uri: None,
              title: None,
              artist: None,
              album: None,
              position_ms: None,
              duration_ms: None,
              volume: Some(volume),
            },
            _ => continue,
          };

          // Best-effort metadata enrichment.
          if payload.title.is_none() || payload.duration_ms.is_none() || payload.uri.is_none() {
            if let Some(tid) = payload.track_id.clone() {
              if let Ok(spotify_id) = SpotifyId::from_base62(&tid) {
                if let Ok(item) = AudioItem::get_audio_item(&session_for_events, spotify_id).await {
                  payload.title = Some(item.name);
                  payload.duration_ms = Some(item.duration as u32);
                  payload.uri = Some(item.uri);
                }
              }
            }
          }

          // Enrich artist/album names if missing.
          if payload.artist.is_none() || payload.album.is_none() {
            if let Some(tid) = payload.track_id.clone() {
              if let Ok(spotify_id) = SpotifyId::from_base62(&tid) {
                if let Ok(track_meta) = Track::get(&session_for_events, spotify_id).await {
                  if payload.album.is_none() {
                    if let Ok(album_meta) = Album::get(&session_for_events, track_meta.album).await {
                      payload.album = Some(album_meta.name);
                    }
                  }
                  if payload.artist.is_none() {
                    if let Some(first_artist) = track_meta.artists.first() {
                      if let Ok(artist_meta) = Artist::get(&session_for_events, *first_artist).await {
                        payload.artist = Some(artist_meta.name);
                      }
                    }
                  }
                }
              }
            }
          }

          let _ = tsfn_ev.call(payload, ThreadsafeFunctionCallMode::NonBlocking);
        }
      });
    }

    let (spirc, spirc_task) = Spirc::new(connect_config, session, player, Box::new(mixer));

    let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
    let task_handle = runtime().spawn(async move {
      tokio::select! {
        _ = spirc_task => {},
        _ = stop_rx.recv() => {},
      }
    });

  Ok(ConnectHandle {
    spirc,
    stop_tx: Some(stop_tx),
    tsfn,
    sample_rate: 44100,
    channels: 2,
    task: task_handle,
  })
  })
}

/// Perform username/password login and return the credentials JSON blob.
#[napi]
pub async fn login_with_user_pass(
  username: String,
  password: String,
  cache_dir: Option<String>,
  device_name: Option<String>,
) -> Result<CredentialsResult> {
  if username.trim().is_empty() || password.trim().is_empty() {
    return Err(Error::from_reason("username and password are required"));
  }
  let cache = cache_dir
    .map(|_| {
      Cache::new::<std::path::PathBuf>(None, None, None, None)
        .map_err(|e| Error::from_reason(format!("cache init failed: {e}")))
    })
    .transpose()?;

  let mut session_config = SessionConfig::default();
  if let Some(name) = device_name {
    session_config.device_id = name;
  }
  let credentials = Credentials::with_password(username.clone(), password);

  let (session, creds) = Session::connect(
    session_config,
    credentials,
    cache.clone().map(|c| c.clone()),
    true,
  )
  .await
  .map_err(|e| Error::from_reason(format!("session connect failed: {e}")))?;

  // Explicitly drop session to stop background tasks after acquiring credentials.
  drop(session);

  let credentials_json =
    serde_json::to_string_pretty(&creds).map_err(|e| Error::from_reason(format!("{e}")))?;

  Ok(CredentialsResult {
    username: creds.username.clone(),
    credentials_json,
  })
}

/// Start a Zeroconf login and return the credentials blob once a user connects in the Spotify app.
#[napi]
pub async fn start_zeroconf_login(
  device_id: String,
  name: Option<String>,
  timeout_ms: Option<u32>,
) -> Result<CredentialsResult> {
  let device_id = device_id.trim();
  if device_id.is_empty() {
    return Err(Error::from_reason("device_id is required"));
  }
  let mut discovery = Discovery::builder(device_id.to_string())
    .name(name.unwrap_or_else(|| "LoxAudio".into()))
    .device_type(DeviceType::Speaker)
    .launch()
    .map_err(|e| Error::from_reason(format!("zeroconf launch failed: {e}")))?;

  let wait_for = timeout_ms.unwrap_or(120_000); // default 120s
  let creds = timeout(Duration::from_millis(wait_for as u64), async {
    discovery.next().await
  })
  .await
  .map_err(|_| Error::from_reason("zeroconf login timed out"))?
  .ok_or_else(|| Error::from_reason("zeroconf ended without credentials"))?;

  let credentials_json =
    serde_json::to_string_pretty(&creds).map_err(|e| Error::from_reason(format!("{e}")))?;

  Ok(CredentialsResult {
    username: creds.username.clone(),
    credentials_json,
  })
}
