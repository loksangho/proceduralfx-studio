// The few web pages people see: signing in (from the app), and after paying.

export function escape(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
}

export function page(title: string, body: string): string {
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>${escape(title)} · ProceduralFX Studio</title>
<style>
  :root { color-scheme: light dark; --bg: #f6f6f7; --card: #fff; --text: #1d1d20; --muted: #6b6b73; --accent: #d9822b; --border: #e2e2e6; }
  @media (prefers-color-scheme: dark) { :root { --bg: #18181b; --card: #222226; --text: #ececf0; --muted: #a0a0aa; --border: #34343a; } }
  body { margin: 0; background: var(--bg); color: var(--text); font: 16px/1.5 system-ui, -apple-system, "Segoe UI", sans-serif; }
  main { max-width: 440px; margin: 10vh auto; padding: 0 16px; }
  .card { background: var(--card); border: 1px solid var(--border); border-radius: 12px; padding: 28px; }
  h1 { font-size: 22px; margin: 0 0 12px; }
  p { margin: 0 0 14px; } .muted { color: var(--muted); font-size: 14px; }
  label { display: block; font-size: 14px; color: var(--muted); margin: 12px 0 4px; }
  input { box-sizing: border-box; width: 100%; padding: 10px 12px; font: inherit; border: 1px solid var(--border); border-radius: 8px; background: var(--bg); color: var(--text); }
  input.code { letter-spacing: 3px; text-transform: uppercase; font-family: ui-monospace, monospace; }
  button { margin-top: 18px; width: 100%; padding: 11px; font: inherit; font-weight: 600; border: 0; border-radius: 8px; background: var(--accent); color: #fff; cursor: pointer; }
  .code-box { font: 600 24px ui-monospace, monospace; letter-spacing: 4px; text-align: center; padding: 10px; border: 1px dashed var(--border); border-radius: 8px; margin-bottom: 14px; }
</style></head><body><main><div class="card"><h1>${escape(title)}</h1>${body}</div></main></body></html>`;
}

export function signInPage(code: string, error = "", site = ""): string {
  return page(
    "Sign in to ProceduralFX Studio",
    `${error ? `<p style="color:#d24">${escape(error)}</p>` : ""}
     <p>Enter the code shown in the app and your email. We'll email you a link to finish signing in.</p>
     <form method="post" action="/device">
       <label for="code">Code from the app</label>
       <input id="code" class="code" name="code" value="${escape(code)}" autocomplete="off" required>
       <label for="email">Email</label>
       <input id="email" type="email" name="email" autocomplete="email" required>
       <button>Email me a sign-in link</button>
     </form>
     ${site ? `<p class="muted" style="margin-top:16px">By signing in you agree to the <a href="${escape(site)}/terms">Terms of Service</a> and <a href="${escape(site)}/privacy">Privacy Policy</a>.</p>` : ""}`,
  );
}

export function checkEmailPage(email: string): string {
  return page("Check your email", `<p>We sent a sign-in link to <b>${escape(email)}</b>. It works for 15 minutes.</p><p class="muted">Didn't get it? Check spam, or go back and try again.</p>`);
}

export function confirmPage(token: string, email: string, code: string): string {
  return page(
    "Finish signing in",
    `<p>Sign in to ProceduralFX Studio as <b>${escape(email)}</b> on the computer showing this code:</p>
     <div class="code-box">${escape(code)}</div>
     <p class="muted">Only continue if you started this in the app yourself just now.</p>
     <form method="post" action="/login"><input type="hidden" name="t" value="${escape(token)}"><button>Sign in</button></form>`,
  );
}

export function messagePage(title: string, text: string): string {
  return page(title, `<p>${escape(text)}</p>`);
}
