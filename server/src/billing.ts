// Payments through Stripe: Checkout for monthly plans and one-off top-ups, the customer
// portal for changing or cancelling a plan, and webhooks that turn payments into credit.

import Stripe from "stripe";
import type { TaxMode } from "./config.js";
import { symbol } from "./pricing.js";

// What a verified webhook means for the app.
export type BillingEvent =
  // A paid top-up: credit it once (id is the Checkout session).
  | { kind: "topup"; id: string; userId: string; micros: number }
  // A paid Founder's license (id is the Checkout session). licensee is the name the buyer
  // gave for the license, or empty.
  | { kind: "founder"; id: string; email: string; licensee: string; paidMicros: number }
  // A plan's first or monthly invoice was paid: add a month of plan credit (id is the invoice).
  | { kind: "renewed"; id: string; customerId: string }
  // A subscription changed plan or ended (id is the event).
  | { kind: "changed"; id: string; customerId: string; priceId: string | null; active: boolean }
  | { kind: "ignored" };

export interface Billing {
  createCustomer(email: string, userId: string): Promise<string>;
  checkoutPlan(customerId: string, userId: string, priceId: string, returnUrl: string): Promise<string>;
  checkoutTopup(customerId: string, userId: string, amount: number, currency: string, returnUrl: string): Promise<string>;
  // Checkout for a Founder's license; Stripe asks for the buyer's email.
  // `siteUrl` is where the license and terms are, which the buyer agrees to before paying.
  checkoutFounder(amount: number, currency: string, returnUrl: string, cancelUrl: string, siteUrl: string): Promise<string>;
  portal(customerId: string, returnUrl: string): Promise<string>;
  // The price of the customer's active subscription, if any.
  activePrice(customerId: string): Promise<string | null>;
  // e.g. "$8/month"; null if Stripe doesn't know the price.
  priceLabel(priceId: string): Promise<string | null>;
  // Verifies the signature (throws if it's wrong) and reads the event.
  parseEvent(raw: Buffer, signature: string): BillingEvent;
  // Whether an error means Stripe doesn't know the customer (e.g. one made in test mode
  // before switching to live keys).
  missingCustomer(error: unknown): boolean;
}

// What a Checkout session needs for Stripe Tax: tax worked out from the buyer's address,
// businesses able to give a tax ID (EU and UK VAT is then theirs to account for), and,
// for a saved customer, the address and name kept on it. Nothing when tax is off.
export function taxParams(tax: TaxMode, savedCustomer: boolean): Partial<Stripe.Checkout.SessionCreateParams> {
  if (tax === "off") return {};
  return {
    automatic_tax: { enabled: true },
    tax_id_collection: { enabled: true },
    ...(savedCustomer ? { customer_update: { address: "auto", name: "auto" } } : {}),
  };
}

// Whether a price built at checkout includes tax or has it added.
export function taxBehavior(tax: TaxMode): { tax_behavior?: "inclusive" | "exclusive" } {
  return tax === "off" ? {} : { tax_behavior: tax };
}

function customerId(c: string | { id: string } | null): string {
  return typeof c === "string" ? c : (c?.id ?? "");
}

export function interpret(event: Stripe.Event): BillingEvent {
  switch (event.type) {
    case "checkout.session.completed":
    case "checkout.session.async_payment_succeeded": {
      const s = event.data.object;
      if (s.mode !== "payment" || s.payment_status !== "paid") return { kind: "ignored" };
      if (s.metadata?.kind === "founder") {
        const email = s.customer_details?.email ?? s.customer_email ?? "";
        if (!email) return { kind: "ignored" };
        const licensee = s.custom_fields?.find((f) => f.key === "licensee")?.text?.value ?? "";
        return { kind: "founder", id: s.id, email, licensee: licensee.trim(), paidMicros: (s.amount_total ?? 0) * 10_000 };
      }
      const userId = s.metadata?.user_id;
      const micros = Number(s.metadata?.credit_micros);
      if (!userId || !Number.isFinite(micros) || micros <= 0) return { kind: "ignored" };
      return { kind: "topup", id: s.id, userId, micros };
    }
    case "invoice.paid": {
      const inv = event.data.object;
      if (inv.billing_reason !== "subscription_create" && inv.billing_reason !== "subscription_cycle") return { kind: "ignored" };
      if (!inv.id) return { kind: "ignored" };
      return { kind: "renewed", id: inv.id, customerId: customerId(inv.customer) };
    }
    case "customer.subscription.updated":
    case "customer.subscription.deleted": {
      const sub = event.data.object;
      const active = event.type === "customer.subscription.updated" && (sub.status === "active" || sub.status === "trialing" || sub.status === "past_due");
      return { kind: "changed", id: event.id, customerId: customerId(sub.customer), priceId: sub.items.data[0]?.price.id ?? null, active };
    }
    default:
      return { kind: "ignored" };
  }
}

export class StripeBilling implements Billing {
  private stripe: Stripe;
  private labels = new Map<string, string | null>();

  constructor(
    secretKey: string,
    private webhookSecret: string,
    private tax: TaxMode = "off",
  ) {
    this.stripe = new Stripe(secretKey);
  }

  async createCustomer(email: string, userId: string) {
    const c = await this.stripe.customers.create({ email, metadata: { user_id: userId } });
    return c.id;
  }

  async checkoutPlan(customer: string, userId: string, priceId: string, returnUrl: string) {
    const s = await this.stripe.checkout.sessions.create({
      mode: "subscription",
      customer,
      client_reference_id: userId,
      line_items: [{ price: priceId, quantity: 1 }],
      ...taxParams(this.tax, true),
      success_url: `${returnUrl}/paid`,
      cancel_url: `${returnUrl}/cancelled`,
    });
    return s.url!;
  }

  async checkoutTopup(customer: string, userId: string, amount: number, currency: string, returnUrl: string) {
    const s = await this.stripe.checkout.sessions.create({
      mode: "payment",
      customer,
      client_reference_id: userId,
      line_items: [{ quantity: 1, price_data: { currency, unit_amount: amount * 100, ...taxBehavior(this.tax), product_data: { name: `${symbol(currency)}${amount} of ProceduralFX AI credit` } } }],
      ...taxParams(this.tax, true),
      metadata: { user_id: userId, credit_micros: String(amount * 1_000_000) },
      success_url: `${returnUrl}/paid`,
      cancel_url: `${returnUrl}/cancelled`,
    });
    return s.url!;
  }

  async checkoutFounder(amount: number, currency: string, returnUrl: string, cancelUrl: string, siteUrl: string) {
    const s = await this.stripe.checkout.sessions.create({
      mode: "payment",
      line_items: [
        {
          quantity: 1,
          price_data: {
            currency,
            unit_amount: amount * 100,
            ...taxBehavior(this.tax),
            product_data: { name: "ProceduralFX Studio Founder's license", description: "Commercial use for one person, every version, forever." },
          },
        },
      ],
      custom_fields: [{ key: "licensee", label: { type: "custom", custom: "Name or studio on the license" }, type: "text", optional: true, text: { maximum_length: 80 } }],
      // Payment waits for a tick confirming the license and terms (Stripe needs a terms of
      // service URL in its public details for this).
      consent_collection: { terms_of_service: "required" },
      custom_text: {
        terms_of_service_acceptance: { message: `I agree to the [Founder's license](${siteUrl}/founder) and the [terms](${siteUrl}/terms).` },
      },
      metadata: { kind: "founder" },
      ...taxParams(this.tax, false),
      // Seats are counted when this opens; an unfinished checkout gives its place back soon.
      expires_at: Math.floor(Date.now() / 1000) + 30 * 60,
      success_url: `${returnUrl}/founder/thanks`,
      cancel_url: cancelUrl,
    });
    return s.url!;
  }

  async portal(customer: string, returnUrl: string) {
    const s = await this.stripe.billingPortal.sessions.create({ customer, return_url: `${returnUrl}/` });
    return s.url;
  }

  async activePrice(customer: string) {
    const subs = await this.stripe.subscriptions.list({ customer, status: "active", limit: 1 });
    return subs.data[0]?.items.data[0]?.price.id ?? null;
  }

  async priceLabel(priceId: string) {
    if (!this.labels.has(priceId)) {
      try {
        const p = await this.stripe.prices.retrieve(priceId);
        const amount = (p.unit_amount ?? 0) / 100;
        const money = new Intl.NumberFormat("en-US", { style: "currency", currency: p.currency.toUpperCase() }).format(amount);
        this.labels.set(priceId, p.recurring ? `${money}/${p.recurring.interval}` : money);
      } catch {
        this.labels.set(priceId, null);
      }
    }
    return this.labels.get(priceId) ?? null;
  }

  parseEvent(raw: Buffer, signature: string) {
    return interpret(this.stripe.webhooks.constructEvent(raw, signature, this.webhookSecret));
  }

  missingCustomer(error: unknown) {
    const e = error as { code?: string; param?: string; message?: string } | null;
    return e?.code === "resource_missing" && (e.param === "customer" || /customer/i.test(e.message ?? ""));
  }
}
