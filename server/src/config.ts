// Settings from the environment (see .env.example).

export interface Plan {
  id: string;
  priceId: string;
  creditMicros: number;
}

export interface Config {
  publicUrl: string;
  port: number;
  anthropicKey: string;
  databaseUrl: string;
  stripeKey: string;
  stripeWebhookSecret: string;
  plans: Plan[];
  topups: number[];
  margin: number;
  // Credit, plans and top-ups are in this currency (lower case, e.g. "aud").
  currency: string;
  // Units of that currency per US dollar, for converting Anthropic's (USD) prices.
  usdRate: number;
  freeCreditMicros: number;
  resendKey: string;
  emailFrom: string;
  adminToken: string;
  // The public website, for links to the terms and privacy policy.
  siteUrl: string;
}

function num(value: string | undefined, fallback: number): number {
  const n = Number(value);
  return value !== undefined && value.trim() !== "" && Number.isFinite(n) ? n : fallback;
}

export function parsePlans(text: string): Plan[] {
  return text
    .split(",")
    .map((p) => p.trim())
    .filter(Boolean)
    .map((p) => {
      const [id, priceId, credit] = p.split("=").map((s) => s.trim());
      const dollars = Number(credit);
      if (!id || !priceId || !Number.isFinite(dollars) || dollars <= 0) throw new Error(`PLANS: can't read "${p}" (want id=price_id=dollars)`);
      return { id, priceId, creditMicros: Math.round(dollars * 1e6) };
    });
}

export function loadConfig(env: NodeJS.ProcessEnv = process.env): Config {
  const config: Config = {
    publicUrl: (env.PUBLIC_URL ?? "http://localhost:8787").replace(/\/+$/, ""),
    port: num(env.PORT, 8787),
    anthropicKey: env.ANTHROPIC_API_KEY ?? "",
    databaseUrl: env.DATABASE_URL ?? "",
    stripeKey: env.STRIPE_SECRET_KEY ?? "",
    stripeWebhookSecret: env.STRIPE_WEBHOOK_SECRET ?? "",
    plans: parsePlans(env.PLANS ?? ""),
    topups: (env.TOPUPS ?? "5,10,25")
      .split(",")
      .map((p) => Math.round(Number(p.trim())))
      .filter((p) => Number.isFinite(p) && p >= 1 && p <= 500),
    margin: num(env.MARGIN, 0.25),
    currency: (env.CURRENCY ?? "usd").trim().toLowerCase(),
    usdRate: num(env.USD_RATE, 1),
    freeCreditMicros: Math.round(num(env.FREE_CREDIT, 0) * 1e6),
    resendKey: env.RESEND_API_KEY ?? "",
    emailFrom: env.EMAIL_FROM ?? "ProceduralFX Studio <login@example.com>",
    adminToken: env.ADMIN_TOKEN ?? "",
    siteUrl: (env.SITE_URL ?? "https://www.build3dstudio.com").replace(/\/+$/, ""),
  };
  if (config.margin < 0 || config.margin > 5) throw new Error("MARGIN must be between 0 and 5");
  if (!/^[a-z]{3}$/.test(config.currency)) throw new Error("CURRENCY must be a three-letter code such as aud or usd");
  if (config.usdRate <= 0) throw new Error("USD_RATE must be above 0");
  if (config.currency === "usd" && config.usdRate !== 1) throw new Error("USD_RATE must be 1 when CURRENCY is usd");
  return config;
}
