# Durable thread engine config — correction 15

Resume the same GPT-5.6 Luna Max session in the existing isolated worker
worktree. Preserve all earlier commits and create one new correction commit.

## Exact gate evidence

The tenth warm root-Clippy attempt at byte-identical replay head
`915c3ad191cafc2291d47e85302fcb3e47c93fbb` clears every earlier target and
reports exactly six SeaORM 2.0.2 type errors in
`tests/database/thread_engine_config.rs`: three `Value::Bytes` arguments wrap
`Vec<u8>` in an obsolete `Box`, and three `ConnectionTrait::execute` calls
pass `Statement` by value instead of by reference. An exact static scan finds
only those six obsolete shapes in the owned file.

## Exact correction

- Own exactly `tests/database/thread_engine_config.rs`.
- Apply the compiler-directed SeaORM 2.0.2 shapes at the six reported sites:
  pass each existing `Vec<u8>` directly to `Value::Bytes(Some(...))`, borrow
  each inline `Statement::from_sql_and_values`, and borrow the existing
  `update` statement when executing it.
- Preserve SQL text, bind order and values, cloning/ownership intent,
  timestamps, assertion messages, and every test semantic.
- Do not suppress lints, change runtime production code, or refactor unrelated
  fixtures.
- Do not edit manifests, BUILD files, locks, frontend, application-shell,
  conversation-state, Statig, agent files, or any other path.
- Do not amend, rebase, reset, or rewrite earlier commits.
- Run only pinned Rust 1.98 edition-2024 rustfmt/check on the owned file,
  `git diff --check`, exact one-file scope review, and the static scan proving
  the six obsolete call shapes are gone. Do not run Cargo, Bazel, rustc,
  Clippy, tests, or any native graph.
- Commit the correction, leave the worktree clean, and report exact parent,
  commit, path, checks, and remaining uncertainty.
