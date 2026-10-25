# Deploying the UI

One UI build (`ui/dist`) deploys three ways (ADR-0013): embedded in the `bia` binary, to Cloudflare Pages, or to S3 behind CloudFront. The build never contains an API URL; the browser reads `config.json` from the site root at startup, so you choose the API per deployment, not per build.

Build the bundle once (Node.js 22+ and pnpm, see `ui/README.md`):

```sh
just ui-build        # ui/dist
```

The UI must be served from the root of its origin (`https://bia.example.com/`, not `https://example.com/bia/`): the bundle uses absolute asset paths and fetches `/config.json` (ADR-0052).

## `config.json`

```json
{ "apiUrl": "https://api.example.com" }
```

Put it next to `index.html`. `apiUrl` is the base URL of the `bia serve` API, without a trailing slash; an empty string means the page's own origin. A malformed file is an error shown on the login screen. Changing the file changes the API without a rebuild. Serve it with `Cache-Control: no-cache` so a change takes effect at once.

## Embedded in the binary

```sh
just build-ui        # target/release/bia, with the UI inside
bia serve
```

This is `cargo build --release -p bia-cli --features ui` after building the bundle; the build fails when `ui/dist` is missing. Without `--features ui` nothing of the UI is compiled in, and Node is not needed.

The UI is served from `/` by the `api` Role, with unknown paths such as `/runs/123` answered with `index.html` so deep links work. It never answers under `/v1`, `/mcp`, `/health`, `/ready` or `/openapi.json`, which stay the API's. `config.json` is generated (`{"apiUrl":""}`), so no CORS setup is needed: the UI and the API share an origin. Responses carry a restrictive `Content-Security-Policy`; hashed files under `/assets/` are cached for a year, everything else revalidates with an `ETag`.

## Cloudflare Pages

1. Create a Pages project (direct upload, no build step) and deploy the bundle:

   ```sh
   cp -r ui/dist site && cd site
   printf '%s' '{"apiUrl":"https://api.example.com"}' > config.json
   printf '/assets/*\n  Cache-Control: public, max-age=31536000, immutable\n/config.json\n  Cache-Control: no-cache\n' > _headers
   npx wrangler pages deploy . --project-name bia
   ```

2. Pages serves `index.html` for paths that match no file, as long as the project has no `404.html`, so deep links work without a `_redirects` file.
3. Allow the Pages origin in the API: set `BIA_CORS_ORIGINS=https://bia.pages.dev` (your project's domain, or your custom domain; a comma-separated list for several) on `bia serve`. A UI on another origin is blocked by the browser until it is listed. Use the exact origin, scheme included, no path.

## S3 and CloudFront

1. Create a private S3 bucket and a CloudFront distribution with the bucket as origin through Origin Access Control, default root object `index.html`.
2. Add custom error responses for 403 and 404 (S3 answers 403 for a missing key when the bucket is private) that return `/index.html` with status 200, so deep links work. A missing `/config.json` then returns `index.html`, which the UI treats as "no config" and falls back to its development default, so always upload one.
3. Upload, with long caching only for the hashed files:

   ```sh
   printf '%s' '{"apiUrl":"https://api.example.com"}' > ui/dist/config.json
   aws s3 sync ui/dist s3://BUCKET --delete \
     --exclude index.html --exclude config.json \
     --cache-control 'public, max-age=31536000, immutable'
   aws s3 cp ui/dist/index.html s3://BUCKET/index.html --cache-control no-cache
   aws s3 cp ui/dist/config.json s3://BUCKET/config.json --cache-control no-cache \
     --content-type application/json
   aws cloudfront create-invalidation --distribution-id DIST --paths /index.html /config.json
   ```

   Run the `sync` before the `index.html` upload so a visitor never gets a page that names files not yet uploaded.
4. Set `BIA_CORS_ORIGINS=https://bia.example.com` (the distribution's domain) on `bia serve`, as for Pages.

## Checks

- Open the site, sign in with a Token. A failure that names `Cannot reach the API` means `apiUrl` is wrong or the API is down; a browser console CORS error means the origin is missing from `BIA_CORS_ORIGINS` (see `docs/configuration.md`).
- Reload on a deep link such as `/runs`: it must show the app, not a 403 or 404.
