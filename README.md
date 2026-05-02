# cokemusic-extractor

A fork of [igorlira/dirplayer-rs](https://github.com/igorlira/dirplayer-rs)
that adds tooling for extracting bitmap art and structural metadata from
the original CokeMusic / Coke Studios Macromedia Director cast files
(`.cct` and `.dcr`). Used to regenerate room backgrounds, foreground
overlays, sprite atlases, and per-member metadata for fan recreations
of the abandoned-but-not-forgotten Coca-Cola social game.

> **Status:** WIP. Paths are env-var-driven; the four bitmap dumpers
> compile and run; documentation for outside consumers is still sparse.
> This fork is being lifted out of a private project in stages — see
> the parent project's `PLAN-publish-toolchain.md` for the roadmap.

## What this fork adds on top of dirplayer-rs

**MCP API additions** (`src/mcp/`):

- `mcp_get_cast_member_picture(cast_lib, cast_member)` — render a
  Bitmap cast member to PNG (palette-resolved RGBA), return base64
  + metadata (regX/regY, bit depth, useAlpha, palette ref).
- `mcp_load_movie(url, autoplay?)` — programmatically load a movie
  by URL without going through the UI.
- `mcp_get_film_loop_frames(...)` — emit per-frame transforms baked
  in `initial_rect` coords for filmLoop members.
- A `REACT_APP_MCP_FORCE_ENABLED` env var so headless / CI harnesses
  don't have to seed `localStorage` to enable MCP.

**Native build support** — `web_sys::console::*` calls in
`vm-rust/src/player/{bitmap,cast_member,virtual_scripts}` are gated
behind `#[cfg(target_arch = "wasm32")]` so vm-rust compiles for native
targets. Required for the integration tests below.

**Bitmap dumper integration tests** (`vm-rust/tests/dump_*_bitmaps.rs`)
— headless, run via `cargo test`, no Electron. Read input cast files
from `CASTS_ROOT`, write PNGs + metadata sidecars to `OUTPUT_ROOT`.
Each test covers a different cast set:

| Test | Source | Output |
|---|---|---|
| `dump_cct_bitmaps` | `${CASTS_ROOT}/publicrooms/*.cct` (per-room casts) | `${OUTPUT_ROOT}/rooms/<room>.png` (bg) + `<room>/<member>.png` + `_members.json` (regX, regY, bitDepth, useAlpha, width, height per member) |
| `dump_studio_bitmaps` | `${CASTS_ROOT}/cc_studio.cct` | studio bgs + `_studios.json` + `_studio_members.json` |
| `dump_engine_bitmaps` | `${CASTS_ROOT}/*.cct` (engine cast libs) | `${OUTPUT_ROOT}/ui/*.png` + `_engine_members.json` |
| `dump_dcr_bitmaps` | `${DCR_PATH}` (default `${CASTS_ROOT}/games/FurniFactory/FurniFactory2.dcr`) | `${DUMP_ROOT}/<member>.png` + `_members.json` |

**FilmLoop support, palette-cycle dumping, cast-name disambiguation**
— a series of commits to `vm-rust` and the dumpers that handle the
edge cases CokeMusic actually exercises (rio's two same-named
`rio_wave1` cast members, indexed bitmaps with sibling palettes
cycling through frames, Director's first-match cast-lookup behavior).

## Quickstart: dump cast bitmaps

```bash
# 1. Mirror the original cast binaries somewhere local. Not distributed
#    here — see the parent project's PROVENANCE.md for sourcing.
export CASTS_ROOT=/path/to/cokemusic-casts/client2

# 2. Choose where dumped PNGs and metadata land.
export OUTPUT_ROOT=./out

# 3. Build the VM and run a dumper.
cd vm-rust
cargo test --test dump_cct_bitmaps -- --nocapture
```

Optional env vars:

- `ROOM_JSON_DIR` — point at a downstream consumer's per-room JSON
  directory; `dump_cct_bitmaps` will narrow each room's FG-member
  output to only the members referenced by `canonical.roomBitmaps`.
  Unset = dump every FG member.
- `DCR_PATH` — explicit path to a `.dcr` file (overrides the default
  for `dump_dcr_bitmaps`).

## What this fork does NOT add

- No cast binaries (`.cct`, `.dcr`) are distributed here. They are the
  original publishers' copyrighted assets; bring your own.
- No CokeMusic-specific schema interpretation — these dumpers emit
  structural data (member metadata + raw PNGs). Translating that into
  a runtime-shape (room layouts, sprite atlases) is a downstream
  consumer's job. See the parent project for one such translator.

## Licensing

GPL-3.0, inherited from upstream `dirplayer-rs`.

## Acknowledgements

100% of the heavy lifting — the Shockwave / Director runtime, the WASM
VM, the MCP infrastructure — is upstream. This fork only adds CokeMusic-
specific extraction conveniences. Massive credit to
[Igor Lira](https://github.com/igorlira) and contributors for
`dirplayer-rs`.

The Shockwave reverse-engineering community (Earthquake-Project,
OpenShockwave, ScummVM, ProjectorRays, csnover/earthquake-rust) is the
foundation everything here stands on.

---

# Upstream: DirPlayer

The remainder of this README is the upstream
[`igorlira/dirplayer-rs`](https://github.com/igorlira/dirplayer-rs)
documentation, preserved verbatim.

![DirPlayer Logo](public/logo128.png)

DirPlayer is a Shockwave Player emulator written in Rust that aims to make playing old browser games possible on modern browsers.

## Demo

Check out a live demo of this project at http://dirplayer-rs.s3-website-us-west-2.amazonaws.com/

## Chrome Extension

Download the Chrome Extension at https://chromewebstore.google.com/detail/dirplayer-shockwave-emula/gpgalkgegfekkmaknocegonkakahkhbc

The extension implements a polyfill that replaces all `<embed>` elements that point to a Shockwave file in websites you visit.

## Standalone App

Alongside the emulator, DirPlayer comes with a standalone application that provides a complete debugging toolset for Lingo scripts and Shockwave files.

Pre-built binaries can be found at https://github.com/igorlira/dirplayer-rs/releases

![./app-screenshot.gif](./app-screenshot.gif)

## Polyfill

DirPlayer can be embedded directly into any webpage as a standalone JavaScript polyfill. The polyfill is a single self-contained JS file that includes the WASM VM and all required assets.

### Usage

Automatic initialization:

```html
<script src="dirplayer-polyfill.js"></script>
```

Manual initialization:

```html
<script src="dirplayer-polyfill.js" data-manual-init></script>
<script>
  DirPlayer.init();
</script>
```

The polyfill automatically detects and replaces `<embed>` and `<object>` elements that reference Shockwave `.dcr` files.

## Requirements
- NodeJS
  - [(LTS or newer)](https://nodejs.org/)
- RustLang
  - [(1.70.0 or newer)](https://www.rust-lang.org/)
- wasm-pack
  - https://github.com/rustwasm/wasm-pack/releases

## Building
> [!NOTE]  
> Before we can start, we need to load the missing modules for NodeJS with the `npm install` command.

### 🪟 Windows
Windows users can use our scripts which are located in the [`scripts`](https://github.com/igorlira/dirplayer-rs/tree/main/scripts) folder and end with `.bat`.
- Build Rust VM with [`scripts/build-vm.bat`](https://github.com/igorlira/dirplayer-rs/blob/main/scripts/build-vm.bat)
- Build extension with [`scripts/build-extension.bat`](https://github.com/igorlira/dirplayer-rs/blob/main/scripts/build-extension.bat)
  - [Further information can be found here](https://github.com/igorlira/dirplayer-rs?tab=readme-ov-file#building-extension)
- Run locally with [`scripts/run.bat`](https://github.com/igorlira/dirplayer-rs/blob/main/scripts/run.bat)

### 🐧 Other platforms
#### Building Rust VM

```bash
npm run build-vm
```

#### Building extension

```bash
npm run build-extension
```

Make sure to build the VM first. The bundled extension will be located in `./dist-extension`. 

You can install the local build by going to `chrome://extensions`, enabling Developer Mode, then clicking the `Load unpacked` button.

Note that the extension is currently only available on Chrome.

#### Building standalone app

Make sure to build the VM before building the standalone app. The build will be located in `./dist`.

```bash
npm run electron-pack
```

#### Building polyfill

As above, ensure the VM is built before running this. The output will be a single file located at `./dist-polyfill/dirplayer-polyfill.js`.

```bash
npm run build-polyfill
```

#### Running locally

```bash
npm run start
```

#### Running standalone app locally

```bash
npm run electron-dev
```

## Join our Discord!

If you have any questions or you're interested in being part of the discussions of this project, please join our Discord!

https://discord.gg/8yKDk9nJH2

## Acknowledgements

This project would have not been possible without the extensive work of the Shockwave reverse engineering community.

A lot of code has been reproduced from the following projects:

https://github.com/Earthquake-Project/Format-Documentation/

https://github.com/Brian151/OpenShockwave/

https://gist.github.com/MrBrax/1f3ae06c9320863f1d7b79b988c03e60

https://www.scummvm.org/

https://github.com/csnover/earthquake-rust/

https://github.com/ProjectorRays/ProjectorRays
