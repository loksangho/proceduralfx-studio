// The metered relay: the app sends a Messages API request (its own prompt, tools and
// conversation); the server checks it, calls Claude with the owner's key, and returns the
// reply with what it cost the user.

import Anthropic from "@anthropic-ai/sdk";
import { ALLOWED_MODELS } from "./pricing.js";

export type Params = Anthropic.Beta.MessageCreateParamsNonStreaming;
export type Send = (params: Params) => Promise<Anthropic.Beta.BetaMessage>;

const MAX_TOKENS = 16000;
const MAX_MESSAGES = 200;
// Refusals retry on Anthropic's recommended fallback model.
const FALLBACK_BETA = "server-side-fallback-2026-07-01";

// Fields the app may set. Anything else (e.g. server tools that cost extra) is refused.
const ALLOWED = new Set(["model", "max_tokens", "system", "tools", "tool_choice", "messages", "output_config", "cache_control", "thinking", "fallbacks"]);

export function prepare(body: unknown): { params: Params } | { error: string } {
  if (!body || typeof body !== "object" || Array.isArray(body)) return { error: "the request must be a JSON object" };
  const b = body as Record<string, unknown>;
  const extra = Object.keys(b).filter((k) => !ALLOWED.has(k));
  if (extra.length) return { error: `unsupported fields: ${extra.join(", ")}` };
  if (typeof b.model !== "string" || !ALLOWED_MODELS.includes(b.model)) return { error: `model must be one of ${ALLOWED_MODELS.join(", ")}` };
  if (!Array.isArray(b.messages) || b.messages.length === 0 || b.messages.length > MAX_MESSAGES) return { error: `messages must be a list of 1 to ${MAX_MESSAGES}` };
  if (Array.isArray(b.tools) && b.tools.some((t) => !t || typeof t !== "object" || "type" in t)) return { error: "only custom tools are allowed" };
  const maxTokens = Math.min(Number(b.max_tokens) || MAX_TOKENS, MAX_TOKENS);
  const params = { ...b, max_tokens: maxTokens } as Record<string, unknown>;
  // Haiku has no server-side fallback; the others use the default one.
  delete params.fallbacks;
  if (!b.model.startsWith("claude-haiku")) {
    params.fallbacks = "default";
    params.betas = [FALLBACK_BETA];
  }
  return { params: params as unknown as Params };
}

export function anthropicSender(apiKey: string): Send {
  const client = new Anthropic({ apiKey, maxRetries: 2 });
  return (params) => client.beta.messages.create(params);
}

// How an Anthropic error goes back to the app: overloads and rate limits keep their status
// (the app retries them); a bad key or account problem is the server's, not the user's.
export function relayError(error: unknown): { status: number; message: string } {
  if (error instanceof Anthropic.APIError) {
    const status = error.status ?? 502;
    if (status === 401 || status === 403) return { status: 502, message: "The AI service is misconfigured; please try again later." };
    if (status === 400 || status === 413 || status === 429 || status >= 500) return { status, message: error.message };
    return { status: 502, message: error.message };
  }
  return { status: 502, message: "Couldn't reach the AI service; please try again." };
}
