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

for (const platform of ['macos', 'windows']) {
  mkdirSync(path.join(icons, 'tray', platform), { recursive: true });
  for (const state of ['disconnected', 'busy', 'connected', 'error']) {
    let source = readFileSync(path.join(design, 'tray', `${state}.svg`), 'utf8');
    if (platform === 'windows') {
      // A white outer edge and black center remain visible on either taskbar theme.
      source = source.replace('id="glyph"', 'id="glyph" filter="url(#outline)"')
        .replace('</svg>', `<defs><filter id="outline" x="-25%" y="-25%" width="150%" height="150%">
          <feMorphology in="SourceAlpha" operator="dilate" radius="3" result="edge"/>
          <feFlood flood-color="#FFF"/>
          <feComposite in2="edge" operator="in"/>
          <feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge>
        </filter></defs></svg>`);
    }
    if (platform === 'macos') {
      // Crop the empty margins so the glyph matches the optical height of other menu bar items (22x18pt @2x).
      source = source.replace('width="36" height="36" viewBox="0 0 100 100"', 'width="44" height="36" viewBox="-2 5 97.78 80"');
      render(source, path.join(icons, 'tray', platform, `${state}.png`), 44, 36);
    } else {
      render(source, path.join(icons, 'tray', platform, `${state}.png`), 36);
    }
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
