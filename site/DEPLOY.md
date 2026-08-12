# Deploying openroutine.dev

## Current state

**The site is live at <https://openroutine.dev>**, deployed as the Worker `openroutine-site`
(Workers Static Assets). The apex domain and its certificate were attached automatically by the
`custom_domain` route in `wrangler.toml`.

Deploys are **manual** right now:

```bash
cd site && npx wrangler deploy
```

Two things are still unconfigured, both dashboard-only. Neither is required for the site to
work; the first removes the manual step, the second makes `www` resolve.

- [ ] Workers Builds — auto-deploy on push to `main`
- [ ] `www.openroutine.dev` → 301 to the apex

> Wrangler's OAuth token carries `zone (read)` but no DNS or Ruleset write scope, so the `www`
> steps cannot be scripted with it. They need the dashboard, or an API token created with
> DNS-edit and Rules-edit permissions.

---

## A. Auto-deploy on push (optional)

Removes the manual `wrangler deploy`. Dashboard only — there is no CLI path for connecting a
git repo.

1. **Workers & Pages → `openroutine-site` → Settings → Builds → Connect.**
2. Authorize the Cloudflare GitHub App for **`soulmachine/openroutine`**, and pick the repo.
3. Build settings:

   | Setting | Value |
   | :-- | :-- |
   | Root directory | `site/` |
   | Build command | **leave empty** |
   | Deploy command | `npx wrangler deploy` |

   There is no build step — `public/` is already the finished site. The root directory is what
   makes `wrangler.toml` findable; without it the deploy runs at the repo root and fails.

4. **Branch control → production branch: `main`.**
5. **Settings → Build → Build watch paths → include `site/*`.**

Step 5 is what makes a shared repo work. Include paths default to `[*]`, so without it every
Rust commit starts a build; with it, a commit that touches no path under `site/` is skipped
before a build is ever queued.

Leave *builds for non-production branches* **on**. A PR touching `site/` then gets a preview
via `npx wrangler versions upload` without promoting it, and the watch path keeps code-only
branches quiet — the reason that toggle used to be off is gone.

The Worker name in the dashboard must stay `openroutine-site` — it has to match `name` in
`wrangler.toml` or the build fails.

Afterwards, confirm it rather than assuming, and confirm both halves: push a trivial edit under
`site/` and watch it go live, then push a code-only commit and watch no build start.

## B. Redirect www to the apex

This cannot live in `public/_redirects` — Cloudflare's redirects file does not support
domain-level redirects. It is a zone rule instead.

1. **DNS → Add record**: type `AAAA`, name `www`, address `100::`, **Proxied (orange cloud)**.
   `100::` is the discard prefix; the record exists only so the hostname reaches Cloudflare's
   edge, where the rule below can fire.
2. **Rules → Redirect Rules → Create rule.** Use the built-in *Redirect from WWW to Root*
   template, or set it manually:
   - **When:** `http.host eq "www.openroutine.dev"`
   - **Then:** dynamic redirect to `concat("https://openroutine.dev", http.request.uri.path)`
   - **Status:** 301, preserve query string

The free plan allows 10 single redirects; this uses one.

Do **not** attach `www` as a second Custom Domain on the Worker instead — that would serve the
site on both hostnames rather than redirecting. The pages already carry
`<link rel="canonical">` pointing at the apex, so it would not be an SEO disaster, but it is not
what was asked for.

---

## Verifying

```bash
dig +short A openroutine.dev               # Cloudflare addresses
curl -s -o /dev/null -w '%{http_code}\n' https://openroutine.dev          # 200
curl -s -o /dev/null -w '%{http_code}\n' https://openroutine.dev/docs     # 200
curl -s -o /dev/null -w '%{http_code}\n' https://openroutine.dev/docs.html # 307 -> /docs
curl -s -o /dev/null -w '%{http_code}\n' https://openroutine.dev/nope     # 404
curl -s -o /dev/null -w '%{http_code}\n' https://www.openroutine.dev      # 301, once B is done
```

Use `-o /dev/null -w '%{http_code}'` (a GET) rather than `curl -I` (a HEAD). Cloudflare's edge
answers HEAD inconsistently on freshly deployed assets, which reads as a broken site when it is
not.

Expect a minute or two of intermittent 500s immediately after a deploy while the new version
propagates across the edge. Sample a dozen requests before concluding anything is wrong.

## Why Workers and not Pages

Cloudflare's current guidance: *"If you are starting a new project, use Workers instead of
Pages. Pages continues to work, but new features and optimizations are focused on Workers."*

`wrangler.toml` deliberately has no `main`. With no Worker script, every request is a free
static-asset request, so the free plan's 100k/day Worker-invocation cap never applies and
bandwidth is unmetered. Adding `main` or `run_worker_first` later would change that.
