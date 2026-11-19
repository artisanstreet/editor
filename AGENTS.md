# Engineering policy

These rules apply to every change in this repository, whoever makes it.

## Errors

Every error belongs to exactly one of two classes.

1. **Expected failures are typed and recoverable.**
   - One `thiserror` enum per operation (`LoadThreadError`, `PairError`), not one catch-all per crate.
   - Each variant names the step that failed, carries the ids involved (thread, run, host, path) as fields, and keeps the underlying error as `#[source]`. Logging the error prints the whole chain; nobody should ever need to add logging afterwards to learn what happened.
   - Callers decide what to do by matching variants (retry, show the user, give up), never by inspecting strings.
   - Errors that cross the Forge HTTP boundary are serde-tagged enums, so the Editor receives typed failures.
   - `anyhow` is not used; `deny.toml` bans it.
2. **Illegal states are unrepresentable.**
   - Model state as enums whose variants carry only the data valid in that state. No status field beside loose `Option`s and flags, no sentinels, no public fields that carry invariants, no stringly-typed kinds or ids, no raw JSON past the boundary that received it.
   - Normalize messy input into these types where it enters (HTTP, engine output, files, settings) and nowhere else.
   - Where an invariant can still break, crash immediately with `expect("invariant: …")` or `unreachable!("…")`, naming the invariant and the ids. Do not bail out with an error for an impossible case, and do not silently drop work with `let … else { return }`.

Never collapse a failure with `.ok()`, `unwrap_or_default()`, `map_err(|_| …)`, `let _ =` or a `_ =>` arm that swallows variants. Never use bare `unwrap()` outside tests. Logs carry ids, counters and typed error text, never message bodies, provider payloads or credentials.

## Code

- No comments, apart from license headers and tool directives. If code needs explaining, rename or restructure it.
- Prefer pure functions and explicit state machines.
- SQL lives in `.sql` files and selects only the columns and rows the caller needs. Queries run through rusqlite.
- No one-off scripts, scratch files or helpers nobody asked for.
- Files are snake_case modules under `src/`, organised by folders, with no name prefixes that repeat the folder.

## Tests

- Test non-trivial behaviour only: parsing, data transformation, state machines, end-to-end flows.
- Every test must fail for a reason a user could see. No tests of constants, design values, markup, derives or a single struct, and no smoke tests.
- Tests live in `tests/`, mirroring the path in `src/`. Shared helpers live in the test support crate, not in production code.
- No sleeps and no unbounded polling: use bounded waits and an injected or paused clock.
- Every fixed bug gets a regression test at the highest layer that would have caught it.

## Working in this checkout

- There is one checkout and no worktrees or clones. Parallel agents work on disjoint files in it; the coordinating agent makes the commits and pull requests.
- Before editing, publish your work and claim your files through the Artisan coordination tools, and check for overlapping claims.
- Builds stall the disks while the Editor is in use. Check `/proc/pressure/io` (`some avg10` under 20) before building, build one crate at a time with `-j2`, never `--workspace`, and use `CARGO_TARGET_DIR=target/integration`. At most two agents build at once.
- One logical change per commit and per pull request. Pull requests land only on green CI.
