# ani-rust

Async Rust CLI (with an optional full-screen terminal UI) for searching the AnimeX catalog, inspecting sources, and bulk-downloading media **you are authorized to save**. No DRM, authentication, paywall, or CAPTCHA bypass is implemented — the tool refuses encrypted/DRM-protected sources outright.

<p>
  <img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg">
  <img alt="Rust edition" src="https://img.shields.io/badge/rust-2021_edition-orange.svg">
  <img alt="Platform" src="https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS-lightgrey.svg">
</p>

## Demo

The unified terminal UI walks you through search → select → configure → download without leaving the screen:

```
┌ ani-rust  /  Buscar → Seleccionar → Descargar ─────────────────────────────┐
├──────────────────────────────┬──────────────────────────────────────────────┤
│ Resultados                   │ Detalles                                      │
│ › Case Closed                │ Case Closed                                   │
│   Case Closed: Special       │ ID: case-closed-5j4se                         │
│   ...                        │ Episodios: 1147                               │
│                               │ Año: 1996  Formato: TV  Estado: RELEASING     │
├──────────────────────────────┴──────────────────────────────────────────────┤
│ ↑/↓: seleccionar · Enter: configurar descarga · Esc: otra búsqueda           │
└───────────────────────────────────────────────────────────────────────────┘
```

And a finished run reports each episode's outcome:

```
Carpeta: downloads/ani-case-closed-5j4se/

Episodio 1: complete
Episodio 2: complete
Episodio 3: complete
```

> Have a real terminal recording? Drop it at `docs/demo.gif` and reference it here — PRs welcome.

## Table of contents

- [Features](#features)
- [Legal / disclaimer](#legal--disclaimer)
- [Prerequisites](#prerequisites)
- [Installation](#installation)
- [Quick start](#quick-start)
- [Usage](#usage)
  - [Interactive terminal UI](#interactive-terminal-ui)
  - [CLI subcommands](#cli-subcommands)
  - [Global flags](#global-flags)
  - [Source flags](#source-flags-search-inspect-download)
- [Advanced workflows](#advanced-workflows)
  - [Resolving from a saved JSON response](#resolving-from-a-saved-json-response)
  - [Bulk playlist-only export](#bulk-playlist-only-export)
  - [Request rate limiting](#request-rate-limiting)
- [Output layout](#output-layout)
- [Architecture](#architecture)
- [Testing](#testing)
- [Contributing](#contributing)
- [License](#license)

## Features

- 🔎 **Catalog search** against AnimeX's GraphQL API, with adult-content filtering and configurable result limits.
- 🖥️ **Unified terminal UI** (Ratatui) — search, configure, inspect, and download without leaving the screen, plus a scripting-friendly plain-output mode.
- 📥 **Bulk episode downloads** with bounded concurrency, automatic retries with exponential backoff, and a global request-rate limiter to stay polite to source servers.
- 🎬 **HLS-aware**: resolves master/media playlists, picks a quality (`best` or an exact resolution), stages segments locally, and remuxes to MP4 with `ffmpeg` (stream copy, no re-encoding).
- 📝 **Subtitles included automatically** — VTT/SRT/ASS/SSA tracks are downloaded as sidecars alongside video, or alongside the exported playlist in playlist-only mode.
- 📁 **Per-anime folders** — episodes are grouped under a parent directory named after the anime, not dumped flat into the output folder.
- 🔒 **Refuses DRM/encrypted sources** — encryption keys, session keys, declared DRM, and protected MP4 signatures all cause a hard failure instead of a silent bypass attempt.
- ♻️ **Resumable-by-skip** — episodes that already published a complete, size-verified manifest are skipped instead of re-downloaded.
- 🧩 **`--source-json` escape hatch** — if the source API denies access but you already hold an authorized resolver response, feed it in directly instead of calling the API.

## Legal / disclaimer

This tool only automates requests you could already make yourself (search, inspect, download) against endpoints that respond to you. It does **not**:

- bypass authentication, paywalls, CAPTCHAs, or DRM,
- decrypt or strip encryption from protected streams (it refuses them instead), or
- provide access to content you are not otherwise authorized to obtain.

You are responsible for complying with the terms of service and copyright law that apply to whatever content and endpoints you point this tool at.

## Prerequisites

- **Rust** (stable toolchain; developed/tested with 1.97.1) — [rustup.rs](https://rustup.rs)
- **ffmpeg** on your `PATH` (or pointed to via `--ffmpeg`) — required for the default `video` download mode; not needed for `--playlist-only` exports. ffmpeg must support the codecs in your source, since HLS output is remuxed with stream copy, not transcoded.
- Optional: **Python 3**, only if you want to run the smoke test suite.

## Installation

```sh
git clone https://github.com/MyNameIsDotPy/ani-rust.git
cd ani-rust
cargo build --release
```

The executable is at `target/release/ani-rust` (`ani-rust.exe` on Windows). You can also install it onto your `PATH`:

```sh
cargo install --path .
```

## Quick start

```sh
# Open the interactive terminal UI (recommended)
ani-rust

# ...or jump straight into a search from the shell
ani-rust search "case closed"

# Non-interactive: search, inspect, then download
ani-rust search "case closed" --limit 10
ani-rust inspect case-closed-5j4se --episode 1
ani-rust download case-closed-5j4se --episodes 1,3,5-10 --type sub --provider beep --quality best --output ./downloads --concurrency 3
```

## Usage

### Interactive terminal UI

Running `ani-rust` with no subcommand (or `ani-rust search "..."`) opens a full-screen UI — no need to chain separate commands. The on-screen text is in Spanish; controls:

| Screen | Key | Action |
| --- | --- | --- |
| Search | `Enter` | Run the search |
| Search | `F2` | Toggle including adult results |
| Search | `F3` | Cycle result limit: 10 → 25 → 50 |
| Results | `↑` / `↓`, `Enter` | Select an anime, open its download configuration |
| Configure | `Tab` / `↑` / `↓` | Move between fields (episodes, sub/dub, provider, quality, output folder, concurrency, ffmpeg path, JSON file, format) |
| Configure | `Space` / `Enter` on **Formato** | Toggle between `video` and `m3u8` (playlist-only) mode |
| Configure | `Ctrl-U` | Clear the focused field |
| Configure | `F2` | Open the paste editor for an authorized JSON resolver response |
| Configure | `F4` | Inspect the first requested episode (prints resolved sources as JSON) |
| Configure | `F5` | Start the download |
| Any operation | `Esc` / `Ctrl-C` (also `q` during downloads) | Cancel and return; already-published episodes are kept |
| Report | `↑` / `↓` / `PageUp` / `PageDown` | Scroll the result report |
| Report | `Enter` / `Esc` | Back to configuration; `/` to search again |

If the source API returns `403`, the report tells you to come back, press `F2`, and paste an authorized JSON response (one episode per response) instead.

### CLI subcommands

Every command supports `--help` for the full built-in reference.

| Command | Purpose |
| --- | --- |
| `search <query>` | Query the catalog and print matching anime with their internal AnimeX `id`. |
| `inspect <anime_id> [--episode N]` | Resolve one episode's sources without downloading; prints JSON (streams, qualities, headers, subtitle tracks). |
| `download <anime_id> --episodes <spec>` | Resolve and download one or more episodes. |

Episode specs accept comma-separated numbers and ranges: `1,3,5-10`.

```sh
ani-rust search "naruto" --limit 10 --include-adult false
ani-rust --debug search "naruto"
ani-rust inspect naruto-oe7a3
ani-rust download case-closed-5j4se --episodes 1,3,5-10 --quality 720p
```

### Global flags

These apply to every command:

| Flag | Default | Description |
| --- | --- | --- |
| `--debug` | off | Verbose logging (also forces plain, non-TUI output). |
| `--plain` | off | Disable the interactive UI (automatic for piped output and `--debug`). |
| `--api-url` | `https://pp.animex.one/rest/api/sources` | Override the source resolver endpoint. |
| `--graphql-url` | `https://graphql.animex.one/graphql` | Override the catalog GraphQL endpoint. |
| `--retries` | `3` (0–10) | Extra attempts for transient failures (timeouts, connect errors, 429, 5xx) with exponential backoff starting at 500 ms. |
| `--rate-limit` | `5` (1–50) | Maximum HTTP requests per second sent to source/media servers, app-wide, independent of `--concurrency`. |

### Source flags (`search`, `inspect`, `download`)

| Flag | Default | Description |
| --- | --- | --- |
| `--type <sub\|dub>` | `sub` | Audio/subtitle track language. |
| `--provider <id>` | `beep` | Source provider identifier. |
| `--quality <best\|720p...>` | `best` | Exact advertised resolution, or the highest available. |
| `--playlist-only` | off | Save the selected HLS playlist and subtitles, without video segments (no ffmpeg needed). |
| `--source-json <file>` | — | Use a saved, authorized resolver response instead of calling the source API (one episode per file). |

`download` additionally accepts `--episodes <spec>` (required), `--output <dir>` (default `./downloads`), `--concurrency <1-64>` (default `3`), and `--ffmpeg <path>` (default `ffmpeg`).

## Advanced workflows

### Resolving from a saved JSON response

If the source API returns `403` but you already have an authorized resolver response (e.g. captured from your own browser session), save the raw JSON and pass `--source-json`. This skips the resolver API entirely and downloads using the supplied headers and subtitle tracks:

```sh
ani-rust inspect case-closed-5j4se --episode 1 --source-json ./examples/case-closed-episode-1.json
ani-rust download case-closed-5j4se --episodes 1 --source-json ./examples/case-closed-episode-1.json --output ./downloads
```

A raw resolver response doesn't identify its own episode number, so set `--episode`/`--episodes` to the actual episode it represents. Exactly one episode is accepted per file, to avoid saving the same video under multiple episode numbers — get a separate authorized response per episode. This doesn't change access controls: if the playlist or segments also reject the request, the transfer still fails normally.

### Bulk playlist-only export

In the interactive UI, set **Formato** to `m3u8` (press `Space` on that field) and press `F5`. From the shell:

```sh
ani-rust download case-closed-5j4se --episodes 1-20 --playlist-only --output ./playlists
```

This saves, per episode, the resolved media playlist (absolute segment/map URLs so it works outside the original server) plus subtitle sidecars and `metadata.json` — no segments, video, or ffmpeg invocation. The saved playlist doesn't carry the required request headers itself (those stay in `metadata.json`), and any signed URLs it references can expire — this is an export, not an offline copy.

### Request rate limiting

`--rate-limit` (default `5` requests/second) caps *every* outgoing HTTP request the tool makes — source API resolution, playlist loads, segment downloads, and subtitle downloads — through a single shared limiter, regardless of how many episodes run in parallel via `--concurrency`. This is separate from `--retries`, which governs backoff after a transient failure. Lower `--rate-limit` if a provider starts rejecting requests under load; raise it (up to `50`) if you know the provider tolerates more.

## Output layout

Episodes are grouped under a parent folder named after the anime (its title when known, e.g. from search results, otherwise its internal id), so a whole series stays together:

```text
downloads/
└── ani-case-closed-5j4se/
    ├── sub-ani-beep-ep0001-best/
    │   ├── video.mp4
    │   ├── subtitle-1-English.vtt
    │   └── metadata.json
    └── sub-ani-beep-ep0002-best/
        ├── video.mp4
        ├── subtitle-1-English.vtt
        └── metadata.json
```

`metadata.json` records the input identity, selected URL, full resolver metadata, and output file sizes. Staging and publication happen on the same filesystem — a directory rename publishes an episode only after media and subtitles both succeed, so a crash mid-download never leaves a half-written episode mistaken for a complete one. An episode whose manifest and file sizes still match is skipped without re-resolving; an existing directory that doesn't match is left untouched (move it aside before retrying) rather than overwritten.

## Architecture

| Module | Responsibility |
| --- | --- |
| `catalog.rs` | Typed GraphQL catalog client, independent of media resolution/transfer. |
| `api.rs` / `models.rs` | `SourceResolver` trait, REST provider adapter, shared rate-limited HTTP transfer layer. |
| `hls.rs` | Playlist parsing/validation, variant selection, and local segment staging. |
| `downloader.rs` / `subtitles.rs` / `filenames.rs` | Episode orchestration, manifests, subtitle sidecars, portable/sanitized names. |
| `app.rs` | Unified interactive workflow: state, validation, cancellable operations. |
| `cli.rs` / `tui.rs` / `main.rs` | Argument parsing, terminal lifecycle, and command dispatch. |

Notable safety properties: HTTP(S)-only transfers, a restricted local HLS playlist passed to ffmpeg (only the `file` protocol, so arbitrary remote tags never reach it), and hard refusal of encrypted/DRM-signaled sources (including plain AES-128 HLS encryption and MP4s with matching protection box signatures).

## Testing

```sh
cargo test
cargo clippy --all-targets -- -D warnings
python tests/smoke.py target/debug/ani-rust
```

The smoke test needs Python 3 and ffmpeg. It builds its own short test video, serves fixtures over loopback, and exercises search payloads/defaults/errors/debug output, required headers, quality selection, retries, subtitles, concurrent downloads, manifests, skip-on-complete behavior, and encryption rejection — it never downloads third-party media.

## Contributing

Issues and pull requests are welcome. Before opening a PR, please run `cargo test`, `cargo clippy --all-targets -- -D warnings`, and the smoke test above. For anything touching request headers, retries, or the resolver, please describe the source/provider behavior you observed — this project avoids speculative endpoint or provider mapping that hasn't been verified against real responses.

## License

Released under the [MIT License](LICENSE).
