# item-player

A terminal radio player with Apple Music library integration. Built with Node.js, blessed, and a custom FFT-based terminal spectrum renderer.

```
┌─ Status ────────────────────────────────────────────────────┐
│ Playlist: Favourites (Apple Music)                          │
│ Now: Nuit Idéale — Flavien Berger                          │
│ Commands: /music, /play [station], /stop, /quit             │
└─────────────────────────────────────────────────────────────┘
┌─ Spectrum ──────────────────────────────────────────────────┐
│ ▂ ▃ ▄ ▅ ▆ ▇ █ ▇ ▆ ▅ ▄ ▃ ▂ ▁                                  │
│ ▂ ▃ ▄ ▅ ▆ ▇ █ ▇ ▆ ▅ ▄ ▃ ▂ ▁                                  │
│ ▂ ▃ ▄ ▅ ▆ ▇ █ ▇ ▆ ▅ ▄ ▃ ▂ ▁                                  │
│ ▂ ▃ ▄ ▅ ▆ ▇ █ ▇ ▆ ▅ ▄ ▃ ▂ ▁                                  │
└─────────────────────────────────────────────────────────────┘
┌─ Command ───────────────────────────────────────────────────┐
│ /music                                                      │
└─────────────────────────────────────────────────────────────┘
```

---

## Requirements

- **Node.js** 18+
- **mpv** — audio playback for radio streams
- **ffmpeg** — decodes stream audio for spectrum analysis
- **macOS** — required for Apple Music integration (Music.app + AppleScript)

Install mpv and ffmpeg via Homebrew:

```sh
brew install mpv ffmpeg
```

---

## Installation

```sh
git clone https://github.com/jeremieb/item-player.git
cd item-player
npm install
```

`npm install` also installs [`fft.js`](https://www.npmjs.com/package/fft.js), which is used for the spectrum analysis pipeline.

---

## Usage

```sh
node index.js
```

Type commands into the **Command** box at the bottom and press **Enter**.

If your terminal advertises `xterm-256color`, the app automatically falls back to a safer `xterm` terminfo profile for blessed compatibility.

| Command | Description |
|---|---|
| `/play` | Open an interactive station picker |
| `/play nts1` | Play NTS 1 directly |
| `/play fip` | Play FIP directly |
| `/play worldwide` | Play Worldwide FM directly |
| `/play nts2` | Play NTS 2 directly |
| `/music` | Browse and play a playlist from your Apple Music library |
| `/stop` | Stop playback (radio or Apple Music) |
| `/quit` or `/q` | Exit |

**Radio stations ship built-in.** Apple Music requires a one-time configuration — see below.

---

## Spectrum visualizer

The spectrum panel is driven by a real FFT pipeline:

- `ffmpeg` reads the live stream and outputs mono PCM
- `fft.js` performs the FFT analysis
- the app groups frequencies into 48 log-spaced bands
- a custom terminal renderer draws evenly spaced Unicode block bars across the available width

The renderer is custom because the stock `blessed-contrib` bar widget did not produce clean terminal output for this use case.

---

## Apple Music integration

### How it works

1. You run `/music` in the terminal.
2. **First time only:** a browser tab opens at `http://localhost:59743`. You click **Sign in with Apple Music** — Apple's MusicKit JS handles the OAuth flow. The Music User Token is saved to `~/.item-player-music-token`. The browser is never opened again.
3. Your library playlists appear in an autocomplete picker inside the terminal.
4. The selected playlist plays via **Music.app** — which is launched hidden in the background using AppleScript. No window ever appears; it stays invisible in the dock. This step is unavoidable: Apple Music tracks use FairPlay DRM and can only be decoded by authorised runtimes (Music.app, a browser, or a native app).
5. The header polls Music.app every 15 seconds and shows the current track and artist.

### Step 1 — Apple Developer account

You need a free or paid Apple Developer account. Sign in at [developer.apple.com](https://developer.apple.com).

> A free account (just your Apple ID) is sufficient to create a MusicKit key.

### Step 2 — Register an App ID (required before creating the key)

Apple requires at least one App ID with MusicKit enabled before the key can be created.

1. Go to [developer.apple.com/account](https://developer.apple.com/account)
2. Click **Certificates, Identifiers & Profiles → Identifiers**
3. Click **+** → select **App IDs** → type **App** → Continue
4. Fill in:
   - **Description:** anything, e.g. `item-player`
   - **Bundle ID:** choose **Explicit** and enter a reverse-domain string, e.g. `com.yourname.item-player` (this does not need to match a real app)
5. Scroll the Capabilities list, find **MusicKit** and check it
6. Click **Continue → Register**

### Step 3 — Create a MusicKit key

1. Click **Keys** in the left sidebar
2. Click **+** (top right) to create a new key
3. Give it a name, e.g. `item-player`
4. Check the **Media Services (MusicKit, ShazamKit, Apple Music Feed)** checkbox — it will now be selectable
5. Click **Continue**, then **Register**
6. Click **Download** — you get a file named `AuthKey_XXXXXXXXXX.p8`

> **Download it now.** Apple only lets you download it once.

Note these three values — you will need all of them:

| Value | Where to find it |
|---|---|
| **Team ID** | Top-right corner of any page in the Developer portal, or under Membership |
| **Key ID** | Shown on the key detail page (the `XXXXXXXXXX` part of the `.p8` filename) |
| **Private key** | The downloaded `.p8` file |

### Step 4 — Configure the app

Copy the example config and edit it:

```sh
cp config.example.json config.json
```

```json
{
  "apple": {
    "teamId": "AB12CD34EF",
    "keyId": "XY98ZW76VU",
    "privateKeyPath": "./AuthKey_XY98ZW76VU.p8"
  }
}
```

Place the `.p8` file in the project directory (or point `privateKeyPath` to wherever you saved it — absolute or relative paths both work).

> `config.json` and `*.p8` are gitignored. Do not commit them.

### Step 5 — Authorise

Run the app and type `/music`. A browser tab opens. Click **Sign in with Apple Music**, approve the prompt, and return to the terminal. The playlist picker appears immediately.

The token is cached at `~/.item-player-music-token` — you will not be asked to sign in again unless you revoke access in **Settings → Privacy & Security → Media & Apple Music** on your Mac.

---

## Project structure

```
item-player/
├── index.js              # Terminal UI and command handling (blessed)
├── musickit.js           # Apple Music module (auth, API, AppleScript)
├── config.example.json   # Credentials template — copy to config.json
├── config.json           # Your credentials (gitignored)
├── package.json
└── README.md
```

---

## Troubleshooting

**`/music` says "Apple Music not configured"**
Make sure `config.json` exists and contains all three fields (`teamId`, `keyId`, `privateKeyPath`).

**`/music` says "MusicKit private key not found"**
Check that the `.p8` file path in `config.json` is correct and the file exists.

**Browser opens but sign-in fails**
The Developer Token may be malformed. Double-check your Team ID and Key ID — they are both exactly 10 characters.

**"Failed to play playlist" after selecting**
Music.app must be running. The playlist name must exactly match a playlist in your library. If it was recently added from Apple Music, give it a moment to sync.

**Apple Music session expired**
Run `/music` again — the app will re-open the browser to get a fresh token.

**Spectrum is not showing**
Ensure `ffmpeg` is installed (`brew install ffmpeg`) and that the stream URL is reachable.

**The UI prints a `Setulc` / `xterm-256color` error on startup**
The app now applies a compatibility fallback automatically. If you still see it in a custom environment, try `TERM=xterm node index.js`.

**FIP behaves differently from the other stations**
FIP is served via HLS (`.m3u8`), so its analysis path is more bursty than direct MP3/AAC radio streams. The app uses a low-latency `ffmpeg` configuration and intentionally drops stale buffered analysis frames to keep the spectrum responsive.

**`mpv` not found**
Install it with `brew install mpv`.
