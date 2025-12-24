/* Auto-generated from the addon shape; keep in sync with N-API exports. */

/** Options used to create a librespot session. */
export interface CreateSessionOpts {
  credentialsPath?: string;
  credentialsJson?: string;
  username?: string;
  password?: string;
  deviceName?: string;
}

/** Options for streaming a track. */
export interface StreamTrackOpts {
  uri: string;
  startPositionMs?: number;
  bitrate?: number;
  output?: string;
  emitEvents?: boolean;
}

/** Result of a credentials login flow. */
export interface CredentialsResult {
  username: string;
  credentialsJson: string;
  credentials_json?: string;
}

/** Helper signature for OAuth-based credentials minting. */
export type LoginWithAccessToken = (
  accessToken: string,
  deviceName?: string,
) => Promise<CredentialsResult>;

export type LibrespotEventType =
  | 'playing'
  | 'paused'
  | 'loading'
  | 'stopped'
  | 'end_of_track'
  | 'unavailable'
  | 'volume'
  | 'position_correction'
  | 'health'
  | 'error'
  | 'metric'
  | 'preloading'
  | 'time_to_preload'
  | 'play_request_id'
  | 'other';

export type LibrespotErrorCode =
  | 'audio_key_error'
  | 'no_pcm'
  | 'end_of_track'
  | 'pcm_missing'
  | 'pcm_stalled'
  | 'pcm_ok'
  | 'unavailable'
  | 'unknown';

/** Event payload emitted by the connect host. */
export interface ConnectEvent {
  type: LibrespotEventType;
  deviceId?: string;
  sessionId?: string;
  trackId?: string;
  uri?: string;
  title?: string;
  artist?: string;
  album?: string;
  durationMs?: number;
  positionMs?: number;
  volume?: number;
  errorCode?: LibrespotErrorCode;
  errorMessage?: string;
  metricName?: string;
  metricValueMs?: number;
  metricMessage?: string;
}

/** Log payload emitted by the native module. */
export interface LogEvent {
  level: string;
  message: string;
  scope?: string;
  deviceId?: string;
  sessionId?: string;
}

/** Handle returned when hosting a connect device. */
export interface ConnectHandle {
  stop(): void;
  shutdown(): void;
  close(): void;
  play(): void;
  pause(): void;
  next(): void;
  prev(): void;
  sampleRate: number;
  channels: number;
}

/** Handle to a librespot session. */
export interface LibrespotSession {
  streamTrack(
    opts: StreamTrackOpts,
    onChunk: (chunk: Buffer) => void,
    onEvent?: (event: ConnectEvent) => void,
    onLog?: (event: LogEvent) => void,
  ): StreamHandle;
  close(): Promise<void>;
}

export interface StreamHandle {
  stop(): void;
  sampleRate: number;
  channels: number;
}

export declare function setLogLevel(level: string): void;
