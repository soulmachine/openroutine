# Putting openroutine.dev live

One-time setup. After this, pushing to the `site` branch is the deploy.

The zone is already on Cloudflare nameservers (`bob/kim.ns.cloudflare.com`) with **no A or
CNAME records** — that is the right starting state. Do not hand-create records for
`openroutine.dev` or `www` before step 3; a Custom Domain cannot be attached over an existing
CNAME.

## 1. Connect the repo to Workers Builds

Dashboard only — there is no CLI path for connecting a git repo.

1. **Workers & Pages → Create → Import a repository.**
2. Authorize the Cloudflare GitHub App for **`soulmachine/openroutine`**.
3. Pick the repo, and set the branch to **`site`**.
4. Name the Worker **`openroutine-site`**.
   ⚠️ This must match `name` in `wrangler.toml` exactly, or the build fails.

## 2. Build settings

| Setting | Value |
| :-- | :-- |
| Root directory | `/` |
| Build command | **leave empty** |
| Deploy command | `npx wrangler deploy` |

There is no build step: `public/` is already the finished site.

Then, under **Settings → Build → Branch control**:

- **Production branch: `site`**
- **Turn OFF "Builds for non-production branches."**

That last one matters. `main` holds the Rust project and has no `wrangler.toml`, so leaving
non-production builds on means every code commit kicks off a build that fails and emails you.

## 3. The apex domain attaches itself

`wrangler.toml` already declares it:

```toml
[[routes]]
pattern = "openroutine.dev"
custom_domain = true
```

On the first successful deploy, Cloudflare creates the DNS record and issues the certificate.
Nothing to do by hand. Certificate issuance can lag a few minutes — retry before assuming it
is broken.

## 4. Redirect www to the apex

This cannot go in `public/_redirects` — Cloudflare's redirects file does not support
domain-level redirects. It is a zone rule instead.

1. **DNS → Add record**: type `AAAA`, name `www`, address `100::`, **Proxied (orange cloud)**.
   `100::` is the discard prefix; the record exists only so the hostname reaches Cloudflare's
   edge, where the rule below can fire.
2. **Rules → Redirect Rules → Create rule.** Use the built-in *Redirect from WWW to Root*
   template, or set it manually:
   - **When:** `http.host eq "www.openroutine.dev"`
   - **Then:** dynamic redirect to
     `concat("https://openroutine.dev", http.request.uri.path)`
   - **Status:** 301, preserve query string

The free plan allows 10 single redirects; this uses one.

## 5. Verify

```bash
dig +short A openroutine.dev              # Cloudflare addresses
curl -sI https://openroutine.dev          # 200
curl -sI https://openroutine.dev/docs     # 200
curl -sI https://openroutine.dev/docs.html # 307 -> /docs
curl -sI https://www.openroutine.dev      # 301 -> https://openroutine.dev
curl -sI https://openroutine.dev/nope     # 404, serving 404.html
```

Then confirm auto-deploy actually works, rather than assuming it: make a trivial edit on the
`site` branch, push, and watch the change go live without running anything.

Finally, check the social card at <https://cards-dev.twitter.com/validator> or by pasting the
URL into any chat client — `public/og.png` should render.

## Why Workers and not Pages

Cloudflare's current guidance: *"If you are starting a new project, use Workers instead of
Pages. Pages continues to work, but new features and optimizations are focused on Workers."*

`wrangler.toml` deliberately has no `main`. With no Worker script, every request is a free
static-asset request, so the free plan's 100k/day Worker-invocation cap never applies and
bandwidth is unmetered. Adding `main` or `run_worker_first` later would change that.
