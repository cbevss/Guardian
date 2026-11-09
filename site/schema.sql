-- Guardian accounts database (Cloudflare D1)
CREATE TABLE IF NOT EXISTS accounts (
  email        TEXT PRIMARY KEY,
  pw_hash      TEXT NOT NULL,
  pw_salt      TEXT NOT NULL,
  verified     INTEGER NOT NULL DEFAULT 0,
  created      INTEGER NOT NULL,
  fails        INTEGER NOT NULL DEFAULT 0,
  locked_until INTEGER NOT NULL DEFAULT 0
);
-- One-time email codes: purpose is 'verify' or 'reset'
CREATE TABLE IF NOT EXISTS codes (
  email   TEXT NOT NULL,
  purpose TEXT NOT NULL,
  hash    TEXT NOT NULL,
  expires INTEGER NOT NULL,
  tries   INTEGER NOT NULL DEFAULT 0,
  sent    INTEGER NOT NULL,
  PRIMARY KEY (email, purpose)
);
CREATE TABLE IF NOT EXISTS sessions (
  token_hash TEXT PRIMARY KEY,
  email      TEXT NOT NULL,
  created    INTEGER NOT NULL,
  expires    INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS settings (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
