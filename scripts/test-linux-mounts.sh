#!/bin/sh
# Pass the compiled ruum-core unit test binary as the first argument.
# All mounts and writes live in an isolated, temporary /run.
set -eu
exec unshare --user --map-root-user --mount sh -eu -c '
    mount --make-rprivate /
    mount -t tmpfs ruum-test-run /run
    mkdir -p /run/media/ruum-test /run/ruum-scan
    mount -t tmpfs tmpfs /run/media/ruum-test
    mkdir /run/media/ruum-test/nested
    mount -t tmpfs tmpfs /run/media/ruum-test/nested
    for dir in original alias one two; do mkdir /run/ruum-scan/$dir; done
    mkdir /run/ruum-scan/original/nested
    mount --bind /run/ruum-scan/original /run/ruum-scan/alias
    for dir in one two; do mount -t tmpfs tmpfs /run/ruum-scan/$dir; done
    for dir in original one two; do printf data > /run/ruum-scan/$dir/file; done
    "$1" --exact removal::tests::mounted_media_and_repeated_views --ignored --nocapture
' sh "$1"
