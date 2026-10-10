import { execFile } from 'node:child_process';
import { networkInterfaces } from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';

const execute = promisify(execFile);
const virtualInterface = /virtual|vethernet|wsl|docker|vmware|hyper.?v|loopback|tailscale|zerotier|wireguard|wintun|mihomo|clash|sing[-_ ]?box|meta.?tunnel|utun\d|(?:^|[^a-z])(?:tun|tap|vpn)(?:[^a-z]|$)/i;
let physicalAdapters = null;
let checkedAt = 0;
let checkedNetwork = '';
let pending = null;

function privateIPv4(address) {
  const parts = address.split('.').map(Number);
  if (parts.length !== 4 || parts.some(part => !Number.isInteger(part) || part < 0 || part > 255)) return false;
  return parts[0] === 10 || parts[0] === 172 && parts[1] >= 16 && parts[1] <= 31
    || parts[0] === 192 && parts[1] === 168;
}

function candidates() {
  return Object.entries(networkInterfaces()).flatMap(([name, entries]) => (entries || [])
    .filter(entry => !entry.internal && entry.family === 'IPv4' && privateIPv4(entry.address))
    .map(entry => ({ name, address: entry.address })));
}

async function refreshPhysicalAdapters(network) {
  if (pending) return pending;
  if (Date.now() - checkedAt < 60000 && checkedNetwork === network) return;
  checkedAt = Date.now();
  checkedNetwork = network;
  // Read Windows' actual hardware/connection flags. Adapter names alone do not
  // identify proxy tunnels reliably, especially with localized/custom names.
  const command = [
    "$ErrorActionPreference = 'Stop'",
    "$ProgressPreference = 'SilentlyContinue'",
    '[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)',
    "@(Get-NetAdapter -IncludeHidden -ErrorAction Stop | Where-Object { $_.HardwareInterface -and $_.Status -eq 'Up' } | Select-Object -ExpandProperty Name) | ConvertTo-Json -Compress",
  ].join('; ');
  const powershell = path.join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  pending = execute(powershell, ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', command], {
    windowsHide: true, timeout: 5000, maxBuffer: 64 * 1024, encoding: 'utf8',
  }).then(({ stdout }) => {
    const names = JSON.parse(stdout.replace(/^\uFEFF/, '').trim() || '[]');
    physicalAdapters = new Set((Array.isArray(names) ? names : [names])
      .filter(name => typeof name === 'string').map(name => name.toLowerCase()));
  }).catch(() => {
    // A restricted PowerShell environment still gets private addresses with
    // known virtual adapters excluded; never fall back to arbitrary IPv4s.
  }).finally(() => { pending = null; });
  return pending;
}

export async function lanAddresses() {
  const interfaces = candidates();
  if (process.platform === 'win32') {
    const network = interfaces.map(entry => `${entry.name}:${entry.address}`).sort().join('|');
    await refreshPhysicalAdapters(network);
  }
  return [...new Set(interfaces.filter(entry => physicalAdapters
    ? physicalAdapters.has(entry.name.toLowerCase())
    : !virtualInterface.test(entry.name)).map(entry => entry.address))];
}

export function activeLanAddresses(addresses) {
  const active = new Set(candidates().map(entry => entry.address));
  return addresses.filter(address => active.has(address));
}
