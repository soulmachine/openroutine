# openroutine.dev

The source for <https://openroutine.dev>. It lives here, inside the repo it documents, so that a
change to the CLI and the change it implies on the website can be one commit and one review.

```
wrangler.toml     Cloudflare Workers Static Assets config
og.html           source for public/og.png (not published — outside public/)
og.zh.html        source for public/og-zh.png, the Chinese card
public/           everything that gets served
public/zh/        the Simplified-Chinese mirror: index, docs, 404
```

Nothing in the Rust build reaches into this directory — `rust-embed` is scoped to `ui/`.

## Working on it

Hand-written HTML and one CSS file. There is no build step and no npm dependency. The only
JavaScript on the site is one small inline script, byte-identical on every page, that powers
the language switch: it remembers a 中文/English choice in `localStorage` and sends a
first-time zh-language browser from an English page to its `/zh/` counterpart.
`public/_headers` pins exactly that script by its sha256 hash — editing it means recomputing
the hash (see the comment there) and keeping all six copies identical, or browsers drop it.

The `/zh/` pages are a hand-maintained mirror. A content edit on an English page is not done
until the same edit lands on its `/zh/` twin — they are one commit apart by design, same as
the CLI and the site.

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

To regenerate the social cards after editing `og.html` or `og.zh.html`:

```bash
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  --headless --disable-gpu --hide-scrollbars \
  --screenshot=public/og.png --window-size=1200,630 og.html

"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  --headless --disable-gpu --hide-scrollbars \
  --screenshot=public/og-zh.png --window-size=1200,630 og.zh.html
```
