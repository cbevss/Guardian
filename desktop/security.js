'use strict';
// The parts of the lock screen that need the app itself (not the page):
//  - a recovery copy of the vault key, sealed with Windows' own encryption for this
//    Windows user (DPAPI via Electron safeStorage), released only after an emailed
//    reset code or a passkey (Windows Hello) has been checked here
//  - sending password-reset emails through the family's own email account
//  - checking passkey signatures

const crypto = require('crypto');
const fs = require('fs');

const CODE_MINUTES = 15;
const ORIGINS = ['http://localhost:8080', 'http://127.0.0.1:8080'];

function createSecurity({ file, safeStorage, sendMail }) {
  // ---------- storage (everything secret is sealed with safeStorage) ----------
  let data = {};
  try { data = JSON.parse(fs.readFileSync(file, 'utf8')); } catch { data = {}; }
  const save = () => { try { fs.writeFileSync(file, JSON.stringify(data, null, 2)); } catch {} };
  const seal = s => safeStorage.encryptString(s).toString('base64');
  const unseal = b => { try { return safeStorage.decryptString(Buffer.from(b, 'base64')); } catch { return null; } };
  const sha = s => crypto.createHash('sha256').update(s).digest('hex');
  data.passkeys = data.passkeys || [];

  // ---------- recovery copy of the vault key ----------
  function saveRecovery(keyB64) {
    if (typeof keyB64 !== 'string' || keyB64.length > 200) return false;
    data.recovery = seal(keyB64); save(); return true;
  }
  const recoveryKey = () => (data.recovery ? unseal(data.recovery) : null);

  // ---------- reset email settings ----------
  function setSmtp(cfg) {
    if (!cfg || !cfg.user) { delete data.smtp; save(); return true; }
    const keep = data.smtp ? JSON.parse(unseal(data.smtp) || '{}') : {};
    const clean = {
      host: String(cfg.host || 'smtp.gmail.com').trim(),
      port: Number(cfg.port) || 465,
      user: String(cfg.user).trim(),
      pass: cfg.pass ? String(cfg.pass).replace(/\s+/g, '') : keep.pass, // blank = keep the old one
    };
    if (!clean.pass) return false;
    data.smtp = seal(JSON.stringify(clean)); save(); return true;
  }
  const smtp = () => (data.smtp ? JSON.parse(unseal(data.smtp) || 'null') : null);
  function smtpInfo() { const s = smtp(); return s ? { configured: true, host: s.host, port: s.port, user: s.user } : { configured: false }; }

  async function mail(to, subject, text) {
    const s = smtp();
    if (!s) throw new Error('not_configured');
    await sendMail(s, { from: `Guardian <${s.user}>`, to, subject, text });
  }

  // ---------- reset codes ----------
  async function sendResetCode(email) {
    if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email || '')) throw new Error('bad_email');
    const now = Date.now();
    if (data.reset && now - data.reset.sent < 60_000) throw new Error('wait');
    if (!smtp()) throw new Error('not_configured');
    const code = String(crypto.randomInt(0, 1_000_000)).padStart(6, '0');
    const reset = { hash: sha(code), sent: now, expires: now + CODE_MINUTES * 60_000, tries: 0 };
    data.reset = reset; save();
    try { await mail(email, 'Your Guardian reset code',
      `Someone (hopefully you) needs to reset the Guardian password on this PC.\n\n` +
      `Your code is: ${code}\n\nIt works for ${CODE_MINUTES} minutes. If this wasn't you, someone may be trying ` +
      `to get into Guardian on your PC. Your password hasn't changed, and they can't get in without this code.`); }
    catch (e) { delete data.reset; save(); throw e; }
    return true;
  }
  /** Right code -> the vault key, so the page can set a new password without losing anything. */
  function checkResetCode(code) {
    const r = data.reset;
    if (!r || Date.now() > r.expires) return { ok: false, reason: 'expired' };
    if (r.tries >= 5) { delete data.reset; save(); return { ok: false, reason: 'expired' }; }
    const good = crypto.timingSafeEqual(Buffer.from(sha(String(code || '').trim())), Buffer.from(r.hash));
    if (!good) { r.tries++; save(); return { ok: false, reason: 'wrong' }; }
    delete data.reset; save();
    return { ok: true, key: recoveryKey() };
  }

  // ---------- passkeys ----------
  let challenge = null;
  function newChallenge() { challenge = { value: crypto.randomBytes(32), at: Date.now() }; return challenge.value.toString('base64url'); }
  const listPasskeys = () => data.passkeys.map(({ id, name, created }) => ({ id, name, created }));
  function addPasskey({ id, name, spki }) {
    if (!id || !spki) return false;
    try { crypto.createPublicKey({ key: Buffer.from(spki, 'base64'), format: 'der', type: 'spki' }); } catch { return false; }
    data.passkeys.push({ id, name: String(name || 'Passkey').slice(0, 40), spki, created: Date.now() });
    save(); return true;
  }
  function removePasskey(id) { data.passkeys = data.passkeys.filter(p => p.id !== id); save(); return true; }

  /** Checks a Windows Hello / passkey signature. Right -> the vault key. */
  function verifyPasskey({ id, clientDataJSON, authenticatorData, signature }) {
    const c = challenge; challenge = null; // single use
    if (!c || Date.now() - c.at > 120_000) return { ok: false };
    const pk = data.passkeys.find(p => p.id === id);
    if (!pk) return { ok: false };
    try {
      const cd = Buffer.from(clientDataJSON, 'base64');
      const client = JSON.parse(cd.toString('utf8'));
      if (client.type !== 'webauthn.get' || !ORIGINS.includes(client.origin)) return { ok: false };
      if (!crypto.timingSafeEqual(Buffer.from(client.challenge), Buffer.from(c.value.toString('base64url')))) return { ok: false };
      const auth = Buffer.from(authenticatorData, 'base64');
      if (!auth.subarray(0, 32).equals(crypto.createHash('sha256').update('localhost').digest())) return { ok: false };
      if ((auth[32] & 0x05) !== 0x05) return { ok: false }; // user present + verified (PIN / face / fingerprint)
      const signed = Buffer.concat([auth, crypto.createHash('sha256').update(cd).digest()]);
      const key = crypto.createPublicKey({ key: Buffer.from(pk.spki, 'base64'), format: 'der', type: 'spki' });
      if (!crypto.verify('sha256', signed, key, Buffer.from(signature, 'base64'))) return { ok: false };
      return { ok: true, key: recoveryKey() };
    } catch { return { ok: false }; }
  }

  function wipe() { data = { passkeys: [], smtp: data.smtp }; save(); } // a full reset keeps the email settings

  return { saveRecovery, setSmtp, smtpInfo, mail, sendResetCode, checkResetCode, newChallenge, listPasskeys, addPasskey, removePasskey, verifyPasskey, wipe };
}

module.exports = { createSecurity };
