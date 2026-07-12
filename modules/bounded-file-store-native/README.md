# Bounded File Store Native

This private N-API package owns the production native adapter for Artisan's
`BoundedRegularFileStore`. It currently exposes a Windows-only pinned-root
bounded read capability; conditional replacement, finalization, and backend
composition remain separate future slices.

`NativeBoundedRegularFileStore` accepts an absolute path on a fixed local NTFS
volume, pins that exact root directory handle, and opens every child component
relative to the preceding directory handle. Reads reject reparse points,
reserved Artisan artifacts (including normalized 8.3 aliases), multiply linked
files, concurrent writers, and concurrent path deletion or replacement. The
single-link rule keeps future conditional replacement semantics unambiguous and
prevents a private stage or backup from being read through a hard-link alias.

The production build targets `x86_64-pc-windows-msvc`. Build output belongs in
`.dist/bounded-file-store-native`, outside the source package and, later, outside
Electron's ASAR archive.

## Local GNU Build

`pnpm run build:debug:gnu` provides a locally verifiable build when the MSVC
linker is unavailable. It requires Rust `1.97.0-x86_64-pc-windows-gnu` and the
user-scoped `BrechtSanders.WinLibs.POSIX.UCRT` winget package.

NAPI-RS resolves Node-API symbols dynamically from the running `node.exe`, but
its GNU build scripts still require `LIBNODE_PATH` and `-lnode`. The local script
therefore creates an ignored DLL/import-library placeholder that exports no
Node-API symbols. Successful direct and generated-loader smoke tests prove the
addon resolves its real Node-API symbols from the host process instead.

The GNU build is a development verification path, not a production artifact.
Production MSVC loading and Electron packaging require their own platform test.
