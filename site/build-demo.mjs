// Builds the clickable app preview for the website from the real app page.
// Run after changing relay/web/index.html:  node site/build-demo.mjs
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
let html = readFileSync(join(here, '..', 'relay', 'web', 'index.html'), 'utf8');

const demoCss = `
<style id="demo">
  #lock { display: none !important; }
  /* same scrollbar as the website */
  * { scrollbar-width: thin; scrollbar-color: #2f3a5c var(--bg); }
  ::-webkit-scrollbar { width: 11px; height: 11px; }
  ::-webkit-scrollbar-track { background: var(--bg); }
  ::-webkit-scrollbar-thumb { background: linear-gradient(180deg, #3b5bd9, #24305a); border-radius: 10px; border: 3px solid var(--bg); }
  ::-webkit-scrollbar-thumb:hover { background: linear-gradient(180deg, #6b8cff, #3b5bd9); }
  /* Look-only: just the side tabs can be clicked */
  body * { cursor: default !important; }
  .nav-btn[data-page], .nav-btn[data-page] * { cursor: pointer !important; }
  input, textarea, select { caret-color: transparent; }
</style>`;

const demoJs = `
<script id="demo-script">
// ---- Website preview: the real Guardian app with example data and no connections ----
(() => {
  const now = Date.now();
  document.body.classList.add('desk');
  store.set('guardian.id', '482913577'); store.set('guardian.name', 'Parent');
  store.set('guardian.account_email', 'parent@example.com');
  vault.code = 'demo';
  // No networking in the preview.
  // A pretend connection, so replies go through the app's own message handling.
  window.WebSocket = class { constructor() { this.readyState = 1; } send() {} close() {} };
  startInbox('482913577', 'demo');
  startInbox = () => {}; startViewer = () => {}; stopViewer = () => {};
  const T = now / 1000, ago = m => T - m * 60;
  const demoActivity = [
    [3,'site','Visited youtube.com'],[9,'app','Opened Steam'],[14,'decision','twitch.tv: Denied'],[15,'request','Asked to visit twitch.tv'],
    [22,'blocked','Tried to open Fortnite (blocked)'],[31,'site','Visited bbc.co.uk'],[40,'closed','Closed Homework app'],[58,'app','Opened Homework app'],
    [75,'time','30 minutes of extra time given'],[96,'site','Visited wikipedia.org'],[120,'app','Opened Steam']].map(([m,kind,text]) => ({ t: ago(m), kind, text }));
  const demoDirs = {
    '': [['Desktop',1],['Documents',1],['Downloads',1],['Pictures',1],['Videos',1],['Music',1],['Drive C:',1]],
    'Documents': [['School',1],['Science project.docx',0,184320],['Reading log.xlsx',0,22016],['Notes.txt',0,1204]],
  };
  const reply = m => setTimeout(() => inbox.onmessage({ data: JSON.stringify(m) }), 120);
  sendInbox = msg => {
    if (msg.type === 'get_activity') reply({ type:'activity', day:'', items: demoActivity, days:[new Date().toISOString().slice(0,10)], usage:{ used_minutes: 74, limit_minutes: 120, bonus_minutes: 30 } });
    if (msg.type === 'list_apps') reply({ type:'apps', apps:[{ home:'c:/program files (x86)/steam', name:'Steam', count:6, memory: 412e6 }, { home:'c:/users/child/appdata/local/homework', name:'Homework app', count:1, memory: 96e6 }] });
    if (msg.type === 'list_dir') { const key = msg.path.split(/[\\/]/).pop() || ''; const list = demoDirs[key] || demoDirs['Documents'];
      reply({ type:'dir', listing:{ path: msg.path, entries: list.map(([name, dir, size]) => ({ name, dir: !!dir, size, path: (msg.path ? msg.path + '/' : 'C:/Users/Child/') + name, mtime: T - 86400 * 2 })) } }); }
    if (msg.type === 'decide') { queue = queue.filter(r => r.app !== msg.app); renderQueue(); toast(msg.decision === 'Deny' ? 'Denied' : msg.decision === 'Temporary' ? 'Temporary access given' : 'Approved'); }
    if (msg.type === 'set_policy') { policy = msg.policy; renderUnrestricted(); }
    if (msg.type === 'approve_parent') { const w = waitingParents.find(x => x.id === msg.id); waitingParents = waitingParents.filter(x => x.id !== msg.id); if (w) parentList.push({ id: w.id, name: w.name, added: now / 1000 }); renderParents(); }
    if (msg.type === 'deny_parent') { waitingParents = waitingParents.filter(x => x.id !== msg.id); renderParents(); }
  };
  send = () => {};

  policy = {
    allowed_apps: ['c:/program files (x86)/steam', 'c:/users/child/appdata/roaming/.minecraft', 'c:/users/child/appdata/local/discord'],
    blocked_apps: ['c:/program files/epic games/fortnite', 'c:/program files/tor browser'],
    allowed_sites: ['youtube.com', 'bbc.co.uk', 'wikipedia.org', 'coolmathgames.com'],
    blocked_sites: ['tiktok.com', 'roblox.com'],
    ask_unknown: true, ask_unknown_sites: true, unrestricted: false,
    schedule: { enabled: true, days: { mon: ['16:00-20:00'], tue: ['16:00-20:00'], wed: ['16:00-20:00'], thu: ['16:00-20:00'], fri: ['16:00-21:30'], sat: ['09:00-21:30'], sun: ['09:00-20:00'] } },
  };
  queue = [
    { app: 'c:/users/child/appdata/local/programs/roblox', kind: 'app', name: 'Roblox', path: 'C:\\\\Users\\\\Child\\\\AppData\\\\Local\\\\Programs\\\\Roblox', requested_at: now - 40000 },
    { app: 'site:twitch.tv', kind: 'site', name: 'twitch.tv', path: 'www.twitch.tv', requested_at: now - 300000 },
    { app: 'c:/program files/spotify', kind: 'app', name: 'Spotify', path: 'C:\\\\Program Files\\\\Spotify', requested_at: now - 1500000 },
  ];
  parentList = [{ id: myId(), name: 'Parent', added: now / 1000 - 86400 * 9 }, { id: 'p2', name: 'Parent 2', added: now / 1000 - 86400 * 7 }];
  waitingParents = [{ id: 'p3', name: 'Parent 3' }];

  // A pretend desktop on his screen.
  function drawDesktop() {
    canvas.width = 1920; canvas.height = 1080;
    const g = ctx.createLinearGradient(0, 0, 1920, 1080); g.addColorStop(0, '#1e3a8a'); g.addColorStop(1, '#0f766e');
    ctx.fillStyle = g; ctx.fillRect(0, 0, 1920, 1080);
    // Plain desktop: wallpaper, a few folders, taskbar. No real apps or brands.
    const glow = ctx.createRadialGradient(1300, 380, 50, 1300, 380, 900); glow.addColorStop(0, 'rgba(255,255,255,.18)'); glow.addColorStop(1, 'rgba(255,255,255,0)');
    ctx.fillStyle = glow; ctx.fillRect(0, 0, 1920, 1080);
    const folder = (x, y, label) => {
      ctx.fillStyle = '#f5c451'; ctx.beginPath(); ctx.roundRect(x, y + 10, 84, 64, 8); ctx.fill();
      ctx.fillStyle = '#e8ad2e'; ctx.beginPath(); ctx.roundRect(x, y, 38, 20, 6); ctx.fill();
      ctx.fillStyle = '#fff'; ctx.font = '22px Segoe UI, sans-serif'; ctx.textAlign = 'center'; ctx.fillText(label, x + 42, y + 108); ctx.textAlign = 'left';
    };
    folder(40, 40, 'Homework'); folder(40, 180, 'Photos'); folder(40, 320, 'Music');
    ctx.fillStyle = 'rgba(255,255,255,.85)'; ctx.beginPath(); ctx.roundRect(54, 470, 56, 66, 8); ctx.fill();
    ctx.fillStyle = '#fff'; ctx.font = '22px Segoe UI, sans-serif'; ctx.textAlign = 'center'; ctx.fillText('Recycle Bin', 82, 572); ctx.textAlign = 'left';
    ctx.fillStyle = 'rgba(16,18,27,.94)'; ctx.fillRect(0, 1032, 1920, 48);
    ['#3b82f6', '#94a3b8', '#94a3b8', '#94a3b8'].forEach((c, i) => { ctx.fillStyle = c; ctx.beginPath(); ctx.roundRect(860 + i * 56, 1040, 32, 32, 7); ctx.fill(); });
    ctx.fillStyle = '#fff'; ctx.font = '22px Segoe UI, sans-serif'; ctx.fillText('16:42', 1830, 1064);
  }

  ws = { readyState: 1, send() {}, close() {} };
  linked = true; document.body.classList.add('linked', 'viewing');
  setStatus('Live', 'live');
  drawDesktop();
  agentInfo = { admin: false, version: '1.0.30' };
  renderQueue(); renderParents(); renderUnrestricted(); renderEmpty(); renderAccount();
  $('pname').value = 'Parent'; $('devid').value = '482913577'; $('code').value = 'demo-code-12';
  goTo('requests');
  setTimeout(() => {
    const sel = document.getElementById('monitors');
    if (sel && !sel.options.length) { sel.add(new Option('Screen 1 (main) · 1920×1080', 0)); sel.add(new Option('Screen 2 · 2560×1440', 1)); }
  }, 50);
  // The Screen page's monitor list is rebuilt each visit.
  const _rta = renderTopbarActions;
  renderTopbarActions = page => { _rta(page); const sel = document.getElementById('monitors'); if (sel && !sel.options.length) { sel.add(new Option('Screen 1 (main) · 1920×1080', 0)); sel.add(new Option('Screen 2 · 2560×1440', 1)); } };
  // Taking control in the preview just shows the banner.
  setControl = on => { control = on; canvas.classList.toggle('control', on); $('control-banner').classList.toggle('show', on); renderTopbarActions('view'); toast(on ? 'Restrictions paused while you have control' : 'Control handed back'); };
  sendClipboard = () => toast('Clipboard sent to his PC');
  // Pre-fill the Rules page each time it opens.
  document.getElementById('nav-rules').click(); goTo('requests');

  // Look-only: block every click, typing and change except switching tabs. Scrolling still works.
  document.querySelectorAll('input, textarea, select').forEach(el => { el.readOnly = true; el.tabIndex = -1; });
  const allowed = e => !!e.target.closest?.('.nav-btn[data-page]');
  ['click', 'mousedown', 'pointerdown', 'dblclick', 'contextmenu', 'change', 'input', 'submit', 'toggle'].forEach(t =>
    document.addEventListener(t, e => { if (!allowed(e)) { e.preventDefault(); e.stopImmediatePropagation(); } }, true));
  document.addEventListener('keydown', e => {
    const scrollKeys = ['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End', ' '];
    if (!scrollKeys.includes(e.key) || e.target.matches?.('input, textarea, select')) { e.preventDefault(); e.stopImmediatePropagation(); }
  }, true);
  document.addEventListener('focusin', e => { if (!allowed(e)) e.target.blur?.(); }, true);
})();
</script>`;

// Stand-in for the desktop app, set before the app's own script reads it.
const headJs = `<script>window.guardianDesktop = { request() {}, show() {}, version: async () => '1.0.30', serverBase: async () => '',
  sec: async name => name === 'passkeys' ? [{ id: 'p1', name: 'Windows Hello on this PC', created: Date.now() - 86400000 * 3 }] : { ok: false } };</script>`;
html = html.replace('</head>', demoCss + '\n' + headJs + '\n</head>');
html = html.replace(/<\/body>\s*<\/html>\s*$/, demoJs + '\n</body>\n</html>\n');
html = html.replace('<title>Guardian</title>', '<title>Guardian preview</title>\n<meta name="robots" content="noindex">');
mkdirSync(join(here, 'public', 'preview'), { recursive: true });
writeFileSync(join(here, 'public', 'preview', 'index.html'), html);
console.log('wrote site/public/preview/index.html');
