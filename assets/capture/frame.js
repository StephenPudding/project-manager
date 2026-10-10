(() => {
  const canvases = [...document.querySelectorAll('canvas')];
  const clip = canvases.map(canvas => {
    const box = canvas.getBoundingClientRect();
    const x = Math.max(0, box.x), y = Math.max(0, box.y);
    return { x, y, width: Math.max(0, Math.min(innerWidth, box.right) - x),
      height: Math.max(0, Math.min(innerHeight, box.bottom) - y) };
  }).filter(box => box.width * box.height > 10000)
    .sort((a, b) => b.width * b.height - a.width * a.height)[0] || null;
  const resources = performance.getEntriesByType('resource');
  const lastResource = resources.reduce((last, entry) => Math.max(last, entry.responseEnd), 0);
  return {
    ready: canvases.some(canvas => canvas.width * canvas.height > 10000)
      ? !!window.__gpmPreviewPaintedAt : document.readyState === 'complete',
    quiet: performance.now() - lastResource > 500,
    width: innerWidth, height: innerHeight, clip,
  };
})();
