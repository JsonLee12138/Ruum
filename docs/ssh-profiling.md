# SSH indexing profile

Measured on 2026-10-22, Intel Core Ultra X7 358H (16 CPUs), release builds.
All fixtures were disposable local **tmpfs** trees with warm metadata.
These are component measurements, not a profile of a6000's storage or UI.
Decode timings include tree destruction at the end of each invocation.
A read-only process check found no active helper there; no new real-server
scan was started. No caches were dropped and no limits were raised.

## Results

| Component / fixture | Before | After |
| --- | ---: | ---: |
| Decode + aggregate + classify 500,000 wire records | 288.3 ± 13.7 ms | 210.2 ± 2.2 ms |
| Stream 100,000 files, output discarded | 103.4 ± 2.7 ms | 101.2 ± 2.0 ms |
| Parallel local scan, 100,000 files, earlier paired run | 21.5 ± 0.5 ms | unchanged |
| Serial streaming, same earlier paired run | 86.8 ± 2.6 ms | — |
| Actual static helper + localhost SSH + decode, 100,000 files | — | 321–347 ms uncompressed; 316–334 ms compressed |
| Helper RSS, 70,000 external hardlinks, 20k / 40k records | scan aborted at 65,536 identities | 2,888 / 2,888 KiB; completes |

Hyperfine used warmups and 5–7 trials. SSH used five trials per mode and an
isolated sshd, temporary keys/config, the shipped static musl helper and no
user SSH settings. SSH measurements include connection/authentication but
not automatic deployment. Do not subtract these runs to claim a precise
network cost: the implementations differ and work overlaps.

### Findings

1. **Avoidable decoder work.** `perf record` showed generic
   `serde_json::Value` construction followed by a second deserialization.
   Every record also allocated a new line buffer. Direct tuple/control
   decoding and one reused line buffer reduced this fixture's decode time
   by 27% (1.37× throughput). This is not a 27% end-to-end scan improvement.
2. **Serial metadata remains the main walker limitation.** In a paired
   fixture, the existing parallel local scanner was 4.04× faster than the
   serial streaming helper. Streaming time did not materially change after
   the decoder fix. Bounded metadata concurrency is the next substantial
   scanner optimization; reverting to the full tree-building local engine
   on the server would bring back file-count-scaled memory.
3. **Compression is not the explanation on loopback.** The compressed and
   uncompressed ranges overlap. This says nothing about a slower real link.
4. **The hardlink failure was our bookkeeping policy, not memory pressure.**
   `nlink` includes names outside the scanned root. Tracking “remaining
   names” cannot drain for such files, so ordinary uv/package caches hit
   the 65,536-entry cap. Modern sessions now negotiate `client_dedup`:
   the helper emits `(device, inode)` with linked file records and keeps no
   identity table. The desktop uses the local scanner's existing `Seen`
   bitmap/sharded set. In-root aliases are charged once; external names
   need not be visited. Legacy peers retain the old bounded wire mode.

The after-profile shows tree sorting/classification, typed JSON parsing,
line reading and clock checks as remaining desktop costs. Its wide,
single-directory fixture exaggerates sorting relative to ordinary trees.
A future profile of an already-running real scan should distinguish helper
CPU, filesystem wait, SSH throughput and desktop finalization before
attributing its wall time to any one of these synthetic results.

## Reproduce safely

```sh
cargo build -p ruum-core --example profile_remote --release \
  --config profile.release.debug=1

target/release/examples/profile_remote generate /tmp/ruum-wire.jsonl
hyperfine --warmup 1 --runs 7 \
  'target/release/examples/profile_remote decode /tmp/ruum-wire.jsonl'
perf record -g --call-graph dwarf -- \
  target/release/examples/profile_remote decode /tmp/ruum-wire.jsonl
perf report
```

For an explicitly created disposable fixture under
`/tmp/ruum-profile-*`, compare `scan PATH` and `stream PATH >/dev/null`
with that example. It refuses non-temporary paths; it has no default root.
The SSH profiler uses the existing isolated test harness:

```sh
RUUM_PROFILE_ROOT=/tmp/ruum-profile-fixture \
  bun scripts/test-ssh.ts
```

That directory must already exist and contain only your disposable test
files. The harness automatically builds the portable helper and requires
the `profile_remote` example built above. Without the environment variable,
it runs the normal four SSH integration tests instead.

Memory/failure regressions against the actual helper executable:

```sh
cargo test -p ruum-scan --test stdio \
  external_hardlinks_have_bounded_resident_memory -- --ignored --nocapture
cargo test -p ruum-scan --test stdio \
  wide_stream_has_bounded_resident_memory -- --ignored --nocapture
```

Both create and clean their own temporary fixtures. The regular core suite
also verifies more than 65,536 identities, duplicate aliases, external
names, disabled de-duplication, legacy records and complete framing.
