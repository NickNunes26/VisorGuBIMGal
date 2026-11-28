// Installer round trip (E9, Windows only). Installs the NSIS installer silently into a throwaway
// folder, opens .ifc files the way a double-click does (the shell's "open" verb, through the
// association the installer wrote), then uninstalls and checks nothing is left behind.
//
//   I1 silent install: the exe, the web app, NOTICE.md and the uninstaller in the folder; the .ifc
//      association, Start-menu shortcut and uninstall entry written for this user.
//   I2 "double-click" an .ifc: the installed exe starts and opens it (WebGPU, one model).
//   I3 "double-click" a second .ifc: same process, two models.
//   I4 silent uninstall: folder, association, shortcut and uninstall entry gone.
//
// It changes this user's .ifc association while it runs, so it refuses to start if one exists.
// Usage: node test/e2e/installer-check.mjs [outDir]   (needs npm run desktop:build first)
import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, readdirSync, rmSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { connect, sleep } from './cdp.mjs';

if (process.platform !== 'win32') {
  console.log('installer-check: Windows only, skipped');
  process.exit(0);
}
const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const BUNDLE = path.join(REPO, 'desktop', 'target', 'release', 'bundle');
const setup = readdirSync(BUNDLE).find((n) => n.endsWith('-setup.exe'));
if (!setup) throw new Error('no installer: run npm run desktop:build');
const OUT = path.resolve(process.argv[2] ?? path.join(os.tmpdir(), 'gubim-installer'));
const INSTALL = path.join(OUT, 'instalado');
const PORT = 9557;

const reg = (key, value) => {
  const args = ['query', key];
  if (value === '') args.push('/ve');
  else if (value) args.push('/v', value);
  const r = spawnSync('reg', args, { encoding: 'utf8' });
  return r.status === 0 ? r.stdout : null;
};
const KEYS = {
  ext: 'HKCU\\Software\\Classes\\.ifc',
  progid: 'HKCU\\Software\\Classes\\VisorGuBIMGal.ifc',
  open: 'HKCU\\Software\\Classes\\VisorGuBIMGal.ifc\\shell\\open\\command',
  uninst: 'HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\VisorGuBIMGal'
};
if (reg(KEYS.ext) || reg(KEYS.progid) || reg(KEYS.uninst)) {
  console.log('installer-check: this user already has an .ifc association or an install; not touching it');
  process.exit(1);
}
const shortcut = path.join(process.env.APPDATA, 'Microsoft', 'Windows', 'Start Menu', 'Programs', 'VisorGuBIMGal.lnk');

rmSync(OUT, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
mkdirSync(OUT, { recursive: true });
const fileA = path.join(OUT, 'doble clic ñ.ifc');
const fileB = path.join(OUT, 'segundo.ifc');
copyFileSync(path.join(REPO, 'public', 'samples', 'SimpleWall.ifc'), fileA);
copyFileSync(path.join(REPO, 'public', 'samples', 'ManySimpleWalls.ifc'), fileB);

const results = [];
const check = (id, ok, detail) => {
  results.push(ok);
  console.log(`${ok ? 'PASS' : 'FAIL'} ${id} ${JSON.stringify(detail)}`);
};
const shellPids = () =>
  spawnSync('tasklist', ['/FI', 'IMAGENAME eq VisorGuBIMGal.exe', '/FO', 'CSV', '/NH'], { encoding: 'utf8' })
    .stdout.split(/\r?\n/)
    .filter((l) => l.startsWith('"VisorGuBIMGal.exe"'))
    .map((l) => Number(l.split('","')[1]));
const exePathOf = (pid) =>
  spawnSync('powershell', ['-NoProfile', '-Command', `(Get-Process -Id ${pid}).Path`], { encoding: 'utf8' }).stdout.trim();
/** Opens a file with its associated program, as Explorer does on a double-click. */
const shellOpen = (file) =>
  spawnSync('powershell', ['-NoProfile', '-Command', `Start-Process -FilePath '${file.replace(/'/g, "''")}'`], {
    encoding: 'utf8',
    env: {
      ...process.env,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${PORT}`,
      GUBIM_DESKTOP_DATA_DIR: path.join(OUT, 'profile')
    }
  });

let cdp = null;
try {
  // I1
  const inst = spawnSync(path.join(BUNDLE, setup), ['/S', `/D=${INSTALL}`], { timeout: 120000 });
  const files = ['VisorGuBIMGal.exe', 'NOTICE.md', 'uninstall.exe', path.join('app', 'index.html')].map((f) => existsSync(path.join(INSTALL, f)));
  const openCmd = reg(KEYS.open, '') ?? '';
  check(
    'I1 silent install writes the files, the .ifc association, the shortcut and the uninstall entry',
    inst.status === 0 &&
      files.every(Boolean) &&
      /VisorGuBIMGal\.ifc/.test(reg(KEYS.ext, '') ?? '') &&
      openCmd.includes(`"${path.join(INSTALL, 'VisorGuBIMGal.exe')}" "%1"`) &&
      existsSync(shortcut) &&
      !!reg(KEYS.uninst, 'UninstallString'),
    { exit: inst.status, files, openCmd: openCmd.trim().split(/\r?\n/).pop(), shortcut: existsSync(shortcut) }
  );

  // I2
  shellOpen(fileA);
  let target = null;
  for (let i = 0; i < 150 && !target; i++) {
    try {
      target = (await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json()).find((t) => t.type === 'page' && t.url.startsWith('https://app.gubim.localhost'));
    } catch {}
    if (!target) await sleep(200);
  }
  if (!target) throw new Error('the installed app did not start');
  cdp = await connect(target.webSocketDebuggerUrl);
  await cdp.send('Runtime.enable');
  const waitFor = async (expr, timeout = 90000) => {
    const t0 = Date.now();
    while (Date.now() - t0 < timeout) {
      try {
        if (await cdp.evaluate(expr)) return;
      } catch {}
      await sleep(200);
    }
    throw new Error('timeout ' + expr);
  };
  await waitFor(`document.querySelectorAll('.model-card[data-status="ready"]').length === 1`);
  const pids1 = shellPids();
  const i2 = await cdp.evaluate(`({ badge: document.querySelector('.status-badge')?.textContent.trim(), card: document.querySelector('.model-card')?.textContent })`);
  check(
    'I2 double-click opens the .ifc in the installed app',
    pids1.length === 1 && exePathOf(pids1[0]).toLowerCase() === path.join(INSTALL, 'VisorGuBIMGal.exe').toLowerCase() && /WebGPU/.test(i2.badge) && i2.card.includes('doble clic ñ.ifc'),
    { pids: pids1, exe: pids1[0] && exePathOf(pids1[0]), ...i2, card: i2.card?.slice(0, 60) }
  );

  // I3
  shellOpen(fileB);
  await waitFor(`document.querySelectorAll('.model-card[data-status="ready"]').length === 2`);
  const pids2 = shellPids();
  check('I3 a second double-click reaches the running window', pids2.length === 1 && pids2[0] === pids1[0], { pids: pids2 });
  cdp.close();
  cdp = null;
} catch (err) {
  console.log('FAIL exception', err.stack ?? err);
  results.push(false);
} finally {
  if (cdp) cdp.close();
  for (const pid of shellPids()) spawnSync('taskkill', ['/F', '/T', '/PID', String(pid)]);
  await sleep(1500);
  // I4: `_?=` runs the uninstaller in place and waits for it.
  const un = existsSync(path.join(INSTALL, 'uninstall.exe'))
    ? spawnSync(path.join(INSTALL, 'uninstall.exe'), ['/S', `_?=${INSTALL}`], { timeout: 120000 })
    : { status: -1 };
  // Checked before any clean-up of ours. Run in place, the uninstaller cannot delete itself, so
  // only uninstall.exe may remain in the folder.
  const remaining = existsSync(INSTALL) ? readdirSync(INSTALL).filter((n) => n !== 'uninstall.exe') : [];
  const left = {
    files: remaining,
    ext: !!reg(KEYS.ext),
    progid: !!reg(KEYS.progid),
    uninst: !!reg(KEYS.uninst),
    shortcut: existsSync(shortcut)
  };
  check(
    'I4 silent uninstall leaves no files, association, shortcut or uninstall entry',
    un.status === 0 && left.files.length === 0 && !left.ext && !left.progid && !left.uninst && !left.shortcut,
    { exit: un.status, ...left }
  );
  rmSync(OUT, { recursive: true, force: true, maxRetries: 10, retryDelay: 300 });
}
const ok = results.length > 0 && results.every(Boolean);
console.log(`installer: ${results.filter(Boolean).length}/${results.length} pass`);
process.exit(ok ? 0 : 1);
