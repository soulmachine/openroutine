# openroutine.dev

The source for <https://openroutine.dev>. It lives here, inside the repo it documents, so that a
change to the CLI and the change it implies on the website can be one commit and one review.

```
wrangler.toml     Cloudflare Workers Static Assets config
og.html           source for public/og.png (not published — outside public/)
public/           everything that gets served
```

Nothing in the Rust build reaches into this directory — `rust-embed` is scoped to `ui/`.

## Working on it

Hand-written HTML and one CSS file. There is no build step, no npm dependency, and no
JavaScript on the site at all — the same discipline the product follows, and what lets
`public/_headers` declare `script-src 'none'` honestly.

The palette and type stack in `public/styles.css` are copied verbatim from the daemon's own
web UI, `../ui/styles.css`. If that changes, change this to match — they are now one `git grep`
apart, which is most of the reason the site sits in this repo.

```bash
cd site
npx wrangler dev   # serve locally, exactly as Cloudflare will
```

Run wrangler from this directory, not the repo root: it reads `site/wrangler.toml`, and
`[assets] directory` is resolved relative to that file.

`docs.html` is served at `/docs`, not `/docs.html` — Cloudflare's default `auto-trailing-slash`
handling drops the extension. Link to `/docs`.

## Deploying

Pushing to `main` is the deploy, once Workers Builds is connected — see [DEPLOY.md](DEPLOY.md).
A build watch path on `site/*` keeps commits that touch only the Rust project from triggering
one. Until then, `npx wrangler deploy` from this directory.

To regenerate the social card after editing `og.html`:

```bash
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  --headless --disable-gpu --hide-scrollbars \
  --screenshot=public/og.png --window-size=1200,630 og.html
```
