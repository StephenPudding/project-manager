import http from 'node:http';
import https from 'node:https';
import { networkInterfaces } from 'node:os';

function addresses() {
  const interfaces = Object.entries(networkInterfaces());
  // Prefer physical adapters while still allowing VPN/virtual network links.
  interfaces.sort(([a], [b]) => Number(/virtual|vethernet|wsl|docker|vmware|tailscale|zerotier/i.test(a))
    - Number(/virtual|vethernet|wsl|docker|vmware|tailscale|zerotier/i.test(b)));
  return [...new Set(interfaces.flatMap(([, entries]) => (entries || [])
    .filter(entry => !entry.internal && entry.family === 'IPv4' && !entry.address.startsWith('169.254.'))
    .map(entry => entry.address)))];
}

// Relay to the existing server so custom npm scripts need no host flags or edits.
// HTTP streams and WebSocket upgrades both stay attached to this project's lifecycle.
export async function startLanPreview(localUrl) {
  const target = new URL(localUrl);
  if (!['http:', 'https:'].includes(target.protocol)
    || !['localhost', '127.0.0.1', '[::1]'].includes(target.hostname)) {
    throw new Error('LAN preview requires a local HTTP development server.');
  }
  const transport = target.protocol === 'https:' ? https : http;
  const agent = new transport.Agent({ keepAlive: true, maxSockets: 32, maxFreeSockets: 4 });
  const sockets = new Set();
  let closed = false;
  let networkAddresses = addresses();
  let checkedAt = Date.now();
  const track = socket => {
    sockets.add(socket);
    socket.once('close', () => sockets.delete(socket));
    socket.on('error', () => socket.destroy());
  };
  const options = req => {
    const headers = { ...req.headers, host: target.host };
    const origin = `http://${req.headers.host}`;
    if (headers.origin === origin) headers.origin = target.origin;
    if (headers.referer?.startsWith(`${origin}/`)) {
      headers.referer = target.origin + headers.referer.slice(origin.length);
    }
    return { protocol: target.protocol, hostname: target.hostname.replace(/^\[|\]$/g, ''),
      port: target.port, method: req.method, path: req.url, headers, agent };
  };
  const responseHeaders = (headers, req) => {
    const result = { ...headers };
    const origin = `http://${req.headers.host}`;
    if (result.location === target.origin || result.location?.startsWith(`${target.origin}/`)) {
      result.location = origin + result.location.slice(target.origin.length);
    }
    if (result['access-control-allow-origin'] === target.origin) result['access-control-allow-origin'] = origin;
    return result;
  };
  const server = http.createServer((req, res) => {
    if (closed || !req.url?.startsWith('/')) { res.writeHead(400); res.end(); return; }
    const outgoing = transport.request(options(req), upstream => {
      res.writeHead(upstream.statusCode || 502, responseHeaders(upstream.headers, req));
      upstream.on('error', () => res.destroy());
      upstream.pipe(res);
    });
    outgoing.on('error', () => {
      if (!res.headersSent) { res.writeHead(502); res.end('Project server unavailable'); }
      else res.destroy();
    });
    req.on('aborted', () => outgoing.destroy());
    req.on('error', () => outgoing.destroy());
    res.on('close', () => { if (!res.writableFinished) outgoing.destroy(); });
    req.pipe(outgoing);
  });
  server.on('connection', track);
  server.on('upgrade', (req, socket, head) => {
    if (closed || !req.url?.startsWith('/')) { socket.destroy(); return; }
    const outgoing = transport.request(options(req));
    outgoing.on('error', () => socket.destroy());
    socket.once('close', () => outgoing.destroy());
    outgoing.on('response', response => { response.resume(); socket.destroy(); });
    outgoing.on('upgrade', (response, upstream, upstreamHead) => {
      track(upstream);
      if (closed || socket.destroyed) { upstream.destroy(); return; }
      const headers = response.rawHeaders.reduce((lines, value, index, values) =>
        index % 2 === 0 ? [...lines, `${value}: ${values[index + 1]}`] : lines, []);
      socket.write(`HTTP/1.1 ${response.statusCode} ${response.statusMessage}\r\n${headers.join('\r\n')}\r\n\r\n`);
      if (upstreamHead.length) socket.write(upstreamHead);
      if (head.length) upstream.write(head);
      socket.once('close', () => upstream.destroy());
      upstream.once('close', () => socket.destroy());
      upstream.pipe(socket).pipe(upstream);
    });
    outgoing.end();
  });
  try {
    await new Promise((resolve, reject) => {
      server.once('error', reject);
      server.listen(0, '0.0.0.0', () => { server.removeListener('error', reject); resolve(); });
    });
  } catch (error) { agent.destroy(); throw error; }
  const port = server.address().port;
  const close = () => {
    if (closed) return;
    closed = true;
    server.close();
    agent.destroy();
    for (const socket of sockets) socket.destroy();
    sockets.clear();
  };
  server.on('error', close);
  return {
    urls() {
      if (closed) return [];
      if (Date.now() - checkedAt > 5000) { networkAddresses = addresses(); checkedAt = Date.now(); }
      return networkAddresses.map(address => `http://${address}:${port}${target.pathname}${target.search}${target.hash}`);
    },
    close,
  };
}
