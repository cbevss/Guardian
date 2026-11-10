// Guardian's website and account server (Cloudflare Worker + D1).
//
// The website (public/) is info and downloads only. The API here handles
// accounts for the Guardian app: sign-up with an emailed code, login with
// lockout, password-reset codes, and the admin's email settings. The email
// account's password lives only here, never on anyone's PC.

import { WorkerMailer } from 'worker-mailer';

const CODE_MIN = 15;            // email codes last 15 minutes
const SESSION_DAYS = 30;
const MAX_FAILS = 5;            // wrong passwords before a reset code is emailed
const LOCK_MIN = 15;
const ITER = 100000;            // PBKDF2 (the most Workers allow)

const enc = new TextEncoder();
const now = () => Date.now();
const b64 = buf => btoa(String.fromCharCode(...new Uint8Array(buf)));
const unb64 = s => Uint8Array.from(atob(s), c => c.charCodeAt(0));
const hex = buf => [...new Uint8Array(buf)].map(b => b.toString(16).padStart(2, '0')).join('');
const sha256 = async s => hex(await crypto.subtle.digest('SHA-256', enc.encode(s)));
const cleanEmail = e => String(e || '').trim().toLowerCase().slice(0, 254);
const validEmail = e => /^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(e);

function json(data, status = 200) {
  return new Response(JSON.stringify(data), {
    status, headers: { 'content-type': 'application/json', 'cache-control': 'no-store' },
  });
}
const fail = (error, status = 400, extra = {}) => json({ ok: false, error, ...extra }, status);

/** Same-length comparison that doesn't leak timing. */
function same(a, b) {
  if (a.length !== b.length) return false;
  let d = 0;
  for (let i = 0; i < a.length; i++) d |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return d === 0;
}

async function hashPassword(pw, saltB64) {
  const salt = saltB64 ? unb64(saltB64) : crypto.getRandomValues(new Uint8Array(16));
  const key = await crypto.subtle.importKey('raw', enc.encode(pw), 'PBKDF2', false, ['deriveBits']);
  const bits = await crypto.subtle.deriveBits({ name: 'PBKDF2', salt, iterations: ITER, hash: 'SHA-256' }, key, 256);
  return { hash: b64(bits), salt: b64(salt) };
}

// ---------- email ----------
async function mailKey(env) {
  return crypto.subtle.importKey('raw', await crypto.subtle.digest('SHA-256', enc.encode(env.MAIL_KEY || 'dev-only')), 'AES-GCM', false, ['encrypt', 'decrypt']);
}
async function getMailSettings(env) {
  const row = await env.DB.prepare('SELECT value FROM settings WHERE key = ?').bind('mail').first();
  if (row) {
    try {
      const [iv, ct] = row.value.split('.');
      const plain = await crypto.subtle.decrypt({ name: 'AES-GCM', iv: unb64(iv) }, await mailKey(env), unb64(ct));
      return JSON.parse(new TextDecoder().decode(plain));
    } catch { /* fall through to the deploy-time secrets */ }
  }
  if (env.SMTP_USER && env.SMTP_PASS) {
    return { host: env.SMTP_HOST || 'smtp.mail.me.com', port: Number(env.SMTP_PORT) || 587, user: env.SMTP_USER, pass: env.SMTP_PASS };
  }
  return null;
}
async function saveMailSettings(env, m) {
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ct = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, await mailKey(env), enc.encode(JSON.stringify(m)));
  await env.DB.prepare('INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value')
    .bind('mail', b64(iv) + '.' + b64(ct)).run();
}

async function sendMail(env, to, subject, text) {
  if (env.MAIL_MODE === 'log') { // local testing only
    await env.DB.prepare('INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value')
      .bind('last_mail', JSON.stringify({ to, subject, text })).run();
    return;
  }
  const m = await getMailSettings(env);
  if (!m) throw new Error('mail_not_set_up');
  const msg = { from: { name: 'Guardian', email: m.user }, to, subject, text };
  const attempt = (port) => {
    const sending = WorkerMailer.send(
      { host: m.host, port, secure: port === 465, startTls: port !== 465, credentials: { username: m.user, password: m.pass }, authType: ['plain', 'login'] }, msg);
    const timeout = new Promise((_, no) => setTimeout(() => no(new Error('mail_timeout')), 12000));
    return Promise.race([sending, timeout]);
  };
  // Try the saved port; if it hangs or fails, try the other usual one (465 <-> 587).
  try { await attempt(m.port); }
  catch (first) {
    const other = m.port === 465 ? 587 : 465;
    try { await attempt(other); }
    catch (second) { console.log('mail failed', m.host, m.port, String(first), other, String(second)); throw new Error('mail_failed'); }
  }
}

// ---------- codes ----------
async function issueCode(env, email, purpose) {
  const prev = await env.DB.prepare('SELECT sent FROM codes WHERE email = ? AND purpose = ?').bind(email, purpose).first();
  if (prev && now() - prev.sent < 60_000) return { ok: false, error: 'wait' };
  const code = String(crypto.getRandomValues(new Uint32Array(1))[0] % 1_000_000).padStart(6, '0');
  await env.DB.prepare('INSERT INTO codes (email, purpose, hash, expires, tries, sent) VALUES (?, ?, ?, ?, 0, ?) ' +
    'ON CONFLICT(email, purpose) DO UPDATE SET hash = excluded.hash, expires = excluded.expires, tries = 0, sent = excluded.sent')
    .bind(email, purpose, await sha256(code + email), now() + CODE_MIN * 60_000, now()).run();
  return { ok: true, code };
}
async function checkCode(env, email, purpose, code) {
  const row = await env.DB.prepare('SELECT * FROM codes WHERE email = ? AND purpose = ?').bind(email, purpose).first();
  if (!row || now() > row.expires || row.tries >= 5) return 'expired';
  if (!same(await sha256(String(code || '').trim() + email), row.hash)) {
    await env.DB.prepare('UPDATE codes SET tries = tries + 1 WHERE email = ? AND purpose = ?').bind(email, purpose).run();
    return 'wrong';
  }
  await env.DB.prepare('DELETE FROM codes WHERE email = ? AND purpose = ?').bind(email, purpose).run();
  return 'ok';
}

async function newSession(env, email) {
  const token = b64(crypto.getRandomValues(new Uint8Array(32))).replace(/[+/=]/g, c => ({ '+': '-', '/': '_', '=': '' }[c]));
  await env.DB.prepare('INSERT INTO sessions (token_hash, email, created, expires) VALUES (?, ?, ?, ?)')
    .bind(await sha256(token), email, now(), now() + SESSION_DAYS * 86_400_000).run();
  return token;
}
async function sessionEmail(env, req) {
  const t = (req.headers.get('authorization') || '').replace(/^Bearer\s+/i, '');
  if (!t) return null;
  const row = await env.DB.prepare('SELECT email, expires FROM sessions WHERE token_hash = ?').bind(await sha256(t)).first();
  return row && row.expires > now() ? row.email : null;
}
const isAdmin = (env, email) => !!email && email === cleanEmail(env.ADMIN_EMAIL || 'cbevs@icloud.com');

// ---------- email texts ----------
const verifyText = code => `Welcome to Guardian.\n\nYour code to finish making your account is: ${code}\n\nIt works for ${CODE_MIN} minutes. If you didn't try to make a Guardian account, you can ignore this email.`;
const resetText = (code, auto) => `${auto ? 'Someone typed the wrong Guardian password 5 times, so Guardian is locked for a while.\n\n' : ''}` +
  `Your Guardian reset code is: ${code}\n\nIt works for ${CODE_MIN} minutes. Type it in Guardian with a new password.\n\n` +
  `If this wasn't you, someone may be trying to get into Guardian. Your password hasn't changed, and they can't get in without this code.`;

// ---------- API ----------
async function api(req, env, path) {
  const body = req.method === 'POST' || req.method === 'PUT' ? await req.json().catch(() => ({})) : {};
  const email = cleanEmail(body.email);

  if (path === '/api/health') {
    const m = await getMailSettings(env);
    return json({ ok: true, mail: m ? { host: m.host, port: m.port, from: m.user.replace(/^(.).*(@.*)$/, '$1***$2') } : null });
  }

  if (path === '/api/signup' && req.method === 'POST') {
    if (!validEmail(email)) return fail('bad_email');
    if (String(body.password || '').length < 8) return fail('short_password');
    const existing = await env.DB.prepare('SELECT verified FROM accounts WHERE email = ?').bind(email).first();
    if (existing?.verified) return fail('exists', 409);
    const { hash, salt } = await hashPassword(body.password);
    await env.DB.prepare('INSERT INTO accounts (email, pw_hash, pw_salt, verified, created) VALUES (?, ?, ?, 0, ?) ' +
      'ON CONFLICT(email) DO UPDATE SET pw_hash = excluded.pw_hash, pw_salt = excluded.pw_salt').bind(email, hash, salt, now()).run();
    const c = await issueCode(env, email, 'verify');
    if (!c.ok) return fail(c.error, 429);
    try { await sendMail(env, email, 'Your Guardian code', verifyText(c.code)); }
    catch (e) { await env.DB.prepare('DELETE FROM codes WHERE email = ? AND purpose = ?').bind(email, 'verify').run(); return fail(e.message || 'mail_failed', 502); }
    return json({ ok: true });
  }

  if (path === '/api/verify' && req.method === 'POST') {
    const r = await checkCode(env, email, 'verify', body.code);
    if (r !== 'ok') return fail(r);
    await env.DB.prepare('UPDATE accounts SET verified = 1 WHERE email = ?').bind(email).run();
    return json({ ok: true, token: await newSession(env, email), admin: isAdmin(env, email) });
  }

  if (path === '/api/login' && req.method === 'POST') {
    const a = await env.DB.prepare('SELECT * FROM accounts WHERE email = ?').bind(email).first();
    if (!a || !a.verified) return fail('wrong_login', 401);
    if (a.locked_until > now()) return fail('locked', 423, { until: a.locked_until });
    const { hash } = await hashPassword(String(body.password || ''), a.pw_salt);
    if (!same(hash, a.pw_hash)) {
      const fails = a.fails + 1;
      if (fails >= MAX_FAILS) {
        await env.DB.prepare('UPDATE accounts SET fails = 0, locked_until = ? WHERE email = ?').bind(now() + LOCK_MIN * 60_000, email).run();
        const c = await issueCode(env, email, 'reset');
        if (c.ok) await sendMail(env, email, 'Your Guardian reset code', resetText(c.code, true)).catch(() => {});
        return fail('locked', 423, { emailed: true });
      }
      await env.DB.prepare('UPDATE accounts SET fails = ? WHERE email = ?').bind(fails, email).run();
      return fail('wrong_login', 401, { left: MAX_FAILS - fails });
    }
    await env.DB.prepare('UPDATE accounts SET fails = 0, locked_until = 0 WHERE email = ?').bind(email).run();
    return json({ ok: true, token: await newSession(env, email), admin: isAdmin(env, email) });
  }

  if (path === '/api/reset/request' && req.method === 'POST') {
    const a = await env.DB.prepare('SELECT verified FROM accounts WHERE email = ?').bind(email).first();
    // Same answer whether or not the account exists, so nobody can test which emails have accounts.
    if (a?.verified) {
      const c = await issueCode(env, email, 'reset');
      if (!c.ok) return fail(c.error, 429);
      try { await sendMail(env, email, 'Your Guardian reset code', resetText(c.code, !!body.auto)); }
      catch (e) { await env.DB.prepare('DELETE FROM codes WHERE email = ? AND purpose = ?').bind(email, 'reset').run(); return fail(e.message || 'mail_failed', 502); }
    }
    return json({ ok: true });
  }

  if (path === '/api/reset/confirm' && req.method === 'POST') {
    if (String(body.password || '').length < 8) return fail('short_password');
    const r = await checkCode(env, email, 'reset', body.code);
    if (r !== 'ok') return fail(r);
    const { hash, salt } = await hashPassword(body.password);
    await env.DB.prepare('UPDATE accounts SET pw_hash = ?, pw_salt = ?, fails = 0, locked_until = 0 WHERE email = ?').bind(hash, salt, email).run();
    await env.DB.prepare('DELETE FROM sessions WHERE email = ?').bind(email).run(); // sign out everywhere else
    return json({ ok: true, token: await newSession(env, email), admin: isAdmin(env, email) });
  }

  // ----- signed-in -----
  const who = await sessionEmail(env, req);
  if (path === '/api/me') return who ? json({ ok: true, email: who, admin: isAdmin(env, who) }) : fail('signed_out', 401);

  if (path === '/api/password' && req.method === 'POST') {
    if (!who) return fail('signed_out', 401);
    const a = await env.DB.prepare('SELECT * FROM accounts WHERE email = ?').bind(who).first();
    if (!same((await hashPassword(String(body.old || ''), a.pw_salt)).hash, a.pw_hash)) return fail('wrong_login', 401);
    if (String(body.password || '').length < 8) return fail('short_password');
    const { hash, salt } = await hashPassword(body.password);
    await env.DB.prepare('UPDATE accounts SET pw_hash = ?, pw_salt = ? WHERE email = ?').bind(hash, salt, who).run();
    return json({ ok: true });
  }

  if (path.startsWith('/api/admin/')) {
    if (!isAdmin(env, who)) return fail('not_admin', 403);
    if (path === '/api/admin/mail' && req.method === 'GET') {
      const m = await getMailSettings(env);
      return json({ ok: true, mail: m ? { host: m.host, port: m.port, user: m.user } : null });
    }
    if (path === '/api/admin/mail' && req.method === 'PUT') {
      const m = body.mail || {};
      if (!validEmail(cleanEmail(m.user)) || !m.pass || !m.host) return fail('bad_mail');
      await saveMailSettings(env, { host: String(m.host).trim(), port: Number(m.port) || 587, user: cleanEmail(m.user), pass: String(m.pass).replace(/\s+/g, '') });
      return json({ ok: true });
    }
    if (path === '/api/admin/test-mail' && req.method === 'POST') {
      try { await sendMail(env, who, 'Guardian test email', 'Guardian emails (sign-up and reset codes) will come from this address. All working.'); }
      catch (e) { return fail(e.message || 'mail_failed', 502); }
      return json({ ok: true });
    }
    if (path === '/api/admin/stats' && req.method === 'GET') {
      const r = await env.DB.prepare('SELECT COUNT(*) AS n, SUM(verified) AS v FROM accounts').first();
      return json({ ok: true, accounts: r.n, verified: r.v || 0 });
    }
  }

  if (path === '/api/dev/last-mail' && env.MAIL_MODE === 'log') {
    const row = await env.DB.prepare('SELECT value FROM settings WHERE key = ?').bind('last_mail').first();
    return json(row ? JSON.parse(row.value) : {});
  }
  return fail('not_found', 404);
}

// Creates the database tables the first time (no setup step needed).
let schemaReady = false;
async function ensureSchema(env) {
  if (schemaReady) return;
  await env.DB.batch([
    env.DB.prepare('CREATE TABLE IF NOT EXISTS accounts (email TEXT PRIMARY KEY, pw_hash TEXT NOT NULL, pw_salt TEXT NOT NULL, verified INTEGER NOT NULL DEFAULT 0, created INTEGER NOT NULL, fails INTEGER NOT NULL DEFAULT 0, locked_until INTEGER NOT NULL DEFAULT 0)'),
    env.DB.prepare('CREATE TABLE IF NOT EXISTS codes (email TEXT NOT NULL, purpose TEXT NOT NULL, hash TEXT NOT NULL, expires INTEGER NOT NULL, tries INTEGER NOT NULL DEFAULT 0, sent INTEGER NOT NULL, PRIMARY KEY (email, purpose))'),
    env.DB.prepare('CREATE TABLE IF NOT EXISTS sessions (token_hash TEXT PRIMARY KEY, email TEXT NOT NULL, created INTEGER NOT NULL, expires INTEGER NOT NULL)'),
    env.DB.prepare('CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)'),
  ]);
  schemaReady = true;
}

export default {
  async fetch(req, env) {
    const url = new URL(req.url);
    if (url.pathname.startsWith('/api/')) {
      try { await ensureSchema(env); return await api(req, env, url.pathname); }
      catch (e) { return fail('server_error', 500); }
    }
    return env.ASSETS.fetch(req); // the website
  },
  // Tidy up old sessions and codes once a day.
  async scheduled(_e, env) {
    await ensureSchema(env);
    await env.DB.prepare('DELETE FROM sessions WHERE expires < ?').bind(now()).run();
    await env.DB.prepare('DELETE FROM codes WHERE expires < ?').bind(now()).run();
    await env.DB.prepare('DELETE FROM accounts WHERE verified = 0 AND created < ?').bind(now() - 86_400_000).run();
  },
};
