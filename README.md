# node-librespot

Native Node.js bindings for [librespot](https://github.com/librespot-org/librespot) (Spotify Connect) using N-API. Streams PCM directly to JavaScript without spawning the librespot binary.

Used by [lox-audioserver](https://github.com/rudyberends/lox-audioserver) to handle spotify traffic, but can be used by other software.

## Features
- Stream a Spotify track/episode to PCM buffers (`streamTrack`).
- Create credentials from username/password (`loginWithUserPass`) or Zeroconf pairing (`startZeroconfLogin`).
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
import { createSession } from 'node-librespot';

const session = await createSession({
  cacheDir: '/tmp/lox-librespot/zone-1',
  credentialsPath: '/tmp/lox-librespot/zone-1/credentials.json',
  deviceName: 'lox-zone-1',
});

const handle = session.streamTrack(
  { uri: 'spotify:track:...', bitrate: 320 },
  (chunk) => {
    // chunk is a Buffer with PCM s16le frames (44.1kHz stereo)
  },
);

handle.stop();
```

### Connect hosting
```ts
import { startConnectDevice } from 'node-librespot';

const host = await startConnectDevice(
  undefined,              // cacheDir (optional)
  credentialsJsonString,  // inline credentials JSON or path to file
  'Lox-AudioServer',      // publish name
  'lox-zone-1',           // device id
  (chunk) => { /* PCM s16le frames */ },
  (event) => { /* playing/paused/track_changed with enriched metadata */ },
);

host.play();
host.pause();
host.next();
host.prev();
host.stop();
```

## Development / publish
- CI: `.github/workflows/prebuild.yml` runs on release creation, building prebuild artifacts for Linux x64 (glibc), Linux arm64 (glibc), and macOS arm64 before publishing to npm (requires `NPM_TOKEN` secret).
- Local publish flow: `npm run build:prebuild` to generate host prebuild + JS/types, then `npm publish`.
- Install from source: `npm install` will try to use an included prebuild; if none matches the host it will build from source automatically (set `LOX_LIBRESPOT_SKIP_BUILD=1` to skip).
