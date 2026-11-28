// Desktop shell check (E9, Windows only): drives the real VisorGuBIMGal.exe through WebView2's
// remote debugging port. Needs `npm run desktop:build` first (it reads the staged folder
// desktop/target/release/bundle/stage) and builds the debug shell for the save check.
//
//   D1 an .ifc passed on the command line (path with spaces and ñ) opens, served from
//      https://app.gubim.localhost with the desktop CSP, WebGPU active, the bytes read through
//      https://gubim-file.localhost in ranges whose sizes add up to the file size.
//   D2 a second launch with another .ifc exits at once and the first window opens it: two models,
//      one VisorGuBIMGal process.
//   D3 a 300 MB file read through gubim-file.localhost in 16 MiB ranges, as the app reads it,
//      arrives intact (size and per-MB markers) while the shell's peak working set grows by less
//      than 256 MB (E4). One whole-file response peaked at 2.4 GB: WebView2 copies responses.
//   D4 (debug shell) the page cannot leave the app: a link to another site, followed in place or
//      in a new window, is handed to the user's browser (here a log, GUBIM_DESKTOP_TEST_EXTERNAL_LOG)
//      and never requested by the WebView; the page and its model stay.
//   D5 the bridge refuses an unknown command with ok:false.
//   D6 saving (debug shell, dialog replaced by GUBIM_DESKTOP_TEST_SAVE_DIR): the CSV and GLB
//      exports land on disk with their real content.
//   D7 no console errors, exceptions or CSP violations.
// Usage: node test/e2e/desktop-check.mjs [outDir]   (outDir outside the repo; default <tmp>/gubim-desktop)
import { spawn, spawnSync } from 'node:child_process';
import { createServer } from 'node:http';
import { closeSync, copyFileSync, existsSync, mkdirSync, openSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync, writeSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { connect, sleep } from './cdp.mjs';

if (process.platform !== 'win32') {
  console.log('desktop-check: Windows only, skipped');
  process.exit(0);
}

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const STAGE = path.join(REPO, 'desktop', 'target', 'release', 'bundle', 'stage');
const EXE = path.join(STAGE, 'VisorGuBIMGal.exe');
const DEBUG_EXE = path.join(REPO, 'desktop', 'target', 'debug', 'VisorGuBIMGal.exe');
const OUT = path.resolve(process.argv[2] ?? path.join(os.tmpdir(), 'gubim-desktop'));
if (OUT.startsWith(REPO)) throw new Error('outDir must be outside the repo');
if (!existsSync(EXE)) throw new Error(`missing ${EXE}: run npm run desktop:build`);
rmSync(OUT, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
mkdirSync(OUT, { recursive: true });

const results = [];
const check = (id, ok, detail) => {
  results.push(ok);
  console.log(`${ok ? 'PASS' : 'FAIL'} ${id} ${JSON.stringify(detail)}`);
};

const fileA = path.join(OUT, 'modelo planta ñ.ifc');
const fileB = path.join(OUT, 'segundo.ifc');
copyFileSync(path.join(REPO, 'public', 'samples', 'SimpleWall.ifc'), fileA);
copyFileSync(path.join(REPO, 'public', 'samples', 'ManySimpleWalls.ifc'), fileB);
const cspFile = readFileSync(path.join(REPO, 'desktop', 'content-security-policy.txt'), 'utf8').trim();

const PORT = 9555;
const envFor = (profile, extra = {}) => ({
  ...process.env,
  WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}`,
  GUBIM_DESKTOP_DATA_DIR: path.join(OUT, profile),
  ...extra
});

function shellPids() {
  const r = spawnSync('tasklist', ['/FI', 'IMAGENAME eq VisorGuBIMGal.exe', '/FO', 'CSV', '/NH'], { encoding: 'utf8' });
  return r.stdout
    .split(/\r?\n/)
    .filter((l) => l.startsWith('"VisorGuBIMGal.exe"'))
    .map((l) => Number(l.split('","')[1]));
}
function peakWorkingSet(pid) {
  const r = spawnSync('powershell', ['-NoProfile', '-Command', `(Get-Process -Id ${pid}).PeakWorkingSet64`], { encoding: 'utf8' });
  return Number(r.stdout.trim());
}
function killShells() {
  for (const pid of shellPids()) spawnSync('taskkill', ['/F', '/T', '/PID', String(pid)]);
}

async function attach() {
  let target = null;
  for (let i = 0; i < 150 && !target; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
      target = list.find((t) => t.type === 'page' && t.url.startsWith('https://app.gubim.localhost'));
    } catch {}
    if (!target) await sleep(200);
  }
  if (!target) throw new Error('no WebView2 page on the debugging port');
  const cdp = await connect(target.webSocketDebuggerUrl);
  const errors = [];
  cdp.on('Runtime.consoleAPICalled', (p) => {
    if (p.type === 'error') errors.push(p.args.map((a) => a.value ?? a.description).join(' '));
  });
  cdp.on('Runtime.exceptionThrown', (p) => errors.push(p.exceptionDetails?.exception?.description ?? p.exceptionDetails?.text));
  cdp.on('Log.entryAdded', (p) => {
    if (p.entry.level === 'error') errors.push(`log ${p.entry.source}: ${p.entry.text}`);
  });
  await cdp.send('Runtime.enable');
  await cdp.send('Log.enable');
  await cdp.send('Page.enable');
  const waitFor = async (expr, timeout = 60000) => {
    const t0 = Date.now();
    while (Date.now() - t0 < timeout) {
      try {
        const v = await cdp.evaluate(expr);
        if (v) return v;
      } catch {}
      await sleep(150);
    }
    throw new Error('timeout ' + expr);
  };
  await waitFor(`document.readyState === 'complete' && !!document.querySelector('input[type=file][accept=".ifc"]')`);
  await cdp.evaluate(`(window.__csp = [], document.addEventListener('securitypolicyviolation', (e) => window.__csp.push(e.violatedDirective + ' ' + e.blockedURI)), true)`);
  return { cdp, errors, waitFor };
}

/** Sends one request over the shell bridge from inside the page and returns its reply. */
const bridgeCall = (cdp, cmd, args) =>
  cdp.evaluate(`new Promise((resolve) => {
    const id = 900000 + Math.floor(Math.random() * 1000);
    const onMsg = (e) => { if (e.data && e.data.id === id) { window.chrome.webview.removeEventListener('message', onMsg); resolve(e.data); } };
    window.chrome.webview.addEventListener('message', onMsg);
    window.chrome.webview.postMessage({ id, cmd: ${JSON.stringify(cmd)}, args: ${JSON.stringify(args ?? null)} });
  })`);

async function shot(cdp, name) {
  const { data } = await cdp.send('Page.captureScreenshot', { format: 'png' });
  writeFileSync(path.join(OUT, `${name}.png`), Buffer.from(data, 'base64'));
}

killShells();
let session = null;
try {
  // D1
  const first = spawn(EXE, [fileA], { env: envFor('profile-release'), stdio: 'ignore' });
  session = await attach();
  const { cdp, errors, waitFor } = session;
  await waitFor(`document.querySelectorAll('.model-card[data-status="ready"]').length === 1`, 90000);
  const d1 = await cdp.evaluate(`(() => ({
    origin: location.origin,
    csp: document.querySelector('meta[http-equiv="Content-Security-Policy"]')?.content ?? null,
    badge: document.querySelector('.status-badge')?.textContent?.trim() ?? null,
    card: document.querySelector('.model-card')?.textContent ?? '',
    file: performance.getEntriesByType('resource').filter((e) => e.name.startsWith('https://gubim-file.localhost/')).map((e) => ({ name: e.name, size: e.decodedBodySize }))
  }))()`);
  check(
    'D1 launch argument opens; app origin, desktop CSP, WebGPU, bytes via gubim-file.localhost',
    d1.origin === 'https://app.gubim.localhost' &&
      d1.csp === cspFile &&
      /WebGPU/.test(d1.badge ?? '') &&
      d1.card.includes('modelo planta ñ.ifc') &&
      d1.file.length >= 1 &&
      d1.file.every((f) => /\/f-1\?start=\d+&end=\d+$/.test(f.name)) &&
      d1.file.reduce((n, f) => n + f.size, 0) === statSync(fileA).size,
    { ...d1, card: d1.card.slice(0, 80), fileSize: statSync(fileA).size }
  );
  await shot(cdp, 'D1-first-model');

  // D2
  const pidsBefore = shellPids();
  const t0 = Date.now();
  const second = spawnSync(EXE, [fileB], { env: envFor('profile-release'), timeout: 20000 });
  const secondMs = Date.now() - t0;
  await waitFor(`document.querySelectorAll('.model-card[data-status="ready"]').length === 2`, 90000);
  const pidsAfter = shellPids();
  const cards = await cdp.evaluate(`[...document.querySelectorAll('.model-card')].map((c) => c.textContent.includes('segundo.ifc'))`);
  check(
    'D2 second launch hands its file over and exits; one shell process; two models',
    second.status === 0 && secondMs < 15000 && pidsAfter.length === 1 && pidsBefore.length === 1 && pidsAfter[0] === first.pid && cards.includes(true),
    { exit: second.status, secondMs, pidsBefore, pidsAfter, firstPid: first.pid }
  );
  await shot(cdp, 'D2-second-instance');

  // D3
  const MB = 1 << 20;
  const big = path.join(OUT, 'grande.ifc');
  const fd = openSync(big, 'w');
  const chunk = Buffer.alloc(MB, 0x20);
  for (let i = 0; i < 300; i++) {
    chunk.write(`#MB${String(i).padStart(4, '0')}`, 0, 'latin1');
    writeSync(fd, chunk);
  }
  closeSync(fd);
  const peak0 = peakWorkingSet(first.pid);
  const reg = await bridgeCall(cdp, 'registerIfcPath', { path: big });
  const read = await cdp.evaluate(`(async () => {
    const t = performance.now();
    const token = ${JSON.stringify(reg.result?.token ?? '')}, size = ${JSON.stringify(reg.result?.size ?? 0)}, CHUNK = 16 * ${MB};
    const u8 = new Uint8Array(size);
    for (let start = 0; start < size; start += CHUNK) {
      const end = Math.min(start + CHUNK, size);
      const r = await fetch('https://gubim-file.localhost/' + encodeURIComponent(token) + '?start=' + start + '&end=' + end);
      u8.set(new Uint8Array(await r.arrayBuffer()), start);
    }
    const ms = performance.now() - t;
    let markers = 0;
    for (let i = 0; i < 300; i++) {
      const s = String.fromCharCode(...u8.subarray(i * ${MB}, i * ${MB} + 7));
      if (s === '#MB' + String(i).padStart(4, '0')) markers++;
    }
    return { bytes: u8.length, markers, ms: Math.round(ms) };
  })()`);
  const peak1 = peakWorkingSet(first.pid);
  const growthMb = (peak1 - peak0) / MB;
  check(
    'D3 300 MB file read in ranges arrives intact; shell peak working set grows < 256 MB',
    reg.ok === true && read.bytes === 300 * MB && read.markers === 300 && growthMb < 256,
    { token: reg.result?.token, ...read, peakBeforeMb: +(peak0 / MB).toFixed(1), peakAfterMb: +(peak1 / MB).toFixed(1), growthMb: +growthMb.toFixed(1) }
  );

  // D5
  const d5 = await bridgeCall(cdp, 'deleteEverything', {});
  check('D5 unknown bridge command is refused', d5.ok === false && /desconocida/.test(d5.error), d5);

  const csp = await cdp.evaluate('window.__csp');
  check('D7 release shell: no console errors, exceptions or CSP violations', errors.length === 0 && csp.length === 0, { errors: errors.slice(0, 5), csp });

  cdp.close();
  killShells();
  await sleep(800);

  // D6
  const build = spawnSync('cargo', ['build'], { cwd: path.join(REPO, 'desktop'), stdio: 'inherit', shell: true });
  if (build.status !== 0) throw new Error('cargo build (debug) failed');
  const saves = path.join(OUT, 'saves');
  const externalLog = path.join(OUT, 'external.log');
  mkdirSync(saves, { recursive: true });
  spawn(DEBUG_EXE, ['--app-dir', path.join(STAGE, 'app'), fileA], {
    env: envFor('profile-debug', { GUBIM_DESKTOP_TEST_SAVE_DIR: saves, GUBIM_DESKTOP_TEST_EXTERNAL_LOG: externalLog }),
    stdio: 'ignore'
  });
  session = await attach();
  const dbg = session;
  await dbg.waitFor(`document.querySelectorAll('.model-card[data-status="ready"]').length === 1`, 90000);
  await dbg.cdp.evaluate(`[...document.querySelectorAll('button')].find((b) => b.textContent.trim() === 'CSV').click(), true`);
  const waitFile = async (pred) => {
    for (let i = 0; i < 100; i++) {
      const f = readdirSync(saves).find(pred);
      if (f) {
        await sleep(300);
        return path.join(saves, f);
      }
      await sleep(200);
    }
    return null;
  };
  const csvPath = await waitFile((n) => n.endsWith('.csv'));
  await dbg.cdp.evaluate(`[...document.querySelectorAll('button')].find((b) => b.textContent.trim() === 'Exportar GLB').click(), true`);
  const glbPath = await waitFile((n) => n.endsWith('.glb'));
  const csv = csvPath ? readFileSync(csvPath, 'utf8') : '';
  const glb = glbPath ? readFileSync(glbPath) : Buffer.alloc(0);
  const glbOk = glb.length > 20 && glb.toString('latin1', 0, 4) === 'glTF' && glb.readUInt32LE(8) === glb.length;
  check(
    'D6 CSV and GLB exports are saved through the shell with their real content',
    !!csvPath && csv.split(/\r?\n/).length > 2 && /muro|wall|IfcWall/i.test(csv) && glbOk,
    { csv: csvPath && path.basename(csvPath), csvLines: csv.split(/\r?\n/).length, glb: glbPath && path.basename(glbPath), glbBytes: glb.length, glbOk }
  );
  await shot(dbg.cdp, 'D6-debug-saved');
  const csp2 = await dbg.cdp.evaluate('window.__csp');
  check('D7 debug shell: no console errors, exceptions or CSP violations', dbg.errors.length === 0 && csp2.length === 0, { errors: dbg.errors.slice(0, 5), csp: csp2 });

  // D4: a stand-in for another site, counting the requests that reach it.
  const hits = [];
  const site = createServer((req, res) => {
    hits.push(req.url);
    res.end('<html><body>fuera</body></html>');
  });
  await new Promise((r) => site.listen(0, '127.0.0.1', r));
  const siteUrl = `http://127.0.0.1:${site.address().port}`;
  await dbg.cdp.evaluate(`(location.href = '${siteUrl}/en-sitio', true)`).catch(() => {});
  await sleep(1500);
  await dbg.cdp.evaluate(`(window.open('${siteUrl}/ventana-nueva'), true)`).catch(() => {});
  await sleep(1500);
  const d4 = await dbg.cdp.evaluate(`({ origin: location.origin, models: document.querySelectorAll('.model-card[data-status="ready"]').length })`);
  const handedOver = existsSync(externalLog) ? readFileSync(externalLog, 'utf8').trim().split(/\r?\n/) : [];
  site.close();
  check(
    "D4 links to other sites go to the user's browser; the WebView never requests them and stays on the app",
    d4.origin === 'https://app.gubim.localhost' &&
      d4.models === 1 &&
      hits.length === 0 &&
      JSON.stringify(handedOver) === JSON.stringify([`${siteUrl}/en-sitio`, `${siteUrl}/ventana-nueva`]),
    { ...d4, hits, handedOver }
  );
  dbg.cdp.close();
} catch (err) {
  console.log('FAIL exception', err.stack ?? err);
  results.push(false);
  try {
    if (session) await shot(session.cdp, '99-error');
  } catch {}
} finally {
  killShells();
  await sleep(1000);
  for (const name of ['profile-release', 'profile-debug', 'grande.ifc']) {
    rmSync(path.join(OUT, name), { recursive: true, force: true, maxRetries: 10, retryDelay: 300 });
  }
}
const ok = results.length > 0 && results.every(Boolean);
console.log(`desktop: ${results.filter(Boolean).length}/${results.length} pass (output ${OUT})`);
process.exit(ok ? 0 : 1);
