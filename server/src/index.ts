// Starts the server with the settings in the environment (see .env.example).

import { loadConfig } from "./config.js";
import { createApp } from "./app.js";
import { MemoryStore, type Store } from "./store.js";
import { PgStore } from "./pgstore.js";
import { StripeBilling } from "./billing.js";
import { resendMailer } from "./mail.js";
import { anthropicSender } from "./relay.js";

const config = loadConfig();
if (!config.anthropicKey) throw new Error("ANTHROPIC_API_KEY is not set");

let store: Store;
if (config.databaseUrl) {
  store = await PgStore.connect(config.databaseUrl);
} else {
  console.warn("DATABASE_URL is not set: keeping accounts in memory (they're lost on restart).");
  store = new MemoryStore();
}

const billing = config.stripeKey ? new StripeBilling(config.stripeKey, config.stripeWebhookSecret) : null;
if (!billing) console.warn("STRIPE_SECRET_KEY is not set: payments are off.");
if (billing && !config.stripeWebhookSecret) throw new Error("STRIPE_WEBHOOK_SECRET is not set");
if (!config.resendKey) console.warn("RESEND_API_KEY is not set: sign-in links are printed to this log.");

const app = createApp({ config, store, billing, send: anthropicSender(config.anthropicKey), mail: resendMailer(config.resendKey, config.emailFrom) });
app.listen(config.port, () => console.log(`ProceduralFX AI server on :${config.port} (${config.publicUrl})`));
