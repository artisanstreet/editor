# Bounded File Store Native

This private N-API package owns the production native adapter for Artisan's
`BoundedRegularFileStore`. It exposes Windows-only pinned-root bounded reads,
authenticated conditional replacement, and idempotent receipt finalization.
The Effect adapter and backend composition remain separate slices.

`NativeBoundedRegularFileStore` accepts an absolute path on a fixed local NTFS
volume, pins that exact root directory handle, and opens every child component
relative to the preceding directory handle. Reads reject reparse points,
reserved Artisan artifacts (including normalized 8.3 aliases), multiply linked
files, concurrent writers, and concurrent path deletion or replacement. The
single-link rule keeps conditional replacement semantics unambiguous and
prevents a private stage or backup from being read through a hard-link alias.

Conditional replacement verifies the expected bytes through exact handles,
creates a private same-directory stage with the target's ordinary attributes
and DACL, and publishes through no-overwrite rename and hard-link operations.
Root-derived HMAC-SHA256 NTFS EA receipts bind the operation, expected and
replacement digests, file identities, and recovery role. The exact stage and
original backup remain available until explicit finalization, so exact retries
can recover every tested process-crash window and fail closed for wrong keys,
corrupt receipts, replayed artifacts, external replacements, and ambiguous
state. This contract targets fixed local NTFS volumes and process crashes; it
does not claim power-loss atomicity or universal filesystem support.

The production build targets `x86_64-pc-windows-msvc`. Build output belongs in
`.dist/bounded-file-store-native`, outside the source package and, later, outside
Electron's ASAR archive.

## Local Verification

Run the complete native gate from the repository root:

```powershell
$env:ARTISAN_RUN_NATIVE_ADDON_SMOKE = "1"
pnpm --filter @artisan/bounded-file-store-native verify:local
```

The command builds isolated production and test-hook GNU release outputs, then
verifies direct and generated-loader loading, declarations, pinned-root reads,
authenticated replacement and finalization, competing operations, deterministic
races, and every process-crash recovery window. It requires Rust
`1.97.0-x86_64-pc-windows-gnu` and the user-scoped
`BrechtSanders.WinLibs.POSIX.UCRT` winget package.

`pnpm run build:local:gnu` is the lower-level single-build command. Both commands
require the explicit `ARTISAN_RUN_NATIVE_ADDON_SMOKE=1` opt-in so ordinary
validation never loads the addon silently.

NAPI-RS resolves Node-API symbols dynamically from the running `node.exe`, but
its GNU build scripts still require `LIBNODE_PATH` and `-lnode`. The local script
therefore creates an ignored DLL/import-library placeholder that exports no
Node-API symbols. Successful direct and generated-loader smoke tests prove the
addon resolves its real Node-API symbols from the host process instead.

GNU output is isolated under `.dist/bounded-file-store-native-gnu*` and is a
local verification artifact, not the packaged binary. Production MSVC loading
and Electron packaging still require their own release-platform check.
