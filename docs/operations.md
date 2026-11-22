# Operations

Runbooks for an installed Artisan: Forge, the Editor and `ae`.

## Where things live

Everything sits under one install root:

| Platform | Root | App folders |
| --- | --- | --- |
| Linux and WSL | `$XDG_DATA_HOME/artisan-street` (usually `~/.local/share/artisan-street`) | `ae`, `forge`, `editor` |
| Windows | `%LOCALAPPDATA%\Artisan Street` | `ae`, `Forge`, `Editor` |
| macOS | `~/Library/Application Support/Artisan Street` | `ae`, `Forge`, `Editor` |

A root chosen with `ae install --location` is remembered in a `location` file inside the default root. Each app keeps its binaries and a `version` file in `<root>/<app>/` and its state in `<root>/<app>/data/`.

| What | Where |
| --- | --- |
| Forge database | `<root>/forge/data/forge.sqlite3` (plus `-wal` and `-shm` while open) |
| Pre-migration backup | `<root>/forge/data/forge.sqlite3.schema-<old schema>` |
| Forge log | `journalctl --user -u artisan-forge.service` under systemd, otherwise `<root>/forge/data/forge.log` (16 MiB, 4 rotated files) |
| Editor log | `<root>/editor/data/logs/editor.log` (8 MiB, 6 rotated files) |
| Crash reports | `<root>/<app>/data/crashes/<app>-<unix ms>-<pid>.txt` |
| Engine traces | `<root>/forge/data/traces/<engine>-<thread>-<unix ms>.jsonl` (newest 256 MiB, at most 7 days old) |
| Install lock | `<root>/.install.lock` |

`ae status` shows what is installed, whether Forge runs and answers its health check, where its log is, and every problem it found. `ae status --json` prints the same for scripts.

## Permission modes

Forge doesn't confine engines itself; in Auto each engine's own harness does, except claude.ai, whose tools run inside Forge. In Auto, claude.ai asks before anything only a project's own settings would allow: allow rules and `acceptEdits` in a project's `.claude/settings.json` or `.claude/settings.local.json` never skip approval, while the engine's own Claude settings still can, and a project's ask and deny rules still apply. Claude runs in its auto permission mode, Codex runs in its `workspace-write` sandbox with its reviewer deciding what else may run, and OpenCode may not read or write anything outside its worktree except the folders Forge hands it, and asks before every shell command. Full access runs every engine without limits.

Where Forge runs in a delegated cgroup, every Auto launch gets its own cgroup, and Forge kills everything in it, including processes detached with `setsid`, when that launch ends, which is normally the end of the run. A Codex Auto app-server is shared by the runs that use it and outlives them: its cgroup is killed when the server exits, which is once it has no runs and every thread on it has been idle for 30 minutes. Processes a Full-access run detaches, such as a dev server, outlive the run. Agents, new threads and messages started through Artisan's tools never get more access than the stricter of the thread's mode and the mode its running engine was launched in.

## Forge is unhealthy

`ae status` asks the running Forge gateway's `/health` endpoint (on Linux and WSL), which checks that the serving backend speaks the gateway's protocol and database schema. An unhealthy Forge is reported as `Forge isn't healthy: <cause>; see <log>`, naming the step that failed: no gateway record, no answer, a refused token, the backend's own reason, or an unexpected status.

1. Read the cause and the log `ae status` points at.
2. `ae forge restart`.
3. If it stays unhealthy after a restart, treat it as a failed update or a damaged database below.

## A failed update

`ae update` and `ae install` hold the install lock, so a second installer waits for the first. An install unpacks the release into `<app>/.incoming-*`, moves each current file aside to `<app>/.old-<n>-<name>`, then renames the new file into place. If any rename fails, the new files are removed and the old ones renamed back, and the error says so; if even restoring fails, the error says that too and the `.old-*` files are still there to rename back by hand. Once every file is in place the old ones are deleted, and Forge is started; the `version` file is written last.

- **Failed before the swap** (download, unpack, stopping Forge, closing the Editor): nothing changed. Fix the cause and run `ae update` again.
- **Forge fails to start after the swap**: the error ends with Forge's last 15 log lines. The new binaries are in place but `version` still names the old release, so `ae update` retries the whole install. If the new release itself is broken, roll back (below).
- **Leftovers**: `.incoming-*` and `.old-*` files from an interrupted install are removed by the next install.

Under systemd, an upgrade whose database schema is unchanged is handed to the running Forge online: running turns finish on the old generation and new turns use the new one. If the schema changes, Forge restarts.

## Rolling back a release

```sh
ae install --version <older version>
```

This installs that exact release from the feed. `ae update` only moves forward, and an install from a local build has to be reinstalled from the feed first.

If the newer release migrated the database, Forge wrote `forge.sqlite3.schema-<old schema>` before migrating (one backup is kept; older ones are deleted). When the older Forge starts and finds a database newer than it supports, it:

1. copies the newer database aside as `forge.sqlite3.rolled-back-from-<newer schema>-<n>`,
2. renames the backup over `forge.sqlite3`,
3. logs `rolled the database back to the schema this Forge supports; newer data was set aside`.

Threads and settings changed after the upgrade stay in the set-aside copy; they are not merged back. If there is no backup, Forge refuses to start and says where it looked. If it reports the database is still open in another process, stop every Forge (`ae forge stop`) and start again.

## A damaged database

Forge does not check the database's integrity itself; corruption shows up as errors opening or reading `forge.sqlite3` in the Forge log or `ae status`.

1. `ae forge stop`, and confirm with `ae status` that Forge is stopped.
2. Copy the whole `<root>/forge/data` folder somewhere safe before touching anything.
3. Check it: `sqlite3 forge.sqlite3 'PRAGMA integrity_check;'` (with the `-wal` file next to it, so committed transactions are included).
4. If it reports problems, recover what SQLite can read: `sqlite3 forge.sqlite3 '.recover' | sqlite3 recovered.sqlite3`, check `recovered.sqlite3` the same way, then move the damaged files (`forge.sqlite3`, `-wal`, `-shm`) aside and rename `recovered.sqlite3` to `forge.sqlite3`.
5. If recovery fails and a `forge.sqlite3.schema-<n>` backup exists for the current schema, renaming it over `forge.sqlite3` restores the state from before the last upgrade.
6. As a last resort, move the database files aside and start Forge with an empty database: threads, projects and paired devices are lost, and Editors have to pair again (`ae pair`).
7. `ae forge start`, then `ae status`.

## Crash reports and telemetry

When the Editor, Forge or `ae` hits a broken invariant, it writes a crash report before aborting: the message, source location, span trace, backtrace and the last 200 log lines. The report's path is printed to stderr. Reports are plain local files under `<root>/<app>/data/crashes/`. Nothing collects or uploads them, and Artisan sends no telemetry. To report a crash, attach the file by hand. Reports are never deleted automatically; remove old ones when they are no longer needed.

Forge also writes one engine trace per engine process to `<root>/forge/data/traces/<engine>-<thread>-<unix ms>.jsonl`: the raw frames Forge sent (`in`) and the engine wrote (`out`), its stderr, notes such as why Forge closed the engine's input, `dropped` and `truncated_from` markers where a trace hit its 64 MiB or 256 KiB-per-record limit, and an `end` record once the process is gone. Traces are private files, and launch credentials (MCP URLs, headers and environment secrets, the Artisan gateway token) are replaced with `[redacted]`; prompts, replies and tool output are kept as the engine saw them. Forge keeps the newest traces up to 256 MiB in total and deletes traces older than 7 days, when a trace starts and when Forge starts. Attach the traces for the affected thread to a bug report by hand; nothing uploads them.

Release builds keep split debug symbols (`.dwp`, `.dSYM`, `.pdb`). They are uploaded as `symbols-<os>-<arch>` artifacts of the release workflow run and kept for 90 days, which bounds how long a release's backtraces can be symbolized.

## CI artifacts and caches

- Release workflow artifacts: the per-platform `release-*` builds and `release-complete-unsigned` are kept for 7 days, since the published release holds the final assets; `symbols-*` are kept for 90 days.
- CI build caches (`cargo-<job>-<os>-<arch>-<lockfile hash>-<commit>`) are saved only on pushes to master. After saving, the job deletes its older caches, so each job keeps one.
- Release build caches (`release-<os>-<arch>-<target>-<lockfile hash>`) change only with the lockfile; GitHub evicts them after 7 days without use.
- The sccache bucket (`artisan-sccache`, prefix `rust`) is outside GitHub; bound it with a lifecycle rule on the bucket.
