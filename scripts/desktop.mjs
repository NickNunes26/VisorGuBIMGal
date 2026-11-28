// Desktop shell (E9) tasks.
//   node scripts/desktop.mjs dev    Vite dev server + the shell pointed at it (--dev-url).
//   node scripts/desktop.mjs build  desktop web build, release shell, staged folder, NSIS installer:
//                                   desktop/target/release/bundle/VisorGuBIMGal_<version>_x64-setup.exe
import { spawn, spawnSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const DESKTOP = path.join(ROOT, 'desktop');
const mode = process.argv[2];

const run = (cmd, args, opts = {}) => {
  const res = spawnSync(cmd, args, { stdio: 'inherit', cwd: ROOT, shell: process.platform === 'win32', ...opts });
  if (res.status !== 0) process.exit(res.status ?? 1);
};

function version() {
  const toml = readFileSync(path.join(DESKTOP, 'Cargo.toml'), 'utf8');
  const m = /^version = "([^"]+)"/m.exec(toml);
  if (!m) throw new Error('no version in desktop/Cargo.toml');
  return m[1];
}

function findMakensis() {
  const candidates = [
    process.env.MAKENSIS,
    process.env.LOCALAPPDATA && path.join(process.env.LOCALAPPDATA, 'tauri', 'NSIS', 'makensis.exe'),
    'C:\Program Files (x86)\NSIS\makensis.exe',
    'C:\Program Files\NSIS\makensis.exe'
  ].filter(Boolean);
  for (const c of candidates) if (existsSync(c)) return c;
  const where = spawnSync('where', ['makensis'], { encoding: 'utf8', shell: true });
  if (where.status === 0) return where.stdout.split(/\r?\n/)[0].trim();
  throw new Error('makensis not found: install NSIS 3 or set MAKENSIS');
}

if (mode === 'dev') {
  const port = process.env.GUBIM_DEV_PORT ?? '5173';
  const url = `http://localhost:${port}`;
  const vite = spawn(process.execPath, [path.join(ROOT, 'node_modules/vite/bin/vite.js'), '--port', port, '--strictPort'], {
    cwd: ROOT,
    stdio: 'inherit',
    env: { ...process.env, GUBIM_DESKTOP: '1' }
  });
  for (let i = 0; i < 300; i++) {
    try {
      if ((await fetch(url)).ok) break;
    } catch {}
    await new Promise((r) => setTimeout(r, 200));
  }
  const shell = spawnSync('cargo', ['run', '--', '--dev-url', url], { cwd: DESKTOP, stdio: 'inherit', shell: true });
  vite.kill();
  process.exit(shell.status ?? 0);
} else if (mode === 'build') {
  run('node', ['scripts/build-desktop.mjs']);
  run('cargo', ['build', '--release'], { cwd: DESKTOP });
  const release = path.join(DESKTOP, 'target', 'release');
  const bundle = path.join(release, 'bundle');
  const stage = path.join(bundle, 'stage');
  rmSync(bundle, { recursive: true, force: true });
  mkdirSync(stage, { recursive: true });
  cpSync(path.join(release, 'VisorGuBIMGal.exe'), path.join(stage, 'VisorGuBIMGal.exe'));
  cpSync(path.join(ROOT, 'NOTICE.md'), path.join(stage, 'NOTICE.md'));
  cpSync(path.join(ROOT, 'dist'), path.join(stage, 'app'), { recursive: true });
  const v = version();
  const outFile = path.join(bundle, `VisorGuBIMGal_${v}_x64-setup.exe`);
  run(findMakensis(), ['/V2', `/DVERSION=${v}`, `/DSTAGE=${stage}`, `/DOUTFILE=${outFile}`, path.join(DESKTOP, 'installer', 'installer.nsi')], { shell: false });
  console.log(`desktop: installer ${outFile}`);
} else {
  console.error('usage: node scripts/desktop.mjs <dev|build>');
  process.exit(2);
}
