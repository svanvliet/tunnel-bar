import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const design = path.dirname(fileURLToPath(import.meta.url));
const icons = path.join(design, '../src-tauri/icons');
const render = (source, output, size, height = size) => execFileSync('rsvg-convert',
  ['-w', String(size), '-h', String(height), '-o', output], { input: source });
const app = readFileSync(path.join(design, 'icon.svg'), 'utf8');
render(app, path.join(design, 'icon.png'), 1024);
render(app, path.join(design, 'icon-128.png'), 128);
render(app, path.join(design, '../static/favicon.png'), 32);

const states = ['disconnected', 'busy', 'connected', 'error'];
const traySource = (state) => readFileSync(path.join(design, 'tray', `${state}.svg`), 'utf8');

// macOS: template images (black ink + alpha), margins cropped so the glyph matches the optical height of
// other menu bar items (22x18pt @2x).
mkdirSync(path.join(icons, 'tray', 'macos'), { recursive: true });
for (const state of states) {
  const source = traySource(state)
    .replace('width="36" height="36" viewBox="0 0 100 100"', 'width="44" height="36" viewBox="-2 5 97.78 80"');
  render(source, path.join(icons, 'tray', 'macos', `${state}.png`), 44, 36);
}

// Windows: solid glyphs like the system tray icons, white for a dark taskbar and black for a light one.
// Tight square crop and heavier strokes so the glyph holds up at 16px; 32px halves cleanly for 100% scale.
for (const [theme, ink] of [['dark', '#FFF'], ['light', '#000']]) {
  mkdirSync(path.join(icons, 'tray', 'windows', theme), { recursive: true });
  for (const state of states) {
    const source = traySource(state)
      .replace('viewBox="0 0 100 100"', 'viewBox="-2 -3 98 98"')
      .replaceAll('stroke-width="7"', 'stroke-width="10"')
      .replaceAll('stroke-width="5"', 'stroke-width="7"')
      .replace('stroke-dasharray="5 4.5"', 'stroke-dasharray="6 5"')
      // The error badge's exclamation is a black mask stroke; keep it black so it still knocks out.
      .replace('stroke="#000" stroke-width="4"', 'stroke="MASK" stroke-width="5.5"')
      .replaceAll('#000"', `${ink}"`)
      .replace('stroke="MASK"', 'stroke="#000"');
    render(source, path.join(icons, 'tray', 'windows', theme, `${state}.png`), 32);
  }
}
const generated = mkdtempSync(path.join(icons, '.generated-'));
try {
  execFileSync('npx', ['--no-install', 'tauri', 'icon', path.join(design, 'icon.png'),
    '--output', generated], { stdio: 'inherit', cwd: path.join(design, '..') });
  for (const entry of readdirSync(generated, { withFileTypes: true })) {
    if (entry.isFile()) copyFileSync(path.join(generated, entry.name), path.join(icons, entry.name));
  }
} finally {
  rmSync(generated, { recursive: true, force: true });
}
