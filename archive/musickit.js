'use strict';

const https    = require('https');
const fs       = require('fs');
const path     = require('path');
const { exec } = require('child_process');
const crypto   = require('crypto');

const TOKEN_FILE = path.join(process.env.HOME, '.item-player-music-token');

// ── Developer Token (JWT / ES256) ─────────────────────────────────────────

function base64url(buf) {
  return buf.toString('base64')
    .replace(/\+/g, '-').replace(/\//g, '_').replace(/=/g, '');
}

function generateDeveloperToken(teamId, keyId, privateKey) {
  const header  = base64url(Buffer.from(JSON.stringify({ alg: 'ES256', kid: keyId })));
  const now     = Math.floor(Date.now() / 1000);
  const payload = base64url(Buffer.from(JSON.stringify({
    iss: teamId, iat: now, exp: now + 15_777_000
  })));
  const input  = `${header}.${payload}`;
  const signer = crypto.createSign('SHA256');
  signer.update(input);
  const sig = signer.sign({ key: privateKey, dsaEncoding: 'ieee-p1363' });
  return `${input}.${base64url(sig)}`;
}

// ── User Token persistence ─────────────────────────────────────────────────

function loadUserToken() {
  if (!fs.existsSync(TOKEN_FILE)) return null;
  return fs.readFileSync(TOKEN_FILE, 'utf8').trim() || null;
}

function saveUserToken(token) {
  fs.writeFileSync(TOKEN_FILE, token, { mode: 0o600 });
}

function clearUserToken() {
  try { fs.unlinkSync(TOKEN_FILE); } catch {}
}

// ── Apple Music API ────────────────────────────────────────────────────────

function apiFetch(apiPath, devToken, userToken) {
  return new Promise((resolve, reject) => {
    https.get({
      hostname: 'api.music.apple.com',
      path: apiPath,
      headers: {
        'Authorization':    `Bearer ${devToken}`,
        'Music-User-Token': userToken
      }
    }, (res) => {
      if (res.statusCode === 401) {
        clearUserToken();
        const err = new Error('Apple Music token expired — run /music again to re-authorise');
        err.code = 'UNAUTH';
        return reject(err);
      }
      let data = '';
      res.on('data', d => data += d);
      res.on('end', () => {
        try { resolve(JSON.parse(data)); } catch (e) { reject(e); }
      });
    }).on('error', reject);
  });
}

async function getLibraryPlaylists(devToken, userToken) {
  const all  = [];
  let   next = '/v1/me/library/playlists?limit=100';
  while (next) {
    const data = await apiFetch(next, devToken, userToken);
    for (const p of (data?.data || [])) {
      all.push({
        id:         p.id,
        name:       p.attributes?.name || 'Unnamed',
        trackCount: p.attributes?.trackCount
      });
    }
    next = data?.next || null;
  }
  return all;
}

// ── Auth: one-time browser flow (minimal — just to get the Music User Token) ──

function requestUserToken(devToken) {
  // We still need a browser once to do the MusicKit OAuth handshake.
  // After that the token is cached on disk and this never runs again.
  const http = require('http');
  const port = 59743;

  return new Promise((resolve, reject) => {
    const server = http.createServer((req, res) => {
      if (req.method === 'GET' && req.url === '/') {
        res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
        res.end(authHtml(devToken));
        return;
      }
      if (req.method === 'POST' && req.url === '/token') {
        let body = '';
        req.on('data', d => body += d);
        req.on('end', () => {
          try {
            const { userToken } = JSON.parse(body);
            saveUserToken(userToken);
            res.writeHead(200, { 'Content-Type': 'application/json' });
            res.end('{"ok":true}');
            server.close();
            resolve(userToken);
          } catch { res.writeHead(400); res.end(); }
        });
        return;
      }
      res.writeHead(404); res.end();
    });

    server.listen(port, '127.0.0.1', () => exec(`open http://localhost:${port}`));
    server.on('error', reject);
    setTimeout(() => { server.close(); reject(new Error('Auth timed out')); }, 300_000).unref();
  });
}

function authHtml(devToken) {
  return `<!DOCTYPE html><html lang="en"><head><meta charset="utf-8">
<title>radio-cli · auth</title>
<style>
  *{box-sizing:border-box;margin:0;padding:0}
  body{font-family:-apple-system,sans-serif;background:#000;color:#fff;
       display:flex;align-items:center;justify-content:center;height:100vh}
  .card{background:#1c1c1e;border-radius:18px;padding:48px 40px;text-align:center;max-width:380px;width:90%}
  h1{font-size:22px;font-weight:700;margin-bottom:8px}
  p{color:#888;font-size:14px;line-height:1.6;margin-bottom:28px}
  button{background:#fc3c44;color:#fff;border:none;border-radius:10px;
         padding:14px 32px;font-size:16px;font-weight:600;cursor:pointer;width:100%}
  button:disabled{opacity:.4;cursor:default}
  #s{margin-top:16px;font-size:13px;color:#aaa;min-height:20px}
</style></head><body>
<div class="card">
  <h1>radio-cli</h1>
  <p>One-time sign-in to access your Apple Music library.<br>
     The token is saved locally — you won't be asked again.</p>
  <button id="btn">Sign in with Apple Music</button>
  <div id="s"></div>
</div>
<script src="https://js-cdn.music.apple.com/musickit/v3/musickit.js" data-web-components></script>
<script>
const btn=document.getElementById('btn'),s=document.getElementById('s');
btn.addEventListener('click',async()=>{
  btn.disabled=true;
  try{
    s.textContent='Configuring\u2026';
    await MusicKit.configure({developerToken:${JSON.stringify(devToken)},app:{name:'radio-cli',build:'1.0.0'}});
    const m=MusicKit.getInstance();
    s.textContent='Waiting for authorisation\u2026';
    const userToken=await m.authorize();
    s.textContent='Saving token\u2026';
    await fetch('/token',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({userToken})});
    s.textContent='\u2713 Done \u2014 you can close this tab.';
  }catch(e){s.textContent='\u2717 '+e.message;btn.disabled=false;}
});
</script></body></html>`;
}

// ── Playback via Music.app (hidden) ───────────────────────────────────────

function runAppleScript(script) {
  const tmp = `/tmp/radio-cli-${Date.now()}.scpt`;
  fs.writeFileSync(tmp, script, 'utf8');
  return new Promise((resolve, reject) => {
    exec(`osascript "${tmp}"`, (err, stdout) => {
      fs.unlink(tmp, () => {});
      if (err) reject(err); else resolve(stdout.trim());
    });
  });
}

/**
 * Play a library playlist via Music.app, then immediately hide Music.app
 * so it never appears on screen. Audio routes through system audio as normal.
 */
async function playPlaylist(name) {
  const safe = name.replace(/\\/g, '\\\\').replace(/"/g, '\\"');
  await runAppleScript(
    `tell application "Music"\n` +
    `  play (first playlist whose name is "${safe}")\n` +
    `end tell\n` +
    `tell application "System Events"\n` +
    `  tell process "Music" to set visible to false\n` +
    `end tell`
  );
}

function stopMusic() {
  return runAppleScript(
    `tell application "Music" to stop`
  ).catch(() => {});
}

/**
 * Returns "Track — Artist" if Music.app is playing, otherwise "".
 */
function getNowPlaying() {
  return runAppleScript(
    `tell application "Music"\n` +
    `  if player state is playing then\n` +
    `    return name of current track & " \u2014 " & artist of current track\n` +
    `  end if\n` +
    `  return ""\n` +
    `end tell`
  ).catch(() => '');
}

// ── Config ─────────────────────────────────────────────────────────────────

function loadConfig() {
  const cfgPath = path.join(__dirname, 'config.json');
  if (!fs.existsSync(cfgPath)) return null;
  const cfg   = JSON.parse(fs.readFileSync(cfgPath, 'utf8'));
  const apple = cfg?.apple;
  if (!apple?.teamId || !apple?.keyId || !apple?.privateKeyPath) return null;
  const pkPath = path.resolve(__dirname, apple.privateKeyPath);
  if (!fs.existsSync(pkPath)) throw new Error(`Private key not found: ${pkPath}`);
  return { teamId: apple.teamId, keyId: apple.keyId, privateKey: fs.readFileSync(pkPath, 'utf8') };
}

module.exports = {
  loadConfig,
  generateDeveloperToken,
  loadUserToken,
  saveUserToken,
  requestUserToken,
  getLibraryPlaylists,
  playPlaylist,
  stopMusic,
  getNowPlaying
};
