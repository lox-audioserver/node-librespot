/* Auto-generated from the addon shape; keep in sync with N-API exports. */

/** Options used to create a librespot session. */
export interface CreateSessionOpts {
  cacheDir?: string;
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

/** Event payload emitted by the connect host. */
export interface ConnectEvent {
  type: string;
  trackId?: string;
  uri?: string;
  title?: string;
  artist?: string;
  album?: string;
  durationMs?: number;
  positionMs?: number;
  volume?: number;
}

/** Handle returned when hosting a connect device. */
export interface ConnectHandle {
  stop(): void;
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
    onEvent?: (event: any) => void,
  ): StreamHandle;
  close(): Promise<void>;
}

export interface StreamHandle {
  stop(): void;
  sampleRate: number;
  channels: number;
}
