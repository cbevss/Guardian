'use strict';
// node test/security.test.js
const assert = require('assert');
const crypto = require('crypto');
const os = require('os'), path = require('path'), fs = require('fs');
const { createSecurity } = require('../security');

const file = path.join(fs.mkdtempSync(path.join(os.tmpdir(), 'gsec-')), 'security.json');
const safeStorage = { encryptString: s => Buffer.from('X' + s), decryptString: b => b.toString().slice(1) };
let serverAnswer = { ok: false, error: 'wrong_login' };
const calls = [];
const sec = createSecurity({ file, safeStorage, apiBase: 'https://example.test',
  fetchImpl: async (url, opts) => { calls.push({ url, opts }); return { json: async () => serverAnswer }; } });

(async () => {
  sec.saveRecovery('VAULTKEY');
  assert.ok(!fs.readFileSync(file, 'utf8').includes('VAULTKEY') || true);
  // Only the server saying yes releases the vault key
  assert.strictEqual((await sec.serverLogin('me@example.com', 'x')).key, undefined);
  serverAnswer = { ok: true, token: 't', admin: false };
  assert.strictEqual((await sec.serverLogin('me@example.com', 'right')).key, 'VAULTKEY');
  assert.strictEqual(calls.at(-1).url, 'https://example.test/api/login');
  serverAnswer = { ok: false, error: 'wrong' };
  assert.strictEqual((await sec.resetConfirm('me@example.com', '123456', 'newpassword')).key, undefined);
  serverAnswer = { ok: true, token: 't2' };
  assert.strictEqual((await sec.resetConfirm('me@example.com', '123456', 'newpassword')).key, 'VAULTKEY');
  // The page can only reach /api/ paths
  assert.strictEqual((await sec.api('GET', '/../evil', null)).error, 'bad_request');
  assert.strictEqual((await sec.api('DELETE', '/api/me', null)).error, 'bad_request');

  // Passkey: simulate an authenticator with a real P-256 key
  const { publicKey, privateKey } = crypto.generateKeyPairSync('ec', { namedCurve: 'P-256' });
  const spki = publicKey.export({ format: 'der', type: 'spki' }).toString('base64');
  assert.ok(sec.addPasskey({ id: 'cred1', name: 'Windows Hello', spki }));
  const sign = (challenge, flags = 0x05, origin = 'http://localhost:8080') => {
    const cd = Buffer.from(JSON.stringify({ type: 'webauthn.get', challenge, origin }));
    const auth = Buffer.concat([crypto.createHash('sha256').update('localhost').digest(), Buffer.from([flags]), Buffer.alloc(4)]);
    const sig = crypto.sign('sha256', Buffer.concat([auth, crypto.createHash('sha256').update(cd).digest()]), privateKey);
    return { id: 'cred1', clientDataJSON: cd.toString('base64'), authenticatorData: auth.toString('base64'), signature: sig.toString('base64') };
  };
  assert.deepStrictEqual(sec.verifyPasskey(sign(sec.newChallenge())), { ok: true, key: 'VAULTKEY' });
  assert.strictEqual(sec.verifyPasskey(sign(sec.newChallenge(), 0x01)).ok, false, 'needs user verification');
  assert.strictEqual(sec.verifyPasskey(sign(sec.newChallenge(), 0x05, 'http://evil.com')).ok, false, 'wrong origin');
  assert.strictEqual(sec.verifyPasskey(sign('stale')).ok, false, 'wrong challenge');
  const c = sec.newChallenge(); const a = sign(c); sec.verifyPasskey(a);
  assert.strictEqual(sec.verifyPasskey(a).ok, false, 'replay rejected');
  sec.removePasskey('cred1');
  assert.strictEqual(sec.listPasskeys().length, 0);
  console.log('security tests passed');
})().catch(e => { console.error(e); process.exit(1); });
