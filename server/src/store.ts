// Accounts, sign-in codes, sessions and balances. Two implementations: MemoryStore
// (tests, trying things locally) and PgStore (pgstore.ts, production).
//
// Each user has two balances: plan credit, refilled to the plan's amount every month
// (unused credit doesn't roll over), and top-up credit, which never expires.
// Requests spend plan credit first.

export interface User {
  id: string;
  email: string;
  plan: string | null;
  planMicros: number;
  topupMicros: number;
  stripeCustomer: string | null;
}

export interface Device {
  deviceHash: string;
  userCode: string;
  expiresAt: number;
  userId: string | null;
}

export interface Login {
  tokenHash: string;
  email: string;
  userCode: string;
  expiresAt: number;
}

export interface UsageRow {
  userId: string;
  model: string;
  inputTokens: number;
  outputTokens: number;
  cacheWriteTokens: number;
  cacheReadTokens: number;
  costMicros: number;
  chargedMicros: number;
}

export interface Stats {
  users: number;
  requests: number;
  costMicros: number;
  chargedMicros: number;
  paidMicros: number;
}

export interface Store {
  createDevice(d: Device): Promise<void>;
  deviceByUserCode(userCode: string): Promise<Device | null>;
  deviceByHash(deviceHash: string): Promise<Device | null>;
  approveDevice(userCode: string, userId: string): Promise<boolean>;
  deleteDevice(deviceHash: string): Promise<void>;

  createLogin(l: Login): Promise<void>;
  peekLogin(tokenHash: string): Promise<Login | null>;
  takeLogin(tokenHash: string): Promise<Login | null>;

  userById(id: string): Promise<User | null>;
  userByEmail(email: string): Promise<User | null>;
  userByCustomer(customerId: string): Promise<User | null>;
  createUser(email: string, freeMicros: number): Promise<User>;
  setCustomer(userId: string, customerId: string): Promise<void>;

  createSession(tokenHash: string, userId: string): Promise<void>;
  userBySession(tokenHash: string): Promise<User | null>;
  deleteSession(tokenHash: string): Promise<void>;

  // Spends plan credit, then top-up credit (which may go below zero). Logs the usage.
  charge(row: UsageRow): Promise<User>;
  // Credits a paid top-up once per payment id; false if it was already counted.
  addTopup(paymentId: string, userId: string, micros: number): Promise<boolean>;
  // Sets the plan, and (when micros isn't null) refills plan credit, once per event id.
  setPlan(eventId: string, userId: string, plan: string | null, micros: number | null): Promise<boolean>;

  stats(): Promise<Stats>;
}

export function balanceMicros(u: User): number {
  return u.planMicros + u.topupMicros;
}

// Plan credit first, then top-up credit.
export function spend(u: User, micros: number): User {
  const fromPlan = Math.min(Math.max(u.planMicros, 0), micros);
  return { ...u, planMicros: u.planMicros - fromPlan, topupMicros: u.topupMicros - (micros - fromPlan) };
}

export class MemoryStore implements Store {
  devices = new Map<string, Device>();
  logins = new Map<string, Login>();
  users = new Map<string, User>();
  sessions = new Map<string, string>();
  processed = new Set<string>();
  usage: UsageRow[] = [];
  paid = 0;
  private next = 1;

  async createDevice(d: Device) {
    this.devices.set(d.deviceHash, { ...d });
  }
  async deviceByUserCode(userCode: string) {
    return [...this.devices.values()].find((d) => d.userCode === userCode && d.expiresAt > Date.now()) ?? null;
  }
  async deviceByHash(deviceHash: string) {
    return this.devices.get(deviceHash) ?? null;
  }
  async approveDevice(userCode: string, userId: string) {
    const d = await this.deviceByUserCode(userCode);
    if (!d || d.userId) return false;
    d.userId = userId;
    return true;
  }
  async deleteDevice(deviceHash: string) {
    this.devices.delete(deviceHash);
  }

  async createLogin(l: Login) {
    this.logins.set(l.tokenHash, { ...l });
  }
  async peekLogin(tokenHash: string) {
    const l = this.logins.get(tokenHash);
    return l && l.expiresAt > Date.now() ? l : null;
  }
  async takeLogin(tokenHash: string) {
    const l = await this.peekLogin(tokenHash);
    this.logins.delete(tokenHash);
    return l;
  }

  async userById(id: string) {
    return this.users.get(id) ?? null;
  }
  async userByEmail(email: string) {
    return [...this.users.values()].find((u) => u.email === email) ?? null;
  }
  async userByCustomer(customerId: string) {
    return [...this.users.values()].find((u) => u.stripeCustomer === customerId) ?? null;
  }
  async createUser(email: string, freeMicros: number) {
    const u: User = { id: `u${this.next++}`, email, plan: null, planMicros: 0, topupMicros: freeMicros, stripeCustomer: null };
    this.users.set(u.id, u);
    return u;
  }
  async setCustomer(userId: string, customerId: string) {
    const u = this.users.get(userId);
    if (u) u.stripeCustomer = customerId;
  }

  async createSession(tokenHash: string, userId: string) {
    this.sessions.set(tokenHash, userId);
  }
  async userBySession(tokenHash: string) {
    const id = this.sessions.get(tokenHash);
    return id ? (this.users.get(id) ?? null) : null;
  }
  async deleteSession(tokenHash: string) {
    this.sessions.delete(tokenHash);
  }

  async charge(row: UsageRow) {
    const u = this.users.get(row.userId);
    if (!u) throw new Error("no such user");
    const after = spend(u, row.chargedMicros);
    this.users.set(u.id, after);
    this.usage.push(row);
    return after;
  }
  async addTopup(paymentId: string, userId: string, micros: number) {
    const u = this.users.get(userId);
    if (!u || this.processed.has(paymentId)) return false;
    this.processed.add(paymentId);
    u.topupMicros += micros;
    this.paid += micros;
    return true;
  }
  async setPlan(eventId: string, userId: string, plan: string | null, micros: number | null) {
    const u = this.users.get(userId);
    if (!u || this.processed.has(eventId)) return false;
    this.processed.add(eventId);
    u.plan = plan;
    if (micros !== null) u.planMicros = micros;
    if (plan === null) u.planMicros = 0;
    return true;
  }

  async stats() {
    return {
      users: this.users.size,
      requests: this.usage.length,
      costMicros: this.usage.reduce((s, r) => s + r.costMicros, 0),
      chargedMicros: this.usage.reduce((s, r) => s + r.chargedMicros, 0),
      paidMicros: this.paid,
    };
  }
}
