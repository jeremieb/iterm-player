# iterm-player

A terminal radio player written in Rust. It plays a small set of predefined stations through a single GStreamer pipeline and renders a live FFT-based spectrum from that same decoded audio stream.

<img width="1106" height="927" alt="Screenshot 2026-04-01 at 7 41 26 AM" src="https://github.com/user-attachments/assets/5cb6fa0b-50b1-4617-9ff1-cc059712d3b3" />

## Requirements

- Rust toolchain (`cargo`)
- GStreamer

This project currently assumes a macOS setup with Homebrew.

Install GStreamer and Rust with Homebrew:

```sh
brew install gstreamer
brew install rust
```

On Apple Silicon, Homebrew is usually installed under `/opt/homebrew`.

On Intel Macs, Homebrew is usually installed under `/usr/local`.

## Run

```sh
cargo run
```

## Install As A Command

If you want to use the app as a normal command instead of running it through Cargo each time:

```sh
cargo install --path .
```

Then run it from anywhere with:

```sh
iterm-player
```

If the command is not found immediately, open a new terminal tab or run:

```sh
rehash
```

You can verify where Cargo installed it with:

```sh
which iterm-player
```

If you update the repo later, reinstall the command with:

```sh
cargo install --path . --force
```

## Build

```sh
cargo build --release
```

The compiled binary will be available at:

```sh
./target/release/iterm-player
```

You can also run that binary directly without installing it globally:

```sh
./target/release/iterm-player
```

Or create a symlink so the command is available on your shell `PATH`:

```sh
ln -sf "$(pwd)/target/release/iterm-player" "$(brew --prefix)/bin/iterm-player"
```

Then run:

```sh
iterm-player
```

If the command does not autocomplete or is not found immediately, run:

```sh
rehash
```

And verify the symlink with:

```sh
ls -l "$(brew --prefix)/bin/iterm-player"
which iterm-player
```

## Commands

- `/play nts1`
- `/play nts2`
- `/play worldwide`
- `/play fip`
- `/color red`
- `/color yellow`
- `/color cyan`
- `/stop`
- `/quit` or `/q`

Running `/play` without a station shows the available station keys in the status panel.

Running `/color` without a value shows the available color names.

## Input helpers

- `Tab` completes commands such as `/pl` -> `/play `
- `Tab` also completes station keys after `/play `
- `Tab` completes color names after `/color `

## Customization

The app accent color can be changed at runtime. This affects:

- panel borders
- spectrum color
- overall interface accent

Available colors:

- `cyan`
- `red`
- `yellow`
- `green`
- `blue`
- `pink`
- `magenta`
- `white`

## Notes

- Playback and analysis now come from the same GStreamer decode pipeline, which makes station behavior more consistent than the previous split `mpv` + `ffmpeg` approach.
- Spectrum analysis is still done in-process in Rust after pulling decoded PCM from GStreamer.
- `FIP` now uses a direct Icecast AAC stream instead of the old HLS playlist.
- The app is radio-only now. Apple Music support was intentionally removed from the main codebase.

## Archive

The old implementation was copied into:

```text
archive/
```

That snapshot includes the original Node.js entrypoint, Apple Music integration code, and the previous README/config files.
