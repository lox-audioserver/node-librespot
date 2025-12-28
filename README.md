# node-librespot

Native Node.js bindings for [librespot](https://github.com/librespot-org/librespot) (Spotify Connect) using N-API. Streams PCM directly to JavaScript without spawning the librespot binary.

Used by [lox-audioserver](https://github.com/rudyberends/lox-audioserver) to handle spotify traffic, but can be used by other software.

## Features
- Stream a Spotify track/episode to PCM buffers (`streamTrack`).
- Create credentials from OAuth access tokens (`loginWithAccessToken`) or Zeroconf pairing (`startZeroconfLogin`).
- Connect hosting (`startConnectDevice`) with basic controls/events, inline credentials, and metadata enrichment (title/artist/album) without touching disk or cache directories.
- Starts Connect hosts at max volume (we keep volume control on the consumer side).

## Install
```bash
npm install
```

## Build
Requires Rust (stable) and `@napi-rs/cli` (installed via devDependencies).
```bash
npm run build          # release build
# or
npm run build:debug    # debug build
# or (for publishing) generate host prebuild + JS/types
npm run build:prebuild
```

Runtime will load `librespot_addon.node` from `prebuilds/<platform-arch>/` when available, otherwise it falls back to `LOX_LIBRESPOT_ADDON_PATH` or the locally compiled `dist/librespot_addon.node`.

## Usage
```ts
import { createSession, setLogLevel } from 'node-librespot';

setLogLevel('debug'); // off|error|warn|info|debug|trace

const session = await createSession({
  credentialsJson,
  deviceName: 'lox-zone-1',
});

const handle = session.streamTrack(
  { uri: 'spotify:track:...', bitrate: 320 },
  (chunk) => {
    // chunk is a Buffer with PCM s16le frames (44.1kHz stereo)
  },
  (event) => {
    // optional events: playing/paused/loading/stopped/end_of_track/error/health/metric
    // error codes include: audio_key_error, no_pcm, end_of_track, pcm_missing, pcm_stalled, unavailable
    // metric names include: first_pcm_ms, buffer_stall_ms, decode_error
  },
  (log) => {
    // optional logging callback { level, message, scope }
  },
);

// Or pull decrypted audio file bytes (Ogg/MP3) without decoding:
const download = await downloadTrack(
  { uri: 'spotify:track:...', bitrate: 320 },
  (chunk) => {
    // chunk is a Buffer with decrypted compressed audio data
  },
  (log) => {
    // optional logging callback { level, message, scope }
  },
);
// download.stop() to cancel
// Promise rejects on initial errors (invalid URI, unavailable track, key/file fetch failure).

handle.stop();
```

## OAuth credentials
This module can mint librespot credentials from a Spotify OAuth access token via `loginWithAccessToken`.
You bring your own OAuth flow (e.g. Authorization Code with PKCE) and pass the **user** access token here.

Key points:
- The access token is only used to bootstrap librespot credentials; it is not stored by this module.
- The returned `credentialsJson` is a serialized librespot credential blob. Store it and reuse it for playback
  and Connect hosting. You do not need a cache directory.
- If the access token is invalid/expired, `loginWithAccessToken` will fail. In that case, fall back to
  `startZeroconfLogin` to re-pair and then replace stored credentials.

Example:
```ts
import { loginWithAccessToken, createSession } from 'node-librespot';

const { credentialsJson, username } = await loginWithAccessToken(accessToken, 'lox-zone-1');

// Persist credentialsJson for reuse, then create a session:
const session = await createSession({
  credentialsJson,
  deviceName: 'lox-zone-1',
});
```

### Connect hosting
```ts
import { startConnectDevice } from 'node-librespot';

const host = await startConnectDevice(
  credentialsJsonString,  // inline credentials JSON or path to file
  'Lox-AudioServer',      // publish name
  'lox-zone-1',           // device id
  (chunk) => { /* PCM s16le frames */ },
  (event) => { /* playing/paused/loading/end_of_track/error/health/metric with metadata */ },
  (log) => { /* optional log callback */ },
);

host.play();
host.pause();
host.next();
host.prev();
host.stop();
host.shutdown(); // alias for stop()
host.close();    // alias for stop()
```

## Development / publish
- CI: `.github/workflows/prebuild.yml` runs on release creation, building prebuild artifacts for Linux x64 (glibc), Linux arm64 (glibc), and macOS arm64 before publishing to npm (requires `NPM_TOKEN` secret).
- Local publish flow: `npm run build:prebuild` to generate host prebuild + JS/types, then `npm publish`.
- Install from source: `npm install` will try to use an included prebuild; if none matches the host it will build from source automatically (set `LOX_LIBRESPOT_SKIP_BUILD=1` to skip).
