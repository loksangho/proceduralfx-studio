// Sign-in emails, sent through Resend's HTTP API; without a key the link goes to the log
// (for local testing).

export type Mailer = (to: string, subject: string, text: string, html: string) => Promise<void>;

export function resendMailer(apiKey: string, from: string): Mailer {
  if (!apiKey) {
    return async (to, subject, text) => console.log(`[mail to ${to}] ${subject}\n${text}`);
  }
  return async (to, subject, text, html) => {
    const r = await fetch("https://api.resend.com/emails", {
      method: "POST",
      headers: { authorization: `Bearer ${apiKey}`, "content-type": "application/json" },
      body: JSON.stringify({ from, to: [to], subject, text, html }),
    });
    if (!r.ok) throw new Error(`Resend answered ${r.status}: ${await r.text()}`);
  };
}
