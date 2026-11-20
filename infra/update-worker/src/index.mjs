const VALID_CHANNELS = new Set(["stable", "beta"]);
const SAFE_FIELD_RE = /^[A-Za-z0-9._+-]{1,64}$/;
const SAFE_COUNTRY_RE = /^[A-Za-z]{2,3}$/;
const SAFE_DOWNLOAD_PATH_RE = /^releases\/[A-Za-z0-9._\/-]+$/;
const SAFE_ORIGIN_RE = /^https?:\/\/[A-Za-z0-9.:-]+$/;
const DEFAULT_GITHUB_REPO = "JsonLee12138/Ruum";

export function sanitizeField(value, maxLen = 64) {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  if (!trimmed || trimmed.length > maxLen) return null;
  if (trimmed.includes("..") || trimmed.includes("/") || trimmed.includes("\\")) {
    return null;
  }
  if (!SAFE_FIELD_RE.test(trimmed)) return null;
  return trimmed;
}

export function sanitizeCountry(request) {
  const raw = request?.cf?.country;
  if (typeof raw !== "string") return null;
  const trimmed = raw.trim().toUpperCase();
  return SAFE_COUNTRY_RE.test(trimmed) ? trimmed : null;
}

export function validateDownloadPath(rawSegment) {
  if (!rawSegment) return null;
  let decoded;
  try {
    decoded = decodeURIComponent(rawSegment);
  } catch {
    return null;
  }
  if (
    !decoded ||
    decoded.includes("..") ||
    decoded.includes("//") ||
    decoded.endsWith("/") ||
    !SAFE_DOWNLOAD_PATH_RE.test(decoded)
  ) {
    return null;
  }
  return decoded;
}

function jsonResponse(payload, status = 200) {
  return new Response(JSON.stringify(payload), {
    status,
    headers: {
      "Content-Type": "application/json; charset=utf-8",
      "Cache-Control": "no-store",
    },
  });
}

function logEvent(record) {
  console.log(JSON.stringify(record));
}

function guessContentType(filename) {
  if (filename.endsWith(".json")) return "application/json; charset=utf-8";
  if (filename.endsWith(".sig") || filename.endsWith(".sha256")) {
    return "text/plain; charset=utf-8";
  }
  return "application/octet-stream";
}

function safeOrigin(url, env) {
  if (SAFE_ORIGIN_RE.test(url.origin)) return url.origin;
  const fallback = typeof env?.PUBLIC_BASE_URL === "string"
    ? env.PUBLIC_BASE_URL.trim().replace(/\/+$/, "")
    : "";
  return SAFE_ORIGIN_RE.test(fallback)
    ? fallback
    : "https://ruum-updates.jsonlee12138.com";
}

function safeGithubRepo(env) {
  const raw = typeof env?.GITHUB_REPO === "string" ? env.GITHUB_REPO.trim() : "";
  return /^[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+$/.test(raw)
    ? raw
    : DEFAULT_GITHUB_REPO;
}

export function renderInstallSh(origin, githubRepo) {
  return `#!/bin/sh
# Ruum installer (macOS / Linux)
# Supports: --cli-only, --version <V>, --channel <stable|beta>, --prefix <DIR>, --yes
set -eu

BASE_URL="\${RUUM_UPDATE_BASE_URL:-${origin}}"
GITHUB_REPO="${githubRepo}"
CHANNEL="stable"
REQ_VERSION=""
PREFIX="\${RUUM_INSTALL_DIR:-\$HOME/.local}"
CLI_ONLY=0
YES=0

usage() {
  cat <<'EOF'
Usage: install.sh [OPTIONS]
  --cli-only             Install only the headless ruum CLI (and Linux scanner pack on macOS)
  --version <V>          Install a specific version (e.g. 0.2.2)
  --channel <stable|beta> Release channel to query (default: stable)
  --prefix <DIR>         Install prefix (default: \$HOME/.local, or RUUM_INSTALL_DIR)
  --yes, -y              Non-interactive mode
  -h, --help             Show this help message
EOF
}

while [ "\$#" -gt 0 ]; do
  case "\$1" in
    --cli-only)
      CLI_ONLY=1
      shift
      ;;
    --version)
      [ "\$#" -ge 2 ] || { echo "error: --version requires a value" >&2; exit 2; }
      REQ_VERSION="\$2"
      shift 2
      ;;
    --version=*)
      REQ_VERSION="\${1#*=}"
      shift
      ;;
    --channel)
      [ "\$#" -ge 2 ] || { echo "error: --channel requires a value" >&2; exit 2; }
      CHANNEL="\$2"
      shift 2
      ;;
    --channel=*)
      CHANNEL="\${1#*=}"
      shift
      ;;
    --prefix)
      [ "\$#" -ge 2 ] || { echo "error: --prefix requires a value" >&2; exit 2; }
      PREFIX="\$2"
      shift 2
      ;;
    --prefix=*)
      PREFIX="\${1#*=}"
      shift
      ;;
    --yes|-y)
      YES=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown option: \$1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "\$CHANNEL" in
  stable|beta) ;;
  *)
    echo "error: invalid channel '\$CHANNEL' (expected stable or beta)" >&2
    exit 2
    ;;
esac

UNAME_S=\$(uname -s)
case "\$UNAME_S" in
  Linux)  OS="linux"; PKG="tarball" ;;
  Darwin) OS="macos"; PKG="app_zip" ;;
  *)
    echo "error: unsupported OS '\$UNAME_S' (use install.ps1 on Windows)" >&2
    exit 1
    ;;
esac

UNAME_M=\$(uname -m)
case "\$UNAME_M" in
  x86_64|amd64)  ARCH="x86_64" ;;
  arm64|aarch64) ARCH="aarch64" ;;
  *)
    echo "error: unsupported architecture '\$UNAME_M'" >&2
    exit 1
    ;;
esac

if [ "\$OS" = "macos" ]; then
  ASSET_KEY="macos-\${ARCH}-app_zip"
else
  ASSET_KEY="linux-\${ARCH}-tarball"
fi

BINDIR="\$PREFIX/bin"
BASE_URL="\${BASE_URL%/}"

fetch_text() {
  url="\$1"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "\$url"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO- "\$url"
  else
    echo "error: curl or wget is required" >&2
    return 1
  fi
}

download_file() {
  url="\$1"
  dest="\$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -o "\$dest" "\$url"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "\$dest" "\$url"
  else
    echo "error: curl or wget is required" >&2
    return 1
  fi
}

sha256_of() {
  file="\$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "\$file" | awk '{print tolower(\$1)}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "\$file" | awk '{print tolower(\$1)}'
  elif command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 "\$file" | awk '{print tolower(\$NF)}'
  else
    echo "error: sha256sum, shasum, or openssl is required to verify download" >&2
    return 1
  fi
}

# Pure POSIX awk JSON extractor so minimal containers without jq/python3 work.
extract_json_field() {
  awk -v target="\$1" -v field="\$2" '
  BEGIN {
    raw = ""
  }
  {
    raw = raw " " \$0
  }
  END {
    n = length(raw)
    i = 1
    depth = 0
    in_assets = 0
    in_target = 0
    expect_val = 0
    cur_key = ""
    result = ""

    while (i <= n) {
      ch = substr(raw, i, 1)
      if (ch == " " || ch == "\\t" || ch == "\\r" || ch == "\\n") {
        i++
        continue
      }
      if (ch == "{") {
        depth++
        if (expect_val && depth == 2 && cur_key == "assets") {
          in_assets = 1
        } else if (expect_val && in_assets && depth == 3 && cur_key == target) {
          in_target = 1
        }
        expect_val = 0
        i++
        continue
      }
      if (ch == "}") {
        if (in_target && depth == 3) in_target = 0
        if (in_assets && depth == 2) in_assets = 0
        depth--
        expect_val = 0
        i++
        continue
      }
      if (ch == ":") {
        expect_val = 1
        i++
        continue
      }
      if (ch == ",") {
        expect_val = 0
        i++
        continue
      }
      if (ch == "\\"") {
        i++
        str = ""
        while (i <= n) {
          c = substr(raw, i, 1)
          if (c == "\\\\") {
            i++
            if (i <= n) {
              str = str substr(raw, i, 1)
            }
            i++
            continue
          }
          if (c == "\\"") {
            i++
            break
          }
          str = str c
          i++
        }
        if (expect_val) {
          if (target == "" && depth == 1 && cur_key == field) {
            result = str
          } else if (target != "" && in_target && depth == 3 && cur_key == field) {
            result = str
          }
          expect_val = 0
        } else {
          cur_key = str
        }
        continue
      }
      expect_val = 0
      i++
    }
    printf "%s", result
  }'
}

TMPDIR_INSTALL=\$(mktemp -d 2>/dev/null || mktemp -d -t ruum-install)
trap 'rm -rf "\$TMPDIR_INSTALL"' EXIT INT TERM

CHECK_URL="\${BASE_URL}/v1/check?channel=\${CHANNEL}&os=\${OS}&arch=\${ARCH}&pkg=\${PKG}&ver=\${REQ_VERSION}&entry=install_sh"
MANIFEST_JSON=\$(fetch_text "\$CHECK_URL") || {
  echo "error: failed to fetch release metadata from \$CHECK_URL" >&2
  exit 1
}

CLEAN_REQ_VER=\${REQ_VERSION#v}
if [ -z "\$MANIFEST_JSON" ] && [ -z "\$CLEAN_REQ_VER" ]; then
  echo "error: no release published yet for channel '\$CHANNEL'" >&2
  exit 1
fi

MANIFEST_VERSION=\$(printf '%s' "\$MANIFEST_JSON" | extract_json_field "" "version")
ASSET_PATH=\$(printf '%s' "\$MANIFEST_JSON" | extract_json_field "\$ASSET_KEY" "path")
FALLBACK_URL=\$(printf '%s' "\$MANIFEST_JSON" | extract_json_field "\$ASSET_KEY" "fallback_url")
EXPECTED_SHA256=\$(printf '%s' "\$MANIFEST_JSON" | extract_json_field "\$ASSET_KEY" "sha256" | tr 'A-F' 'a-f')
ASSET_FILENAME=\$(printf '%s' "\$MANIFEST_JSON" | extract_json_field "\$ASSET_KEY" "filename")

if [ -n "\$CLEAN_REQ_VER" ] && [ "\$CLEAN_REQ_VER" != "\$MANIFEST_VERSION" ]; then
  TARGET_VERSION="\$CLEAN_REQ_VER"
  if [ "\$OS" = "macos" ]; then
    ASSET_FILENAME="ruum-\${TARGET_VERSION}-\${ARCH}-macos.zip"
  else
    ASSET_FILENAME="ruum-\${TARGET_VERSION}-\${ARCH}-linux.tar.gz"
  fi
  ASSET_PATH="releases/v\${TARGET_VERSION}/\${ASSET_FILENAME}"
  FALLBACK_URL="https://github.com/\${GITHUB_REPO}/releases/download/v\${TARGET_VERSION}/\${ASSET_FILENAME}"
  SHA_TEXT=\$(fetch_text "\${BASE_URL}/v1/download/\${ASSET_PATH}.sha256" 2>/dev/null || fetch_text "\${FALLBACK_URL}.sha256" 2>/dev/null || true)
  EXPECTED_SHA256=\$(printf '%s' "\$SHA_TEXT" | awk 'NR==1 {print tolower(\$1)}')
else
  TARGET_VERSION="\$MANIFEST_VERSION"
fi

if [ -z "\$ASSET_PATH" ] || [ -z "\$EXPECTED_SHA256" ]; then
  echo "error: release asset '\$ASSET_KEY' not found in manifest" >&2
  exit 1
fi

PRIMARY_URL="\${BASE_URL}/v1/download/\${ASSET_PATH}"
ARCHIVE_NAME=\${ASSET_FILENAME:-\$(basename "\$ASSET_PATH")}
ARCHIVE_FILE="\$TMPDIR_INSTALL/\$ARCHIVE_NAME"

echo "Downloading ruum \${TARGET_VERSION} (\${OS}/\${ARCH})..."
if ! download_file "\$PRIMARY_URL" "\$ARCHIVE_FILE"; then
  if [ -n "\$FALLBACK_URL" ]; then
    echo "Primary download failed, falling back to GitHub Releases..."
    download_file "\$FALLBACK_URL" "\$ARCHIVE_FILE"
  else
    echo "error: download failed from \$PRIMARY_URL" >&2
    exit 1
  fi
fi

ACTUAL_SHA256=\$(sha256_of "\$ARCHIVE_FILE")
if [ "\$ACTUAL_SHA256" != "\$EXPECTED_SHA256" ]; then
  echo "error: sha256 mismatch for \$ARCHIVE_NAME (expected \$EXPECTED_SHA256, got \$ACTUAL_SHA256)" >&2
  exit 1
fi

EXTRACT_DIR="\$TMPDIR_INSTALL/extract"
mkdir -p "\$EXTRACT_DIR"

if [ "\$OS" = "macos" ]; then
  if command -v ditto >/dev/null 2>&1; then
    ditto -xk "\$ARCHIVE_FILE" "\$EXTRACT_DIR"
  else
    unzip -q "\$ARCHIVE_FILE" -d "\$EXTRACT_DIR"
  fi
  APP_SRC=\$(find "\$EXTRACT_DIR" -maxdepth 2 -type d -name "ruum.app" | head -n 1)
  if [ -z "\$APP_SRC" ] || [ ! -f "\$APP_SRC/Contents/MacOS/ruum" ]; then
    echo "error: invalid macOS archive layout (ruum.app missing)" >&2
    exit 1
  fi

  if [ "\$CLI_ONLY" -eq 1 ]; then
    APP_PARENT=\$(dirname "\$APP_SRC")
    mkdir -p "\$BINDIR"
    install -m 755 "\$APP_PARENT/ruum.app/Contents/MacOS/ruum" "\$BINDIR/ruum"
    if [ -f "\$APP_PARENT/ruum.app/Contents/Resources/ruum-scan-linux.pack" ]; then
      install -m 644 "\$APP_PARENT/ruum.app/Contents/Resources/ruum-scan-linux.pack" "\$BINDIR/ruum-scan-linux.pack"
    fi
    echo "Installed headless ruum CLI to \$BINDIR/ruum"
  else
    APPS_DIR="\${RUUM_APPS_DIR:-\$HOME/Applications}"
    mkdir -p "\$APPS_DIR" "\$BINDIR"
    DEST_APP="\$APPS_DIR/ruum.app"
    BACKUP_APP="\$APPS_DIR/ruum.app.bak"
    rm -rf "\$BACKUP_APP"
    if [ -d "\$DEST_APP" ]; then
      mv "\$DEST_APP" "\$BACKUP_APP"
    fi
    if ditto "\$APP_SRC" "\$DEST_APP" 2>/dev/null || cp -R "\$APP_SRC" "\$DEST_APP"; then
      rm -rf "\$BACKUP_APP"
    else
      rm -rf "\$DEST_APP"
      if [ -d "\$BACKUP_APP" ]; then
        mv "\$BACKUP_APP" "\$DEST_APP"
      fi
      echo "error: failed to install ruum.app into \$APPS_DIR" >&2
      exit 1
    fi
    ln -sf "\$APPS_DIR/ruum.app/Contents/MacOS/ruum" "\$BINDIR/ruum"
    echo "Installed ruum \${TARGET_VERSION} to \$DEST_APP and linked \$BINDIR/ruum"
  fi
else
  tar -xzf "\$ARCHIVE_FILE" -C "\$EXTRACT_DIR"
  BIN_SRC=\$(find "\$EXTRACT_DIR" -maxdepth 2 -type f -name "ruum" | head -n 1)
  if [ -z "\$BIN_SRC" ]; then
    echo "error: invalid Linux archive layout (ruum binary missing)" >&2
    exit 1
  fi
  SRC_DIR=\$(dirname "\$BIN_SRC")

  if [ "\$CLI_ONLY" -eq 1 ]; then
    mkdir -p "\$BINDIR"
    install -m 755 "\$SRC_DIR/ruum" "\$BINDIR/ruum"
    echo "Installed headless ruum CLI to \$BINDIR/ruum"
  elif [ -f "\$SRC_DIR/install.sh" ]; then
    PREFIX="\$PREFIX" BINDIR="\$BINDIR" sh "\$SRC_DIR/install.sh"
  else
    mkdir -p "\$BINDIR"
    install -m 755 "\$SRC_DIR/ruum" "\$BINDIR/ruum"
    if [ -f "\$SRC_DIR/ruum-gui" ]; then
      install -m 755 "\$SRC_DIR/ruum-gui" "\$BINDIR/ruum-gui"
    fi
    echo "Installed ruum \${TARGET_VERSION} to \$BINDIR"
  fi
fi

case ":\${PATH:-}:" in
  *:"\$BINDIR":*) ;;
  *)
    echo "Note: add \$BINDIR to PATH, e.g.: export PATH=\\"\$BINDIR:\\\$PATH\\""
    ;;
esac

if [ "\$YES" -eq 1 ]; then
  true
fi
`;
}

export function renderInstallPs1(origin, githubRepo) {
  return `# Ruum installer for Windows (PowerShell)
# Usage: install.ps1 [-Channel stable|beta] [-Version <V>] [-InstallDir <DIR>]
param(
  [string]$Channel = $(if ($env:RUUM_CHANNEL) { $env:RUUM_CHANNEL } else { "stable" }),
  [string]$Version = $(if ($env:RUUM_VERSION) { $env:RUUM_VERSION } else { "" }),
  [string]$InstallDir = $(if ($env:RUUM_INSTALL_DIR) { $env:RUUM_INSTALL_DIR } else { "" })
)

$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.ServicePointManager]::SecurityProtocol

if ($Channel -notin @("stable", "beta")) {
  throw "Invalid channel '$Channel'. Expected 'stable' or 'beta'."
}

$BaseUrl = if ($env:RUUM_UPDATE_BASE_URL) {
  $env:RUUM_UPDATE_BASE_URL.TrimEnd("/")
} else {
  "${origin}"
}
$GithubRepo = "${githubRepo}"

$Arch = $null
try {
  $osArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
  if ($osArch -eq "Arm64") { $Arch = "aarch64" }
  elseif ($osArch -eq "X64") { $Arch = "x86_64" }
} catch {}
if (-not $Arch) {
  $procArch = ($env:PROCESSOR_ARCHITECTURE + "").ToUpperInvariant()
  if ($procArch -eq "ARM64") { $Arch = "aarch64" }
  elseif ($procArch -eq "AMD64") { $Arch = "x86_64" }
  else { throw "Unsupported Windows architecture: $procArch" }
}

$TargetDir = if (-not [string]::IsNullOrWhiteSpace($InstallDir)) {
  $InstallDir
} else {
  if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
    throw "LOCALAPPDATA is not set; specify -InstallDir"
  }
  Join-Path $env:LOCALAPPDATA "Programs\\ruum\\bin"
}

$CheckUrl = "$BaseUrl/v1/check?channel=$Channel&os=windows&arch=$Arch&pkg=portable_zip&ver=$Version&entry=install_ps1"
$Manifest = Invoke-RestMethod -Uri $CheckUrl -UseBasicParsing
$AssetKey = "windows-$Arch-portable_zip"

$CleanReqVer = $Version.TrimStart("v")
if (-not $Manifest -or [string]::IsNullOrWhiteSpace([string]$Manifest)) {
  if ([string]::IsNullOrWhiteSpace($CleanReqVer)) {
    throw "no release published yet for channel '$Channel'"
  }
  $Manifest = [pscustomobject]@{ version = ""; assets = [pscustomobject]@{} }
}
$TargetVersion = $Manifest.version
$AssetPath = $null
$FallbackUrl = $null
$ExpectedSha256 = $null

if (-not [string]::IsNullOrWhiteSpace($CleanReqVer) -and ($CleanReqVer -ne $Manifest.version)) {
  $TargetVersion = $CleanReqVer
  $ZipName = "ruum-$TargetVersion-$Arch-windows.zip"
  $AssetPath = "releases/v$TargetVersion/$ZipName"
  $FallbackUrl = "https://github.com/$GithubRepo/releases/download/v$TargetVersion/$ZipName"
  try {
    $shaRaw = Invoke-RestMethod -Uri "$BaseUrl/v1/download/$AssetPath.sha256" -UseBasicParsing
  } catch {
    $shaRaw = Invoke-RestMethod -Uri "$FallbackUrl.sha256" -UseBasicParsing
  }
  $ExpectedSha256 = (($shaRaw -split "\\s+")[0]).Trim().ToLowerInvariant()
} else {
  $Asset = $Manifest.assets.$AssetKey
  if (-not $Asset) {
    throw "Release asset '$AssetKey' not found in manifest."
  }
  $AssetPath = $Asset.path
  $FallbackUrl = $Asset.fallback_url
  $ExpectedSha256 = ([string]$Asset.sha256).Trim().ToLowerInvariant()
}

if ([string]::IsNullOrWhiteSpace($AssetPath) -or [string]::IsNullOrWhiteSpace($ExpectedSha256)) {
  throw "Missing asset path or sha256 for '$AssetKey'."
}

$TempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("ruum-install-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $TempRoot | Out-Null

try {
  $ZipPath = Join-Path $TempRoot "ruum.zip"
  $PrimaryUrl = "$BaseUrl/v1/download/$AssetPath"
  Write-Host "Downloading ruum $TargetVersion (windows/$Arch)..."
  try {
    Invoke-WebRequest -Uri $PrimaryUrl -OutFile $ZipPath -UseBasicParsing
  } catch {
    if (-not [string]::IsNullOrWhiteSpace($FallbackUrl)) {
      Write-Host "Primary download failed, falling back to GitHub Releases..."
      Invoke-WebRequest -Uri $FallbackUrl -OutFile $ZipPath -UseBasicParsing
    } else {
      throw
    }
  }

  $ActualSha256 = (Get-FileHash -LiteralPath $ZipPath -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($ActualSha256 -ne $ExpectedSha256) {
    throw "SHA256 mismatch (expected $ExpectedSha256, got $ActualSha256)."
  }

  $ExtractDir = Join-Path $TempRoot "extract"
  Expand-Archive -LiteralPath $ZipPath -DestinationPath $ExtractDir -Force

  # Release archives wrap files inside ruum-$VERSION-$ARCH-windows/ subdirectory.
  $RuumExe = Get-ChildItem -Path $ExtractDir -Filter "ruum.exe" -Recurse -File | Select-Object -First 1
  if (-not $RuumExe) {
    throw "Invalid archive layout: ruum.exe not found."
  }
  $SourceDir = $RuumExe.DirectoryName

  New-Item -ItemType Directory -Force -Path $TargetDir | Out-Null
  foreach ($FileName in @("ruum.exe", "ruum-gui.exe", "ruum-scan-linux.pack")) {
    $SrcFile = Join-Path $SourceDir $FileName
    if (Test-Path -LiteralPath $SrcFile -PathType Leaf) {
      Copy-Item -LiteralPath $SrcFile -Destination (Join-Path $TargetDir $FileName) -Force
    }
  }

  $NormTarget = $TargetDir.Replace("/", "\\").TrimEnd("\\")
  $CurrentUserPath = [Environment]::GetEnvironmentVariable("Path", "User")
  $Parts = @()
  if (-not [string]::IsNullOrWhiteSpace($CurrentUserPath)) {
    $Parts = @($CurrentUserPath -split ";" | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
  }
  $AlreadyOnPath = $false
  foreach ($Part in $Parts) {
    if ($Part.Trim().Trim('"').Replace("/", "\\").TrimEnd("\\") -ieq $NormTarget) {
      $AlreadyOnPath = $true
      break
    }
  }
  if (-not $AlreadyOnPath) {
    $UpdatedPath = ($Parts + @($TargetDir)) -join ";"
    [Environment]::SetEnvironmentVariable("Path", $UpdatedPath, "User")
    $env:Path = "$TargetDir;$env:Path"
    Write-Host "Added $TargetDir to User Path."
  }

  Write-Host "Installed ruum $TargetVersion to $TargetDir"
} finally {
  if (Test-Path -LiteralPath $TempRoot) {
    Remove-Item -Recurse -Force -LiteralPath $TempRoot -ErrorAction SilentlyContinue
  }
}
`;
}

async function handleCheck(request, url, env, ctx) {
  const rawChannel = url.searchParams.get("channel") ?? "stable";
  if (!VALID_CHANNELS.has(rawChannel)) {
    return jsonResponse({ error: "invalid channel" }, 400);
  }
  const channel = rawChannel;

  const telemetry = {
    event: "update_check",
    channel,
    os: sanitizeField(url.searchParams.get("os"), 32),
    arch: sanitizeField(url.searchParams.get("arch"), 32),
    pkg: sanitizeField(url.searchParams.get("pkg"), 32),
    ver: sanitizeField(url.searchParams.get("ver"), 32),
    lang: sanitizeField(url.searchParams.get("lang"), 16),
    entry: sanitizeField(url.searchParams.get("entry"), 32),
    client_id: sanitizeField(request.headers.get("X-Ruum-Client-Id"), 64),
    country: sanitizeCountry(request),
  };

  const cache = globalThis.caches?.default ?? null;
  const cacheKey = new Request(
    `https://cache.ruum.internal/v1/manifest/${channel}`,
    { method: "GET" }
  );

  if (cache) {
    const cached = await cache.match(cacheKey);
    if (cached) {
      logEvent({ ...telemetry, cache_hit: true });
      return cached;
    }
  }

  const bucket = env?.RELEASES_BUCKET;
  if (!bucket) {
    return jsonResponse({ error: "releases bucket not configured" }, 500);
  }

  const manifestKey = `channels/${channel}/manifest.json`;
  const sigKey = `channels/${channel}/manifest.json.sig`;
  const [manifestObj, sigObj] = await Promise.all([
    bucket.get(manifestKey),
    bucket.get(sigKey),
  ]);

  if (!manifestObj && !sigObj) {
    logEvent({ ...telemetry, cache_hit: false, no_release: true });
    return new Response(null, {
      status: 204,
      headers: {
        "X-Ruum-Channel": channel,
        "Cache-Control": "public, max-age=60",
      },
    });
  }

  if (!manifestObj || !sigObj) {
    return jsonResponse({ error: "incomplete manifest publication" }, 502);
  }

  // Raw bytes passthrough: never parse & re-serialize manifest.json so Ed25519 signature stays valid.
  const manifestBytes = await manifestObj.arrayBuffer();
  const signature = (await sigObj.text()).trim();
  if (!signature || /[\r\n]/.test(signature)) {
    return jsonResponse({ error: "invalid manifest signature" }, 502);
  }

  const response = new Response(manifestBytes, {
    status: 200,
    headers: {
      "Content-Type": "application/json; charset=utf-8",
      "Cache-Control": "public, max-age=300",
      "X-Ruum-Signature": signature,
      "X-Ruum-Channel": channel,
    },
  });

  if (cache) {
    const putPromise = cache.put(cacheKey, response.clone());
    if (ctx && typeof ctx.waitUntil === "function") {
      ctx.waitUntil(putPromise);
    } else {
      await putPromise;
    }
  }

  logEvent({ ...telemetry, cache_hit: false });
  return response;
}

async function handleDownload(request, url, env) {
  const rawPath = url.pathname.slice("/v1/download/".length);
  const objectKey = validateDownloadPath(rawPath);
  if (!objectKey) {
    return jsonResponse({ error: "invalid download path" }, 400);
  }

  const bucket = env?.RELEASES_BUCKET;
  if (!bucket) {
    return jsonResponse({ error: "releases bucket not configured" }, 500);
  }

  const rangeHeader = request.headers.get("Range");
  const ifNoneMatch = request.headers.get("If-None-Match");
  const getOptions = {};
  if (rangeHeader) getOptions.range = request.headers;
  if (ifNoneMatch) getOptions.onlyIf = request.headers;

  const object =
    Object.keys(getOptions).length > 0
      ? await bucket.get(objectKey, getOptions)
      : await bucket.get(objectKey);

  if (!object) {
    return jsonResponse({ error: "asset not found" }, 404);
  }

  const etag = object.httpEtag || (object.etag ? `"${object.etag}"` : null);
  const filename = objectKey.split("/").pop() || "artifact";
  const country = sanitizeCountry(request);

  if (!object.body) {
    const headers = new Headers({
      "Cache-Control": "public, max-age=31536000, immutable",
    });
    if (etag) headers.set("ETag", etag);
    logEvent({
      event: "asset_download",
      path: objectKey,
      filename,
      status: 304,
      range: Boolean(rangeHeader),
      country,
    });
    return new Response(null, { status: 304, headers });
  }

  const headers = new Headers({
    "Content-Type": object.httpMetadata?.contentType || guessContentType(filename),
    "Cache-Control": "public, max-age=31536000, immutable",
    "Content-Disposition": `attachment; filename="${filename}"`,
    "Accept-Ranges": "bytes",
  });
  if (etag) headers.set("ETag", etag);

  let status = 200;
  if (rangeHeader && object.range && typeof object.size === "number") {
    status = 206;
    const { offset, length } = object.range;
    headers.set("Content-Range", `bytes ${offset}-${offset + length - 1}/${object.size}`);
    headers.set("Content-Length", String(length));
  } else if (typeof object.size === "number") {
    headers.set("Content-Length", String(object.size));
  }

  logEvent({
    event: "asset_download",
    path: objectKey,
    filename,
    status,
    range: Boolean(rangeHeader),
    country,
  });

  return new Response(request.method === "HEAD" ? null : object.body, {
    status,
    headers,
  });
}

export default {
  async fetch(request, env = {}, ctx = null) {
    if (request.method !== "GET" && request.method !== "HEAD") {
      return jsonResponse({ error: "method not allowed" }, 405);
    }

    const url = new URL(request.url);
    const { pathname } = url;

    if (pathname === "/health") {
      return jsonResponse({ ok: true, service: "ruum-update-worker" }, 200);
    }

    if (pathname === "/v1/check") {
      return handleCheck(request, url, env, ctx);
    }

    if (pathname.startsWith("/v1/download/")) {
      return handleDownload(request, url, env);
    }

    if (pathname === "/install.sh") {
      const origin = safeOrigin(url, env);
      const repo = safeGithubRepo(env);
      logEvent({
        event: "install_script",
        target: "posix",
        country: sanitizeCountry(request),
      });
      return new Response(renderInstallSh(origin, repo), {
        status: 200,
        headers: {
          "Content-Type": "text/x-shellscript; charset=utf-8",
          "Cache-Control": "no-store",
        },
      });
    }

    if (pathname === "/install.ps1") {
      const origin = safeOrigin(url, env);
      const repo = safeGithubRepo(env);
      logEvent({
        event: "install_script",
        target: "windows",
        country: sanitizeCountry(request),
      });
      return new Response(renderInstallPs1(origin, repo), {
        status: 200,
        headers: {
          "Content-Type": "text/plain; charset=utf-8",
          "Cache-Control": "no-store",
        },
      });
    }

    return jsonResponse({ error: "not found" }, 404);
  },
};
