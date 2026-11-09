// End-to-end API test against `wrangler dev --local --var MAIL_MODE:log`.
// Run: node test/api.test.mjs [base]
import assert from 'node:assert';
const BASE = process.argv[2] || 'http://localhost:8787';
const call = async (method, path, body, token) => {
  const r = await fetch(BASE + path, { method, headers: { 'content-type': 'application/json', ...(token ? { authorization: 'Bearer ' + token } : {}) }, body: body ? JSON.stringify(body) : undefined });
  return { status: r.status, ...(await r.json()) };
};
const lastCode = async () => (await call('GET', '/api/dev/last-mail')).text.match(/\b(\d{6})\b/)[1];
const wait = ms => new Promise(r => setTimeout(r, ms));
const rnd = Math.random().toString(36).slice(2, 8);
const mum = `mum-${rnd}@example.com`, admin = 'cbevs@icloud.com';

// Sign up + verify
assert.strictEqual((await call('POST', '/api/signup', { email: 'nope', password: 'longpassword' })).error, 'bad_email');
assert.strictEqual((await call('POST', '/api/signup', { email: mum, password: 'short' })).error, 'short_password');
assert.ok((await call('POST', '/api/signup', { email: mum, password: 'longpassword' })).ok);
assert.strictEqual((await call('POST', '/api/login', { email: mum, password: 'longpassword' })).error, 'wrong_login', 'unverified accounts cannot log in');
assert.strictEqual((await call('POST', '/api/verify', { email: mum, code: '000000' === (await lastCode()) ? '111111' : '000000' })).error, 'wrong');
const v = await call('POST', '/api/verify', { email: mum, code: await lastCode() });
assert.ok(v.ok && v.token && v.admin === false);
assert.strictEqual((await call('POST', '/api/signup', { email: mum, password: 'longpassword' })).error, 'exists');

// Login, me
const l = await call('POST', '/api/login', { email: mum, password: 'longpassword' });
assert.ok(l.ok && l.token);
assert.deepStrictEqual((await call('GET', '/api/me', null, l.token)).email, mum);
assert.strictEqual((await call('GET', '/api/admin/mail', null, l.token)).error, 'not_admin');

// 5 wrong passwords -> locked + reset code emailed
for (let i = 0; i < 4; i++) assert.strictEqual((await call('POST', '/api/login', { email: mum, password: 'bad' })).left, 4 - i);
const locked = await call('POST', '/api/login', { email: mum, password: 'bad' });
assert.ok(locked.error === 'locked' && locked.emailed);
assert.strictEqual((await call('POST', '/api/login', { email: mum, password: 'longpassword' })).error, 'locked', 'even the right password waits');
const mail = await call('GET', '/api/dev/last-mail');
assert.strictEqual(mail.to, mum); assert.match(mail.text, /wrong Guardian password 5 times/);
const reset = await call('POST', '/api/reset/confirm', { email: mum, code: await lastCode(), password: 'newpassword1' });
assert.ok(reset.ok && reset.token, 'reset unlocks');
assert.strictEqual((await call('GET', '/api/me', null, l.token)).error, 'signed_out', 'old sessions signed out');
assert.ok((await call('POST', '/api/login', { email: mum, password: 'newpassword1' })).ok);

// Reset request for unknown email looks the same
assert.ok((await call('POST', '/api/reset/request', { email: `ghost-${rnd}@example.com` })).ok);

// Admin
const existingAdmin = await call('POST', '/api/login', { email: admin, password: 'adminpassword' });
let at = existingAdmin.token;
if (!at) {
  await call('POST', '/api/signup', { email: admin, password: 'adminpassword' });
  at = (await call('POST', '/api/verify', { email: admin, code: await lastCode() })).token;
}
assert.ok((await call('GET', '/api/me', null, at)).admin === true);
assert.ok((await call('PUT', '/api/admin/mail', { mail: { host: 'smtp.mail.me.com', port: 587, user: 'cbevs@icloud.com', pass: 'abcd-efgh-ijkl-mnop' } }, at)).ok);
const got = await call('GET', '/api/admin/mail', null, at);
assert.deepStrictEqual(got.mail, { host: 'smtp.mail.me.com', port: 587, user: 'cbevs@icloud.com' }, 'password never sent back');
assert.ok((await call('GET', '/api/admin/stats', null, at)).accounts >= 2);

// Change password
assert.strictEqual((await call('POST', '/api/password', { old: 'wrong', password: 'whatever12' }, reset.token)).error, 'wrong_login');
assert.ok((await call('POST', '/api/password', { old: 'newpassword1', password: 'newpassword2' }, reset.token)).ok);
console.log('API tests passed');
