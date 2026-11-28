// Web build for the desktop shell (E3, E9): relative `base` (GUBIM_DESKTOP=1) and the
// Content-Security-Policy of desktop/content-security-policy.txt as a <meta> in index.html.
// The shell serves dist/ from its own origin, so the policy travels with the page.
import { spawnSync } from 'node:child_process';
import { copyFileSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const env = { ...process.env, GUBIM_DESKTOP: '1' };
const run = (cmd, args) => {
  const res = spawnSync(cmd, args, { stdio: 'inherit', env, cwd: ROOT, shell: process.platform === 'win32' });
  if (res.status !== 0) process.exit(res.status ?? 1);
};

run('npm', ['run', 'kernel']);
run('npx', ['tsc', '--noEmit']);
run('npx', ['vite', 'build']);

const csp = readFileSync(path.join(ROOT, 'desktop', 'content-security-policy.txt'), 'utf8').trim();
if (/frame-ancestors/.test(csp)) throw new Error('frame-ancestors is ignored in a <meta> CSP: remove it');
const indexPath = path.join(ROOT, 'dist', 'index.html');
const html = readFileSync(indexPath, 'utf8');
if (html.includes('http-equiv="Content-Security-Policy"')) throw new Error('index.html already has a CSP');
const meta = `<meta http-equiv="Content-Security-Policy" content="${csp.replace(/"/g, '&quot;')}">`;
const out = html.replace(/<head>/i, `<head>\n    ${meta}`);
if (out === html) throw new Error('no <head> in dist/index.html');
writeFileSync(indexPath, out);
// WebView2 asks for /favicon.ico; without it every start logs a 404.
copyFileSync(path.join(ROOT, 'desktop', 'icons', 'icon.ico'), path.join(ROOT, 'dist', 'favicon.ico'));
console.log('build-desktop: CSP added to dist/index.html, favicon.ico copied');
