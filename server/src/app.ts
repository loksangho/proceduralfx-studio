// The HTTP server: sign-in for the app (a device code confirmed by an emailed link),
// the account and payments, and the metered relay to Claude.

import express, { type Request, type Response, type NextFunction } from "express";
import type { Billing } from "./billing.js";
import type { Config, Plan } from "./config.js";
import type { Mailer } from "./mail.js";
import { chargeMicros, messageCostMicros, ALLOWED_MODELS, money, symbol } from "./pricing.js";
import { prepare, relayError, type Send } from "./relay.js";
import { balanceMicros, type Store, type User } from "./store.js";
import { hash, Limiter, normalizeEmail, normalizeUserCode, secret, userCode } from "./auth.js";
import { checkEmailPage, confirmPage, escape as escapeHtml, messagePage, signInPage } from "./pages.js";

export interface Deps {
  config: Config;
  store: Store;
  send: Send;
  billing: Billing | null;
  mail: Mailer;
}

const DEVICE_MINUTES = 15;
const MAX_IN_FLIGHT = 2;

type Authed = Request & { user?: User; token?: string };

export function createApp({ config, store, send, billing, mail }: Deps) {
  const app = express();
  app.set("trust proxy", 1);
  app.disable("x-powered-by");
  const perIp = new Limiter(30, 60_000);
  const emails = new Limiter(5, 60 * 60_000);
  const crashes = new Limiter(5, 60 * 60_000);
  const inFlight = new Map<string, number>();

  const ip = (req: Request) => req.ip ?? "unknown";
  const fail = (res: Response, status: number, type: string, message: string) => res.status(status).json({ type: "error", error: { type, message } });
  const wrap = (fn: (req: Authed, res: Response) => Promise<unknown>) => (req: Request, res: Response, next: NextFunction) => fn(req as Authed, res).catch(next);

  // Stripe needs the exact bytes it signed, so this comes before the JSON parser.
  app.post(
    "/stripe/webhook",
    express.raw({ type: "application/json", limit: "1mb" }),
    wrap(async (req, res) => {
      if (!billing) return res.status(404).end();
      let event;
      try {
        event = billing.parseEvent(req.body as Buffer, String(req.headers["stripe-signature"] ?? ""));
      } catch {
        return res.status(400).send("bad signature");
      }
      await applyBillingEvent(event);
      res.json({ received: true });
    }),
  );

  async function applyBillingEvent(event: ReturnType<Billing["parseEvent"]>) {
    if (event.kind === "topup") {
      await store.addTopup(event.id, event.userId, event.micros);
      return;
    }
    if (event.kind === "ignored") return;
    if (event.kind === "founder") {
      await recordFounder(event);
      return;
    }
    const user = await store.userByCustomer(event.customerId);
    if (!user) return;
    if (event.kind === "renewed") {
      const priceId = await billing!.activePrice(event.customerId);
      const plan = config.plans.find((p) => p.priceId === priceId);
      if (plan) await store.setPlan(event.id, user.id, plan.id, plan.creditMicros);
    } else if (!event.active) {
      await store.setPlan(event.id, user.id, null, null);
    } else {
      const plan = config.plans.find((p) => p.priceId === event.priceId);
      // A new plan's name now; its credit comes with its next paid invoice.
      if (plan && plan.id !== user.plan) await store.setPlan(event.id, user.id, plan.id, null);
    }
  }

  // A paid Founder's license: the next seat, the AI credit on the account for that email
  // (made if need be, so signing in with it finds the credit), and the license by email.
  async function recordFounder(event: { id: string; email: string; licensee: string; paidMicros: number }) {
    const email = normalizeEmail(event.email) ?? event.email.trim().toLowerCase();
    const user = (await store.userByEmail(email)) ?? (await store.createUser(email, config.freeCreditMicros));
    const founder = await store.addFounder(event.id, user.id, email, event.licensee, event.paidMicros, config.founderCreditMicros);
    if (!founder) return;
    const mail_ = founderEmail(founder.seat, email, event.licensee);
    try {
      await mail(email, mail_.subject, mail_.text, mail_.html);
    } catch (e) {
      // The license is recorded either way; the email can be sent again by hand.
      console.error(`Couldn't email Founder's license #${founder.seat} to ${email}:`, e);
    }
  }

  function founderEmail(seat: number, email: string, licensee: string) {
    const who = licensee || email;
    const credit = money(config.founderCreditMicros);
    const sym = symbol(config.currency);
    const terms = `${config.siteUrl}/founder`;
    const lines = [
      `Thank you for backing ProceduralFX Studio early.`,
      ``,
      `This email is your Founder's license:`,
      ``,
      `  Founder's license #${seat} of ${config.founderSeats}`,
      `  Licensed to: ${who}`,
      `  Email: ${email}`,
      ``,
      `It lets you use ProceduralFX Studio commercially, including effects for games you sell, for every version, forever. The full terms: ${terms}`,
      ``,
      ...(config.founderCreditMicros > 0
        ? [`We've also added ${sym}${credit} of AI credit to the account for ${email}. Sign in with this email in the AI assistant's settings to use it.`, ``]
        : []),
      `As a founder, your feedback shapes what comes next. Email support@build3dstudio.com any time with what works, what doesn't, and what you'd like.`,
      ``,
      `Keep this email: it's your proof of license.`,
      ``,
      `Lok Sang Ho, ProceduralFX Studio`,
    ];
    const text = lines.join("\n");
    const html = lines
      .map((l) => escapeHtml(l))
      .join("<br>")
      .replace(escapeHtml(terms), `<a href="${terms}">${escapeHtml(terms)}</a>`);
    return { subject: `Your ProceduralFX Studio Founder's license (#${seat})`, text, html };
  }

  app.use(express.json({ limit: "25mb" }));
  app.use(express.urlencoded({ extended: false, limit: "16kb" }));

  app.get("/healthz", (_req, res) => {
    res.json({ ok: true });
  });

  // --- Signing in from the app ---------------------------------------------------------

  app.post(
    "/v1/device/start",
    wrap(async (req, res) => {
      if (!perIp.allow(`start:${ip(req)}`)) return fail(res, 429, "rate_limited", "Too many sign-in attempts; wait a minute.");
      const device = secret("dev_");
      const code = userCode();
      await store.createDevice({ deviceHash: hash(device), userCode: code, expiresAt: Date.now() + DEVICE_MINUTES * 60_000, userId: null });
      res.json({ device_code: device, user_code: code, verification_url: `${config.publicUrl}/device?code=${code}`, interval: 3, expires_in: DEVICE_MINUTES * 60 });
    }),
  );

  app.post(
    "/v1/device/token",
    wrap(async (req, res) => {
      const device = typeof req.body?.device_code === "string" ? await store.deviceByHash(hash(req.body.device_code)) : null;
      if (!device || device.expiresAt < Date.now()) return res.status(400).json({ status: "expired" });
      if (!device.userId) return res.json({ status: "pending" });
      await store.deleteDevice(device.deviceHash);
      const user = await store.userById(device.userId);
      const token = secret("pfx_");
      await store.createSession(hash(token), device.userId);
      res.json({ status: "ok", token, email: user?.email ?? "" });
    }),
  );

  app.get("/device", (req, res) => {
    res.send(signInPage(normalizeUserCode(String(req.query.code ?? "")), "", config.siteUrl));
  });

  app.post(
    "/device",
    wrap(async (req, res) => {
      const code = normalizeUserCode(String(req.body?.code ?? ""));
      const email = normalizeEmail(String(req.body?.email ?? ""));
      if (!email) return res.status(400).send(signInPage(code, "That email doesn't look right.", config.siteUrl));
      if (!perIp.allow(`device:${ip(req)}`) || !emails.allow(email)) return res.status(429).send(signInPage(code, "Too many attempts; try again later.", config.siteUrl));
      const device = await store.deviceByUserCode(code);
      if (!device || device.userId) return res.status(400).send(signInPage(code, "That code isn't valid any more: start signing in again from the app.", config.siteUrl));
      const token = secret();
      await store.createLogin({ tokenHash: hash(token), email, userCode: code, expiresAt: Date.now() + DEVICE_MINUTES * 60_000 });
      const link = `${config.publicUrl}/login?t=${token}`;
      try {
        await mail(
        email,
        "Sign in to ProceduralFX Studio",
        `Open this link to finish signing in to ProceduralFX Studio (code ${code}):\n\n${link}\n\nIf you didn't ask for this, ignore this email.`,
        `<p>Open this link to finish signing in to ProceduralFX Studio (code <b>${code}</b>):</p><p><a href="${link}">Sign in</a></p><p>If you didn't ask for this, ignore this email.</p>`,
        );
      } catch (e) {
        console.error("Couldn't send the sign-in email:", e);
        return res.status(502).send(signInPage(code, `We couldn't send the email (${e instanceof Error ? e.message : String(e)}). Please try again in a few minutes.`, config.siteUrl));
      }
      res.send(checkEmailPage(email));
    }),
  );

  // The emailed link shows a button rather than signing in at once, so mail scanners that
  // open links can't sign anyone in.
  app.get(
    "/login",
    wrap(async (req, res) => {
      const token = String(req.query.t ?? "");
      const login = await store.peekLogin(hash(token));
      if (!login) return res.status(400).send(messagePage("Link expired", "This sign-in link has expired or was already used. Start again from the app."));
      res.send(confirmPage(token, login.email, login.userCode));
    }),
  );

  app.post(
    "/login",
    wrap(async (req, res) => {
      const login = await store.takeLogin(hash(String(req.body?.t ?? "")));
      if (!login) return res.status(400).send(messagePage("Link expired", "This sign-in link has expired or was already used. Start again from the app."));
      const user = (await store.userByEmail(login.email)) ?? (await store.createUser(login.email, config.freeCreditMicros));
      if (!(await store.approveDevice(login.userCode, user.id))) {
        return res.status(400).send(messagePage("Code expired", "The code from the app has expired. Start signing in again from the app."));
      }
      res.send(messagePage("You're signed in", "Go back to ProceduralFX Studio: it will finish signing in by itself in a few seconds."));
    }),
  );

  app.get("/paid", (_req, res) => {
    res.send(messagePage("Thank you!", "Your credit will show in ProceduralFX Studio within a minute. You can close this page."));
  });
  // --- The Founder's license (from the website, no sign-in) -----------------------------

  // How many are left, for the website's pricing section.
  app.get(
    "/v1/founders",
    wrap(async (_req, res) => {
      const sold = config.founderSeats > 0 ? await store.founderCount() : 0;
      res.set("access-control-allow-origin", "*");
      res.set("cache-control", "public, max-age=60");
      res.json({
        open: config.founderSeats > 0 && billing !== null && sold < config.founderSeats,
        seats: config.founderSeats,
        sold,
        left: Math.max(0, config.founderSeats - sold),
        price: config.founderPrice,
        currency: config.currency,
        symbol: symbol(config.currency),
      });
    }),
  );

  // The website's Buy button: checks a seat is left, then opens Stripe Checkout.
  app.get(
    "/founder",
    wrap(async (req, res) => {
      if (!perIp.allow(`founder:${ip(req)}`)) return res.status(429).send(messagePage("Too many tries", "Please wait a minute and try again."));
      if (!billing || config.founderSeats <= 0) return res.status(404).send(messagePage("Not on sale", "Founder's licenses aren't on sale right now."));
      if ((await store.founderCount()) >= config.founderSeats) {
        return res.status(410).send(messagePage("Sold out", `All ${config.founderSeats} Founder's licenses have been sold. Thank you to everyone who took one!`));
      }
      res.redirect(303, await billing.checkoutFounder(config.founderPrice, config.currency, config.publicUrl, `${config.siteUrl}/#pricing`));
    }),
  );

  app.get("/founder/thanks", (_req, res) => {
    res.send(messagePage("Thank you, founder!", "Your Founder's license is on its way by email (check spam if it hasn't arrived in a few minutes). You can close this page."));
  });

  app.get("/cancelled", (_req, res) => {
    res.send(messagePage("Payment cancelled", "Nothing was charged. You can close this page."));
  });
  app.get("/", (_req, res) => {
    res.redirect(config.siteUrl);
  });

  // --- The account (the app, signed in) ------------------------------------------------

  const signedIn = (req: Request, res: Response, next: NextFunction) => {
    (async () => {
      const header = String(req.headers.authorization ?? "");
      const token = header.startsWith("Bearer ") ? header.slice(7).trim() : "";
      const user = token ? await store.userBySession(hash(token)) : null;
      if (!user) return fail(res, 401, "authentication_error", "You're signed out: sign in again in the assistant's settings.");
      (req as Authed).user = user;
      (req as Authed).token = token;
      next();
    })().catch(next);
  };

  async function planInfo(p: Plan, user: User) {
    return { id: p.id, credit: money(p.creditMicros), price: billing ? await billing.priceLabel(p.priceId) : null, current: user.plan === p.id };
  }

  app.get(
    "/v1/account",
    signedIn,
    wrap(async (req, res) => {
      const u = req.user!;
      res.json({
        email: u.email,
        currency: config.currency,
        symbol: symbol(config.currency),
        plan: u.plan,
        plan_credit: money(u.planMicros),
        topup_credit: money(u.topupMicros),
        balance: money(balanceMicros(u)),
        // What one US dollar of list-price usage is charged, so the app can estimate costs.
        per_usd: config.usdRate * (1 + config.margin),
        plans: await Promise.all(config.plans.map((p) => planInfo(p, u))),
        topups: config.topups,
        models: ALLOWED_MODELS,
        payments: billing !== null,
      });
    }),
  );

  async function customerOf(user: User, fresh = false) {
    if (user.stripeCustomer && !fresh) return user.stripeCustomer;
    const id = await billing!.createCustomer(user.email, user.id);
    await store.setCustomer(user.id, id);
    user.stripeCustomer = id;
    return id;
  }

  // Runs a Stripe call for the user's customer, making a new customer once if Stripe
  // doesn't know the saved one.
  async function withCustomer(user: User, call: (customer: string) => Promise<string>) {
    try {
      return await call(await customerOf(user));
    } catch (e) {
      if (!billing!.missingCustomer(e)) throw e;
      return call(await customerOf(user, true));
    }
  }

  app.post(
    "/v1/checkout",
    signedIn,
    wrap(async (req, res) => {
      if (!billing) return fail(res, 503, "unavailable", "Payments aren't set up yet.");
      const u = req.user!;
      const { plan: planId, topup } = req.body ?? {};
      if (typeof planId === "string") {
        const plan = config.plans.find((p) => p.id === planId);
        if (!plan) return fail(res, 400, "invalid_request_error", "No such plan.");
        // Already subscribed: changing plan happens in Stripe's portal.
        if (u.plan) return res.json({ url: await withCustomer(u, (c) => billing.portal(c, config.publicUrl)) });
        return res.json({ url: await withCustomer(u, (c) => billing.checkoutPlan(c, u.id, plan.priceId, config.publicUrl)) });
      }
      if (typeof topup === "number" && config.topups.includes(topup)) {
        return res.json({ url: await withCustomer(u, (c) => billing.checkoutTopup(c, u.id, topup, config.currency, config.publicUrl)) });
      }
      fail(res, 400, "invalid_request_error", "Choose a plan or a top-up.");
    }),
  );

  app.post(
    "/v1/portal",
    signedIn,
    wrap(async (req, res) => {
      if (!billing) return fail(res, 503, "unavailable", "Payments aren't set up yet.");
      res.json({ url: await withCustomer(req.user!, (c) => billing.portal(c, config.publicUrl)) });
    }),
  );

  app.post(
    "/v1/logout",
    signedIn,
    wrap(async (req, res) => {
      await store.deleteSession(hash(req.token!));
      res.json({ ok: true });
    }),
  );

  // --- The relay ---------------------------------------------------------------------

  app.post(
    "/v1/messages",
    signedIn,
    wrap(async (req, res) => {
      const u = req.user!;
      if (balanceMicros(u) <= 0) return fail(res, 402, "insufficient_credit", "You're out of AI credit: subscribe or top up in the assistant's settings.");
      const prepared = prepare(req.body);
      if ("error" in prepared) return fail(res, 400, "invalid_request_error", prepared.error);
      const running = inFlight.get(u.id) ?? 0;
      if (running >= MAX_IN_FLIGHT) return fail(res, 429, "rate_limit_error", "Another request is still running; wait for it to finish.");
      inFlight.set(u.id, running + 1);
      try {
        let message;
        try {
          message = await send(prepared.params);
        } catch (e) {
          const { status, message: text } = relayError(e);
          return fail(res, status, "api_error", text);
        }
        const cost = messageCostMicros(message);
        const charged = chargeMicros(cost, config.margin, config.usdRate);
        const usage = message.usage;
        const after = await store.charge({
          userId: u.id,
          model: message.model,
          inputTokens: usage.input_tokens,
          outputTokens: usage.output_tokens,
          cacheWriteTokens: usage.cache_creation_input_tokens ?? 0,
          cacheReadTokens: usage.cache_read_input_tokens ?? 0,
          costMicros: Math.round(cost),
          chargedMicros: charged,
        });
        res.json({ ...message, pfx_billing: { charged: charged / 1e6, balance: money(balanceMicros(after)), currency: config.currency, symbol: symbol(config.currency) } });
      } finally {
        const n = (inFlight.get(u.id) ?? 1) - 1;
        if (n <= 0) inFlight.delete(u.id);
        else inFlight.set(u.id, n);
      }
    }),
  );

  // --- Crash reports --------------------------------------------------------------------

  // A crash report the user chose to send from the app's recovery prompt (they see the
  // whole text first). No sign-in: kept without an account or IP address, for
  // CRASH_REPORT_DAYS. The IP only counts towards the rate limit, in memory.
  app.post(
    "/v1/crash-reports",
    wrap(async (req, res) => {
      if (!crashes.allow(`crash:${ip(req)}`)) return fail(res, 429, "rate_limited", "Too many reports; try again later.");
      const body = (req.body ?? {}) as Record<string, unknown>;
      const text = (key: string, max: number, required: boolean) => {
        const v = body[key];
        if (v === undefined && !required) return "";
        return typeof v === "string" && v.length <= max && (!required || v.trim().length > 0) ? v : null;
      };
      const appVersion = text("app_version", 40, true);
      const os = text("os", 60, true);
      const report = text("report", 64_000, true);
      const note = text("note", 2_000, false);
      if (appVersion === null || os === null || report === null || note === null) {
        return fail(res, 400, "invalid_request_error", "A report needs app_version, os and report (at most 64,000 characters), and an optional note.");
      }
      await store.addCrashReport({ appVersion, os, report, note });
      res.status(201).json({ ok: true });
    }),
  );

  // --- For you ------------------------------------------------------------------------

  app.get(
    "/admin/crash-reports",
    wrap(async (req, res) => {
      if (!config.adminToken || req.headers.authorization !== `Bearer ${config.adminToken}`) return res.status(404).end();
      const limit = Math.min(Math.max(Number(req.query.limit) || 50, 1), 500);
      const reports = await store.crashReports(limit);
      res.json(reports.map((r) => ({ id: r.id, at: r.at.toISOString(), app_version: r.appVersion, os: r.os, note: r.note, report: r.report })));
    }),
  );

  app.get(
    "/admin/stats",
    wrap(async (req, res) => {
      if (!config.adminToken || req.headers.authorization !== `Bearer ${config.adminToken}`) return res.status(404).end();
      const s = await store.stats();
      // Anthropic's cost is in US dollars; the rest is in the credit currency.
      res.json({
        currency: config.currency,
        users: s.users,
        requests: s.requests,
        anthropic_cost_usd: money(s.costMicros),
        anthropic_cost: money(s.costMicros * config.usdRate),
        charged: money(s.chargedMicros),
        margin: money(s.chargedMicros - s.costMicros * config.usdRate),
        topups_paid: money(s.paidMicros),
        founders: s.founders,
      });
    }),
  );

  app.use((err: unknown, _req: Request, res: Response, _next: NextFunction) => {
    console.error(err);
    if (!res.headersSent) fail(res, 500, "api_error", "Something went wrong on the server.");
  });

  return app;
}
