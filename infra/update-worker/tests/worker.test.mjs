import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import worker from "../src/index.mjs";

function createMockR2(initialObjects = {}) {
  const store = new Map();
  for (const [key, spec] of Object.entries(initialObjects)) {
    const bodyBytes =
      typeof spec.body === "string"
        ? new TextEncoder().encode(spec.body)
        : spec.body instanceof Uint8Array
          ? spec.body
          : new Uint8Array(spec.body);
    store.set(key, {
      bytes: bodyBytes,
      etag: spec.etag ?? `"etag-${key}"`,
      contentType: spec.contentType,
    });
  }

  const getCalls = [];

  return {
    getCalls,
    async get(key, options = {}) {
      getCalls.push({ key, options });
      const entry = store.get(key);
      if (!entry) return null;

      const ifNoneMatch = options?.onlyIf?.get?.("If-None-Match") ?? null;
      if (ifNoneMatch && ifNoneMatch === entry.etag) {
        return {
          key,
          size: entry.bytes.byteLength,
          etag: entry.etag,
          httpEtag: entry.etag,
          httpMetadata: entry.contentType
            ? { contentType: entry.contentType }
            : {},
          body: null,
        };
      }

      const rangeHeader = options?.range?.get?.("Range") ?? null;
      if (rangeHeader) {
        const match = /^bytes=(\d+)-(\d*)$/.exec(rangeHeader.trim());
        if (match) {
          const start = Number(match[1]);
          const end =
            match[2] !== ""
              ? Math.min(Number(match[2]), entry.bytes.byteLength - 1)
              : entry.bytes.byteLength - 1;
          const slice = entry.bytes.slice(start, end + 1);
          return {
            key,
            size: entry.bytes.byteLength,
            etag: entry.etag,
            httpEtag: entry.etag,
            httpMetadata: entry.contentType
              ? { contentType: entry.contentType }
              : {},
            range: { offset: start, length: slice.byteLength },
            body: new ReadableStream({
              start(controller) {
                controller.enqueue(slice);
                controller.close();
              },
            }),
            async arrayBuffer() {
              return slice.buffer.slice(
                slice.byteOffset,
                slice.byteOffset + slice.byteLength
              );
            },
            async text() {
              return new TextDecoder().decode(slice);
            },
          };
        }
      }

      const bytes = entry.bytes;
      return {
        key,
        size: bytes.byteLength,
        etag: entry.etag,
        httpEtag: entry.etag,
        httpMetadata: entry.contentType
          ? { contentType: entry.contentType }
          : {},
        body: new ReadableStream({
          start(controller) {
            controller.enqueue(bytes);
            controller.close();
          },
        }),
        async arrayBuffer() {
          return bytes.buffer.slice(
            bytes.byteOffset,
            bytes.byteOffset + bytes.byteLength
          );
        },
        async text() {
          return new TextDecoder().decode(bytes);
        },
      };
    },
  };
}

function createMockCache() {
  const entries = new Map();
  const matchCalls = [];
  const putCalls = [];

  return {
    matchCalls,
    putCalls,
    async match(req) {
      const key = typeof req === "string" ? req : req.url;
      matchCalls.push(key);
      const stored = entries.get(key);
      return stored ? stored.clone() : undefined;
    },
    async put(req, res) {
      const key = typeof req === "string" ? req : req.url;
      putCalls.push(key);
      entries.set(key, res.clone());
    },
  };
}

function createExecutionCtx() {
  const promises = [];
  return {
    promises,
    waitUntil(p) {
      promises.push(Promise.resolve(p));
    },
    async flush() {
      await Promise.all(promises);
    },
  };
}

function captureConsoleLogs(fn) {
  const original = console.log;
  const lines = [];
  console.log = (...args) => {
    lines.push(args.map(String).join(" "));
  };
  return Promise.resolve()
    .then(fn)
    .then(
      (res) => {
        console.log = original;
        return { res, lines };
      },
      (err) => {
        console.log = original;
        throw err;
      }
    );
}

const RAW_MANIFEST = `{
  "schema_version":   1,
  "version": "0.2.2",
  "tag": "v0.2.2",
  "channel": "stable",
  "published_at": "2026-11-20T00:00:00Z",
  "release_url": "https://github.com/JsonLee12138/Ruum/releases/tag/v0.2.2",
  "notes": "### Features\\n- Added \`updater\`",
  "assets": {
    "macos-aarch64-app_zip": {
      "filename": "ruum-0.2.2-aarch64-macos.zip",
      "path": "releases/v0.2.2/ruum-0.2.2-aarch64-macos.zip",
      "fallback_url": "https://github.com/JsonLee12138/Ruum/releases/download/v0.2.2/ruum-0.2.2-aarch64-macos.zip",
      "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "size": 1024
    },
    "linux-x86_64-tarball": {
      "filename": "ruum-0.2.2-x86_64-linux.tar.gz",
      "path": "releases/v0.2.2/ruum-0.2.2-x86_64-linux.tar.gz",
      "fallback_url": "https://github.com/JsonLee12138/Ruum/releases/download/v0.2.2/ruum-0.2.2-x86_64-linux.tar.gz",
      "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      "size": 2048
    },
    "windows-x86_64-portable_zip": {
      "filename": "ruum-0.2.2-x86_64-windows.zip",
      "path": "releases/v0.2.2/ruum-0.2.2-x86_64-windows.zip",
      "fallback_url": "https://github.com/JsonLee12138/Ruum/releases/download/v0.2.2/ruum-0.2.2-x86_64-windows.zip",
      "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
      "size": 4096
    }
  }
}
`;

test("GET /health returns 200 ok json", async () => {
  const req = new Request("https://ruum-updates.example.workers.dev/health");
  const res = await worker.fetch(req, {}, createExecutionCtx());
  assert.equal(res.status, 200);
  assert.match(res.headers.get("Content-Type") ?? "", /application\/json/);
  assert.deepEqual(await res.json(), {
    ok: true,
    service: "ruum-update-worker",
  });
});

test("non-GET requests return 405", async () => {
  const req = new Request("https://ruum-updates.example.workers.dev/v1/check", {
    method: "POST",
  });
  const res = await worker.fetch(req, {}, createExecutionCtx());
  assert.equal(res.status, 405);
});

test("GET /v1/check rejects invalid channel with 400", async () => {
  const req = new Request(
    "https://ruum-updates.example.workers.dev/v1/check?channel=nightly"
  );
  const res = await worker.fetch(
    req,
    { RELEASES_BUCKET: createMockR2() },
    createExecutionCtx()
  );
  assert.equal(res.status, 400);
});

test("GET /v1/check returns 404 when manifest or sig is missing", async () => {
  const bucketOnlyManifest = createMockR2({
    "channels/beta/manifest.json": { body: RAW_MANIFEST },
  });
  globalThis.caches = { default: createMockCache() };

  const res = await worker.fetch(
    new Request("https://ruum-updates.example.workers.dev/v1/check?channel=beta"),
    { RELEASES_BUCKET: bucketOnlyManifest },
    createExecutionCtx()
  );
  assert.equal(res.status, 404);
});

test("GET /v1/check preserves raw manifest bytes, trims signature newline, normalizes edge cache, and logs on both miss and hit", async () => {
  const bucket = createMockR2({
    "channels/stable/manifest.json": { body: RAW_MANIFEST },
    "channels/stable/manifest.json.sig": { body: "  deadbeefsig==\r\n  " },
  });
  const cache = createMockCache();
  globalThis.caches = { default: cache };

  const ctx1 = createExecutionCtx();
  const req1 = new Request(
    "https://ruum-updates.example.workers.dev/v1/check?os=macos&arch=aarch64&pkg=app_zip&ver=0.2.1&lang=zh&entry=gui",
    {
      headers: {
        "X-Ruum-Client-Id": "uuid-1111-2222",
        "CF-Connecting-IP": "203.0.113.42",
        Cookie: "session=supersecret",
        Authorization: "Bearer secret-token",
      },
    }
  );
  req1.cf = { country: "JP" };

  const { res: res1, lines: logs1 } = await captureConsoleLogs(() =>
    worker.fetch(req1, { RELEASES_BUCKET: bucket }, ctx1)
  );
  await ctx1.flush();

  assert.equal(res1.status, 200);
  assert.equal(res1.headers.get("Content-Type"), "application/json; charset=utf-8");
  assert.equal(res1.headers.get("Cache-Control"), "public, max-age=300");
  assert.equal(res1.headers.get("X-Ruum-Signature"), "deadbeefsig==");
  assert.equal(res1.headers.get("X-Ruum-Channel"), "stable");
  // Exact byte-for-byte preservation (note triple space after schema_version).
  assert.equal(await res1.text(), RAW_MANIFEST);

  assert.deepEqual(cache.matchCalls, [
    "https://cache.ruum.internal/v1/manifest/stable",
  ]);
  assert.deepEqual(cache.putCalls, [
    "https://cache.ruum.internal/v1/manifest/stable",
  ]);
  assert.equal(bucket.getCalls.length, 2);

  assert.equal(logs1.length, 1);
  const parsedLog1 = JSON.parse(logs1[0]);
  assert.equal(parsedLog1.event, "update_check");
  assert.equal(parsedLog1.channel, "stable");
  assert.equal(parsedLog1.os, "macos");
  assert.equal(parsedLog1.arch, "aarch64");
  assert.equal(parsedLog1.pkg, "app_zip");
  assert.equal(parsedLog1.ver, "0.2.1");
  assert.equal(parsedLog1.lang, "zh");
  assert.equal(parsedLog1.entry, "gui");
  assert.equal(parsedLog1.client_id, "uuid-1111-2222");
  assert.equal(parsedLog1.country, "JP");
  assert.equal(parsedLog1.cache_hit, false);
  assert.equal(logs1[0].includes("203.0.113.42"), false);
  assert.equal(logs1[0].includes("supersecret"), false);
  assert.equal(logs1[0].includes("secret-token"), false);

  // Second request with completely different query params hits the same normalized cache key!
  const ctx2 = createExecutionCtx();
  const req2 = new Request(
    "https://ruum-updates.example.workers.dev/v1/check?channel=stable&os=/Users/alice/.ssh/id_rsa&arch=x86_64&ver=0.2.0%0aevil&lang=en&entry=cli",
    {
      headers: {
        "X-Ruum-Client-Id": "uuid-3333-4444",
      },
    }
  );
  req2.cf = { country: "US" };

  const { res: res2, lines: logs2 } = await captureConsoleLogs(() =>
    worker.fetch(req2, { RELEASES_BUCKET: bucket }, ctx2)
  );
  await ctx2.flush();

  assert.equal(res2.status, 200);
  assert.equal(await res2.text(), RAW_MANIFEST);
  assert.equal(res2.headers.get("X-Ruum-Signature"), "deadbeefsig==");
  // R2 was NOT called again on cache hit.
  assert.equal(bucket.getCalls.length, 2);

  // Telemetry still emitted on cache hit, and dangerous paths/newlines stripped.
  assert.equal(logs2.length, 1);
  assert.equal(logs2[0].includes("\n"), false);
  const parsedLog2 = JSON.parse(logs2[0]);
  assert.equal(parsedLog2.cache_hit, true);
  assert.equal(parsedLog2.client_id, "uuid-3333-4444");
  assert.equal(parsedLog2.country, "US");
  assert.equal(logs2[0].includes("/Users/alice"), false);
  assert.equal(parsedLog2.os, null);
  assert.equal(parsedLog2.ver, null);
});

test("GET /v1/download streams asset, supports Range and If-None-Match, and blocks traversal/double-encoding", async () => {
  const payload = "0123456789abcdef";
  const bucket = createMockR2({
    "releases/v0.2.2/ruum-0.2.2-aarch64-macos.zip": {
      body: payload,
      etag: '"zip-etag-1"',
    },
  });

  // 1. Normal full download
  const reqFull = new Request(
    "https://ruum-updates.example.workers.dev/v1/download/releases/v0.2.2/ruum-0.2.2-aarch64-macos.zip"
  );
  reqFull.cf = { country: "DE" };
  const { res: resFull, lines: logsFull } = await captureConsoleLogs(() =>
    worker.fetch(reqFull, { RELEASES_BUCKET: bucket }, createExecutionCtx())
  );
  assert.equal(resFull.status, 200);
  assert.equal(await resFull.text(), payload);
  assert.equal(
    resFull.headers.get("Cache-Control"),
    "public, max-age=31536000, immutable"
  );
  assert.equal(resFull.headers.get("ETag"), '"zip-etag-1"');
  assert.equal(
    resFull.headers.get("Content-Disposition"),
    'attachment; filename="ruum-0.2.2-aarch64-macos.zip"'
  );
  assert.equal(logsFull.length, 1);
  const dlLog = JSON.parse(logsFull[0]);
  assert.equal(dlLog.event, "asset_download");
  assert.equal(dlLog.path, "releases/v0.2.2/ruum-0.2.2-aarch64-macos.zip");
  assert.equal(dlLog.country, "DE");

  // 2. Range request
  const reqRange = new Request(
    "https://ruum-updates.example.workers.dev/v1/download/releases/v0.2.2/ruum-0.2.2-aarch64-macos.zip",
    { headers: { Range: "bytes=4-7" } }
  );
  const resRange = await worker.fetch(
    reqRange,
    { RELEASES_BUCKET: bucket },
    createExecutionCtx()
  );
  assert.equal(resRange.status, 206);
  assert.equal(resRange.headers.get("Content-Range"), "bytes 4-7/16");
  assert.equal(await resRange.text(), "4567");

  // 3. If-None-Match -> 304
  const reqEtag = new Request(
    "https://ruum-updates.example.workers.dev/v1/download/releases/v0.2.2/ruum-0.2.2-aarch64-macos.zip",
    { headers: { "If-None-Match": '"zip-etag-1"' } }
  );
  const resEtag = await worker.fetch(
    reqEtag,
    { RELEASES_BUCKET: bucket },
    createExecutionCtx()
  );
  assert.equal(resEtag.status, 304);

  // 4. Missing asset -> 404
  const resMissing = await worker.fetch(
    new Request(
      "https://ruum-updates.example.workers.dev/v1/download/releases/v0.2.2/missing.zip"
    ),
    { RELEASES_BUCKET: bucket },
    createExecutionCtx()
  );
  assert.equal(resMissing.status, 404);

  // 5. Traversal, double encoding, non-releases prefix, invalid URI encoding -> 400
  const badPaths = [
    "/v1/download/",
    "/v1/download/releases/../channels/stable/manifest.json",
    "/v1/download/releases/%2e%2e/channels/stable/manifest.json",
    "/v1/download/releases/%252e%252e/secret",
    "/v1/download/releases/v0.2.2//ruum.zip",
    "/v1/download/channels/stable/manifest.json",
    "/v1/download/releases/%ZZbad",
    "/v1/download/releases/v0.2.2/foo%20bar.zip",
  ];
  for (const p of badPaths) {
    const resBad = await worker.fetch(
      new Request(`https://ruum-updates.example.workers.dev${p}`),
      { RELEASES_BUCKET: bucket },
      createExecutionCtx()
    );
    assert.equal(resBad.status, 400, `expected 400 for ${p}`);
  }
});

test("GET /install.sh returns valid POSIX shell script with dynamic origin, --cli-only, SHA256 verification, and telemetry log", async () => {
  const req = new Request("https://updates.ruum.dev/install.sh");
  req.cf = { country: "CA" };
  const { res, lines } = await captureConsoleLogs(() =>
    worker.fetch(
      req,
      { GITHUB_REPO: "JsonLee12138/Ruum" },
      createExecutionCtx()
    )
  );
  assert.equal(res.status, 200);
  assert.equal(res.headers.get("Content-Type"), "text/x-shellscript; charset=utf-8");

  const script = await res.text();
  assert.match(script, /^#!\/bin\/sh/);
  assert.ok(script.includes("https://updates.ruum.dev"));
  assert.ok(script.includes("--cli-only"));
  assert.ok(script.includes("--version"));
  assert.ok(script.includes("--channel"));
  assert.ok(script.includes("--prefix"));
  assert.ok(script.includes("--yes"));
  assert.ok(script.includes("RUUM_INSTALL_DIR"));
  assert.ok(script.includes("/v1/check"));
  assert.ok(script.includes("macos-${ARCH}-app_zip"));
  assert.ok(script.includes("linux-${ARCH}-tarball"));
  assert.ok(script.includes("ruum.app/Contents/MacOS/ruum"));
  assert.ok(script.includes("ruum.app/Contents/Resources/ruum-scan-linux.pack"));
  assert.ok(script.includes("sha256"));
  assert.ok(script.includes("fallback_url"));

  // Syntax-check generated POSIX shell script using `sh -n`.
  execFileSync("sh", ["-n"], { input: script, encoding: "utf8" });

  // Verify the embedded POSIX awk JSON extractor parses RAW_MANIFEST accurately.
  const fnMatch = script.match(
    /extract_json_field\(\) \{[\s\S]*?\n\}/
  );
  assert.ok(fnMatch, "extract_json_field helper must be present");
  const manifestWithBracesInNotes = JSON.stringify({
    ...JSON.parse(RAW_MANIFEST),
    notes: '### Notes\n- Escaped "quotes" and {braces} inside notes',
  });
  const extractedPath = execFileSync(
    "sh",
    [
      "-c",
      `${fnMatch[0]}\nprintf '%s' "$MANIFEST" | extract_json_field "macos-aarch64-app_zip" "path"`,
    ],
    {
      env: { ...process.env, MANIFEST: manifestWithBracesInNotes },
      encoding: "utf8",
    }
  );
  assert.equal(extractedPath, "releases/v0.2.2/ruum-0.2.2-aarch64-macos.zip");

  const extractedVersion = execFileSync(
    "sh",
    [
      "-c",
      `${fnMatch[0]}\nprintf '%s' "$MANIFEST" | extract_json_field "" "version"`,
    ],
    {
      env: { ...process.env, MANIFEST: manifestWithBracesInNotes },
      encoding: "utf8",
    }
  );
  assert.equal(extractedVersion, "0.2.2");

  assert.equal(lines.length, 1);
  assert.deepEqual(JSON.parse(lines[0]), {
    event: "install_script",
    target: "posix",
    country: "CA",
  });
});

test("GET /install.ps1 returns PowerShell script with dynamic origin, nested zip handling, SHA256 verification, and telemetry log", async () => {
  const req = new Request("https://updates.ruum.dev/install.ps1");
  req.cf = { country: "GB" };
  const { res, lines } = await captureConsoleLogs(() =>
    worker.fetch(
      req,
      { GITHUB_REPO: "JsonLee12138/Ruum" },
      createExecutionCtx()
    )
  );
  assert.equal(res.status, 200);
  assert.equal(res.headers.get("Content-Type"), "text/plain; charset=utf-8");

  const script = await res.text();
  assert.ok(script.includes("https://updates.ruum.dev"));
  assert.ok(script.includes("-Channel"));
  assert.ok(script.includes("-Version"));
  assert.ok(script.includes("-InstallDir"));
  assert.ok(script.includes("/v1/check"));
  assert.ok(script.includes("portable_zip"));
  assert.ok(script.includes("Get-FileHash"));
  assert.ok(script.includes("ruum.exe"));
  assert.ok(script.includes("ruum-gui.exe"));
  assert.ok(script.includes("ruum-scan-linux.pack"));
  assert.ok(script.includes("Programs\\ruum\\bin"));
  assert.ok(script.includes("SetEnvironmentVariable"));

  assert.equal(lines.length, 1);
  assert.deepEqual(JSON.parse(lines[0]), {
    event: "install_script",
    target: "windows",
    country: "GB",
  });
});
