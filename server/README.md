# ProceduralFX credits server

This server lets people use the AI assistant in ProceduralFX Studio without an API key. They sign in with their email and pay with a monthly plan or a top-up. The server calls Claude with your Anthropic key and charges each request at Anthropic's list price plus your margin (`MARGIN`, 25% by default).

The app keeps the other two options as well: a user's own Claude key, or any OpenAI-compatible or local model. This server only handles the credits option.

## How it works

- **Signing in:** the app asks for a code and opens `/device` in the browser. The user enters their email and clicks the emailed link. The confirm page shows the code. The app then receives a session token, and the server stores only its SHA-256 hash.
- **Credit:** each user has two balances.
  - Plan credit is set to the plan's amount each time the monthly invoice is paid. Unused plan credit doesn't carry over.
  - Top-up credit never expires.
  - Requests spend plan credit first. A request is refused when the total is zero or less. A request that is already running can take the balance slightly below zero.
- **The relay:** `POST /v1/messages` takes the app's Messages API request.
  - It accepts the three Claude models only, caps `max_tokens` at 16,000, refuses server tools and unknown fields, and allows two requests in flight per user.
  - It calls Claude with server-side refusal fallbacks (`fallbacks: "default"`) on Opus and Sonnet. Each fallback attempt is priced at its own model's rate, and a model with no known price is charged at a deliberately high rate.
  - It charges the user and returns the reply with `pfx_billing` (the amount charged and the new balance).
  - Prompts and replies pass through without being stored. Only token counts and costs are logged.
- **Payments:** payments go through Stripe Checkout, plans are changed or cancelled in Stripe's customer portal, and webhooks add the credit. Each payment is credited once.
- **Your numbers:** `GET /admin/stats` with `Authorization: Bearer $ADMIN_TOKEN` returns users, requests, Anthropic cost, the amount charged, your margin, and top-ups paid.

## Run it locally

```sh
cd server
npm install
cp .env.example .env    # fill in ANTHROPIC_API_KEY at least
npm run dev             # http://localhost:8787
npm test                # PFX_TEST_DATABASE_URL=postgres://… npm test also runs the tests on Postgres
```

Without `DATABASE_URL`, accounts are kept in memory. Without `RESEND_API_KEY`, sign-in links are printed to the log. Without `STRIPE_SECRET_KEY`, payments are off.

To point the app at it, run the app with `PFX_AI_SERVER=http://localhost:8787`. Building the app with that variable set makes it the built-in server.

## Setting it up for real

Do everything in Stripe's **test mode** first and switch to live keys at the end.

1. **Anthropic.** In console.anthropic.com, create an API key just for this server and set a monthly spend limit. Check your rate-limit tier, because all users share your organisation's limits. Make sure this use fits Anthropic's Commercial Terms and Usage Policy.
2. **Postgres.** Create a database, for example on Neon or Supabase (both have free tiers), or with your host. Copy its connection string into `DATABASE_URL`. The tables are created when the server starts.
3. **Email.** Create a Resend account, add and verify your domain (it gives you DNS records to add), and create an API key. Set `RESEND_API_KEY` and `EMAIL_FROM`, for example `ProceduralFX Studio <login@yourdomain.com>`.
4. **Hosting.** Deploy this folder with its `Dockerfile`, for example on Render (a Docker web service with root directory `server`) or Fly.io (`fly launch` in this folder).
   - Give it a domain such as `ai.yourdomain.com` and set `PUBLIC_URL` to it.
   - Set every variable from `.env.example` as a secret, including a long random `ADMIN_TOKEN`.
   - Check that `https://ai.yourdomain.com/healthz` answers `{"ok":true}`.
5. **Stripe.**
   1. Create the account and complete the business and bank details.
   2. Under **Products**, create a product for each plan (for example "Basic", $8 a month, recurring). Copy each **price ID** (`price_…`) into `PLANS`, for example `PLANS=basic=price_123=8,pro=price_456=20`. Each entry is plan id = price ID = dollars of credit per month. Top-ups need no products: set the amounts in `TOPUPS`.
   3. Under **Developers → Webhooks**, add the endpoint `https://ai.yourdomain.com/stripe/webhook` with these events: `checkout.session.completed`, `checkout.session.async_payment_succeeded`, `invoice.paid`, `customer.subscription.updated` and `customer.subscription.deleted`. Put its signing secret (`whsec_…`) in `STRIPE_WEBHOOK_SECRET` and the secret key in `STRIPE_SECRET_KEY`.
   4. Under **Settings → Billing → Customer portal**, allow cancelling and switching between your plans, and save.
   5. Test it: sign in from the app, buy a top-up with card `4242 4242 4242 4242`, subscribe, and cancel. Check `/admin/stats` along the way. Then switch to live keys and a live webhook. Live mode has its own price IDs, so update `PLANS` too.
6. **Tax and legal.** You are the seller, so you may need to collect GST, VAT or sales tax. Stripe Tax can calculate it at Checkout. You also need terms of service, a privacy policy (emails, usage counts, prompts sent to Anthropic) and a refund policy. Link them from your site.
7. **The app.** Build the release with `PFX_AI_SERVER=https://ai.yourdomain.com` set, so the credits option appears and becomes the default for new users.

## Margins, roughly

A $5 top-up costs about $0.45 in Stripe fees. With `MARGIN=0.25`, $5 of credit costs you $4 in tokens, so you keep about $0.55 once it's all spent.

An $8 plan that gives $8 of credit earns about $1.07 a month when fully used. Unused plan credit is extra, because it doesn't carry over.

Raise `MARGIN`, or give plans less credit than their price, to earn more per user. Credit is shown to users in dollars of usage, and the margin is already included in what each request costs them.

Prices per million tokens are in `src/pricing.ts`. Update them when Anthropic's prices change.
