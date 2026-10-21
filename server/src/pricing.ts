// What a request costs: Anthropic's list price for the tokens it used, plus the margin.
// Money is kept in micro-dollars (1e-6 USD) as integers so nothing rounds away.

import type Anthropic from "@anthropic-ai/sdk";

// Dollars per million tokens: input, output, cache write (5 min), cache read.
// Check these against https://www.anthropic.com/pricing when models or prices change.
export const PRICES: Record<string, [number, number, number, number]> = {
  "claude-opus-5-5": [4, 20, 5, 0.2],
  "claude-sonnet-5-5": [2, 10, 2.5, 0.2],
  "claude-haiku-5-5": [0.1, 0.5, 0.125, 0.01],
};

// Models the app may ask for.
export const ALLOWED_MODELS = Object.keys(PRICES);

// A model with no price here (e.g. a server-side fallback model) is charged at this
// deliberately high rate, so an unknown model never loses money.
const UNKNOWN: [number, number, number, number] = [5, 25, 6.25, 0.5];

interface Tokens {
  input_tokens?: number | null;
  output_tokens?: number | null;
  cache_creation_input_tokens?: number | null;
  cache_read_input_tokens?: number | null;
}

// One attempt's price in micro-dollars (dollars per million tokens = micro-dollars per token).
export function tokensMicros(model: string, t: Tokens): number {
  const p = PRICES[model] ?? UNKNOWN;
  return (
    (t.input_tokens ?? 0) * p[0] +
    (t.output_tokens ?? 0) * p[1] +
    (t.cache_creation_input_tokens ?? 0) * p[2] +
    (t.cache_read_input_tokens ?? 0) * p[3]
  );
}

// Anthropic's cost of a whole response. With fallbacks, usage.iterations lists every
// attempt (each at its own model's price); otherwise the top-level usage is the request.
export function messageCostMicros(message: Pick<Anthropic.Beta.BetaMessage, "model" | "usage">): number {
  const iterations = message.usage.iterations ?? [];
  if (iterations.length === 0) return tokensMicros(message.model, message.usage);
  return iterations.reduce((sum, it) => sum + tokensMicros(("model" in it && it.model) || message.model, it as Tokens), 0);
}

// What the user pays, in the credit currency: the US-dollar cost converted at `usdRate`,
// plus margin, rounded up to the micro-unit.
export function chargeMicros(costMicros: number, margin: number, usdRate = 1): number {
  return Math.ceil(costMicros * usdRate * (1 + margin));
}

// Micro-units to money, rounded to the cent.
export function money(micros: number): number {
  return Math.round(micros / 1e4) / 100;
}

export function symbol(currency: string): string {
  return ({ aud: "A$", usd: "$", nzd: "NZ$", cad: "CA$", eur: "€", gbp: "£" } as Record<string, string>)[currency] ?? `${currency.toUpperCase()} `;
}
