# openroutine.dev

The source for <https://openroutine.dev>. This is an **orphan branch** — it shares no history
with `main`, and holds no Rust code. `main` holds the project; this holds its website.

```
wrangler.toml     Cloudflare Workers Static Assets config
og.html           source for public/og.png (not published — outside public/)
public/           everything that gets served
```

## Working on it

Hand-written HTML and one CSS file. There is no build step, no npm dependency, and no
JavaScript on the site at all — the same discipline the product follows, and what lets
`public/_headers` declare `script-src 'none'` honestly.

The palette and type stack in `public/styles.css` are copied verbatim from the daemon's own
web UI (`ui/styles.css` on `main`). If that changes, change this to match.

```bash
git worktree add ../openroutine-site site   # if you don't have this checked out yet
npx wrangler dev                            # serve locally, exactly as Cloudflare will
```

`docs.html` is served at `/docs`, not `/docs.html` — Cloudflare's default `auto-trailing-slash`
handling drops the extension. Link to `/docs`.

## Deploying

Pushing to this branch is the deploy. Cloudflare Workers Builds watches `site` as the production
branch and runs `npx wrangler deploy` on every push.

To regenerate the social card after editing `og.html`:

```bash
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  --headless --disable-gpu --hide-scrollbars \
  --screenshot=public/og.png --window-size=1200,630 og.html
```
