'use strict';
// The parts of the lock screen that need the app itself (not the page):
//  - a recovery copy of the vault key, sealed by Windows for this Windows user
//    (DPAPI via Electron safeStorage). It is only handed back after the Guardian
//    server confirms the account (right password, or an emailed reset code) or
//    after a passkey (Windows Hello) signature is checked here.
//  - talking to the Guardian server (accounts, reset codes, admin settings).
//    Emails are sent by the server; no email password is ever on this PC.

const crypto = require('crypto');
const fs = require('fs');

const ORIGINS = ['http://localhost:8080', 'http://127.0.0.1:8080'];

function createSecurity({ file, safeStorage, apiBase, fetchImpl = fetch }) {
  let data = {};
  try { data = JSON.parse(fs.readFileSync(file, 'utf8')); } catch { data = {}; }
  const save = () => { try { fs.writeFileSync(file, JSON.stringify(data, null, 2)); } catch {} };
  const seal = s => safeStorage.encryptString(s).toString('base64');
  const unseal = b => { try { return safeStorage.decryptString(Buffer.from(b, 'base64')); } catch { return null; } };
  data.passkeys = data.passkeys || [];
  delete data.smtp; delete data.reset; // older builds kept email settings here

  // ---------- recovery copy of the vault key ----------
  function saveRecovery(keyB64) {
    if (typeof keyB64 !== 'string' || keyB64.length > 200) return false;
    data.recovery = seal(keyB64); save(); return true;
  }
  const recoveryKey = () => (data.recovery ? unseal(data.recovery) : null);

  // ---------- Guardian server ----------
  async function api(method, path, body, token) {
    if (!/^\/api\/[a-z0-9/_-]+$/.test(path) || !['GET', 'POST', 'PUT'].includes(method)) return { ok: false, error: 'bad_request' };
    try {
      const r = await fetchImpl(apiBase + path, {
        method, headers: { 'content-type': 'application/json', ...(token ? { authorization: 'Bearer ' + token } : {}) },
        body: body && method !== 'GET' ? JSON.stringify(body) : undefined,
      });
      return await r.json();
    } catch { return { ok: false, error: 'offline' }; }
  }
  /** The server says the password is right (e.g. it was changed on the website): hand back the vault key too. */
  async function serverLogin(email, password) {
    const r = await api('POST', '/api/login', { email, password });
    return r.ok ? { ...r, key: recoveryKey() } : r;
  }
  async function resetConfirm(email, code, password) {
    const r = await api('POST', '/api/reset/confirm', { email, code, password });
    return r.ok ? { ...r, key: recoveryKey() } : r;
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

  function wipe() { data = { passkeys: [] }; save(); }

  return { saveRecovery, api, serverLogin, resetConfirm, newChallenge, listPasskeys, addPasskey, removePasskey, verifyPasskey, wipe };
}

module.exports = { createSecurity };
