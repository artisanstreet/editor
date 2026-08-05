# Stashed: skill tracking

**Status:** stashed 2026-08-05. Removed whole — UI, projection, protocol, and
engine adapter. Revisit rather than rebuild.

## Why it was pulled

The inspector panel listed the skills a thread had loaded, counted per skill and
ordered by most recent use. The card never rendered usable results. Rather than
leave a broken surface in the panel over a pipeline nothing else consumed, the
whole feature came out together.

Nothing about the idea is wrong. The failure was in the frontend read path; the
engine and projection halves were never implicated and are preserved intact in
the patch below.

## Restoring it

`restore-thread-skills.patch.txt` reverses the entire removal across all 23
files. From the repository root:

```sh
patch -p1 --directory=. -i docs/prds/stashed/thread-skills/restore-thread-skills.patch.txt
```

The patch was generated as `diff -ruN after before`, so applying it forward puts
the feature back. It was taken against the working tree at removal time, so
expect conflicts in files that have moved on since — the skill hunks themselves
are self-contained and read cleanly in isolation.

### What the patch contains

- **Protocol** — `engine-skills.ts` (`EngineSkillDescriptor`, `EngineSkillReport`,
  `EngineSkillQuery`), the `engine.skill.query` envelopes in
  `control-contract/inspection.ts`, the wire and `control-rpc` registrations, and
  the `index.ts` re-export.
- **Engines** — `EngineSkillObservation`, `EngineSkillCatalog`,
  `EngineSkillSource`, `EngineSkillDescriptor`, and the optional `Engine.Skills`
  member in `engine.ts`; `claude/skills.ts`, which reads the on-disk catalog; the
  `Skill` tool-call branch in `claude/normalizer.ts`; and the adapter wiring in
  `claude/engine.ts`.
- **Backend** — the `skill` case in `conversation/projection/activity.ts` and its
  dispatch in `observation.ts`, the `capability` mapping in
  `surfaces/engine-observation.ts`, the `engine-skills` query handler, and its
  registration in `protocol/server.ts` and `rpc/ready-dispatch.ts`.
- **Transport** — `GetEngineSkills` on the client service, `get_engine_skills` in
  `internal/api/queries.ts`, and the `client-service.ts` binding.
- **Frontend** — the fixture responses in `runtime/fixtures/`.
- **Tests** — `.tests/engines/claude-skills.test.ts` and
  `.tests/backend/conversation-skill-projection.test.ts`.

## Not in the patch — lost, and recoverable only as compiled output

Two files were deleted with `rm` before the snapshot was taken, and both were
untracked, so they are in neither git nor the patch:

- `modules/frontend/src/lib/skills/thread-skills.ts` — `ThreadSkillStore`.
- `modules/frontend/src/routes/components/panel/thread-skills-card.sv` — the card.
- `.tests/frontend/thread-skills-card.test.ts`.

What survives is the compiled output of the last build before removal:

- `thread-skills.store.compiled.js.txt` — the whole store, minified but complete.
  The tally is legible in it: group `activity` items of kind `skill` by `label`,
  count invocations, track `last_ordinal` and the set of `turn_id`s, then sort by
  `last_ordinal` descending and name ascending. That is the entire algorithm.
- `thread-panel-node.compiled.js.txt` — the built panel bundle; the card's markup
  is inside it, minified.

The store is short enough to reconstruct from the compiled form in minutes. The
card's markup is recoverable but unpleasant to read; rewriting it against the
existing panel card conventions is likely faster than extracting it.

## When it is picked up again

Start from the projection rather than the UI. Once the patch is applied, `skill`
activity items carry name, turn, and ordinal on the conversation snapshot
directly, so a card can be written against that and may not need a store at all —
which is also where the original went wrong. Find what broke that read path
before rebuilding on the same shape.
