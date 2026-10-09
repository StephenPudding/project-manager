// Use the installed graphics driver. SwiftShader would render the game on the CPU.
export const captureBrowserOptions = {
  headless: true,
  args: ['--enable-webgl', '--use-angle=default'],
};

export async function prepareCapture(page) {
  await page.addInitScript(() => {
    const undo = [];
    const observe = (prototype, names) => {
      if (!prototype) return;
      for (const name of names) {
        const original = prototype[name];
        if (typeof original !== 'function') continue;
        prototype[name] = function (...args) {
          const result = original.apply(this, args);
          if (this.canvas?.width * this.canvas?.height > 10000) {
            window.__gpmPreviewPaintedAt = performance.now();
            // Remove instrumentation immediately after the first real game draw.
            for (const restore of undo) restore();
          }
          return result;
        };
        undo.push(() => { prototype[name] = original; });
      }
    };
    observe(window.CanvasRenderingContext2D?.prototype,
      ['drawImage', 'fill', 'fillRect', 'fillText', 'stroke', 'strokeRect', 'strokeText', 'putImageData']);
    observe(window.WebGLRenderingContext?.prototype, ['drawArrays', 'drawElements']);
    observe(window.WebGL2RenderingContext?.prototype,
      ['drawArrays', 'drawElements', 'drawArraysInstanced', 'drawElementsInstanced']);
  });
}

export async function captureGame(page, url, file) {
  const started = Date.now();
  await prepareCapture(page);
  await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 45000 });
  // Resource quietness is bounded: analytics or polling must not stall a preview forever.
  await Promise.all([
    page.waitForLoadState('networkidle', { timeout: 4000 }).catch(() => {}),
    page.waitForFunction(() => {
      const canvases = [...document.querySelectorAll('canvas')];
      const gameCanvas = canvases.some(canvas => canvas.width * canvas.height > 10000);
      return gameCanvas ? !!window.__gpmPreviewPaintedAt : document.readyState === 'complete';
    }, { }, { timeout: 6000 }).catch(() => {}),
  ]);
  // Give the frame containing the draw a chance to reach the compositor.
  await page.evaluate(() => new Promise(resolve => {
    const fallback = setTimeout(resolve, 250);
    requestAnimationFrame(() => requestAnimationFrame(() => { clearTimeout(fallback); resolve(); }));
  }));
  const clip = await page.evaluate(() => {
    return [...document.querySelectorAll('canvas')].map(canvas => {
      const box = canvas.getBoundingClientRect();
      const x = Math.max(0, box.x), y = Math.max(0, box.y);
      return { x, y, width: Math.max(0, Math.min(innerWidth, box.right) - x), height: Math.max(0, Math.min(innerHeight, box.bottom) - y) };
    }).filter(box => box.width * box.height > 10000)
      .sort((a, b) => b.width * b.height - a.width * a.height)[0];
  });
  // A viewport clip does not wait for an animated canvas to become motionless.
  await page.screenshot({ path: file, type: 'jpeg', quality: 85, ...(clip ? { clip } : {}), timeout: 10000 });
  return Date.now() - started;
}
