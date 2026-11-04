import { test } from "node:test";
import assert from "node:assert/strict";
import type { AddressInfo } from "node:net";
import type Anthropic from "@anthropic-ai/sdk";
import { createApp } from "../src/app.js";
import type { Billing, BillingEvent } from "../src/billing.js";
import { interpret, taxBehavior, taxParams } from "../src/billing.js";
import { loadConfig, parsePlans } from "../src/config.js";
import { chargeMicros, messageCostMicros } from "../src/pricing.js";
import { prepare } from "../src/relay.js";
import { MemoryStore, spend, type Store } from "../src/store.js";
import { PgStore } from "../src/pgstore.js";
import pg from "pg";

// PFX_TEST_DATABASE_URL runs the flows on Postgres too (the database is wiped first).
const DB = process.env.PFX_TEST_DATABASE_URL;
const stores: [string, () => Promise<Store>][] = [["memory", async () => new MemoryStore()]];
if (DB) {
  stores.push([
    "postgres",
    async () => {
      const c = new pg.Client({ connectionString: DB });
      await c.connect();
      await c.query("DROP SCHEMA public CASCADE; CREATE SCHEMA public;");
      await c.end();
      return PgStore.connect(DB);
    },
  ]);
}
import type { Params } from "../src/relay.js";

function fakeMessage(model: string, usage: Partial<Anthropic.Beta.BetaUsage>): Anthropic.Beta.BetaMessage {
  return {
    id: "msg_1",
    type: "message",
    role: "assistant",
    model,
    content: [{ type: "text", text: "Made a box.", citations: null }],
    stop_reason: "end_turn",
    stop_sequence: null,
    usage: { input_tokens: 0, output_tokens: 0, cache_creation_input_tokens: 0, cache_read_input_tokens: 0, iterations: null, ...usage },
  } as unknown as Anthropic.Beta.BetaMessage;
}

class FakeBilling implements Billing {
  next: BillingEvent = { kind: "ignored" };
  active: string | null = null;
  // Customers Stripe knows; others fail like a test-mode customer under live keys.
  known = new Set<string>();
  made = 0;
  async createCustomer() {
    const id = this.made++ === 0 ? "cus_1" : `cus_${this.made}`;
    this.known.add(id);
    return id;
  }
  private check(c: string) {
    if (!this.known.has(c)) throw Object.assign(new Error(`No such customer: '${c}'`), { code: "resource_missing", param: "customer" });
  }
  missingCustomer(e: unknown) {
    return (e as { code?: string }).code === "resource_missing";
  }
  async checkoutPlan(c: string, _u: string, priceId: string) {
    this.check(c);
    return `https://stripe.test/plan/${priceId}`;
  }
  async checkoutTopup(c: string, _u: string, amount: number, currency: string) {
    this.check(c);
    return `https://stripe.test/topup/${amount}${currency}`;
  }
  async checkoutFounder(amount: number, currency: string) {
    return `https://stripe.test/founder/${amount}${currency}`;
  }
  async portal() {
    return "https://stripe.test/portal";
  }
  async activePrice() {
    return this.active;
  }
  async priceLabel() {
    return "$8/month";
  }
  parseEvent(_raw: Buffer, signature: string) {
    if (signature !== "good") throw new Error("bad signature");
    return this.next;
  }
}

async function setup(makeStore: () => Promise<Store>, env: Record<string, string> = {}) {
  const config = loadConfig({ PLANS: "basic=price_basic=8", TOPUPS: "5,10", FREE_CREDIT: "0.05", MARGIN: "0.25", CURRENCY: "aud", USD_RATE: "1.6", ADMIN_TOKEN: "admin", ...env });
  const store = await makeStore();
  const billing = new FakeBilling();
  const mails: string[] = [];
  const sent: Params[] = [];
  const app = createApp({
    config,
    store,
    billing,
    mail: async (_to, _subject, text) => {
      mails.push(text);
    },
    send: async (params) => {
      sent.push(params);
      // 10k input + 1k output on Opus 5.5 = $0.04 + $0.02 = $0.06.
      return fakeMessage(params.model, { input_tokens: 10_000, output_tokens: 1_000 });
    },
  });
  const server = app.listen(0);
  await new Promise((r) => server.once("listening", r));
  const base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  const post = (path: string, body: unknown, token?: string) =>
    fetch(base + path, { method: "POST", headers: { "content-type": "application/json", ...(token ? { authorization: `Bearer ${token}` } : {}) }, body: JSON.stringify(body) });
  const form = (path: string, fields: Record<string, string>) => fetch(base + path, { method: "POST", body: new URLSearchParams(fields) });
  return { base, server, store, billing, mails, sent, post, form, config };
}

async function signIn(t: Awaited<ReturnType<typeof setup>>, email = "Artist@Example.com") {
  const start = await (await t.post("/v1/device/start", {})).json();
  assert.match(start.user_code, /^[A-Z]{4}-[A-Z]{4}$/);
  assert.deepEqual(await (await t.post("/v1/device/token", { device_code: start.device_code })).json(), { status: "pending" });
  // The browser: code + email, then the emailed link, then confirm.
  const page = await t.form("/device", { code: start.user_code.toLowerCase().replace("-", ""), email });
  assert.equal(page.status, 200);
  const link = t.mails.at(-1)!.match(/https?:\/\/\S+\/login\?t=(\S+)/)!;
  const confirm = await fetch(`${t.base}/login?t=${link[1]}`);
  assert.match(await confirm.text(), new RegExp(start.user_code));
  assert.equal((await t.form("/login", { t: link[1] })).status, 200);
  // The link works once.
  assert.equal((await t.form("/login", { t: link[1] })).status, 400);
  const done = await (await t.post("/v1/device/token", { device_code: start.device_code })).json();
  assert.equal(done.status, "ok");
  assert.equal(done.email, "artist@example.com");
  // The device code is used up.
  assert.equal((await t.post("/v1/device/token", { device_code: start.device_code })).status, 400);
  return done.token as string;
}

const request = { model: "claude-opus-5-5", max_tokens: 99999, messages: [{ role: "user", content: "a box" }], tools: [{ name: "edit_scene", input_schema: { type: "object" } }], cache_control: { type: "ephemeral" }, fallbacks: "default" };

for (const [name, makeStore] of stores) test(`sign in, spend free credit, top up, subscribe (${name})`, async () => {
  const t = await setup(makeStore);
  try {
    const token = await signIn(t);
    const account = await (await fetch(`${t.base}/v1/account`, { headers: { authorization: `Bearer ${token}` } })).json();
    assert.equal(account.balance, 0.05);
    assert.equal(account.per_usd, 2);
    assert.equal(account.symbol, "A$");
    assert.deepEqual(account.plans, [{ id: "basic", credit: 8, price: "$8/month", current: false }]);

    // US$0.06 at cost is A$0.096 at 1.6, A$0.12 with the 25% margin: the free A$0.05 goes below zero once.
    const r = await (await t.post("/v1/messages", request, token)).json();
    assert.equal(r.content[0].text, "Made a box.");
    assert.equal(r.pfx_billing.charged, 0.12);
    assert.equal(r.pfx_billing.balance, -0.07);
    assert.equal(r.pfx_billing.currency, "aud");
    assert.equal(t.sent[0].max_tokens, 16000);
    assert.equal((t.sent[0] as unknown as { fallbacks: string }).fallbacks, "default");
    assert.deepEqual(t.sent[0].betas, ["server-side-fallback-2026-07-01"]);
    // Out of credit now.
    const broke = await t.post("/v1/messages", request, token);
    assert.equal(broke.status, 402);

    // A top-up: Checkout, then the webhook credits it once.
    const checkout = await (await t.post("/v1/checkout", { topup: 10 }, token)).json();
    assert.equal(checkout.url, "https://stripe.test/topup/10aud");
    const user = (await t.store.userByEmail("artist@example.com"))!;
    assert.equal(user.stripeCustomer, "cus_1");
    t.billing.next = { kind: "topup", id: "cs_1", userId: user.id, micros: 10_000_000 };
    const hook = (sig: string) => fetch(`${t.base}/stripe/webhook`, { method: "POST", headers: { "content-type": "application/json", "stripe-signature": sig }, body: "{}" });
    assert.equal((await hook("bad")).status, 400);
    assert.equal((await hook("good")).status, 200);
    assert.equal((await hook("good")).status, 200);
    assert.equal((await t.store.userById(user.id))!.topupMicros, 10_000_000 - 70_000);

    // A plan: its paid invoice fills plan credit, which is spent first.
    t.billing.active = "price_basic";
    t.billing.next = { kind: "renewed", id: "in_1", customerId: "cus_1" };
    await hook("good");
    let u = (await t.store.userById(user.id))!;
    assert.equal(u.plan, "basic");
    assert.equal(u.planMicros, 8_000_000);
    await t.post("/v1/messages", request, token);
    u = (await t.store.userById(user.id))!;
    assert.equal(u.planMicros, 8_000_000 - 120_000);
    assert.equal(u.topupMicros, 10_000_000 - 70_000);
    // The next month's credit adds to what's left.
    t.billing.next = { kind: "renewed", id: "in_2", customerId: "cus_1" };
    await hook("good");
    u = (await t.store.userById(user.id))!;
    assert.equal(u.planMicros, 2 * 8_000_000 - 120_000);
    // Subscribed: the plan button now opens the portal.
    assert.equal((await (await t.post("/v1/checkout", { plan: "basic" }, token)).json()).url, "https://stripe.test/portal");
    // Cancelled: the plan's unused credit is kept, as top-up credit.
    t.billing.next = { kind: "changed", id: "evt_9", customerId: "cus_1", priceId: "price_basic", active: false };
    await hook("good");
    u = (await t.store.userById(user.id))!;
    assert.equal(u.plan, null);
    assert.equal(u.planMicros, 0);
    assert.equal(u.topupMicros, 10_000_000 - 70_000 + 2 * 8_000_000 - 120_000);

    const stats = await (await fetch(`${t.base}/admin/stats`, { headers: { authorization: "Bearer admin" } })).json();
    assert.deepEqual(stats, { currency: "aud", users: 1, requests: 2, anthropic_cost_usd: 0.12, anthropic_cost: 0.19, charged: 0.24, margin: 0.05, topups_paid: 10, founders: 0 });
    assert.equal((await fetch(`${t.base}/admin/stats`)).status, 404);

    // A customer Stripe no longer knows (made with test keys) is replaced.
    t.billing.known.clear();
    assert.equal((await (await t.post("/v1/checkout", { topup: 5 }, token)).json()).url, "https://stripe.test/topup/5aud");
    assert.equal((await t.store.userById(user.id))!.stripeCustomer, "cus_2");

    // Signing out ends the session.
    await t.post("/v1/logout", {}, token);
    assert.equal((await t.post("/v1/messages", request, token)).status, 401);
  } finally {
    t.server.close();
  }
});

for (const [name, makeStore] of stores) test(`requests are checked before they cost anything (${name})`, async () => {
  const t = await setup(makeStore);
  try {
    const token = await signIn(t);
    const bad = async (body: unknown) => (await (await t.post("/v1/messages", body, token)).json()).error.message as string;
    assert.match(await bad({ ...request, model: "some-other-model" }), /model must be/);
    assert.match(await bad({ ...request, stream: true }), /unsupported fields: stream/);
    assert.match(await bad({ ...request, tools: [{ type: "web_search_20260209", name: "web_search" }] }), /custom tools/);
    assert.equal(t.sent.length, 0);
    assert.equal((await t.post("/v1/messages", request)).status, 401);
    assert.equal((await t.post("/v1/messages", request, "pfx_wrong")).status, 401);
  } finally {
    t.server.close();
  }
});

for (const [name, makeStore] of stores) test(`Founder's licenses: checkout, seats, credit and the license email (${name})`, async () => {
  const t = await setup(makeStore, { FOUNDER_SEATS: "2", FOUNDER_PRICE: "49", FOUNDER_CREDIT: "10" });
  try {
    const seats = async () => (await fetch(`${t.base}/v1/founders`)).json();
    const first = await fetch(`${t.base}/v1/founders`);
    assert.equal(first.headers.get("access-control-allow-origin"), "*");
    assert.deepEqual(await first.json(), { open: true, seats: 2, sold: 0, left: 2, price: 49, currency: "aud", symbol: "A$" });
    // The website's button goes straight to Stripe.
    const buy = await fetch(`${t.base}/founder`, { redirect: "manual" });
    assert.equal(buy.status, 303);
    assert.equal(buy.headers.get("location"), "https://stripe.test/founder/49aud");

    // A buyer who has used the assistant already, under the same email.
    await signIn(t);
    const hook = () => fetch(`${t.base}/stripe/webhook`, { method: "POST", headers: { "content-type": "application/json", "stripe-signature": "good" }, body: "{}" });
    t.billing.next = { kind: "founder", id: "cs_f1", email: "Artist@Example.com", licensee: "Tiny Studio", paidMicros: 49_000_000 };
    await hook();
    await hook();
    const early = (await t.store.userByEmail("artist@example.com"))!;
    assert.equal(early.topupMicros, 50_000 + 10_000_000, "the free credit plus the founder's A$10, once");
    const letter = t.mails.at(-1)!;
    assert.match(letter, /Founder's license #1 of 2/);
    assert.match(letter, /Licensed to: Tiny Studio/);
    assert.match(letter, /A\$10 of AI credit/);
    assert.equal(t.mails.filter((m) => m.includes("Founder's license #")).length, 1, "one email per payment");

    // A buyer with no account yet gets one, holding the credit for when they sign in.
    t.billing.next = { kind: "founder", id: "cs_f2", email: "new@example.com", licensee: "", paidMicros: 49_000_000 };
    await hook();
    assert.equal((await t.store.userByEmail("new@example.com"))!.topupMicros, 50_000 + 10_000_000);
    assert.match(t.mails.at(-1)!, /#2 of 2[\s\S]*Licensed to: new@example.com/);

    // Sold out: the counter says so and the button stops selling.
    assert.deepEqual(await seats(), { open: false, seats: 2, sold: 2, left: 0, price: 49, currency: "aud", symbol: "A$" });
    assert.equal((await fetch(`${t.base}/founder`, { redirect: "manual" })).status, 410);
    const stats = await (await fetch(`${t.base}/admin/stats`, { headers: { authorization: "Bearer admin" } })).json();
    assert.equal(stats.founders, 2);
  } finally {
    t.server.close();
  }
});

for (const [name, makeStore] of stores) test(`Founder's licenses are off unless FOUNDER_SEATS is set (${name})`, async () => {
  const t = await setup(makeStore);
  try {
    assert.equal((await (await fetch(`${t.base}/v1/founders`)).json()).open, false);
    assert.equal((await fetch(`${t.base}/founder`, { redirect: "manual" })).status, 404);
  } finally {
    t.server.close();
  }
});

test("pricing, plans and haiku", () => {
  // Fallback attempts are each priced at their own model; unknown models at a high rate.
  const msg = fakeMessage("claude-sonnet-5-5", {
    input_tokens: 1,
    iterations: [
      { type: "message", model: "claude-opus-5-5", input_tokens: 1_000_000, output_tokens: 0, cache_creation_input_tokens: 0, cache_read_input_tokens: 0 },
      { type: "fallback_message", model: "claude-mystery", input_tokens: 0, output_tokens: 1_000_000, cache_creation_input_tokens: 0, cache_read_input_tokens: 0 },
    ] as unknown as Anthropic.Beta.BetaIterationsUsage,
  });
  assert.equal(messageCostMicros(msg), 4_000_000 + 25_000_000);
  assert.equal(chargeMicros(100, 0.25), 125);
  assert.equal(chargeMicros(1, 0.25), 2);
  assert.equal(chargeMicros(100, 0.25, 1.6), 200);
  assert.throws(() => loadConfig({ CURRENCY: "usd", USD_RATE: "1.6" }));
  assert.throws(() => loadConfig({ CURRENCY: "dollars" }));
  assert.deepEqual(spend({ id: "1", email: "", plan: "p", planMicros: 50, topupMicros: 100, stripeCustomer: null }, 80), { id: "1", email: "", plan: "p", planMicros: 0, topupMicros: 70, stripeCustomer: null });
  assert.deepEqual(parsePlans("basic=price_1=8, pro=price_2=20.5"), [
    { id: "basic", priceId: "price_1", creditMicros: 8_000_000 },
    { id: "pro", priceId: "price_2", creditMicros: 20_500_000 },
  ]);
  assert.throws(() => parsePlans("basic=price_1"));
  const haiku = prepare({ ...request, model: "claude-haiku-5-5" });
  assert.ok("params" in haiku && !("fallbacks" in haiku.params) && !("betas" in haiku.params));
});

test("stripe events", () => {
  const ev = (type: string, object: unknown) => ({ id: "evt_1", type, data: { object } }) as never;
  assert.deepEqual(interpret(ev("checkout.session.completed", { id: "cs_1", mode: "payment", payment_status: "paid", metadata: { user_id: "7", credit_micros: "5000000" } })), { kind: "topup", id: "cs_1", userId: "7", micros: 5_000_000 });
  assert.deepEqual(interpret(ev("checkout.session.completed", { id: "cs_2", mode: "payment", payment_status: "unpaid", metadata: {} })), { kind: "ignored" });
  const founder = { id: "cs_f", mode: "payment", payment_status: "paid", amount_total: 4900, metadata: { kind: "founder" }, customer_details: { email: "a@b.co" }, custom_fields: [{ key: "licensee", text: { value: " Tiny Studio " } }] };
  assert.deepEqual(interpret(ev("checkout.session.completed", founder)), { kind: "founder", id: "cs_f", email: "a@b.co", licensee: "Tiny Studio", paidMicros: 49_000_000 });
  assert.deepEqual(interpret(ev("checkout.session.completed", { ...founder, custom_fields: [] })), { kind: "founder", id: "cs_f", email: "a@b.co", licensee: "", paidMicros: 49_000_000 });
  assert.deepEqual(interpret(ev("checkout.session.completed", { id: "cs_3", mode: "subscription", payment_status: "paid" })), { kind: "ignored" });
  assert.deepEqual(interpret(ev("invoice.paid", { id: "in_1", billing_reason: "subscription_cycle", customer: "cus_1" })), { kind: "renewed", id: "in_1", customerId: "cus_1" });
  assert.deepEqual(interpret(ev("invoice.paid", { id: "in_2", billing_reason: "manual", customer: "cus_1" })), { kind: "ignored" });
  assert.deepEqual(interpret(ev("customer.subscription.deleted", { customer: { id: "cus_1" }, status: "canceled", items: { data: [{ price: { id: "price_1" } }] } })), { kind: "changed", id: "evt_1", customerId: "cus_1", priceId: "price_1", active: false });
});

test("Stripe Tax stays off until it's turned on", () => {
  assert.equal(loadConfig({}).stripeTax, "off");
  assert.equal(loadConfig({ STRIPE_TAX: " Inclusive " }).stripeTax, "inclusive");
  assert.throws(() => loadConfig({ STRIPE_TAX: "maybe" }), /STRIPE_TAX/);
  // Off: checkouts are exactly as before.
  assert.deepEqual(taxParams("off", true), {});
  assert.deepEqual(taxBehavior("off"), {});
  // On: tax from the buyer's address, tax IDs for businesses, saved customers keep their address.
  assert.deepEqual(taxParams("inclusive", true), { automatic_tax: { enabled: true }, tax_id_collection: { enabled: true }, customer_update: { address: "auto", name: "auto" } });
  assert.deepEqual(taxParams("exclusive", false), { automatic_tax: { enabled: true }, tax_id_collection: { enabled: true } });
  assert.deepEqual(taxBehavior("inclusive"), { tax_behavior: "inclusive" });
});

for (const [name, makeStore] of stores) {
  test(`crash reports: sent without an account, read with the admin token (${name})`, async () => {
    const t = await setup(makeStore);
    try {
      const report = "ProceduralFX Studio crash report\nVersion: 0.22.0\nPanic: index out of bounds\n";
      const sent = await t.post("/v1/crash-reports", { app_version: "0.22.0", os: "macos aarch64", report, note: "while baking a splash" });
      assert.equal(sent.status, 201);
      assert.deepEqual(await sent.json(), { ok: true });
      // Missing or oversized parts are refused.
      assert.equal((await t.post("/v1/crash-reports", { app_version: "0.22.0", os: "macos" })).status, 400);
      assert.equal((await t.post("/v1/crash-reports", { app_version: "0.22.0", os: "macos", report: "x".repeat(64_001) })).status, 400);
      assert.equal((await t.post("/v1/crash-reports", { app_version: 3, os: "macos", report })).status, 400);
      // Only with the admin token.
      assert.equal((await fetch(`${t.base}/admin/crash-reports`)).status, 404);
      const listed = await (await fetch(`${t.base}/admin/crash-reports`, { headers: { authorization: "Bearer admin" } })).json();
      assert.equal(listed.length, 1);
      assert.equal(listed[0].report, report);
      assert.equal(listed[0].note, "while baking a splash");
      assert.equal(listed[0].os, "macos aarch64");
      assert.ok(!("ip" in listed[0]), "no IP address is kept");
      // At most five tries an hour from one address (refused ones count too): four so far.
      assert.equal((await t.post("/v1/crash-reports", { app_version: "0.22.0", os: "linux", report: "again" })).status, 201);
      assert.equal((await t.post("/v1/crash-reports", { app_version: "0.22.0", os: "linux", report: "again" })).status, 429);
    } finally {
      t.server.close();
    }
  });
}
