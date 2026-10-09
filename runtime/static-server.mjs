import http from 'node:http';
import fs from 'node:fs/promises';
import path from 'node:path';
const root = path.resolve(process.argv[2]);
const types = { '.html': 'text/html', '.js': 'text/javascript', '.mjs': 'text/javascript', '.css': 'text/css', '.json': 'application/json', '.png': 'image/png', '.jpg': 'image/jpeg', '.svg': 'image/svg+xml', '.wasm': 'application/wasm', '.mp3': 'audio/mpeg', '.glb': 'model/gltf-binary' };
const server = http.createServer(async (req, res) => {
  try {
    const pathname = decodeURIComponent(new URL(req.url, 'http://localhost').pathname);
    const file = path.resolve(root, `.${pathname === '/' ? '/index.html' : pathname}`);
    if (!file.startsWith(root + path.sep)) { res.writeHead(403); return res.end(); }
    const buffer = await fs.readFile(file);
    res.writeHead(200, { 'Content-Type': types[path.extname(file)] || 'application/octet-stream' });
    res.end(buffer);
  } catch { res.writeHead(404); res.end('Not found'); }
});
server.listen(0, '127.0.0.1', () => console.log(`http://127.0.0.1:${server.address().port}/`));
