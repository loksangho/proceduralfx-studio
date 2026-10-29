// The Store on Postgres. The schema is created on start (CREATE ... IF NOT EXISTS).

import pg from "pg";
import type { Device, Login, Stats, Store, UsageRow, User } from "./store.js";

export const SCHEMA = `
CREATE TABLE IF NOT EXISTS users (
  id BIGSERIAL PRIMARY KEY,
  email TEXT NOT NULL UNIQUE,
  plan TEXT,
  plan_micros BIGINT NOT NULL DEFAULT 0,
  topup_micros BIGINT NOT NULL DEFAULT 0,
  stripe_customer TEXT UNIQUE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS devices (
  device_hash TEXT PRIMARY KEY,
  user_code TEXT NOT NULL UNIQUE,
  expires_at TIMESTAMPTZ NOT NULL,
  user_id BIGINT REFERENCES users(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS logins (
  token_hash TEXT PRIMARY KEY,
  email TEXT NOT NULL,
  user_code TEXT NOT NULL,
  expires_at TIMESTAMPTZ NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
  token_hash TEXT PRIMARY KEY,
  user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS processed_events (
  id TEXT PRIMARY KEY,
  user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
  credit_micros BIGINT NOT NULL DEFAULT 0,
  kind TEXT NOT NULL,
  at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS usage (
  id BIGSERIAL PRIMARY KEY,
  user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  model TEXT NOT NULL,
  input_tokens BIGINT NOT NULL,
  output_tokens BIGINT NOT NULL,
  cache_write_tokens BIGINT NOT NULL,
  cache_read_tokens BIGINT NOT NULL,
  cost_micros BIGINT NOT NULL,
  charged_micros BIGINT NOT NULL,
  at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS usage_user_at ON usage (user_id, at);
`;

type Row = Record<string, unknown>;

function user(r: Row): User {
  return {
    id: String(r.id),
    email: String(r.email),
    plan: (r.plan as string | null) ?? null,
    planMicros: Number(r.plan_micros),
    topupMicros: Number(r.topup_micros),
    stripeCustomer: (r.stripe_customer as string | null) ?? null,
  };
}

function device(r: Row): Device {
  return { deviceHash: String(r.device_hash), userCode: String(r.user_code), expiresAt: new Date(r.expires_at as string).getTime(), userId: r.user_id == null ? null : String(r.user_id) };
}

function login(r: Row): Login {
  return { tokenHash: String(r.token_hash), email: String(r.email), userCode: String(r.user_code), expiresAt: new Date(r.expires_at as string).getTime() };
}

export class PgStore implements Store {
  constructor(private pool: pg.Pool) {}

  static async connect(url: string): Promise<PgStore> {
    const pool = new pg.Pool({ connectionString: url, max: 10 });
    await pool.query(SCHEMA);
    return new PgStore(pool);
  }

  private async one<T>(sql: string, args: unknown[], map: (r: Row) => T): Promise<T | null> {
    const { rows } = await this.pool.query(sql, args);
    return rows.length ? map(rows[0]) : null;
  }

  async createDevice(d: Device) {
    await this.pool.query("DELETE FROM devices WHERE expires_at < now()");
    await this.pool.query("INSERT INTO devices (device_hash, user_code, expires_at) VALUES ($1, $2, $3)", [d.deviceHash, d.userCode, new Date(d.expiresAt)]);
  }
  deviceByUserCode(userCode: string) {
    return this.one("SELECT * FROM devices WHERE user_code = $1 AND expires_at > now()", [userCode], device);
  }
  deviceByHash(deviceHash: string) {
    return this.one("SELECT * FROM devices WHERE device_hash = $1", [deviceHash], device);
  }
  async approveDevice(userCode: string, userId: string) {
    const r = await this.pool.query("UPDATE devices SET user_id = $2 WHERE user_code = $1 AND user_id IS NULL AND expires_at > now()", [userCode, userId]);
    return (r.rowCount ?? 0) > 0;
  }
  async deleteDevice(deviceHash: string) {
    await this.pool.query("DELETE FROM devices WHERE device_hash = $1", [deviceHash]);
  }

  async createLogin(l: Login) {
    await this.pool.query("DELETE FROM logins WHERE expires_at < now()");
    await this.pool.query("INSERT INTO logins (token_hash, email, user_code, expires_at) VALUES ($1, $2, $3, $4)", [l.tokenHash, l.email, l.userCode, new Date(l.expiresAt)]);
  }
  peekLogin(tokenHash: string) {
    return this.one("SELECT * FROM logins WHERE token_hash = $1 AND expires_at > now()", [tokenHash], login);
  }
  takeLogin(tokenHash: string) {
    return this.one("DELETE FROM logins WHERE token_hash = $1 AND expires_at > now() RETURNING *", [tokenHash], login);
  }

  userById(id: string) {
    return this.one("SELECT * FROM users WHERE id = $1", [id], user);
  }
  userByEmail(email: string) {
    return this.one("SELECT * FROM users WHERE email = $1", [email], user);
  }
  userByCustomer(customerId: string) {
    return this.one("SELECT * FROM users WHERE stripe_customer = $1", [customerId], user);
  }
  async createUser(email: string, freeMicros: number) {
    // Another sign-in may have created it a moment ago.
    const made = await this.one("INSERT INTO users (email, topup_micros) VALUES ($1, $2) ON CONFLICT (email) DO NOTHING RETURNING *", [email, freeMicros], user);
    return made ?? (await this.userByEmail(email))!;
  }
  async setCustomer(userId: string, customerId: string) {
    await this.pool.query("UPDATE users SET stripe_customer = $2 WHERE id = $1", [userId, customerId]);
  }

  async createSession(tokenHash: string, userId: string) {
    await this.pool.query("INSERT INTO sessions (token_hash, user_id) VALUES ($1, $2)", [tokenHash, userId]);
  }
  userBySession(tokenHash: string) {
    return this.one("SELECT u.* FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.token_hash = $1", [tokenHash], user);
  }
  async deleteSession(tokenHash: string) {
    await this.pool.query("DELETE FROM sessions WHERE token_hash = $1", [tokenHash]);
  }

  async charge(row: UsageRow) {
    const client = await this.pool.connect();
    try {
      await client.query("BEGIN");
      // Plan credit first (never below zero), the rest from top-ups.
      const { rows } = await client.query(
        `UPDATE users SET
           plan_micros = plan_micros - LEAST(GREATEST(plan_micros, 0), $2),
           topup_micros = topup_micros - ($2 - LEAST(GREATEST(plan_micros, 0), $2))
         WHERE id = $1 RETURNING *`,
        [row.userId, row.chargedMicros],
      );
      await client.query(
        "INSERT INTO usage (user_id, model, input_tokens, output_tokens, cache_write_tokens, cache_read_tokens, cost_micros, charged_micros) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        [row.userId, row.model, row.inputTokens, row.outputTokens, row.cacheWriteTokens, row.cacheReadTokens, row.costMicros, row.chargedMicros],
      );
      await client.query("COMMIT");
      return user(rows[0]);
    } catch (e) {
      await client.query("ROLLBACK");
      throw e;
    } finally {
      client.release();
    }
  }

  // Runs `apply` in the same transaction as recording `eventId`, so an event counts once.
  private async once(eventId: string, userId: string, kind: string, micros: number, apply: (c: pg.PoolClient) => Promise<unknown>) {
    const client = await this.pool.connect();
    try {
      await client.query("BEGIN");
      const r = await client.query("INSERT INTO processed_events (id, user_id, kind, credit_micros) VALUES ($1, $2, $3, $4) ON CONFLICT (id) DO NOTHING", [eventId, userId, kind, micros]);
      if ((r.rowCount ?? 0) === 0) {
        await client.query("ROLLBACK");
        return false;
      }
      await apply(client);
      await client.query("COMMIT");
      return true;
    } catch (e) {
      await client.query("ROLLBACK");
      throw e;
    } finally {
      client.release();
    }
  }

  addTopup(paymentId: string, userId: string, micros: number) {
    return this.once(paymentId, userId, "topup", micros, (c) => c.query("UPDATE users SET topup_micros = topup_micros + $2 WHERE id = $1", [userId, micros]));
  }
  setPlan(eventId: string, userId: string, plan: string | null, micros: number | null) {
    const sql =
      plan === null
        ? "UPDATE users SET plan = NULL, topup_micros = topup_micros + plan_micros, plan_micros = 0 WHERE id = $1"
        : micros === null
          ? "UPDATE users SET plan = $2 WHERE id = $1"
          : "UPDATE users SET plan = $2, plan_micros = plan_micros + $3 WHERE id = $1";
    const args = plan === null ? [userId] : micros === null ? [userId, plan] : [userId, plan, micros];
    return this.once(eventId, userId, plan === null ? "plan_end" : "plan", micros ?? 0, (c) => c.query(sql, args));
  }

  async stats(): Promise<Stats> {
    const { rows } = await this.pool.query(
      `SELECT (SELECT count(*) FROM users) AS users,
              (SELECT count(*) FROM usage) AS requests,
              (SELECT coalesce(sum(cost_micros), 0) FROM usage) AS cost,
              (SELECT coalesce(sum(charged_micros), 0) FROM usage) AS charged,
              (SELECT coalesce(sum(credit_micros), 0) FROM processed_events WHERE kind = 'topup') AS paid`,
    );
    const r = rows[0];
    return { users: Number(r.users), requests: Number(r.requests), costMicros: Number(r.cost), chargedMicros: Number(r.charged), paidMicros: Number(r.paid) };
  }
}
