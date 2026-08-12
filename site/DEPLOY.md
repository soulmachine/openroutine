# Deploying openroutine.dev

## Current state

**The site is live at <https://openroutine.dev>**, deployed as the Worker `openroutine-site`
(Workers Static Assets). The apex domain and its certificate were attached automatically by the
`custom_domain` route in `wrangler.toml`.

Deploys are **automatic**: Workers Builds is connected to `soulmachine/openroutine`, and a push
to `main` that touches `site/` deploys the site. Nothing is needed by hand. The manual path
still works if you want it — `cd site && npx wrangler deploy`.

- [x] Workers Builds — auto-deploy on push to `main`
- [ ] `www.openroutine.dev` → 301 to the apex

> Wrangler's OAuth token carries `zone (read)` but no DNS or Ruleset write scope, so the `www`
> steps cannot be scripted with it. They need the dashboard, or an API token created with
> DNS-edit and Rules-edit permissions.

---

## A. Auto-deploy on push — done

Connected under **Workers & Pages → `openroutine-site` → Settings → Build**. Dashboard only;
there is no CLI or API path for attaching a git repo, because it runs through the GitHub App
install flow. This is the configuration of record:

| Setting | Value |
| :-- | :-- |
| Git repository | `soulmachine/openroutine` |
| Root directory | `site` |
| Build command | *(none)* |
| Deploy command | `npx wrangler deploy` |
| Version command | `npx wrangler versions upload` |
| Production branch | `main` |
| Builds for non-production branches | **on** |
| Build watch paths → include | `site/*` |

There is no build step — `public/` is already the finished site. The root directory is what
makes `wrangler.toml` findable; without it the deploy runs at the repo root and fails.

The watch path is what makes a shared repo work. Include paths default to `*`, so without it
every Rust commit starts a build; with `site/*`, a commit touching no path under `site/` is
skipped before a build is queued. That is also why *builds for non-production branches* can stay
on: a PR touching `site/` gets a preview via `npx wrangler versions upload` without promoting
it, and code-only branches stay quiet.

Cloudflare mints its own API token for this (`Workers Builds - <timestamp>`, visible under
Settings → Build → API token). It is not the wrangler OAuth session and not the token that was
revoked earlier; leave it alone.

The Worker name in the dashboard must stay `openroutine-site` — it has to match `name` in
`wrangler.toml` or the build fails.

### What has actually been observed

Commit `ada2d64` touched only `site/`. The build was queued within about fifteen seconds of the
push, succeeded, and promoted version `9081fb96` to 100% of traffic — listed under **Deployments**
against the commit and its author, rather than as the `Manually deployed … Wrangler` entries the
earlier hand-deploys produced. Every route answered afterwards: `/` 200, `/docs` 200,
`/docs.html` 307, `/nope` 404, `/styles.css` 200, `/og.png` 200.

The other half is unconfirmed: no code-only commit has been pushed since the watch path was set,
so nothing has yet demonstrated that `site/*` suppresses a build. Watch the next Rust-only push —
**Deployments** should stay unchanged. If a build starts anyway, the include pattern is matching
more than intended; narrowing it is a one-field edit under Settings → Build.

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
