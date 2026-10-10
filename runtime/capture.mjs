import { spawn, execFile } from 'node:child_process';
import { createInterface } from 'node:readline';

// The native executable hosts WebView2 in a short-lived STA process. No browser
// downloads, Playwright dependency, or HTTP screenshot control endpoint.
export function createCaptureWorker() {
  const executable = process.env.GPM_EXECUTABLE;
  if (!executable) throw new Error('请从 Project Manager 启动预览截图。');
  const child = spawn(executable, ['--capture-webview2'], {
    windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'], env: process.env,
  });
  let pending;
  let failure;
  let diagnostic = '';
  let closed = false;
  let closing;
  let resolveExit;
  const exited = new Promise(resolve => { resolveExit = resolve; });
  const fail = error => { failure = error; pending?.reject(error); };
  child.stderr.on('data', data => { diagnostic = (diagnostic + data).slice(-16000); });
  child.on('error', error => fail(error));
  child.stdin.on('error', error => fail(error));
  child.on('close', code => {
    closed = true;
    if (pending) fail(new Error(diagnostic.trim() || `WebView2 截图进程已退出（${code}）`));
    resolveExit();
  });
  const lines = createInterface({ input: child.stdout });
  lines.on('line', line => {
    if (!pending) return;
    try {
      const result = JSON.parse(line);
      if (result.ok) pending.resolve(result.duration);
      else pending.reject(new Error(result.error || 'WebView2 截图失败'));
    } catch (error) { fail(error); }
  });
  return {
    async capture(url, file) {
      if (failure) throw failure;
      if (closed) throw new Error(diagnostic.trim() || 'WebView2 截图进程已关闭');
      if (pending) throw new Error('WebView2 正在获取画面');
      let timer;
      try {
        return await new Promise((resolve, reject) => {
          pending = { resolve, reject };
          timer = setTimeout(() => fail(new Error('WebView2 获取画面超时')), 100000);
          child.stdin.write(`${JSON.stringify({ url, file })}\n`, error => { if (error) fail(error); });
        });
      } finally { clearTimeout(timer); pending = null; }
    },
    close() {
      if (closing) return closing;
      closing = (async () => {
        if (!closed) {
          child.stdin.end();
          let timer;
          await Promise.race([exited, new Promise(resolve => { timer = setTimeout(resolve, 8000); })]);
          clearTimeout(timer);
          if (!closed && child.pid) {
            await new Promise(resolve => execFile('taskkill', ['/PID', String(child.pid), '/T', '/F'],
              { windowsHide: true }, resolve));
          }
        }
        lines.close();
      })();
      return closing;
    },
  };
}
