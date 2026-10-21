// Payments through Stripe: Checkout for monthly plans and one-off top-ups, the customer
// portal for changing or cancelling a plan, and webhooks that turn payments into credit.

import Stripe from "stripe";

// What a verified webhook means for the app.
export type BillingEvent =
  // A paid top-up: credit it once (id is the Checkout session).
  | { kind: "topup"; id: string; userId: string; micros: number }
  // A plan's first or monthly invoice was paid: refill plan credit (id is the invoice).
  | { kind: "renewed"; id: string; customerId: string }
  // A subscription changed plan or ended (id is the event).
  | { kind: "changed"; id: string; customerId: string; priceId: string | null; active: boolean }
  | { kind: "ignored" };

export interface Billing {
  createCustomer(email: string, userId: string): Promise<string>;
  checkoutPlan(customerId: string, userId: string, priceId: string, returnUrl: string): Promise<string>;
  checkoutTopup(customerId: string, userId: string, dollars: number, returnUrl: string): Promise<string>;
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

function customerId(c: string | { id: string } | null): string {
  return typeof c === "string" ? c : (c?.id ?? "");
}

export function interpret(event: Stripe.Event): BillingEvent {
  switch (event.type) {
    case "checkout.session.completed":
    case "checkout.session.async_payment_succeeded": {
      const s = event.data.object;
      if (s.mode !== "payment" || s.payment_status !== "paid") return { kind: "ignored" };
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
      success_url: `${returnUrl}/paid`,
      cancel_url: `${returnUrl}/cancelled`,
    });
    return s.url!;
  }

  async checkoutTopup(customer: string, userId: string, dollars: number, returnUrl: string) {
    const s = await this.stripe.checkout.sessions.create({
      mode: "payment",
      customer,
      client_reference_id: userId,
      line_items: [{ quantity: 1, price_data: { currency: "usd", unit_amount: dollars * 100, product_data: { name: `$${dollars} of ProceduralFX AI credit` } } }],
      metadata: { user_id: userId, credit_micros: String(dollars * 1_000_000) },
      success_url: `${returnUrl}/paid`,
      cancel_url: `${returnUrl}/cancelled`,
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
