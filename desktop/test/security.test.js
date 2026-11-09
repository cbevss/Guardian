'use strict';
// node test/security.test.js
const assert = require('assert');
const crypto = require('crypto');
const os = require('os'), path = require('path'), fs = require('fs');
const { createSecurity } = require('../security');

const file = path.join(fs.mkdtempSync(path.join(os.tmpdir(), 'gsec-')), 'security.json');
const safeStorage = { encryptString: s => Buffer.from('X' + s), decryptString: b => b.toString().slice(1) };
const sent = [];
const sec = createSecurity({ file, safeStorage, sendMail: async (s, m) => sent.push({ s, m }) });

(async () => {
  // Reset email needs settings first
  await assert.rejects(sec.sendResetCode('me@example.com'), /not_configured/);
  assert.ok(sec.setSmtp({ user: 'family@gmail.com', pass: 'abcd efgh ijkl mnop' }));
  assert.deepStrictEqual(sec.smtpInfo(), { configured: true, host: 'smtp.gmail.com', port: 465, user: 'family@gmail.com' });
  assert.ok(sec.setSmtp({ user: 'family@gmail.com' })); // blank password keeps the old one
  assert.ok(!fs.readFileSync(file, 'utf8').includes('abcdefgh'), 'smtp password not stored in the clear');

  sec.saveRecovery('VAULTKEY');
  await sec.sendResetCode('me@example.com');
  const code = sent[0].m.text.match(/code is: (\d{6})/)[1];
  assert.strictEqual(sent[0].m.to, 'me@example.com');
  await assert.rejects(sec.sendResetCode('me@example.com'), /wait/); // one per minute
  assert.deepStrictEqual(sec.checkResetCode('000000' === code ? '111111' : '000000'), { ok: false, reason: 'wrong' });
  assert.deepStrictEqual(sec.checkResetCode(code), { ok: true, key: 'VAULTKEY' });
  assert.strictEqual(sec.checkResetCode(code).ok, false, 'codes are single use');

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
