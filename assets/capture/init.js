(() => {
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
})();
