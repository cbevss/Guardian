'use strict';
// Guardian for Windows (the parent's app).
// Runs the home relay in the background, shows the Guardian window, keeps
// running in the system tray when the window is closed (so approval requests
// still pop up), and updates itself from the GitHub releases of cbevss/Guardian.

const { app, BrowserWindow, Tray, Menu, Notification, ipcMain, shell, nativeImage, safeStorage } = require('electron');
const { createSecurity } = require('./security');
const { autoUpdater } = require('electron-updater');
const { spawn } = require('child_process');
const net = require('net');
const path = require('path');
const fs = require('fs');

const PORT = 8080;
const URL = `http://localhost:${PORT}`;
const startHidden = process.argv.includes('--hidden');

let win = null, tray = null, relay = null, quitting = false, updateReady = false;

if (!app.requestSingleInstanceLock()) { app.quit(); }
app.on('second-instance', () => showWindow());
app.setAppUserModelId('com.cbevss.guardian');

const res = (...p) => app.isPackaged ? path.join(process.resourcesPath, ...p) : path.join(__dirname, '..', ...p);
const icon = path.join(__dirname, 'build', 'icon.png');

// ---------- the relay (links this PC to his PC) ----------
function portInUse() {
  return new Promise(resolve => {
    const s = net.connect(PORT, '127.0.0.1');
    s.once('connect', () => { s.destroy(); resolve(true); });
    s.once('error', () => resolve(false));
  });
}

// Is the page on port 8080 this version's? (Old "Guardian Parent" installs run their own.)
function servingOurPage() {
  return new Promise(resolve => {
    require('http').get({ host: '127.0.0.1', port: PORT, path: '/', timeout: 2000 }, r => {
      let body = ''; r.on('data', d => { body += d; if (body.length > 200000) r.destroy(); });
      r.on('end', () => resolve(body.includes('id="lock"')));
    }).on('error', () => resolve(false)).on('timeout', function () { this.destroy(); resolve(false); });
  });
}
function stopOldRelay() {
  return new Promise(resolve => {
    if (process.platform !== 'win32') return resolve();
    require('child_process').execFile('taskkill', ['/IM', 'guardian-relay.exe', '/F'], { windowsHide: true }, () => resolve());
  });
}

async function startRelay() {
  if (await portInUse()) {
    if (relay || await servingOurPage()) return;
    // An old Guardian is holding the port: close it and run ours.
    await stopOldRelay();
    for (let i = 0; i < 20 && await portInUse(); i++) await new Promise(r => setTimeout(r, 250));
  }
  const exe = app.isPackaged ? res('guardian-relay.exe') : res('target', 'release', 'guardian-relay.exe');
  if (!fs.existsSync(exe)) return;
  relay = spawn(exe, [], {
    env: { ...process.env, GUARDIAN_WEB: app.isPackaged ? res('web') : res('relay', 'web') },
    windowsHide: true, stdio: 'ignore',
  });
  relay.on('exit', () => { relay = null; if (!quitting) setTimeout(startRelay, 3000); });
}

async function waitForRelay() {
  for (let i = 0; i < 50; i++) { if (await portInUse()) return; await new Promise(r => setTimeout(r, 200)); }
}

// ---------- window ----------
const stateFile = () => path.join(app.getPath('userData'), 'window.json');
function loadState() {
  try { const s = JSON.parse(fs.readFileSync(stateFile(), 'utf8')); if (s.width >= 800) return s; } catch {}
  return { width: 1280, height: 820 };
}
function saveState() {
  try { fs.writeFileSync(stateFile(), JSON.stringify({ ...win.getNormalBounds(), maximized: win.isMaximized() })); } catch {}
}

function createWindow() {
  const s = loadState();
  win = new BrowserWindow({
    x: s.x, y: s.y, width: s.width, height: s.height, minWidth: 900, minHeight: 600,
    title: 'Guardian', icon, backgroundColor: '#0b0d12', autoHideMenuBar: true, show: false,
    frame: false, // Guardian draws its own title bar to match the app
    webPreferences: { preload: path.join(__dirname, 'preload.js'), contextIsolation: true, nodeIntegration: false, sandbox: true, backgroundThrottling: false },
  });
  Menu.setApplicationMenu(null);
  if (s.maximized) win.maximize();
  win.once('ready-to-show', () => { if (!startHidden) win.show(); });
  const sendMax = () => win.webContents.send('guardian:maximized', win.isMaximized());
  win.on('maximize', sendMax); win.on('unmaximize', sendMax);
  win.on('close', e => {
    saveState();
    if (!quitting) {
      e.preventDefault(); win.hide();
      // An update is waiting and nobody's looking: install it now.
      if (updateReady) { quitting = true; autoUpdater.quitAndInstall(true, true); return; }
      hintTray();
    }
  });
  win.webContents.setWindowOpenHandler(({ url }) => { shell.openExternal(url); return { action: 'deny' }; });
  win.webContents.on('did-fail-load', () => setTimeout(() => win && win.loadURL(URL), 1500));
  win.loadURL(URL);
}

function showWindow() {
  if (!win) return;
  if (win.isMinimized()) win.restore();
  win.show(); win.focus();
}

let hinted = false;
function hintTray() {
  if (hinted) return; hinted = true;
  new Notification({ title: 'Guardian is still running', body: 'Requests will still pop up. Right-click the shield by the clock to quit.', icon }).show();
}

function createTray() {
  tray = new Tray(nativeImage.createFromPath(icon).resize({ width: 16, height: 16 }));
  tray.setToolTip('Guardian');
  tray.setContextMenu(Menu.buildFromTemplate([
    { label: 'Open Guardian', click: showWindow },
    { label: `Version ${app.getVersion()}`, enabled: false },
    { type: 'separator' },
    { label: 'Quit Guardian', click: () => { quitting = true; app.quit(); } },
  ]));
  tray.on('click', showWindow);
}

ipcMain.on('guardian:show', showWindow);
ipcMain.on('win:min', () => win && win.minimize());
ipcMain.on('win:max', () => win && (win.isMaximized() ? win.unmaximize() : win.maximize()));
ipcMain.on('win:close', () => win && win.close());
ipcMain.handle('win:isMax', () => !!(win && win.isMaximized()));
ipcMain.on('guardian:request', () => {
  if (!win) return;
  if (!win.isVisible() || win.isMinimized()) { win.showInactive(); }
  win.flashFrame(true);
});
ipcMain.handle('guardian:version', () => app.getVersion());
ipcMain.handle('guardian:appFolder', () => path.dirname(process.execPath));
ipcMain.on('guardian:openAppFolder', () => shell.openPath(path.dirname(process.execPath)));
ipcMain.handle('guardian:server', () => process.env.GUARDIAN_API || require('./config').API_BASE);

// ---------- account security (see security.js) ----------
let sec = null;
function security() {
  if (!sec) sec = createSecurity({
    file: path.join(app.getPath('userData'), 'security.json'),
    safeStorage,
    apiBase: process.env.GUARDIAN_API || require('./config').API_BASE,
  });
  return sec;
}
const fromOurPage = e => { try { return new URL(e.senderFrame.url).origin === URL; } catch { return false; } };
const handle = (name, fn) => ipcMain.handle(name, async (e, ...a) => {
  if (!fromOurPage(e)) throw new Error('not allowed');
  try { return await fn(...a); } catch (err) { return { error: err.message || String(err) }; }
});
handle('sec:saveRecovery', k => security().saveRecovery(k));
handle('sec:api', (m, p, b, t) => security().api(m, p, b, t));
handle('sec:serverLogin', (e, p) => security().serverLogin(e, p));
handle('sec:resetConfirm', (e, c, p) => security().resetConfirm(e, c, p));
handle('sec:challenge', () => security().newChallenge());
handle('sec:passkeys', () => security().listPasskeys());
handle('sec:addPasskey', p => security().addPasskey(p));
handle('sec:removePasskey', id => security().removePasskey(id));
handle('sec:verifyPasskey', a => security().verifyPasskey(a));
handle('sec:wipe', () => security().wipe());

// ---------- auto-update ----------
function setupUpdates() {
  if (!app.isPackaged) return;
  autoUpdater.autoDownload = true;
  autoUpdater.autoInstallOnAppQuit = true;
  autoUpdater.on('update-downloaded', () => {
    updateReady = true;
    // Nobody looking: install straight away, silently, and reopen hidden.
    if (!win || !win.isVisible()) { quitting = true; autoUpdater.quitAndInstall(true, true); return; }
    new Notification({ title: 'Guardian update ready', body: 'It installs next time Guardian closes, or when the window is hidden.', icon }).show();
  });
  const check = () => autoUpdater.checkForUpdates().catch(() => {});
  check();
  setInterval(check, 30 * 60 * 1000);
}

// ---------- start ----------
app.whenReady().then(async () => {
  if (app.isPackaged) app.setLoginItemSettings({ openAtLogin: true, args: ['--hidden'] });
  await startRelay();
  await waitForRelay();
  createTray();
  createWindow();
  setupUpdates();
});

app.on('window-all-closed', e => e.preventDefault?.());
app.on('before-quit', () => { quitting = true; if (relay) relay.kill(); });
