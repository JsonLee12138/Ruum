# Ruum Update Worker (`infra/update-worker`)

Cloudflare Worker + private R2 (`ruum-releases`) edge distribution proxy and installer script server for Ruum.

## Endpoints

- `GET /health` — Health check (`{"ok":true,"service":"ruum-update-worker"}`).
- `GET /v1/check?channel=<stable|beta>&os=<os>&arch=<arch>&pkg=<pkg>&ver=<ver>&lang=<lang>&entry=<gui|cli>` — Edge-cached (`300s` / 5 min) raw byte-for-byte passthrough (`200`) of `channels/<channel>/manifest.json` with `X-Ruum-Signature` and `X-Ruum-Channel` from `channels/<channel>/manifest.json.sig`. When no release is published yet on `<channel>` (both `manifest.json` and `.sig` are absent), returns **`204 No Content`** (`X-Ruum-Channel`, `Cache-Control: public, max-age=60`) and logs `no_release: true`; when only one of the pair exists (incomplete publication) or the signature is invalid, fails closed with **`502`**. Emits single-line sanitized `update_check` JSON logs on every request (cache hit, miss, or `no_release`).
- `GET /v1/download/<path>` — Streams R2 objects under `releases/` — including installers, `.AppImage`, `.AppImage.zsync` delta metadata (for `AppImageUpdate` / `Gear Lever`), `.sha256`, and `SHA256SUMS` — with `Accept-Ranges: bytes`, `Range` (`206`), `If-None-Match` (`304`), `ETag`, and `Cache-Control: public, max-age=31536000, immutable`.
- `GET /install.sh` — Dynamic macOS/Linux installer supporting `--cli-only`, `--version`, `--channel`, `--prefix`, `--yes`, and `RUUM_INSTALL_DIR` (exits cleanly with `"no release published yet for channel '<channel>'"` if `/v1/check` returns `204`).
- `GET /install.ps1` — Dynamic Windows PowerShell installer supporting `-Channel`, `-Version`, `-InstallDir`, SHA256 verification, and User `Path` registration (exits cleanly with `"no release published yet for channel '<channel>'"` if `/v1/check` returns `204`).

## Local Tests

Zero `npm install` required (uses built-in Node test runner):

```sh
node --test infra/update-worker/tests/worker.test.mjs
```

## Deployment

1. Create the private R2 bucket once in Cloudflare Dashboard or Wrangler:

   ```sh
   npx wrangler r2 bucket create ruum-releases
   ```

2. Deploy from `infra/update-worker/`:

   ```sh
   cd infra/update-worker
   npx wrangler deploy
   ```

3. Required GitHub Actions Repository Secrets & Variables (for `.github/workflows/release.yml`):
   - **Secret `CF_ACCOUNT_ID`**: Cloudflare Account ID.
   - **Secret `CF_API_TOKEN`**: API Token scoped to `Workers Scripts:Edit` and `Workers R2 Storage:Edit`.
   - **Secret `RUUM_UPDATE_SIGNING_KEY`**: Ed25519 private signing key (used offline in CI to sign `manifest.json`).
   - **Variable `RUUM_UPDATE_BASE_URL`**: Deployed Worker base URL (current production custom domain: `https://ruum-updates.jsonlee12138.com`; fallback `workers.dev` route: `https://ruum-updates.lijunsong2.workers.dev`).
   - **Variable `GITHUB_REPO`** (in `wrangler.toml` `[vars]`): Upstream fallback repository (`JsonLee12138/Ruum`).

## Zero-Database Native Analytics (Workers Observability)

This Worker intentionally uses **zero D1/KV databases** and exposes **no `/admin/stats` endpoint**. Every invocation writes sanitized single-line JSON logs indexed automatically by Cloudflare Workers Observability (`[observability.logs]` `invocation_logs = true`):

- **DAU / MAU**: Open Cloudflare Dashboard -> **Workers & Pages -> `ruum-updates` -> Observability -> Query Builder**:
  - Filter: `event = "update_check"`
  - Aggregation: `Count Distinct(client_id)` grouped by day (`1d`) or month (`30d`).
- **OS / Architecture / Package / Version / Locale Breakdown**:
  - Filter: `event = "update_check"`
  - Aggregation: `Count` or `Count Distinct(client_id)` grouped by `os`, `arch`, `pkg`, `ver`, `channel`, `lang`, or `country`.
- **Installer & Asset Download Funnel**:
  - Compare `Count` for `event = "install_script"` (grouped by `target`: `posix` / `windows`) against `event = "asset_download"` (grouped by `filename` / `path`) and Cloudflare's native R2 egress/request metrics.
