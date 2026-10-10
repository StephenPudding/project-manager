import fs from 'node:fs/promises';
import path from 'node:path';

// Specific frameworks precede their underlying libraries; all matches remain filterable.
const rules = [
  { name: 'Phaser', game: true, packages: /^(?:phaser|phaser-ce)(?:\/|$)/, global: /\bPhaser\s*\.\s*(?:Game|Scene|AUTO|CANVAS|WEBGL)\b/, script: /(?:^|\/)phaser(?:[.@/-]|$)/i },
  { name: 'Babylon.js', game: true, packages: /^(?:@babylonjs\/core|babylonjs)(?:\/|$)/, global: /\bBABYLON\s*\.\s*(?:Engine|Scene|MeshBuilder|ArcRotateCamera)\b/, script: /(?:^|\/)babylon(?:\.max|\.min)?\.js(?:$|\?)/i },
  { name: 'Cocos', game: true, packages: /^(?:cc|cocos2d|cocos2d-js|@cocos\/engine)(?:\/|$)/, global: /\bcc\s*\.\s*(?:game|director|Class|_decorator)\b/, script: /(?:^|\/)(?:cocos2d(?:-js)?|cocos-creator)(?:[.@/-]|$)/i },
  { name: 'LayaAir', game: true, packages: /^(?:layaair|layaair-js|layaair2-cmd|layaair3-cmd)(?:\/|$)/, global: /\bLaya\s*\.\s*(?:init|stage|Scene|Sprite|Laya3D)\b/, script: /(?:^|\/)(?:laya\.(?:core|webgl|d3)|layaair|layaAir)(?:[.@/-]|$)/i },
  { name: 'PixiJS', game: true, packages: /^(?:pixi\.js(?:-legacy)?|@pixi\/app|@pixi\/core|@pixi\/react)(?:\/|$)/, global: /\bPIXI\s*\.\s*(?:Application|Container|Sprite|Renderer)\b/, script: /(?:^|\/)pixi(?:[.@/-]|$)/i },
  { name: 'Three.js', game: true, packages: /^(?:three|@react-three\/fiber)(?:\/|$)/, global: /\bTHREE\s*\.\s*(?:Scene|WebGLRenderer|PerspectiveCamera|Mesh)\b/, script: /(?:^|\/)three(?:[.@/-]|$)/i },
  { name: 'Galacean', game: true, packages: /^(?:@galacean\/engine(?:-core)?|oasis-engine)(?:\/|$)/, global: /\b(?:Galacean|galacean|oasis)\s*\.\s*(?:WebGLEngine|Engine)\b/, script: /(?:^|\/)(?:galacean|oasis-engine)(?:[.@/-]|$)/i },
  { name: 'Next.js', packages: /^next(?:\/|$)/, config: /^next\.config\.[cm]?[jt]s$/ },
  { name: 'Nuxt', packages: /^(?:nuxt|nuxt3)(?:\/|$)/, config: /^nuxt\.config\.[cm]?[jt]s$/ },
  { name: 'SvelteKit', packages: /^@sveltejs\/kit(?:\/|$)/ },
  { name: 'Astro', packages: /^astro(?:\/|$)/, config: /^astro\.config\.[cm]?[jt]s$/ },
  { name: 'Angular', packages: /^@angular\/core(?:\/|$)/, config: /^angular\.json$/ },
  { name: 'React', packages: /^(?:react|react-dom)(?:\/|$)/, global: /\bReact(?:DOM)?\s*\.\s*(?:createElement|createRoot|render|useState)\b/, script: /(?:^|\/)react(?:-dom)?(?:[.@/-]|$)/i },
  { name: 'Vue', packages: /^(?:vue|@vitejs\/plugin-vue)(?:\/|$)/, extension: '.vue', global: /\bVue\s*\.\s*(?:createApp|createSSRApp|extend|component)\b/, script: /(?:^|\/)vue(?:[.@/-]|$)/i },
  { name: 'Svelte', packages: /^svelte(?:\/|$)/, config: /^svelte\.config\.[cm]?[jt]s$/, extension: '.svelte' },
  { name: 'Solid', packages: /^solid-js(?:\/|$)/ },
  { name: 'Preact', packages: /^preact(?:\/|$)/, global: /\bpreact\s*\.\s*(?:h|render)\b/, script: /(?:^|\/)preact(?:[.@/-]|$)/i },
];

// Never traverse dependencies, build output, or the entire asset tree.
// Per project: at most 32 files, 64 KiB each, 512 KiB of source in total.
async function sourceSamples(directory, files) {
  const candidates = files.filter(name => /\.(?:html|[cm]?[jt]sx?|vue|svelte|astro)$/i.test(name))
    .sort((a, b) => Number(b === 'index.html') - Number(a === 'index.html') || a.localeCompare(b))
    .slice(0, 12);
  const folders = ['src', 'app', 'pages', 'scripts', 'assets/scripts'].map(name => [name, 0]);
  let visited = 0;
  while (folders.length && visited++ < 24 && candidates.length < 32) {
    const [relative, depth] = folders.shift();
    const folder = path.join(directory, relative);
    try {
      if (!(await fs.lstat(folder)).isDirectory()) continue;
      const entries = await fs.opendir(folder);
      let count = 0;
      for await (const entry of entries) {
        if (++count > 128 || candidates.length >= 32) break;
        if (entry.name.startsWith('.') || /^(?:node_modules|dist|build|vendor|coverage|__tests__)$/.test(entry.name)) continue;
        const child = path.join(relative, entry.name);
        if (entry.isDirectory() && depth < 2) folders.push([child, depth + 1]);
        else if (entry.isFile() && /\.(?:html|[cm]?[jt]sx?|vue|svelte|astro)$/i.test(entry.name) && !/\.(?:min|test|spec)\./i.test(entry.name)) candidates.push(child);
      }
    } catch { /* Unreadable source does not prevent project discovery. */ }
  }
  const samples = [];
  let remaining = 512 * 1024;
  for (const name of candidates) {
    if (remaining <= 0) break;
    let file;
    try {
      const target = path.join(directory, name);
      if (!(await fs.lstat(target)).isFile()) continue;
      file = await fs.open(target, 'r');
      const buffer = Buffer.alloc(Math.min(64 * 1024, remaining));
      const { bytesRead } = await file.read(buffer, 0, buffer.length, 0);
      remaining -= bytesRead;
      samples.push({ name, source: buffer.subarray(0, bytesRead).toString('utf8') });
    } catch { /* The next scan can retry files that are temporarily unavailable. */ }
    finally { await file?.close().catch(() => {}); }
  }
  return samples;
}

function packageFromSpecifier(specifier) {
  // Support bare imports and common CDN /npm/, unpkg and esm.sh URLs with versions.
  let value = specifier.replace(/[?#].*$/, '');
  if (/^(?:https?:)?\/\//.test(value)) {
    try { value = new URL(value.startsWith('//') ? `https:${value}` : value).pathname.replace(/^\/(?:npm\/)?/, ''); }
    catch { return ''; }
  }
  return value.replace(/(@[^/@]+\/[^/@]+|^[^/@]+)@[^/]+/, '$1');
}

export async function detectProjectTypes(directory, pkg, files) {
  const dependencies = { ...pkg?.dependencies, ...pkg?.devDependencies, ...pkg?.peerDependencies, ...pkg?.optionalDependencies };
  const packages = new Set(Object.keys(dependencies));
  for (const value of Object.values(dependencies)) {
    if (typeof value === 'string' && value.startsWith('npm:')) packages.add(packageFromSpecifier(value.slice(4)));
  }
  const samples = await sourceSamples(directory, files);
  const scripts = [];
  for (const { source } of samples) {
    for (const match of source.matchAll(/(?:\bfrom\s*|\bimport\s*(?:\(\s*)?|\brequire\s*\(\s*)['"]([^'"\r\n]+)['"]/g)) packages.add(packageFromSpecifier(match[1]));
    for (const match of source.matchAll(/<script\b[^>]*\bsrc\s*=\s*['"]([^'"]+)['"]/gi)) {
      scripts.push(match[1]); packages.add(packageFromSpecifier(match[1]));
    }
    for (const match of source.matchAll(/<script\b[^>]*\btype\s*=\s*['"]importmap['"][^>]*>([\s\S]*?)<\/script>/gi)) {
      try {
        const map = JSON.parse(match[1]);
        for (const [name, url] of Object.entries(map.imports || {})) {
          packages.add(name);
          if (typeof url === 'string') packages.add(packageFromSpecifier(url));
        }
      } catch { /* Ignore incomplete import maps. */ }
    }
  }
  const matches = new Set();
  for (const rule of rules) {
    if ([...packages].some(name => rule.packages.test(name))
        || rule.config && files.some(name => rule.config.test(name))
        || rule.extension && samples.some(sample => sample.name.endsWith(rule.extension))
        || rule.global && samples.some(sample => rule.global.test(sample.source))
        || rule.script && scripts.some(url => rule.script.test(url))) matches.add(rule.name);
  }
  if (pkg?.creator?.version || files.includes('project.json') && files.includes('assets') && files.includes('settings')) matches.add('Cocos');
  if (files.some(name => /\.laya$/i.test(name)) || files.includes('.laya') || files.includes('LayaAir.laya')) matches.add('LayaAir');
  return {
    gameEngines: rules.filter(rule => rule.game && matches.has(rule.name)).map(rule => rule.name),
    frameworks: rules.filter(rule => !rule.game && matches.has(rule.name)).map(rule => rule.name),
  };
}

export function projectCategories(classification, gameEnginesOnly) {
  const categories = [...classification.gameEngines, ...(gameEnginesOnly ? [] : classification.frameworks)];
  return categories.length ? categories : ['Web'];
}
