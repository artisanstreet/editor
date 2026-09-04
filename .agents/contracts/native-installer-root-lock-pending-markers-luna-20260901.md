# Installer root lifecycle lock and detached-helper markers

Implement one bounded installer packet in the supplied isolated worktree.

## Base and ownership

- Exact parent: `0f318458d0db8e98766776de85ad9b5f2ef06bb4`, the locally integrated exact equivalent of published PR #390 and accepted StageLease source.
- Verify exact HEAD and clean status before editing.
- Writable paths only:
  - `modules/installer/rust/install.rs`
  - `modules/installer/rust/main.rs`
  - `modules/installer/rust/error.rs`
- Do not touch manifests, BUILD/lock files, other installer modules, packaging, CLI, native-engine, backend/frontend/UI/protocol/database/Forge, conversation-state/Statig/application-shell, docs/controller files, remote state, or foreign worktrees.
- Do not rebase, merge, publish, or spawn descendants.

## Objective

Serialize every installer lifecycle operation for one resolved installation root and fence detached stable-CLI replacement/full-root cleanup so an old helper can never overwrite or delete a later installation.

Add private `InstallerLock`/`RootMode` support using `fs2::FileExt::try_lock_exclusive()` at `root/.installer.lock`. The VP will add manifest/BUILD dependencies after accepting source; write source against the workspace APIs already present.

Acquire the lock inside every direct lifecycle entry point: install before root creation or mutation; repair, diagnose, and uninstall as their first root operation; move prepare-update ownership into `install.rs` so direct callers are protected. Do not rely only on main dispatch.

## Invariants

- Support Create and Existing root modes while safely validating absolute root/ancestors.
- Reject symlink/reparse/non-directory root/ancestors, symlink/reparse/non-file lock path, and opened-handle/path identity substitution.
- Contention returns immediately with a fixed path-free busy error. No PID ownership or wait loop.
- Retain the open handle for the whole operation; never delete or truncate the sentinel. Re-fence before stage creation, activation, destructive removal, and helper handoff.
- Preserve exact-path StageLease behavior from #390: no `.stage-*` scan, PID liveness inference, or foreign-stage deletion; disarm immediately after successful stage-to-release rename; preserve release rollback bytes thereafter.
- Use exact external sibling marker directories:
  - `<root-parent>/.<root-name>.artisan-installer-ae-replacement.pending`
  - `<root-parent>/.<root-name>.artisan-installer-cleanup.pending`
- Create markers atomically with `create_dir`. Existing marker means fixed path-free pending failure and is never removed/inspected as ownership proof.
- Detached helpers remove their own marker only after successful move/cleanup. Spawn or cleanup failure leaves it permanently fail-closed.
- `schedule_self_cleanup()` remains outside this fence because it removes only the temporary installer executable.
- Add `symlink_metadata`/reparse/type fencing to recursive owned removal; preserve foreign links and targets.
- Do not add activation power-loss recovery, stale-stage reclamation, version pruning, global custom-root arbitration, or broaden uninstall ownership.
- No global lint suppression, retries, sleeps, unsafe path-bearing diagnostics, or hidden default roots.

## Tests/checks

Inline tests must cover: second acquisition busy; drop releases OS lock but sentinel remains; unsafe root/ancestor/lock/marker shapes; identity substitution; marker collision preservation; external cleanup marker blocks root recreation; stable replacement marker clears only after successful move; cleanup marker clears only after successful target cleanup; recursive removal preserves foreign links/targets; all existing StageLease regressions remain semantically intact.

Source-only checks are authorized: pinned rustfmt for owned files, `git diff --check`, exact scope/parent/status audit. Do not run Bazel/Cargo/rustc/Clippy/tests; the VP owns the native lease and integration manifests.

Commit once, leave the tree clean, and report exact commit/parent/paths/diff, checks/tests added, unresolved activation power-loss limitation, and confirmation that no native graph was claimed.
