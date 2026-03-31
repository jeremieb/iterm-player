#!/usr/bin/env node
'use strict';

const blessed = require('blessed');
const contrib = require('blessed-contrib');
const { AutoComplete } = require('enquirer');
const FFT = require('fft.js');
const { spawn } = require('child_process');
const https = require('https');
const musickit = require('./musickit');

const FFT_SIZE = 2048;
const FFT_HOP_SIZE = FFT_SIZE / 2;
const SAMPLE_RATE = 22050;
const SPECTRUM_BAR_COUNT = 48;
const MIN_SPECTRUM_HZ = 30;
const MAX_SPECTRUM_HZ = 10000;
const SPECTRUM_FPS = 12;
const MAX_ANALYSIS_BACKLOG_FRAMES = 1;
const BLESSED_TERM = process.env.TERM === 'xterm-256color' ? 'xterm' : process.env.TERM;

const STATIONS = {
  'nts1': {
	label: 'NTS 1',
	stream: 'https://stream-relay-geo.ntslive.net/stream?client=direct',
	nowPlaying: async () => {
	  const data = await fetchJson('https://www.nts.live/api/v2/live');
	  const ch = (data?.results || []).find(r => r.channel_name === '1');
	  const now = ch?.now?.broadcast_title || 'Unknown';
	  const loc = ch?.now?.embeds?.details?.location_long;
	  return loc ? `${now} — ${loc}` : now;
	}
  },
  'nts2': {
	label: 'NTS 2',
	stream: 'https://stream-relay-geo.ntslive.net/stream2?client=direct',
	nowPlaying: async () => {
	  const data = await fetchJson('https://www.nts.live/api/v2/live');
	  const ch = (data?.results || []).find(r => r.channel_name === '2');
	  const now = ch?.now?.broadcast_title || 'Unknown';
	  const loc = ch?.now?.embeds?.details?.location_long;
	  return loc ? `${now} — ${loc}` : now;
	}
  },
  'worldwide': {
	label: 'Worldwide FM',
	stream: 'https://worldwide-fm.radiocult.fm/stream',
	nowPlaying: async () => {
	  // Try ICY metadata (StreamTitle=...)
	  const title = await fetchIcyStreamTitle('https://worldwide-fm.radiocult.fm/stream', 6000);
	  return title || 'Unknown (no ICY metadata)';
	}
  },
  'fip': {
	label: 'FIP',
	stream: 'https://stream.radiofrance.fr/fip/fip.m3u8',
	nowPlaying: async () => {
	  const data = await fetchJson('https://api.radiofrance.fr/livemeta/pull/7');
	  // Same parsing pattern as common scripts:
	  const level = data?.levels?.[0];
	  const uid = level?.items?.[level?.position];
	  const step = uid ? data?.steps?.[uid] : null;
	  if (!step) return 'Unknown';
	  const title = step.title || 'Unknown';
	  const authors = step.authors || '';
	  return authors ? `${title} — ${authors}` : title;
	}
  }
};

// ---------- UI ----------
const screen = blessed.screen({
  smartCSR: true,
  title: 'radio-cli',
  terminal: BLESSED_TERM
});

const grid = new contrib.grid({ rows: 12, cols: 12, screen });

const header = grid.set(0, 0, 2, 12, blessed.box, {
  label: 'Status',
  tags: true,
  padding: { left: 1, right: 1 },
  content: 'Not playing',
  border: 'line'
});

const spectrum = grid.set(2, 0, 7, 12, blessed.box, {
  label: 'Spectrum',
  border: 'line',
  style: {
    fg: 'cyan',
    border: { fg: 'cyan' }
  }
});

const input = grid.set(9, 0, 3, 12, blessed.textbox, {
  label: 'Command',
  inputOnFocus: true,
  padding: { left: 1, right: 1 },
  border: 'line'
});

screen.key(['q', 'C-c'], () => shutdown('quit'));
screen.on('resize', () => {
  renderSpectrum(lastSpectrumBins);
  screen.render();
});
input.focus();
screen.render();

// ---------- Player state ----------
let currentKey = null;
let mpvProc = null;
let fftProc = null;
let nowPlayingTimer = null;
let spectrumState = null;
let lastSpectrumBins = [];
let lastSpectrumRenderAt = 0;

// Apple Music state
let appleDevToken      = null;
let appleUserToken     = null;
let appleMusicPlaylist = null; // playlist name when playing via Music.app (hidden)

function setHeader() {
  if (appleMusicPlaylist) {
	header.setContent(`Playing playlist: {bold}${appleMusicPlaylist}{/bold} (Apple Music)\nCommands: /music, /play [station], /stop, /quit`);
	screen.render();
	return;
  }
  if (!currentKey) {
	header.setContent('Not playing\n\nCommands: /play [station], /music, /stop, /quit');
	screen.render();
	return;
  }
  const st = STATIONS[currentKey];
  header.setContent(`Playing: {bold}${st.label}{/bold}\nStream: ${st.stream}\nCommands: /play [station], /music, /stop, /quit`);
  screen.render();
}

async function refreshNowPlaying() {
  if (appleMusicPlaylist) {
	try {
	  const np = await musickit.getNowPlaying();
	  if (np) {
		header.setContent(
		  `Playlist: {bold}${appleMusicPlaylist}{/bold} (Apple Music)\n` +
		  `Now: ${np}\n` +
		  `Commands: /music, /play [station], /stop, /quit`
		);
		screen.render();
	  }
	} catch {}
	return;
  }
  if (!currentKey) return;
  const st = STATIONS[currentKey];
  try {
	const np = await st.nowPlaying();
	header.setContent(`Playing: {bold}${st.label}{/bold}\nNow: ${np}\nStream: ${st.stream}\nCommands: /play [station], /music, /stop, /quit`);
	screen.render();
  } catch {
	// Keep previous
  }
}

function startNowPlayingPoll() {
  clearInterval(nowPlayingTimer);
  nowPlayingTimer = setInterval(refreshNowPlaying, 15000);
  refreshNowPlaying();
}

function stopNowPlayingPoll() {
  clearInterval(nowPlayingTimer);
  nowPlayingTimer = null;
}

function startPlayback(key) {
  stopPlayback();

  currentKey = key;
  setHeader();
  startNowPlayingPoll();

  const url = STATIONS[key].stream;

  // Playback via mpv
  mpvProc = spawn('mpv', [
	url,
	'--no-video',
	'--quiet',
	'--cache=yes',
	'--cache-secs=10'
  ], {
	stdio: 'ignore',
	detached: true
  });

  mpvProc.on('exit', () => {
	mpvProc = null;
	if (currentKey === key) {
	  // If it dies unexpectedly, stop everything cleanly
	  stopPlayback();
	  setHeader();
	}
  });

  // Analysis via ffmpeg -> PCM -> simple FFT bins
  fftProc = spawn('ffmpeg', [
	'-hide_banner',
	'-loglevel', 'error',
	'-fflags', 'nobuffer',
	'-flags', 'low_delay',
	'-reconnect', '1',
	'-reconnect_streamed', '1',
	'-reconnect_delay_max', '2',
	'-i', url,
	'-vn',
	'-ac', '1',
	'-ar', String(SAMPLE_RATE),
	'-f', 's16le',
	'pipe:1'
  ], {
	stdio: ['ignore', 'pipe', 'ignore'],
	detached: true
  });

  spectrumState = createSpectrumState(SAMPLE_RATE, FFT_SIZE, SPECTRUM_BAR_COUNT);
  let buf = Buffer.alloc(0);

  fftProc.stdout.on('data', chunk => {
	buf = Buffer.concat([buf, chunk]);
	const bytesPerSample = 2;
	const needed = FFT_HOP_SIZE * bytesPerSample;
	const maxBufferedBytes = needed * MAX_ANALYSIS_BACKLOG_FRAMES;

	if (buf.length > maxBufferedBytes) {
	  buf = buf.subarray(buf.length - maxBufferedBytes);
	}

	while (buf.length >= needed) {
	  const frame = buf.subarray(0, needed);
	  buf = buf.subarray(needed);

	  const samples = new Float32Array(FFT_HOP_SIZE);
	  for (let i = 0; i < FFT_HOP_SIZE; i++) {
		samples[i] = frame.readInt16LE(i * 2) / 32768;
	  }

	  const bins = pushSpectrumSamples(spectrumState, samples);
	  if (!bins) continue;

	  scheduleSpectrumRender(bins);
	}
  });

  fftProc.on('exit', () => { fftProc = null; });
}

function stopPlayback() {
  stopNowPlayingPoll();

  if (mpvProc) {
	terminateProcess(mpvProc);
	mpvProc = null;
  }
  if (fftProc) {
	terminateProcess(fftProc);
	fftProc = null;
  }

  if (appleMusicPlaylist) {
	musickit.stopMusic();
	appleMusicPlaylist = null;
  }

  currentKey = null;
  spectrumState = null;
  lastSpectrumRenderAt = 0;
  renderSpectrum([]);
  screen.render();
  setHeader();
}

// ---------- Command handling ----------
input.on('submit', async (value) => {
  const cmd = (value || '').trim();
  input.clearValue();
  input.focus();
  screen.render();

  if (cmd === '/quit' || cmd === '/q') return shutdown('quit');
  if (cmd === '/stop') { stopPlayback(); return; }

  if (cmd === '/music') {
	// 1. Load config
	let cfg;
	try {
	  cfg = musickit.loadConfig();
	} catch (e) {
	  header.setContent(`Apple Music config error: ${e.message}`);
	  screen.render();
	  return;
	}
	if (!cfg) {
	  header.setContent(
		'Apple Music not configured.\n' +
		'Copy config.example.json → config.json and fill in your Apple Developer credentials.'
	  );
	  screen.render();
	  return;
	}

	// 2. Developer token (session-cached)
	if (!appleDevToken) {
	  appleDevToken = musickit.generateDeveloperToken(cfg.teamId, cfg.keyId, cfg.privateKey);
	}

	// 3. User token — try disk cache, otherwise do the one-time browser auth
	if (!appleUserToken) {
	  appleUserToken = musickit.loadUserToken();
	}

	if (!appleUserToken) {
	  header.setContent(
		'One-time Apple Music sign-in required.\n' +
		'A browser tab will open — sign in, then return here.\n' +
		'This will not happen again after the first time.'
	  );
	  screen.render();
	  screen.leave();
	  let authErr = null;
	  try {
		appleUserToken = await musickit.requestUserToken(appleDevToken);
	  } catch (e) {
		authErr = e;
	  } finally {
		screen.enter();
		screen.realloc();
		input.focus();
		screen.render();
	  }
	  if (authErr) {
		header.setContent(`Apple Music auth failed: ${authErr.message}`);
		screen.render();
		return;
	  }
	}

	// 4. Fetch playlists via Apple Music API
	let playlists;
	try {
	  header.setContent('Loading Apple Music playlists…');
	  screen.render();
	  playlists = await musickit.getLibraryPlaylists(appleDevToken, appleUserToken);
	} catch (e) {
	  if (e.code === 'UNAUTH') { appleUserToken = null; }
	  header.setContent(`Apple Music error: ${e.message}`);
	  screen.render();
	  return;
	}

	if (!playlists.length) {
	  header.setContent('No playlists found in your Apple Music library.');
	  screen.render();
	  return;
	}

	// 5. Autocomplete playlist picker
	const choices = playlists.map(p => ({
	  name:    p.name,
	  message: p.trackCount != null ? `${p.name} (${p.trackCount} tracks)` : p.name
	}));

	screen.leave();
	let selected;
	try {
	  selected = await new AutoComplete({
		name:    'playlist',
		message: 'Select playlist',
		choices
	  }).run();
	} catch { /* cancelled */ } finally {
	  screen.enter();
	  screen.realloc();
	  input.focus();
	  screen.render();
	}

	if (!selected) return;

	// 6. Play via Music.app — immediately hidden, never visible on screen
	stopPlayback();
	header.setContent(`Starting playlist: ${selected}…`);
	screen.render();
	try {
	  await musickit.playPlaylist(selected);
	} catch (e) {
	  header.setContent(
		`Failed to play "${selected}": ${e.message}\n` +
		`Make sure the playlist is in your library and has finished syncing.`
	  );
	  screen.render();
	  return;
	}

	appleMusicPlaylist = selected;
	setHeader();
	startNowPlayingPoll();
	return;
  }

  // /play <station> — direct playback without secondary menu
  if (cmd.startsWith('/play ')) {
	const query = cmd.slice(6).trim().toLowerCase();
	const match = Object.entries(STATIONS).find(([key, st]) =>
	  key === query ||
	  st.label.toLowerCase() === query ||
	  key.startsWith(query) ||
	  st.label.toLowerCase().startsWith(query)
	);
	if (match) {
	  startPlayback(match[0]);
	} else {
	  header.setContent(
		`Unknown station: "${query}"\n` +
		`Available: ${Object.keys(STATIONS).join(', ')}\n` +
		`Commands: /play [station], /stop, /quit`
	  );
	  screen.render();
	}
	return;
  }

  if (cmd === '/play') {
	const choices = Object.entries(STATIONS).map(([key, st]) => ({
	  name: key,
	  message: st.label
	}));

	// Temporarily leave blessed UI: prompt in stdout
	screen.leave();
	try {
	  const ans = await new AutoComplete({
		name: 'station',
		message: 'Select station',
		choices
	  }).run();
	  startPlayback(ans);
	} catch {
	  // user cancelled autocomplete
	} finally {
	  screen.enter();
	  screen.realloc();
	  input.focus();
	  screen.render();
	}
	return;
  }
});

function shutdown() {
  stopPlayback();
  screen.destroy();
  process.exit(0);
}

// ---------- Helpers ----------
function fetchJson(url) {
  return new Promise((resolve, reject) => {
	https.get(url, { headers: { 'User-Agent': 'radio-cli' } }, (res) => {
	  let data = '';
	  res.on('data', d => data += d);
	  res.on('end', () => {
		try { resolve(JSON.parse(data)); } catch (e) { reject(e); }
	  });
	}).on('error', reject);
  });
}

// Minimal ICY metadata reader: returns StreamTitle if seen within timeoutMs
function fetchIcyStreamTitle(url, timeoutMs) {
  return new Promise((resolve) => {
	const req = https.get(url, {
	  headers: { 'Icy-MetaData': '1', 'User-Agent': 'radio-cli' }
	}, (res) => {
	  const metaint = parseInt(res.headers['icy-metaint'] || '0', 10);
	  if (!metaint) {
		res.destroy();
		return resolve(null);
	  }

	  let bytesUntilMeta = metaint;
	  let title = null;

	  res.on('data', (chunk) => {
		let offset = 0;

		while (offset < chunk.length) {
		  if (bytesUntilMeta > 0) {
			const take = Math.min(bytesUntilMeta, chunk.length - offset);
			offset += take;
			bytesUntilMeta -= take;
		  } else {
			// Next byte is metadata length in 16-byte blocks
			const len = chunk[offset] * 16;
			offset += 1;

			if (len > 0 && offset + len <= chunk.length) {
			  const meta = chunk.subarray(offset, offset + len).toString('utf8');
			  offset += len;

			  const m = /StreamTitle='([^']*)'/.exec(meta);
			  if (m && m[1]) title = m[1].trim();
			} else {
			  // Not enough data in this chunk; ignore for simplicity
			  offset = chunk.length;
			}

			bytesUntilMeta = metaint;
		  }
		}

		if (title) {
		  res.destroy();
		  return resolve(title);
		}
	  });

	  setTimeout(() => {
		res.destroy();
		resolve(title);
	  }, timeoutMs).unref();
	});

	req.on('error', () => resolve(null));
	setTimeout(() => {
	  req.destroy();
	  resolve(null);
	}, timeoutMs).unref();
  });
}

function terminateProcess(proc) {
  if (!proc) return;

  try {
	if (proc.pid) {
	  process.kill(-proc.pid, 'SIGKILL');
	  return;
	}
  } catch {}

  try {
	proc.kill('SIGKILL');
  } catch {}
}

function renderSpectrum(bins) {
  lastSpectrumBins = Array.isArray(bins) ? bins : [];

  const innerWidth = Math.max(1, spectrum.width - 2);
  const innerHeight = Math.max(1, spectrum.height - 2);

  if (!lastSpectrumBins.length) {
	spectrum.setContent(Array.from({ length: innerHeight }, () => ' '.repeat(innerWidth)).join('\n'));
	return;
  }

  const displayBins = spreadBins(lastSpectrumBins, innerWidth);
  const rows = Array.from({ length: innerHeight }, () => new Array(innerWidth).fill(' '));

  for (let col = 0; col < displayBins.length; col++) {
	const totalUnits = Math.round((displayBins[col] / 100) * innerHeight * 8);

	for (let row = 0; row < innerHeight; row++) {
	  const unitsAboveBottom = totalUnits - ((innerHeight - row - 1) * 8);
	  rows[row][col] = verticalBlock(unitsAboveBottom);
	}
  }

  spectrum.setContent(rows.map(row => row.join('')).join('\n'));
}

function scheduleSpectrumRender(bins) {
  const now = Date.now();
  const minFrameMs = 1000 / SPECTRUM_FPS;

  lastSpectrumBins = Array.isArray(bins) ? bins : [];

  if ((now - lastSpectrumRenderAt) < minFrameMs) {
	return;
  }

  renderSpectrum(lastSpectrumBins);
  lastSpectrumRenderAt = now;
  screen.render();
}

function spreadBins(bins, width) {
  if (width <= 0) {
	return [];
  }

  const slotCount = Math.min(bins.length, width);
  const slotBins = resampleBins(bins, slotCount);
  const output = new Array(width).fill(0);

  if (slotCount === 1) {
	output[Math.floor(width / 2)] = slotBins[0];
	return output;
  }

  for (let i = 0; i < slotBins.length; i++) {
	const column = Math.round((i * (width - 1)) / (slotCount - 1));
	output[column] = slotBins[i];
  }

  return output;
}

function verticalBlock(units) {
  if (units <= 0) return ' ';
  if (units >= 8) return '█';
  return [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'][units];
}

function resampleBins(bins, targetCount) {
  if (targetCount <= 0) {
	return [];
  }

  if (bins.length <= targetCount) {
	return bins.slice();
  }

  const output = new Array(targetCount);

  for (let i = 0; i < targetCount; i++) {
	const start = Math.floor((i * bins.length) / targetCount);
	const end = Math.max(start + 1, Math.floor(((i + 1) * bins.length) / targetCount));
	let sum = 0;

	for (let j = start; j < end; j++) {
	  sum += bins[j];
	}

	output[i] = sum / (end - start);
  }

  return output;
}

function createSpectrumState(sampleRate, fftSize, barCount) {
  const fft = new FFT(fftSize);
  const input = new Float32Array(fftSize);
  const window = new Float32Array(fftSize);
  const complex = fft.createComplexArray();
  const smoothed = new Float32Array(barCount);
  const bandRanges = [];

  for (let i = 0; i < fftSize; i++) {
	window[i] = 0.5 * (1 - Math.cos((2 * Math.PI * i) / (fftSize - 1)));
  }

  const nyquist = sampleRate / 2;
  const maxHz = Math.min(MAX_SPECTRUM_HZ, nyquist - 1);
  const minLog = Math.log10(MIN_SPECTRUM_HZ);
  const maxLog = Math.log10(maxHz);
  const binHz = sampleRate / fftSize;

  for (let band = 0; band < barCount; band++) {
	const startHz = 10 ** (minLog + ((maxLog - minLog) * band) / barCount);
	const endHz = 10 ** (minLog + ((maxLog - minLog) * (band + 1)) / barCount);
	const startBin = Math.max(1, Math.floor(startHz / binHz));
	const endBin = Math.min((fftSize / 2) - 1, Math.max(startBin + 1, Math.ceil(endHz / binHz)));
	bandRanges.push([startBin, endBin]);
  }

  return {
    fft,
    fftSize,
    input,
    window,
    complex,
    smoothed,
    bandRanges,
    noiseFloorDb: -105,
    peakDb: -55
  };
}

function pushSpectrumSamples(state, samples) {
  state.input.copyWithin(0, samples.length);
  state.input.set(samples, state.fftSize - samples.length);

  const windowed = new Float32Array(state.fftSize);
  for (let i = 0; i < state.fftSize; i++) {
	windowed[i] = state.input[i] * state.window[i];
  }

  state.fft.realTransform(state.complex, windowed);
  state.fft.completeSpectrum(state.complex);

  const rawBars = new Float32Array(state.bandRanges.length);
  const rawDbs = new Float32Array(state.bandRanges.length);
  let frameMinDb = Infinity;
  let frameMaxDb = -Infinity;

  for (let band = 0; band < state.bandRanges.length; band++) {
	const [startBin, endBin] = state.bandRanges[band];
	let energy = 0;
	let count = 0;

	for (let bin = startBin; bin <= endBin; bin++) {
	  const re = state.complex[bin * 2];
	  const im = state.complex[(bin * 2) + 1];
	  energy += (re * re) + (im * im);
	  count++;
	}

	const rms = count ? Math.sqrt(energy / count) / state.fftSize : 0;
	const db = 20 * Math.log10(rms + 1e-12);
	rawDbs[band] = db;
	frameMinDb = Math.min(frameMinDb, db);
	frameMaxDb = Math.max(frameMaxDb, db);
  }

  state.noiseFloorDb = (state.noiseFloorDb * 0.97) + (frameMinDb * 0.03);
  state.peakDb = Math.max(frameMaxDb, (state.peakDb * 0.92) + (frameMaxDb * 0.08));

  const dynamicRange = Math.max(18, state.peakDb - state.noiseFloorDb);

  for (let band = 0; band < rawDbs.length; band++) {
	const normalized = clamp((rawDbs[band] - state.noiseFloorDb) / dynamicRange, 0, 1);
	const weighted = Math.pow(normalized, 0.85);
	rawBars[band] = weighted * 100;
  }

  const smoothedAcrossBands = smoothBandNeighbors(rawBars);
  const output = new Array(smoothedAcrossBands.length);

  for (let i = 0; i < smoothedAcrossBands.length; i++) {
	const target = smoothedAcrossBands[i];
	const prev = state.smoothed[i];
	const alpha = target > prev ? 0.42 : 0.14;
	const next = prev + ((target - prev) * alpha);
	state.smoothed[i] = next;
	output[i] = Math.round(next);
  }

  return output;
}

function smoothBandNeighbors(values) {
  const out = new Float32Array(values.length);

  for (let i = 0; i < values.length; i++) {
	const left = i > 0 ? values[i - 1] : values[i];
	const center = values[i];
	const right = i < values.length - 1 ? values[i + 1] : values[i];
	out[i] = (left + (center * 2) + right) / 4;
  }

  return out;
}

function clamp(value, min, max) {
  return Math.max(min, Math.min(max, value));
}

setHeader();
