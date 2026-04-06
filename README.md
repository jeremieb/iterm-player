# iterm-player

A terminal radio player written in Rust. It plays a small set of predefined stations through a single GStreamer pipeline and renders a live FFT-based spectrum from that same decoded audio stream.

<img width="1106" height="927" alt="Screenshot 2026-04-01 at 8 18 14 AM" src="https://github.com/user-attachments/assets/fb8c46ba-82ed-478e-94b8-e579e768259b" />

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

Make sure `~/.cargo/bin` is on your shell `PATH` if you installed with `cargo install --path .`. The included iTerm2 widget launches `iterm-player` through a login `zsh` shell, so whatever works in a normal terminal should also work from the widget.

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
- `/volume 3`
- `/stop`
- `/quit` or `/q`

Running `/play` without a station shows the available station keys in the status panel.

Running `/color` without a value shows the available color names.

Running `/volume` without a value shows the current volume and expected range.

## Input helpers

- `Tab` completes commands such as `/pl` -> `/play `
- `Tab` also completes station keys after `/play `
- `Tab` completes color names after `/color `
- `Tab` completes `/volume ` like the other top-level commands

## iTerm2 Widget

The repo includes an iTerm2 Python status bar script at `iterm2/iterm_player_statusbar.py`.

![widget](https://github.com/user-attachments/assets/e82e6ba0-2098-488f-a55b-eeb327efde47)


It provides one compact status bar widget in this order:

- `▶ or ■ | ▶▶ | Radio Name`

When the player is stopped, the first control shows `▶`. When the player is running, it shows `■`. Clicking the widget opens a small popover with `▶/■` and `▶▶` buttons. The widget only controls the running player through the local Unix socket at `/tmp/iterm-player.sock`. If no player is running, using the widget starts a new iTerm2 window, launches `iterm-player`, and starts the first station.

### Install The Widget

1. Create the iTerm2 AutoLaunch directory if it does not already exist:

```sh
mkdir -p "$HOME/Library/Application Support/iTerm2/Scripts/AutoLaunch"
```

2. Symlink the script from this repo into AutoLaunch:

```sh
ln -sf "$(pwd)/iterm2/iterm_player_statusbar.py" "$HOME/Library/Application Support/iTerm2/Scripts/AutoLaunch/iterm_player_statusbar.py"
```

3. Restart iTerm2, or launch the script manually from `Scripts > Manage`.

4. In iTerm2, enable the status bar for your profile:
   `Settings > Profiles > Session > Status Bar Enabled`

5. Open the status bar configuration:
   `Settings > Profiles > Session > Configure Status Bar...`

6. Add this component:

- `iTerm Player`

### Widget Notes

- The widget expects the `iterm-player` command to work in a normal login shell.
- If the widget shows a bug icon or does not update, open `Scripts > Manage > Console` in iTerm2 to inspect Python script errors.
- The widget reads `/tmp/iterm-player.json` for display state and sends commands to `/tmp/iterm-player.sock`.

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
- `grey`
- `dark-grey`
- `orange`
- `brown`

The player volume can also be changed at runtime with `/volume [0-10]`, where `0` is muted and `10` is full volume.

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
