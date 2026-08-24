# Native baseline harness (NATIVE-0001)

Measures what the operating system can observe about the current Electron +
TypeScript Forge product today, so the GPUI/Rust port has frozen numbers to
beat. Part of Phase 0 of `docs/plans/gpui-rust-native-port.md`.

Everything here is read-only observation: no code paths in the product are
changed to produce a baseline.

## Components

| File | Role |
| --- | --- |
| `Measure-ProcessTree.ps1` | Samples working set, private commit, CPU %, threads, and handles for an entire process tree; emits JSON Lines. |
| `Record-Baseline.ps1` | Launches or attaches, samples, summarizes (peaks + p95), and writes one versioned JSON artifact plus the raw samples file. |
| `scenarios.json` | Registry of the required scenarios with their automation status. |

Artifacts land in `.dist/native-baseline/` (gitignored build output) as
`<timestamp>-<scenario>.json`. The schema string is
`artisan.native.baseline/1`; bump it whenever fields change meaning.

## Usage

```powershell
# From a pwsh session (the call operator allows inline arrays):

# Self-test the harness against synthetic load:
& tools/native/baseline/Record-Baseline.ps1 -Name harness-selftest `
  -Command @('pwsh', '-NoProfile', '-Command', '$blob=[byte[]]::new(120MB); $blob[0]=1; Start-Sleep -Seconds 30') `
  -DurationSeconds 12 -IntervalSeconds 2

# Measure a running Forge daemon by pid:
& tools/native/baseline/Record-Baseline.ps1 -Name forge-idle `
  -AttachPid (Get-Process ae | Sort-Object StartTime -Descending |
              Select-Object -First 1 -ExpandProperty Id) `
  -SettleSeconds 10 -DurationSeconds 120
```

## Metric coverage matrix

The plan's Phase 0 requires more than process metrics. This table states,
honestly, what is covered now and where the rest lands.

| Required metric | Covered by this harness | Gap and owning follow-up |
| --- | --- | --- |
| Idle memory per process tree | Yes (`peakTotalWorkingSetBytes`, `peakTotalPrivateCommitBytes`, per-process peaks) | — |
| CPU while idle/loaded | Yes (`cpuPercent` samples, p95/max) | — |
| Process trees (provider/LSP/terminal helpers attributed correctly) | Yes (tree sampling with parent links) | Orphaned grandchildren drop out when an intermediate parent dies; acceptable for supervised launches |
| Cold/warm startup wall clock | Partial: artifact timestamps bracket the run | Needs a desktop-shell "first interactive frame" signal; lands with the GPUI shell instrumentation spike (Phase 1) and an Electron equivalent hook |
| Frame times, input latency | No | Requires renderer hooks; NATIVE-0006 adds the GPUI overlay/histogram; Electron needs its own `requestAnimationFrame` probe before cutover |
| Stream throughput, Forge event latency | No | Needs a protocol-level probe client speaking the real transport; tracked for the protocol oracle work (NATIVE-0002+) |
| GPU memory | No | Windows performance counters do not expose per-process VRAM reliably; requires D3D kernel-mode queries or vendor APIs; deferred to the performance program |
| Screenshots at scale factors/themes | No | Separate capture tooling during Phase 0 visual-baseline work |

## Reproducibility rules

- Record on the named Windows test machine recorded in the artifact's
  `machine` header (host, model, OS build, CPU, cores, RAM); comparisons are
  only valid within that machine.
- Every artifact records `gitRevision`; never compare baselines across
  different revisions without re-running both sides.
- Keep `settleSeconds` generous for daemon-style scenarios so startup churn
  does not pollute idle numbers.
