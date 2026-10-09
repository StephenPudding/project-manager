import { test } from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { chromium } from 'playwright';
import { captureBrowserOptions, captureGame } from '../capture.mjs';

test('capture waits for a delayed game frame but never waits for canvas animation to stop', async () => {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'gpm-capture-'));
  const server = http.createServer((req, res) => {
    res.setHeader('Content-Type', 'text/html');
    res.end(`<!doctype html><style>body{margin:0;background:#000}canvas{animation:move .7s infinite alternate linear}@keyframes move{to{transform:translateX(16px)}}</style><canvas width="800" height="500"></canvas><script>
      setTimeout(() => { const c=document.querySelector('canvas').getContext('2d'); function draw(){c.fillStyle='#22dd44';c.fillRect(0,0,800,500);requestAnimationFrame(draw)}draw(); }, 900);
    </script>`);
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  let browser;
  try {
    browser = await chromium.launch(captureBrowserOptions);
    const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
    const file = path.join(directory, 'frame.jpg');
    const duration = await captureGame(page, `http://127.0.0.1:${server.address().port}`, file);
    assert.ok(duration >= 850, `Captured before game drew its first frame: ${duration}ms`);
    assert.ok(duration < 5000, `Capture waited too long for an animated canvas: ${duration}ms`);
    const jpeg = await fs.readFile(file);
    const pixel = await page.evaluate(async base64 => {
      const picture = new Image();
      picture.src = `data:image/jpeg;base64,${base64}`;
      await picture.decode();
      const canvas = document.createElement('canvas'); canvas.width = 1; canvas.height = 1;
      const context = canvas.getContext('2d');
      context.drawImage(picture, -200, -200);
      return [...context.getImageData(0, 0, 1, 1).data];
    }, jpeg.toString('base64'));
    assert.ok(pixel[1] > 180 && pixel[0] < 70, `Expected the rendered green game, got ${pixel}`);
    console.log(`Adaptive capture fixture: ${duration} ms`);
  } finally {
    await browser?.close();
    await new Promise(resolve => server.close(resolve));
    if (path.dirname(directory) === os.tmpdir() && path.basename(directory).startsWith('gpm-capture-')) await fs.rm(directory, { recursive: true, force: true });
  }
});
