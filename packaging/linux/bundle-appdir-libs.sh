#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
  echo "usage: $0 <appdir> [<excludelist>]" >&2
  exit 2
fi

appdir=$1
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
excludelist=${2:-"$script_dir/appimage-excludelist"}

if [ ! -d "$appdir" ]; then
  echo "AppDir does not exist: $appdir" >&2
  exit 1
fi
if [ ! -f "$excludelist" ]; then
  echo "excludelist file not found: $excludelist" >&2
  exit 1
fi

ruum_bin="$appdir/usr/bin/ruum"
ruum_gui_bin="$appdir/usr/bin/ruum-gui"
if [ ! -f "$ruum_gui_bin" ]; then
  echo "missing required binary: $ruum_gui_bin" >&2
  exit 1
fi

lib_dir="$appdir/usr/lib"
mkdir -p "$lib_dir"

excluded_set=$'\n'
while IFS= read -r raw_line || [ -n "$raw_line" ]; do
  entry=${raw_line%%#*}
  entry=${entry//[[:space:]]/}
  [ -z "$entry" ] && continue
  excluded_set+="$entry"$'\n'
done < "$excludelist"

is_excluded() {
  local soname=$1
  case "$excluded_set" in
    *$'\n'"$soname"$'\n'*)
      return 0
      ;;
  esac
  case "$soname" in
    linux-vdso.so.*|linux-gate.so.*|ld-linux*.so.*)
      return 0
      ;;
  esac
  return 1
}

copied_set=$'\n'
bins=("$ruum_gui_bin")
if [ -f "$ruum_bin" ]; then
  bins+=("$ruum_bin")
fi

for bin in "${bins[@]}"; do
  ldd_out=""
  if ! ldd_out=$(ldd "$bin" 2>&1); then
    # Static binaries exit non-zero from ldd ("not a dynamic executable").
    if printf '%s\n' "$ldd_out" | grep -qiE 'not a dynamic executable|statically linked'; then
      continue
    fi
    echo "ldd failed on $bin: $ldd_out" >&2
    exit 1
  fi

  if printf '%s\n' "$ldd_out" | grep -q '=> not found'; then
    echo "unresolved shared library dependency in $bin:" >&2
    printf '%s\n' "$ldd_out" | grep '=> not found' >&2
    exit 1
  fi

  while IFS= read -r line; do
    case "$line" in
      *" => "*)
        soname=${line%% => *}
        soname=${soname##*[[:space:]]}
        rhs=${line#* => }
        resolved=${rhs%% (*}
        resolved=${resolved%"${resolved##*[![:space:]]}"}
        [ -z "$soname" ] && continue
        [ -z "$resolved" ] && continue
        if is_excluded "$soname"; then
          continue
        fi
        case "$copied_set" in
          *$'\n'"$soname"$'\n'*)
            continue
            ;;
        esac
        if [ ! -e "$resolved" ]; then
          echo "resolved library path missing for $soname: $resolved" >&2
          exit 1
        fi
        cp -L "$resolved" "$lib_dir/$soname"
        chmod u+w "$lib_dir/$soname"
        copied_set+="$soname"$'\n'
        ;;
    esac
  done <<< "$ldd_out"
done

if command -v patchelf >/dev/null 2>&1; then
  shopt -s nullglob
  for so_file in "$lib_dir"/*.so*; do
    [ -f "$so_file" ] || continue
    chmod u+w "$so_file"
    patchelf --set-rpath '$ORIGIN' "$so_file"
  done
  shopt -u nullglob
  # NEVER run patchelf on usr/bin/ruum: it carries an appended ruum-scan trailer.
  patchelf --set-rpath '$ORIGIN/../lib' "$ruum_gui_bin"
fi

chmod -R a+rX "$appdir"
