# Findings

Every finding from the quality review of master at `10f84a7ada6519109564cd433aed23c4e36676fa` (2026-11-17), one section per slice of `quality.md`. Each slice was reviewed by one GPT 6.1 Sol agent, read-only, with no builds or test runs. Each slice has the main review (top findings only) and a second agent's remaining findings and nits. Reviewer text is recorded verbatim; links are relative to the repository root and line numbers refer to that commit.

## Verification and disagreements

Checked against the code by the coordinating agent:

- Confirmed: `crates/ae/src/legacy.rs` removes every old Editor data directory even when migration was skipped.
- Confirmed: `crates/forge-runs/src/run.rs` takes the continuation before the new run is saved, outside one transaction.
- Confirmed: `crates/editor-core/src/core/store.rs:146` replaces unreadable settings with an empty map.
- Confirmed: `scripts/install.sh` verifies a checksum from the unsigned manifest and never fetches `release.json.sig`.
- Confirmed: the Founders Grotesk test fonts are embedded by `crates/editor-assets/src/fonts.rs`, and their notice grants no redistribution rights.
- Confirmed: `crates/editor-aqua/src/shell/root.rs` mounts `center` only for Thread and NewThread; every other screen gets an empty body.
- Confirmed: `inspector_elements` in `crates/editor-aqua/src/shell/workspace.rs` has no callers.
- Confirmed: `crates/forge-store/src/retention/threads.sql` ignores drafts when choosing inactive threads, and `retention/remove.sql` deletes the thread's draft.
- Confirmed: `crates/ae/src/account.rs` `save_account` opens `ast_account.json` with `truncate(true)` and writes in place, with no temporary file.
- Confirmed: `crates/ae/src/legacy.rs` sends `taskkill /T /F` or `kill -TERM` to whatever PID an old `forge.pid` holds, without checking the process.

Disagreements kept visible:

- Slice 6 asks for SQLx; the recorded project decision is rusqlite. Moving queries into `.sql` files still applies.
- Slice 7 corrected the testing diagnosis in `quality.md` (the layers are partly joined, schema v3 has a frozen fixture, some listed deletions are real invariants); `quality.md` has been updated.
- Slice 3 found no fsync per commit: SQLite runs `synchronous=NORMAL`, which slice 4 reports as a durability problem instead.
- Slice 3 could not verify a 512-item transcript freeze at this commit.
- Slice 9 spawned its own helper reviewers; its table includes their scores.
- Duplicates across slices: the bootstrap signature gap (8 and 9), the inert Help and Upgrade menu items (1 and 10), endless read-ack retries (4 and 5), settings save failures (5 and 9).

## 1. Product and experience

Reviewer: Julia (GPT 6.1 Sol).

### 1.1 Main review

Reviewed `10f84a7ada6519109564cd433aed23c4e36676fa`. Findings are verified statically; no builds, tests, edits, or browser actions. The modified overview file was read from HEAD. Accessibility and Internationalization are excluded.

| Category | Rating 0–3 | Evidence and justification |
|---|---:|---|
| Product definition | 1 | [README.md:3](README.md:3) describes capabilities, but lacks target users, non-goals, and measurable success criteria. |
| UX | 1 | Several complete route views are discarded by [shell/root.rs:3264](crates/editor-aqua/src/shell/root.rs:3264). |
| UI and visual design | 2 | Shared semantic surfaces and tokens exist, with [enforcement.rs:93](crates/editor-ui/tests/ui/kit/enforcement.rs:93); colour definitions still duplicate theme values. |
| Interaction design | 1 | Visible Help and Upgrade actions deliberately return no effect: [chrome/menu.rs:118](crates/editor-ui/src/ui/chrome/menu.rs:118). |
| Motion | 2 | [chrome/glide.rs:119](crates/editor-ui/src/ui/chrome/glide.rs:119) supports interruption and reduced motion; custom animations bypass it. |
| State coverage | 1 | [shell/connection.rs:243](crates/editor-aqua/src/shell/connection.rs:243) replaces ordinary offline failures with indefinite reassurance. |
| Content and copy | 1 | [settings/view.rs:369](crates/editor-settings/src/settings/view.rs:369) directs users to a pairing flow that page does not provide. |
| Onboarding | 1 | [composer/picker.rs:148](crates/editor-composer/src/composer/picker.rs:148) can hide every engine and its installation guidance. |
| Adaptivity | 2 | Responsive sidebar collapse is deliberate: [frame/sidebar.rs:128](crates/editor-aqua/src/frame/sidebar.rs:128). Setup and overview overflow remain unhandled. |
| Delight and polish | 1 | Crafted visuals coexist with inert actions, invisible routes, and ineffective visibility shortcuts: [shell/root.rs:3810](crates/editor-aqua/src/shell/root.rs:3810). |

**Findings, most severe first**

1. **High — Home, Marketplace, and Usage render empty.** [shell/root.rs:3264](crates/editor-aqua/src/shell/root.rs:3264): these routes construct `center`, but only Thread/NewThread insert it into the frame. Opening Marketplace or landing on Home without an active host produces empty content. Mount `center` for every applicable route and verify rendered route outcomes.

2. **High — The full inspector is disconnected.** [shell/workspace.rs:635](crates/editor-aqua/src/shell/workspace.rs:635): `inspector_elements` has no callers. The replacement overview mounts facts, context, and agents, omitting checklist, terminal, worktree, and edit surfaces. Users cannot reach those inspector functions. Restore the complete panel or provide equivalent reachable controls.

3. **High — No installed engines means no installation guidance in the model picker.** [composer/picker.rs:148](crates/editor-composer/src/composer/picker.rs:148): gated engines are excluded unless current. With no installed engine or models, `default_config` returns `None`, so every engine—and every Install gate—is excluded. Show unavailable engines with actionable installation/sign-in states.

4. **Medium — Visibility shortcuts update unused state.** [shell/root.rs:3810](crates/editor-aqua/src/shell/root.rs:3810): Ctrl+B and Ctrl+Alt+B toggle workspace flags, while actual sidebar rendering uses a separate `frame::Sidebar`; overview rendering checks width alone. Both shortcuts can rebuild without changing visibility. Connect them to the rendered state.

5. **Medium — Help, Upgrade, and account identity are inert controls.** [chrome/menu.rs:112](crates/editor-ui/src/ui/chrome/menu.rs:112): identity returns nothing; Help and Upgrade merely close the menu. Clicking “Help & Feedback” offers neither help nor feedback. Implement navigation or remove the interactive affordances.

6. **Medium — Setup cannot accommodate large machine lists.** [setup/view.rs:104](crates/editor-aqua/src/setup/view.rs:104): every discovered distro is rendered in an unbounded column inside a fixed, non-resizable 480×640 window. Enough distros or a long installation path pushes choices and Install beyond the window. Bound and scroll the list while retaining the action.

7. **Medium — Setup failure traps users in the same configuration.** [setup/model.rs:211](crates/editor-aqua/src/setup/model.rs:211): Failed permits another Started event but no return to Choosing. Selection and location changes are accepted only while Choosing. A bad destination or unusable WSL target requires restarting setup. Add a configuration-recovery transition.

8. **Medium — Setup’s spinner rebuilds the complete screen and ignores reduced motion.** [setup/window.rs:196](crates/editor-aqua/src/setup/window.rs:196): each 33 ms deadline increments the spinner and calls root `rebuild`, without consulting motion policy. This violates “Repeating animations must not dirty ancestors every frame.” Animate the spinner locally and provide a static reduced-motion state.

9. **Medium — Engine-light motion is neither continuous on interruption nor reduced-motion aware.** [composer/engine_light.rs:28](crates/editor-composer/src/composer/engine_light.rs:28): retargeting starts from the previous target rather than the current painted position. Rapid engine changes jump; the caller always advances animation despite reduced motion. Use the shared retargeting policy.

10. **Medium — Overview has no large-list overflow treatment.** [frame/overview.rs:122](crates/editor-aqua/src/frame/overview.rs:122), committed version: all agents are inserted into an ordinary column without a scroll container or height limit. A thread with dozens of agents places lower agents outside the window. Constrain and scroll or virtualize the agent region.

11. **Medium — Ordinary offline failures hide their cause and recovery path.** [shell/connection.rs:158](crates/editor-aqua/src/shell/connection.rs:158): the view extracts only the error code, then handles unauthorized/version mismatch specially. Other non-WSL failures show “Connecting…” or “getting you back…” indefinitely. A DNS or certificate failure gives users no diagnosis. Display the cause with relevant host-management actions.

12. **Medium — Clipboard failure is invisible.** [shell/root.rs:341](crates/editor-aqua/src/shell/root.rs:341): failed source copying only logs a warning and returns no changed state. If another application locks the clipboard, users paste stale content without knowing copying failed. Return the failure to the source UI and distinguish success from failure.

13. **Low — Recovery copy names the wrong destination.** [settings/view.rs:369](crates/editor-settings/src/settings/view.rs:369): “Pair a host on the Hosts page first” conflicts with that page’s own instruction to add/connect Forges on Account. A new user follows a dead end. Use consistent terminology and provide a direct Account action.

14. **Low — Chrome duplicates the theme’s colour authority.** [chrome/card.rs:48](crates/editor-ui/src/ui/chrome/card.rs:48): `Surfaces::of` repeats literal palette values already defined in `theme.rs`. Updating theme colours leaves Field/Inset materials using independent values. Express these materials through shared semantic colour tokens.

**What perfect looks like**

| Category | Concrete changes needed for 3 |
|---|---|
| Product definition | Record supported capabilities, target users, non-goals, and measurable first-value criteria; verify advertised flows before release. |
| UX | Mount every route; restore inspector functions; add scenarios that verify reachable user outcomes. |
| UI and visual design | Consolidate colour authorities; enforce token use across chrome; validate canonical light/dark views. |
| Interaction design | Connect visibility controls and menu actions; verify each action’s observable result. |
| Motion | Route custom animations through shared policy; test interruption/reduced motion; enforce ancestor-rebuild budgets. |
| State coverage | Exercise empty, loading, failed, offline, and overflow states through the actual shell. |
| Content and copy | Standardize Machine/Forge/host terminology; make recovery messages link to working actions. |
| Onboarding | Keep installation/sign-in guidance visible; make setup failures editable; verify a fresh-machine first-task scenario. |
| Adaptivity | Bound setup and overview content; verify minimum-window and mixed-DPI scenarios with meaningful failure thresholds. |
| Delight and polish | Eliminate inert affordances; surface copy outcomes; make routed experience checks release gates. |

| Score | /10 |
|---|---:|
| Correctness | 3 |
| State handling | 4 |
| Code quality | 6 |
| Rule adherence | 5 |
| **Overall** | **3** |

### 1.2 Remaining findings and nits

15. **Medium — Composer errors and limit notices never appear.** `crates/editor-composer/src/composer/view.rs:2592` renders session errors but omits `composer.notice` and `attachments.notice`; those fields are only captured for change detection. Failed Stop/Continue/access changes, excess attachments, and unreadable drafts can therefore produce no explanation. Render these notices with appropriate recovery actions.

16. **Medium — Changing hosts discards unsaved global instructions.** `crates/editor-settings/src/settings/controller.rs:128` resets the prompt form and clears its editor whenever the active host instance changes, without preserving dirty text. Editing instructions and then switching hosts loses the draft. Keep drafts per host or explicitly resolve unsaved changes before clearing them.

17. **Medium — An older settings failure can overwrite a newer choice.** `crates/editor-settings/src/settings/preferences.rs:19` unconditionally restores a request’s previous value. Multiple requests for the same key can overlap through `controller.rs:252`; a delayed failure can roll back a subsequently successful selection. Serialize mutations per key or track revisions, and test out-of-order completions.

18. **Medium — Sidebar message-search results survive a host switch.** `crates/editor-sidebar/src/sidebar/controller.rs:331` clears several host-dependent fields but retains `text_hits`; line 374 validates those hits only against the query. Search on Forge A, then switch to B with the same query: A’s hits are presented with B’s action scope. Clear or host-scope search results and restart the search on host changes.

19. **Medium — MCP edits made during saving are discarded.** `crates/editor-settings/src/settings/mcp_view.rs:171` leaves every field editable while saving, while `extensions.rs:166` closes the form on success without checking for subsequent edits. On a slow save, further typing disappears when the response arrives. Lock the submitted form or preserve edits using a revision check.

20. **Medium — MCP “Cancel” leaves the mutation running.** `crates/editor-settings/src/settings/mcp_view.rs:193` enables Cancel during saving; `extensions.rs:118` merely hides the form and invalidates its completion. The task created at `controller.rs:410` continues changing the server configuration. Make cancellation semantics explicit and prevent a dismissed request from silently committing.

21. **Medium — Questionnaires have no bounded scrolling region.** `crates/editor-composer/src/composer/view.rs:509` inserts every inline option into an ordinary column; line 648 similarly appends every other questionnaire. The composer mounts these extras without a height limit at line 749. A long questionnaire can consume the window and displace the message controls. Bound and scroll questionnaire content, and verify reachability at minimum window height.

22. **Medium — Code copying displays success before copying succeeds.** `crates/editor-ui/src/ui/code_copy.rs:141` starts the checkmark before dispatching the clipboard operation. Its handler at `crates/editor-aqua/src/shell/workspace.rs:473` only logs failures. A locked clipboard therefore still produces a success checkmark. Drive success feedback from the actual clipboard result.

23. **Medium — Marketplace searches are not scoped to a host change.** `crates/editor-marketplace/src/marketplace/controller.rs:130` cancels searches only when the same host ID gets a new instance. Changing to another host leaves the task, search serial, and listings intact; `mod.rs:160` checks only serial and query. An old host’s response can populate the new host’s marketplace. Reset and restart search on every host-identity change.

24. **Medium — Marketplace loading animation rebuilds the workspace repeatedly.** `crates/editor-aqua/src/shell/application.rs:972` turns `needs_animation()` into a refresh cause; `shell/refresh.rs:37` classifies that cause as `Workspace`. Loading the catalog consequently drives ancestor rebuilds for the animated wordmark. This violates **“Repeating animations must not dirty ancestors every frame.”** Animate the wordmark locally.

25. **Medium — Inbox and History overwrite the same pagination viewport.** `crates/editor-sidebar/src/sidebar/controller.rs:663` stores one `thread_viewport`, although `crates/editor-aqua/src/frame/inbox.rs:102` creates a second independent collection using the same callback. Both reports lack a scroller identity. Pagination can consequently use the other pane’s distances; the resulting paging behavior is **suspected**, not runtime-verified. Track each pane separately and test scrolling both.

26. **Low — Failed Forge settings saves have no visible explanation.** `crates/editor-settings/src/settings/controller.rs:561` logs the failure and rolls the control back, without storing a displayable error. An offline settings change simply snaps back. Show the failed setting and cause, with a deliberate retry action.

27. **Low — WSL discovery has neither loading nor failure feedback.** `crates/editor-aqua/src/setup/window.rs:64` converts discovery failures into an empty list, indistinguishable from no distributions. Setup initially offers Windows and permits installation immediately; results arriving after Started are ignored by the state machine. Show discovery progress and errors, and allow users to retry discovery before committing their target.

28. **Low — The mounted sidebar has no loading or empty-state guidance.** `crates/editor-aqua/src/frame/inbox.rs:27` builds collections directly from the current lists without distinguishing loading, a genuinely empty inbox, or a search with no matches. A first-time user sees a blank region. Add distinct states with a relevant next action.

29. **Low — Sidebar message-search failures look like missing results.** `crates/editor-sidebar/src/sidebar/controller.rs:455` logs the error and clears `text_hits`, without exposing a search error or recovery control. A network failure is presented like a successful search finding nothing. Preserve an explicit search state and render the failure with Retry.

30. **Low — The replacement sidebar drops existing recovery controls.** `crates/editor-aqua/src/frame/inbox.rs:124` renders only error text. The existing sidebar implementation at `crates/editor-sidebar/src/sidebar/view.rs:1078` offers project-icon Retry and error Dismiss, neither of which the replacement mounts. Failed icons cannot be retried there, and action errors remain visible indefinitely. Retain those controls in the mounted sidebar.

31. **Low — Default palette commands crowd out recent threads and other commands.** `crates/editor-aqua/src/shell/palette.rs:288` allocates recent-thread slots only after adding the fixed commands, which already exceed `SHOWN = 12`; line 314 then truncates everything. Opening the palette with an empty query shows no recent threads and hides later commands. Allocate sections deliberately instead of truncating a combined list.

32. **Low — The command palette lacks loading and no-results feedback.** `crates/editor-aqua/src/shell/palette/view.rs:191` renders errors and available entries, but no search progress or empty result message. During a slow search—or after a successful search with zero matches—the dialog can contain only its input. Render explicit pending and empty states.

33. **Low — Questionnaire answers cannot be corrected before submission.** `crates/editor-composer/src/composer/questionnaire.rs:163` records the answer and advances the step; the view provides no backward transition. Clicking the wrong option in the first of several questions leaves no way to revise it. Add Back/review behavior that restores recorded choices.

34. **Low — A failed question load hides questionnaire dismissal and switching.** `crates/editor-composer/src/composer/view.rs:437` returns the loading/error fragment before constructing Dismiss or the other questionnaire selectors. If one stored question cannot load, the user can only retry it, even when other questionnaires are available. Keep dismissal and questionnaire navigation outside the loaded-question branch.

35. **Low — Image-preview errors discard the cause and offer no retry.** `crates/editor-ui/src/ui/media/preview.rs:258` reduces the error to a Boolean; line 535 displays only “Couldn't load image.” Its action enum has no Retry. A transient fetch failure leaves the preview unrecoverable through its controls. Retain a useful error and provide cache-invalidating retry.

36. **Low — Image comparisons cannot recover from transient fetch failures.** `crates/editor-ui/src/ui/rich/media/image_compare/view.rs:138` records fetch failures, but the action enum at line 55 provides no retry operation. An unavailable Forge can leave either comparison side failed after connectivity returns. Add per-side retry using the image loader’s invalidation mechanism.

37. **Low — Audio fetch errors become cached, unrecoverable placeholders.** `crates/editor-ui/src/ui/media/audio.rs:109` discards the error and publishes `None`; `ui/rich/media/audio/view.rs:257` renders a generic failure without controls. A transient fetch failure removes playback without an explanation or retry path. Preserve typed load failures and expose retry.

38. **Low — Resource previews require closing and reopening after failure.** `crates/editor-aqua/src/shell/resource_preview/view.rs:149` displays the error as text; its actions contain only Copy and Dismiss. A temporary resource-read failure has no recovery action in the dialog. Add Retry and refresh the request while preserving the requested line.

39. **Low — Model-discovery failures provide no recovery action in the picker.** `crates/editor-composer/src/composer/view.rs:3193` offers actions only for “Not installed” and “Sign in.” “Discovery failed” and “No models” leave the user with a status sentence. Offer discovery refresh or a direct route to the relevant engine settings.

40. **Low — Marketplace search failures cannot be retried directly.** `crates/editor-marketplace/src/marketplace/mod.rs:305` renders a remote-search failure as a group heading. The Retry handler at `controller.rs:286` reloads prompt/catalog data rather than repeating the search. After a transient search failure, users must alter their query to retry. Provide a search-specific retry action.

41. **Low — Overview visibility calculations use a different width from the rendered panel.** `crates/editor-aqua/src/shell/root.rs:3211` only checks whether `inspector_width(card_width)` returns something, then mounts an overview whose fixed width is defined at `frame/overview.rs:134`. Near the threshold, the calculation permits a narrower panel than the overview actually consumes. Use the calculated width or base the threshold on the real overview width.

42. **Low — Enter does not submit an engine sign-in code.** `crates/editor-settings/src/settings/engine_view.rs:348` handles Changed and PresentationChanged but ignores Submitted. Pasting a code and pressing Enter does nothing despite the adjacent “Submit code” action. Route Submitted through the same guarded submission path.

43. **Low — Engine sign-in Cancel can silently do nothing.** `crates/editor-settings/src/settings/engine_view.rs:378` leaves Cancel enabled while the engine is busy, but `controller.rs:434` rejects every action while `EnginesForm::begin` reports busy. Clicking Cancel during a pending operation produces no effect or explanation. Give cancellation a separate path or accurately disable the control.

44. **Low — MCP sign-in cannot be cancelled while starting.** `crates/editor-settings/src/settings/mcp_view.rs:97` renders only a disabled “Opening…” control until an authorization URL becomes available. A slow or stuck initiation has no Cancel even though cancellation is supported once Waiting has a URL. Expose cancellation throughout the pending states.

45. **Low — Repository-link launch failures have no user feedback.** `crates/editor-aqua/src/shell/root.rs:2266` logs browser-launch errors and otherwise consumes the navigation action. A missing or broken browser association makes the repository link appear inert. Display the failure and retain an option to copy the URL.

46. **Low — OpenCode sign-in copy contradicts itself.** `crates/editor-settings/src/settings/engine_view.rs:250` says its free models work while signed out; line 282 then says OpenCode “can't run until you sign in.” A user can reasonably conclude authentication is mandatory. Use engine-specific guidance consistent with its actual capabilities.

47. **Low — Archive copy promises retention that cleanup can revoke.** `crates/editor-settings/src/settings/archive_view.rs:290` says archived threads “stay here until you restore them,” while line 116 explicitly says cleanup deletes archived threads too. Users may archive a thread believing it is protected. State the retention exception beside the archive guidance.

48. **Low — MCP program input is inconsistently normalized.** `crates/editor-settings/src/settings/extensions.rs:39` checks the untrimmed program field, and line 70 preserves it verbatim, while name and URL are trimmed. Whitespace-only input passes the program-presence check; surrounding whitespace can also cause avoidable execution failures. This violates **“Normalize messy input data into clean structures at the boundary.”** Validate and normalize executable input while preserving legitimate internal spaces.

49. **Low — Goal-strip reversals jump instead of continuing from the painted state.** `crates/editor-composer/src/composer/goal_strip.rs:66` restarts growth when a leaving strip reappears; line 96 starts disappearance from a full reveal regardless of current growth. Rapid goal/note changes can snap between heights. Retarget from sampled reveal and label values, with interruption tests.

50. **Low — Primary-icon swaps restart from a fully visible previous icon.** `crates/editor-composer/src/composer/swap.rs:70` replaces an interrupted transition with `(previous, now)`. If the face changes twice within 250 ms, a partly revealed icon becomes the new fully visible outgoing icon. Preserve current layer opacities when retargeting and test rapid face changes.

51. **Low — Long marketplace names and error headings lack overflow treatment.** `crates/editor-marketplace/src/marketplace/view.rs:517` makes item names non-shrinking and non-wrapping; the shared `muted` renderer at line 203 also disables wrapping for headings, including remote-search errors. Long names or error chains can consume space needed by controls or extend outside the row. Truncate names with full-text disclosure and wrap diagnostic text.

52. **Low — Pending discovered-skill operations have no operation-specific feedback.** `crates/editor-marketplace/src/marketplace/view.rs:663` keeps showing “Install” for a discovered skill even when its operation is pending. All controls become disabled, while only remote listings receive an “Installing…” label at line 703. Show the pending operation on the affected skill.

53. **Low — Static token-value tests violate the stated testing rule.** `crates/editor-ui/tests/ui/kit/tokens/radius.rs:25` asserts a literal radius table; `tokens/text_role.rs:33` similarly pins literal font metrics. These fail on intentional token changes without demonstrating a user-visible regression. The rule is **“No tests of static values or markup.”** Replace literal-value assertions with meaningful behavioral or rendered-layout checks.

54. **Low — Token enforcement silently succeeds when source files cannot be read.** `crates/editor-ui/tests/ui/kit/enforcement.rs:68` flattens directory-read errors away, and line 98 substitutes an empty string for unreadable source. The enforcement test can report no violations because it inspected nothing. Fail explicitly on traversal/read errors and require a nonempty source inventory.

55. **Low — Sidebar tests cover presentation state that the shell no longer consumes.** `crates/editor-aqua/tests/frame/sidebar.rs:6` tests project accordion state, and `tests/frame/tabs.rs:5` tests working-thread tabs. The shell at `src/shell/root.rs:3158` constructs only the open-thread tab independently; no production caller consumes the tested project grouping/tab APIs. Add tests through the mounted navigation path, and remove obsolete state and tests.

56. **Nit — History sizing hardcodes an outdated chrome height.** `crates/editor-aqua/src/frame/inbox.rs:148` subtracts `43.0` instead of using `sidebar::geometry::history_height()`, which derives its height from current row/divider tokens. **Scenario:** cosmetic spacing drift, especially after token changes. Derive this reservation from the shared geometry.

57. **Nit — An actively used controller is named as unused.** `crates/editor-settings/src/settings/view.rs:97` calls the parameter `_controller` and uses it throughout the dispatcher. **Scenario:** cosmetic; the underscore falsely signals an intentionally unused argument. Rename it `controller`.

58. **Nit — `forge_error` is dead presentation state.** `crates/editor-settings/src/settings/controller.rs:42` declares it, initializes it to `None`, and includes it in view snapshots; no code assigns an error to it. **Scenario:** cosmetic maintenance overhead and a misleading error path. Remove it or connect it to an actual operation.

59. **Nit — Automation descriptions contain multiple visible typos.** `crates/editor-settings/src/settings/view.rs:529` includes “indefintely,” “capacityis,” and “theturn”; nearby descriptions use “depletage” and “similiar.” **Scenario:** cosmetic. Rewrite the descriptions in plain, grammatical language.

60. **Nit — Notification copy is grammatically broken.** `crates/editor-settings/src/settings/view.rs:491` says “operating systems,” “threads finish, is stuck,” and “out right failed.” **Scenario:** cosmetic. Use concise copy such as “Notify me when a thread finishes, needs attention, or fails.”

61. **Nit — Font-selection copy is malformed.** `crates/editor-settings/src/settings/view.rs:1120` says “Choose what set of fonts to Artisan Editor will render in.” **Scenario:** cosmetic. Use “Choose the fonts Artisan Editor uses,” and consistently label the control “Fonts” or “Font set.”

62. **Nit — Account settings introduce an unexplained product abbreviation.** `crates/editor-settings/src/settings/page.rs:49` calls it an “AST account,” while surrounding account surfaces use Artisan terminology. **Scenario:** cosmetic terminology inconsistency. Use the established product/account name.

63. **Nit — Account instructions display Markdown backticks literally.** `crates/editor-settings/src/settings/account_view.rs:296` passes “Claim the code its `ae account enroll` printed” to ordinary text rendering. **Scenario:** cosmetic; raw Markdown punctuation appears in native copy. Use styled inline code or remove the backticks.

64. **Nit — Empty resource previews report an impossible line range.** `crates/editor-aqua/src/shell/resource_preview.rs:42` formats an empty file as “Lines 1–0 of 0.” **Scenario:** cosmetic. Give empty files an explicit empty-state message.

65. **Low — Working-pulse parameters do not control the actual pulse.** `crates/editor-sidebar/src/sidebar/controller.rs:148` accepts and ignores the pulse duration and opacity arguments supplied by `row.rs:260`; `crates/editor-ui/src/ui/motion.rs:39` independently hardcodes those values. Changing the row’s pulse constants silently changes nothing. Make the implementation consume its parameters or remove the duplicate configuration.

## 2. Accessibility and internationalization

Reviewer: Amanda (GPT 6.1 Sol).

### 2.1 Main review

| Category | Rating 0–3 | Justification |
|---|---:|---|
| Accessibility | **2** | Deliberate roles, focus styles, modal handling and reduced-motion support exist, but keyboard, announcements, contrast and native editing remain incomplete ([controls:14](crates/editor-ui/src/ui/kit/control/mod.rs:14), [dialog:229](crates/editor-ui/src/ui/dialog.rs:229)). |
| Internationalization | **1** | Grapheme navigation and UTF-16 integration exist; strings, plurals and most formatting remain English-specific ([editor:501](crates/editor-ui/src/ui/text/editor.rs:501), [settings:979](crates/editor-settings/src/settings/view.rs:979)). |

Reviewed Editor commit `10f84a7` and its exact Aqua dependency, `e6fea72`. Findings below are verified by static tracing and token calculations. No builds, tests or live screen-reader sessions were run.

**Findings, most severe first**

1. **High — Native IME edits bypass the authoritative editor.** [mount.rs:248](crates/editor-ui/src/ui/text/mount.rs:248) handles composition events but omits `ImeReplace` and `ImeSelect`. Aqua’s [events.rs:298](/home/sander/.cargo/git/checkouts/aqua-5a0ca0a4d1059220/e6fea72/crates/aqua-ui/src/events.rs:298) therefore changes only its internal text backing. TSF reconversion or deletion can change the displayed text while the submitted/persisted draft retains the old value. Route every native replacement and selection through `Editor`.

2. **High — UIA text changes have the same ownership failure.** Aqua’s [runtime.rs:2884](/home/sander/.cargo/git/checkouts/aqua-5a0ca0a4d1059220/e6fea72/crates/aqua-runtime/src/runtime.rs:2884) implements `SetValue` and `ReplaceSelectedText` by mutating its backing editor and firing `TextChanged`; the external controller mounted at [mount.rs:633](crates/editor-ui/src/ui/text/mount.rs:633) has no corresponding action handler. Assistive software can apparently enter text that the composer never sends. Delegate these actions to the external text model.

3. **High — Single-line fields block forward keyboard navigation.** [editor.rs:484](crates/editor-ui/src/ui/text/editor.rs:484) returns `Ignored` for Tab; [mount.rs:444](crates/editor-ui/src/ui/text/mount.rs:444) treats that as handled and prevents default traversal. After entering an email or pairing code, Tab cannot reach the next control. Return `Propagate` when Tab performs no editing action.

4. **High — Cancelling composition keeps provisional text.** [mount.rs:260](crates/editor-ui/src/ui/text/mount.rs:260) calls `unmark_text`, which only clears the mark at [editor.rs:373](crates/editor-ui/src/ui/text/editor.rs:373). An `Update("候") → Cancel` sequence leaves `候` in the draft. Track the precomposition text and selection, and restore them on cancellation.

5. **High — Streaming replies have no announcement channel.** Aqua’s complete [SemanticNode:59](/home/sander/.cargo/git/checkouts/aqua-5a0ca0a4d1059220/e6fea72/crates/aqua-core/src/semantic.rs:59) has no live-region, busy or announcement fields; its [UIA mapping:94](/home/sander/.cargo/git/checkouts/aqua-5a0ca0a4d1059220/e6fea72/crates/aqua-a11y-uia/src/tree.rs:94) supplies none. With focus in the composer, incoming answers have no explicit automatic announcement. Add polite, coalesced announcements for meaningful reply chunks, completion and errors.

6. **Medium — Missing cursor metadata turns preedit into committed input.** [mount.rs:250](crates/editor-ui/src/ui/text/mount.rs:250) uses cursor presence to determine whether text is marked. With `cursor: None`, subsequent updates append and Enter can submit provisional text. [The existing test:230](crates/editor-aqua/tests/text/mod.rs:230) explicitly accepts this behavior. Track composition independently of cursor metadata. The inspected TSF producer supplies a cursor; this affects the supported cursor-less event path.

7. **Medium — Audio seeking is pointer-only.** [audio/view.rs:485](crates/editor-ui/src/ui/rich/media/audio/view.rs:485) installs pointer handlers without focusability, keyboard handlers or slider semantics. Keyboard and UIA users cannot seek. Expose position, duration and seek actions through an accessible slider.

8. **Medium — Image comparison exposes an unnamed focus target.** [image_compare/view.rs:641](crates/editor-ui/src/ui/rich/media/image_compare/view.rs:641) makes the comparison frame focusable and keyboard-operated, but assigns no accessible name, control role or numeric range. A screen reader cannot identify or inspect its split/blend state. Supply slider/toggle semantics and matching accessibility actions.

9. **Medium — Composer suggestions remove its accessible name.** [mount.rs:650](crates/editor-ui/src/ui/text/mount.rs:650) clears the placeholder when a suggestion exists and supplies no explicit label. Aqua’s [tree.rs:1814](/home/sander/.cargo/git/checkouts/aqua-5a0ca0a4d1059220/e6fea72/crates/aqua-ui/src/tree.rs:1814) derives editable names from placeholders. An empty composer with a suggestion becomes unnamed. Give inputs stable purpose-based names.

10. **Medium — Tokens fail contrast requirements.** [theme.rs:635](crates/editor-ui/src/ui/theme.rs:635) selects `GRAY_400` for light faint text; [metrics.rs:73](crates/editor-ui/src/ui/text/metrics.rs:73) uses it for enabled placeholders. Calculated contrast is approximately **2.4:1** against their surface. The light focus-ring token at [theme.rs:704](crates/editor-ui/src/ui/theme.rs:704) is approximately **1.7:1** against the panel. Adjust semantic tokens and enforce contrast checks for actual foreground/background pairs.

11. **Medium — Text scaling stops at display DPI.** [text_role.rs:52](crates/editor-ui/src/ui/kit/tokens/text_role.rs:52) fixes caption/label sizes at 12/14 px; [metrics.rs:28](crates/editor-ui/src/ui/text/metrics.rs:28) separately fixes editor sizes. Preferences expose neither text scaling nor application zoom. Increasing Windows text size cannot enlarge these independently of display scaling. Apply a shared text-scale factor and verify reflow.

12. **Medium — High contrast does not affect the content palette.** Aqua reads Windows high-contrast preferences, but [preferences.rs:50](crates/editor-ui/src/ui/preferences.rs:50) chooses only light/dark tokens. Enabling a contrast theme leaves custom text, borders and focus colors unchanged. Propagate system contrast preferences into an explicit accessible palette.

13. **Medium — Connected UIA still pays for the whole tree each frame.** Aqua’s [runtime.rs:1722](/home/sander/.cargo/git/checkouts/aqua-5a0ca0a4d1059220/e6fea72/crates/aqua/src/runtime.rs:1722) requests a snapshot every produced frame; [runtime.rs:2775](/home/sander/.cargo/git/checkouts/aqua-5a0ca0a4d1059220/e6fea72/crates/aqua-runtime/src/runtime.rs:2775) traverses semantics and clones text runs. [bridge.rs:65](/home/sander/.cargo/git/checkouts/aqua-5a0ca0a4d1059220/e6fea72/crates/aqua-a11y-uia/src/bridge.rs:65) compares/diffs afterward. Animation-only frames therefore incur tree-sized work. Gate snapshots on semantic revisions and enforce work budgets.

14. **Medium — User-facing strings are compiled English.** For example, [settings/view.rs:984](crates/editor-settings/src/settings/view.rs:984) embeds complete labels and explanations; equivalent literals span the Editor crates, with no message catalogs or locale-selection layer. Translation requires source edits. Externalize complete messages, including accessible labels and errors.

15. **Medium — Plurals are assembled manually and already produce incorrect English.** [usage_page.rs:172](crates/editor-sidebar/src/sidebar/usage_page.rs:172) renders `1 tokens`; [activity.rs:203](crates/editor-conversation/src/conversation/activity.rs:203) renders `1 files`. [presentation.rs:314](crates/editor-marketplace/src/marketplace/presentation.rs:314) supports only English singular/plural. Use locale-aware plural messages.

16. **Medium — Locale formatting is partial.** [rich/kit.rs:62](crates/editor-ui/src/ui/rich/kit.rs:62) hardcodes decimal points and `k/M/B`; [local_time.rs:24](crates/editor-ui/src/ui/local_time.rs:24) combines localized Windows clock/date output with English phrases and English weekday formatting. Users receive mixed-language output. Centralize locale-aware numbers and complete date/relative-time messages.

17. **Medium — Horizontal editing ignores visual bidi order.** [editor.rs:501](crates/editor-ui/src/ui/text/editor.rs:501) maps Left/Right directly to previous/next logical graphemes. In Hebrew/Arabic runs, Right can move the caret visually left. Use shaped bidi caret positions and affinities while preserving grapheme boundaries.

**What perfect looks like**

- **Accessibility:** Unify native editing with the authoritative model; complete keyboard/control semantics and streaming announcements; enforce semantic scenarios, contrast, text scaling/high contrast and UIA work budgets, supplemented by Windows screen-reader passes.
- **Internationalization:** Introduce message catalogs with plural rules and locale-aware formatting; implement visual bidi navigation and RTL layout; test composition cancellation, reconversion, cursor-less updates and mixed-script editing through the mounted Editor.

| Score | Slice assessment |
|---|---:|
| Correctness | 4/10 |
| State handling | 3/10 |
| Code quality | 5/10 |
| Rule adherence | 6/10 |
| **Overall** | **4/10** |

### 2.2 Remaining findings and nits

18. **Medium — Audio playback is pointer-only.** `crates/editor-ui/src/ui/markdown/presenter.rs:1647` at `10f84a7` wraps play/pause in a button that explicitly rejects every activation source except a primary pointer and sets `focusable(false)`. The icon-only child supplies no accessible name. Keyboard users cannot start playback, and UIA invocation does nothing. Use a named, focusable play/pause control whose activation works for every input source.

19. **Medium — Collapsed callouts cannot be opened from the keyboard or UIA.** `crates/editor-ui/src/ui/rich/narrative/callout.rs:166` installs only pointer-down/up handlers on the header, without focusability, button semantics, an expanded state or an accessibility action. A collapsed callout can hide its entire message from these users. Implement the header as a named disclosure button with Enter/Space activation and expanded semantics.

20. **Medium — Markdown links have no keyboard or accessible link representation.** `crates/editor-ui/src/ui/markdown/visual.rs:749` emits one text leaf with interaction ranges and a `TextRangeActivated` handler. Individual links receive neither focus targets nor semantic nodes. Aqua’s corresponding event carries pointer data, and its semantic role enum has no link role. A keyboard or screen-reader user cannot enumerate or activate links in an answer. Expose inline links as semantic links with keyboard activation.

21. **Medium — Shared sliders do not expose an operable UIA range.** `crates/editor-ui/src/ui/slider.rs:234` sets the slider role and a string readout but supplies neither Aqua’s native slider control nor numeric range semantics. In pinned Aqua `e6fea72`, `crates/aqua-a11y-uia/src/tree.rs:117` adds SetValue/Increment/Decrement only when a numeric range exists. Assistive software cannot adjust chart thresholds. Use the native control or bridge the range and actions to `SliderState`.

22. **Medium — Segmented controls change selection without moving keyboard focus.** `crates/editor-ui/src/ui/segmented_control.rs:187` changes the value, but moves focus only through optional callbacks or `focus_host`. Neither production caller supplies either mechanism: `crates/editor-ui/src/ui/rich/media/model.rs:227` and `crates/editor-ui/src/ui/rich/media/image_compare/view.rs:404`. After one Right press, focus remains on the previous item; another Right computes the same next item again. Make focus movement intrinsic to the control.

23. **Medium — File-tree leaves cannot be opened with Enter or Space.** `crates/editor-ui/src/ui/rich/code/file_tree/view.rs:384` registers accessibility activation, but its keyboard handler at line 428 handles only navigation. Aqua’s default keyboard path fires `Activate`, for which this element has no handler. Files can be reached but never opened using the keyboard. Route keyboard activation to `Action::Activate`; update `crates/editor-ui/tests/ui/rich/code/file_tree.rs:147`, which currently asserts that Enter/Space produce no action.

24. **Medium — Headings are visual styling without heading semantics.** `crates/editor-ui/src/ui/markdown/presenter.rs:2838` passes the heading level into typography, but `crates/editor-ui/src/ui/markdown/visual.rs:749` still builds an ordinary text node. Likewise, `crates/editor-ui/src/ui/kit/tokens/text_role.rs:94` never assigns a heading role. Screen-reader heading navigation cannot traverse answer sections or page headings. Emit `Role::Heading(level)` independently of typography.

25. **Medium — Markdown tables lose their entire accessible structure.** `crates/editor-ui/src/ui/markdown/table_view.rs:33` constructs ordinary boxes for tables, rows and cells; header status only changes text styling. No table, row, cell, column-header or collection semantics are emitted. A screen-reader user receives disconnected text without column relationships. Preserve the table structure and header associations in the semantic tree.

26. **Medium — Inline color swatches erase the value from accessible text.** `crates/editor-ui/src/ui/markdown/inline.rs:79` replaces recognized prose colors with an invisible placeholder sequence; line 98 does the same for a standalone color code. The actual value survives in copy substitutions and paint metadata, not the text exposed to UIA. For example, “Use #ff0000” loses the color value when read aloud. Preserve an accessible textual equivalent for each swatch.

27. **Medium — Rendered Mermaid diagrams expose instructions, not their information.** `crates/editor-ui/src/ui/markdown/diagram/view.rs:153` paints the diagram as SVG; line 184 supplies only the generic label “Mermaid diagram…” and interaction instructions. Node labels and relationships have no accessible representation. A screen-reader user cannot understand the diagram. Retain and expose a textual or structured equivalent of the parsed graph.

28. **Medium — Mermaid’s font database cannot render common non-Latin labels.** `crates/editor-ui/src/ui/markdown/diagram.rs:117` creates a fresh font database containing only bundled faces before converting text to outlines. The bundled fonts’ character maps contain none of the checked Japanese characters `日本語` or Arabic characters in `العربية`; system fonts are never loaded into this database. These labels cannot obtain the required glyphs through this path. Add suitable fallback fonts or use the application’s system-aware text pipeline.

29. **Medium — Focusable chart groups are not exposed as UIA-focusable controls.** `crates/editor-ui/src/ui/rich/charts/chart/view.rs:463` and `crates/editor-ui/src/ui/rich/charts/hierarchy/view.rs:597` make groups keyboard-focusable. Pinned Aqua’s `crates/aqua-a11y-uia/src/tree.rs:98` derives Focus support from role alone and omits groups; the semantic model carries no focusable property. UIA clients cannot discover or request the same focus available through Tab. Propagate focusability independently of role.

30. **Medium — UIA cannot scroll the application’s scroll regions.** `crates/editor-ui/src/ui/select.rs:547` uses Aqua scroll regions, but pinned Aqua’s `crates/aqua-a11y-uia/src/tree.rs:95` supplies only clipping semantics, without scroll actions or ranges. Its `crates/aqua-a11y-uia/src/action.rs:19` collapses scroll requests to `Other("scroll")`, which the runtime does not implement. Assistive clients cannot reveal offscreen content through scrolling patterns. Implement scroll properties and actions end to end.

31. **Medium — Modal dialogs leave background controls in the accessibility tree.** `crates/editor-ui/src/ui/dialog.rs:229` opens a modal popup, but pinned Aqua’s `crates/aqua-runtime/src/runtime.rs:2775` still snapshots the complete tree. `crates/aqua-ui/src/tree.rs:1804` does not filter against the modal scope, and the semantic model has no modal property. Screen-reader navigation can reach background controls while the keyboard is confined to the dialog. Hide the background accessibility subtree and expose modal state while a dialog is open.

32. **Medium — Search visually covers the workspace without containing focus.** `crates/editor-aqua/src/frame/search.rs:222` inserts search as an ordinary overlay subtree, without a modal host or focus scope. `crates/editor-aqua/src/shell/root.rs:3292` only focuses the query once. Shift+Tab can move into obscured workspace controls; Escape handling is attached to the search subtree at `search.rs:227`. Give search an explicit focus scope, background exclusion and focus restoration.

33. **Medium — Select keyboard highlighting is invisible to assistive technology.** `crates/editor-ui/src/ui/select.rs:472` preserves focus on the trigger, while line 543 exposes the committed selection rather than the keyboard-highlighted option. There is no active-descendant relationship or option-focus update. After opening a select, successive arrow presses can change the visual highlight without communicating which option Enter will choose. Expose the active option through focus or an equivalent semantic relationship.

34. **Medium — Select navigation computes scrolling instructions and discards them.** `crates/editor-ui/src/ui/select.rs:220` returns `reveal: Some(index)`, but the event handler at line 411 consumes only `commit` and `handled`. The menu is height-limited at line 545. Navigating toward the end of a long menu highlights and commits options outside the visible viewport. Apply the reveal request to the menu’s scroll position.

35. **Medium — Failed images lose keyboard-accessible recovery.** `crates/editor-ui/src/ui/rich/media/image/view.rs:254` installs retry only as a pointer-up handler. This branch retains an image role and receives none of the ready-image branch’s focus, keyboard or accessibility activation support. A keyboard user cannot retry a transient image failure. Expose a named Retry control with equivalent activation paths.

36. **Medium — Composition protection is bypassed by SingleLine and Shift+Enter.** `crates/editor-ui/src/ui/text/editor.rs:455` processes Shift+Enter before checking marked text, and line 461 suppresses ordinary Enter only in Prompt mode. If these key events reach the controller during composition, a single-line form submits provisional text, while Shift+Enter can replace the preedit with a newline. Check composition state before every Enter branch and defer to the IME.

37. **Medium — Home and End ignore soft-wrapped visual lines.** `crates/editor-ui/src/ui/text/editor.rs:630` searches exclusively for newline characters. `crates/editor-ui/src/ui/text/mount.rs:302` consumes Home/End before Aqua can use shaped line geometry. In a long wrapped prompt, Home jumps across preceding visible lines, and Shift+Home selects substantially more text than expected. Resolve unmodified line-edge movement through the current visual layout.

38. **Medium — TSF geometry queries return fabricated positions.** In the Aqua dependency pinned by `10f84a7`, `crates/aqua-ime-tsf/src/store.rs:522` ignores the requested screen point and returns the composition or selection start. `GetTextExt` at line 530 ignores both requested text offsets and returns the same rectangle with `clipped=false`. IME clients asking about another character receive the caret’s geometry instead. Bridge actual hit-testing and range geometry, or return the appropriate unsupported/unavailable result.

39. **Medium — Animated phrases expose hidden measurement text and reel contents.** `crates/editor-ui/src/ui/phrase.rs:353` retains an invisible full word beside its visible fragments; line 456 creates every intermediate reel character as text; line 473 adds another invisible target character. These nodes are not accessibility-hidden, and Aqua’s semantic traversal does not exclude opacity-zero nodes. Reading a changing status can include duplicate words and intermediate digits. Expose one stable semantic phrase and hide its rendering machinery.

40. **Medium — Audio has no transcript or equivalent-content facility.** `crates/editor-ui/src/ui/rich/media/audio.rs:28` accepts only sources, names and tracks; the track definition at line 38 likewise has no transcript. The renderer supplies playback controls and timing but no text equivalent. Spoken instructions delivered only through an audio block are unavailable to deaf users. Support an associated transcript or equivalent textual content.

41. **Low — Markdown lists have no list, item or task-state semantics.** `crates/editor-ui/src/ui/markdown/list.rs:135` flattens the list into boxes, and line 214 emits task states as Unicode glyphs. Screen readers cannot announce item counts, nesting or checked status reliably. Preserve list/item semantics and expose task state explicitly.

42. **Low — Shared rows and hit areas provide no visible keyboard focus.** `crates/editor-ui/src/ui/kit/row.rs:397` makes rows focusable but adds only pointer hover tracking; `crates/editor-ui/src/ui/kit/control/hit_area.rs:49` likewise supplies no focus style. Aqua’s raw button builder supplies no default focus paint. Tabbing through menu rows, panel tabs or the rail search hit area gives no corresponding focus indicator. Apply the shared focus-ring treatment.

43. **Low — Diagram and model viewports also lack visible keyboard focus.** `crates/editor-ui/src/ui/markdown/diagram/view.rs:183` and `crates/editor-ui/src/ui/rich/media/model/view.rs:503` create keyboard-operated focus targets without focus-visible styling. Users cannot tell when arrow keys will pan/orbit the viewport instead of operating surrounding UI. Add a persistent, unobscured focus indicator around each viewport.

44. **Low — Shared tabs lack tab-list keyboard behavior.** `crates/editor-ui/src/ui/kit/control/tabs.rs:59` creates independently tabbable buttons with tab roles but supplies no arrow navigation, Home/End handling or roving tab stop. A large tab set requires repeated Tab presses and does not follow the interaction implied by its semantics. Implement orientation-aware tab navigation and keep one tab in sequential focus order.

45. **Low — Media context-menu actions are unavailable from the keyboard.** `crates/editor-ui/src/ui/rich/media/image/view.rs:188` exposes path copying only on secondary pointer-down. `crates/editor-ui/src/ui/rich/media/model/view.rs:515` does the same for model source copying. Neither keyboard handler supports ContextMenu or Shift+F10. Route those keys and UIA context-menu requests to the same actions.

46. **Low — Picker explanations are hover-only.** `crates/editor-composer/src/composer/view.rs:3146` opens engine explanations only through `on_hover_change`; the reason is not attached as an accessible description. A keyboard user focusing the engine icon cannot obtain that explanation. Show the same information on focus and expose it through the control’s description.

47. **Low — Reduced-motion preferences do not reach pressed-state movement.** `crates/editor-ui/src/ui/kit/control/icon_button.rs:86` defaults to `MotionPolicy::Full`, and line 224 applies a translated pressed state. Production callers do not pass the reduced policy. The same pattern occurs in shared buttons and key controls. Enabling reduced motion therefore leaves these movements active. Derive the control policy from the application preference.

48. **Low — Programmatic truncation can split a grapheme cluster.** `crates/editor-ui/src/ui/text/editor.rs:783` truncates at a UTF-8 character boundary, not a grapheme boundary; `crates/editor-ui/src/ui/page.rs:69` does the same for previews. A limit can retain only part of a combining sequence or joined emoji. Truncate display text at grapheme boundaries and preserve an explicit truncation indicator.

49. **Low — Transcript search reveal is case-insensitive only for ASCII.** `crates/editor-aqua/src/shell/root/reveal.rs:46` lowercases with `to_ascii_lowercase`. Searching for `ÉTÉ` cannot locate `été`, and canonically equivalent composed/decomposed text is also unmatched. Use Unicode-aware matching with a mapping back to original byte offsets.

50. **Low — Select typeahead cannot consume composed text.** `crates/editor-ui/src/ui/select.rs:377` listens only to KeyDown, while line 198 accepts a single scalar and compares lowercased prefixes without normalization. Dead-key and IME text is never delivered to the query, and canonically equivalent accents can fail to match. Handle committed text input and normalize the typeahead comparison.

51. **Low — Numeric phrase rendering breaks surrounding Unicode shaping.** `crates/editor-ui/src/ui/phrase.rs:252` sends any word containing an ASCII digit through `retained_columns`, which at line 378 creates one text leaf per scalar. A word such as `1مرحلة` loses Arabic joining and coherent bidi shaping; combining sequences are also split. Keep nonnumeric text in shaped runs and animate only suitable numeric segments.

52. **Low — Terminal cursor columns count emoji components separately.** `crates/editor-ui/src/ui/markdown/terminal/screen.rs:308` adds width per scalar. A joined emoji such as `👩‍💻` consumes separate widths for the woman and laptop despite forming one displayed cluster. Subsequent cursor-addressed output overwrites the wrong location. Track grapheme/emoji cluster widths across input chunks.

53. **Low — Terminal erasure can leave half a wide character alive.** `crates/editor-ui/src/ui/markdown/terminal/screen.rs:179` truncates or clears cells without repairing wide-character pairs. After printing `界`, moving into its continuation cell and erasing to end leaves the leading character intact, which still materializes as the full glyph. Expand erasure to cover any intersected wide character and test cursor/erase sequences involving CJK text.

54. **Low — Accessibility output carries no language metadata.** In pinned Aqua, `crates/aqua-core/src/semantic.rs:59` has no language field, and `crates/aqua-a11y-uia/src/tree.rs:94` never assigns one. Mixed-language answers cannot identify their language to assistive speech systems. Propagate application and content language metadata into accessible text nodes. The resulting pronunciation behavior is **suspected**; no screen-reader session was run.

55. **Low — Progress controls expose text but no numeric progress range.** `crates/editor-ui/src/ui/progress.rs:85` assigns a progress role and percentage string only. Aqua’s semantic emitter at `crates/aqua-ui/src/tree.rs:1848` creates numeric ranges exclusively for native sliders and resize controls. Assistive clients cannot inspect progress through a numeric range pattern. Expose minimum, maximum and current progress independently of display formatting.

56. **Low — UIA tests miss these controls and include a prohibited smoke test.** `crates/editor-aqua/tests/shell/uia.rs:27` asserts only that some nodes and a button exist. The “every actionable element” test at line 34 actually checks only buttons in one fixture, excluding text fields, sliders, options and custom focus targets. The former violates **“No smoke tests”**; both leave missing operations undetected. Replace the smoke assertion with semantic/action scenarios covering representative controls and verify resulting user-visible state.

57. **Nit — The font catalog violates the no-comments rule throughout.** `crates/editor-assets/src/fonts.rs:1`, line 15 and the field documentation beginning at line 18 contain module, type and field comments. This violates **“Never write code comments: no file headers, doc comments, inline explanations or TODOs.”** Failure scenario: cosmetic/rule adherence. Remove the prose comments and express necessary distinctions through names and structure; retain required license headers.

## 3. Performance, resource efficiency, scalability, determinism

Reviewer: Lea (GPT 6.1 Sol).

### 3.1 Main review

| Category | Rating 0–3 | Justification |
|---|---:|---|
| Performance | 2 | Deliberate section refreshes, but streaming still refolds the loaded transcript: [controller.rs:1298](crates/editor-conversation/src/conversation/controller.rs:1298). |
| Resource efficiency | 1 | Several useful budgets coexist with unbounded queues and continuous idle polling: [server.rs:103](crates/forge-engines/src/codex/server.rs:103), [generation.rs:163](crates/forge-http/src/generation.rs:163). |
| Scalability | 2 | Transcript virtualization and item/byte windows exist, but hidden work still requires an unbounded scan: [transcript.rs:786](crates/editor-conversation/src/conversation/view/transcript.rs:786), [read.rs:451](crates/forge-store/src/read.rs:451). |
| Determinism | 1 | HashMap order affects run selection and SSE output; UI decisions read real time directly: [fold.rs:527](crates/editor-conversation/src/conversation/fold.rs:527), [thread.rs:29](crates/editor-conversation/src/conversation/thread.rs:29). |

Reviewed committed `10f84a7`, ignoring the specified working-tree edits. Static review only; no builds, tests, or modifications.

**Findings, most severe first**

1. **High — Codex output bypasses backpressure.** [server.rs:103](crates/forge-engines/src/codex/server.rs:103) creates an unbounded channel per attached run; delivery never waits. Slow SQLite persistence stalls consumption while the shared process keeps filling these queues. Individual line limits do not bound accumulated memory. Add per-run byte budgets and an explicit overflow/cancellation policy that preserves service for other runs.

2. **High — SSE’s byte budget excludes the upstream broadcast ring.** [lib.rs:256](crates/forge-store/src/lib.rs:256) creates an 8,192-envelope ring containing owned payloads. A stalled subscriber can retain thousands of large item events before lag recovery; the downstream 8 MiB queue does not constrain this memory. At the permitted 512 KiB item size, payload retention alone can approach 4 GiB. Budget the upstream ring by bytes and share immutable payloads.

3. **High — Forge performs disk work continuously while idle.** [generation.rs:163](crates/forge-http/src/generation.rs:163) synchronizes SQLite and calls registry discovery every 50 ms. Registry discovery performs synchronous directory reads, file opens, locks, and JSON reads directly on the async worker. Slow storage blocks unrelated tasks; idle operation produces twenty polling cycles per second. Replace polling with change notifications and isolate necessary filesystem work. This violates “idle CPU must stay near zero.”

4. **Medium — Continuation folding repeatedly scans every loaded run.** [fold.rs:527](crates/editor-conversation/src/conversation/fold.rs:527) searches all runs for each predecessor, repeating that search along the continuation chain. Folding invokes this resolution repeatedly across items and summaries. Long continued turns multiply work substantially on every update. Build predecessor links once per run revision and memoize resolved roots.

5. **Medium — Work-header insertion is quadratic.** [fold.rs:781](crates/editor-conversation/src/conversation/fold.rs:781) searches the complete item and row arrays for each header, then repeatedly inserts into a `Vec`. A window containing hundreds of prompt/reply turns pays repeated scans and shifts during streaming refolds. Index positions and emit headers during one ordered pass.

6. **Medium — Growing streamed Markdown is repeatedly processed from the beginning.** [projection.rs:111](crates/editor-conversation/src/conversation/projection.rs:111) fingerprints the complete source; every changed prefix then materializes the rope and rebuilds Markdown leaf signatures synchronously. Small chunks accumulating into a long response produce quadratic cumulative parsing work on the UI path. Track revisions explicitly and incrementally reparse the affected suffix, with preparation off the UI thread.

7. **Medium — Virtualization still copies and rebuilds transcript-wide state.** [adapter.rs:173](crates/editor-conversation/src/conversation/adapter.rs:173) clones the entire `ThreadView`, rows, and summaries for each factory capture. [root.rs:675](crates/editor-aqua/src/shell/root.rs:675) additionally rebuilds item/resource maps during transcript refresh. Streaming and animation-driven refreshes therefore allocate across the loaded window even when few rows are visible. Retain revisioned snapshots and update changed entries.

8. **Medium — Collapsed work remains proportional to its entire stored history.** [read.rs:451](crates/forge-store/src/read.rs:451) reads every root item between transcript boundaries to construct work spans. A turn containing 100,000 hidden tool events still requires 100,000 row visits to display one collapsed section. Persist span summaries or aggregate them through an indexed representation; enforce a query-work budget.

9. **Medium — The hot-page cache leaks thread metadata beyond its byte budget.** [hot.rs:147](crates/forge-store/src/hot.rs:147) creates a map entry for every invalidated thread. Eviction clears held pages but never removes these entries, including after thread removal. Long-running Forge instances accumulate IDs and version records indefinitely. Bound metadata separately and preserve invalidation safety when retiring entries.

10. **Medium — Authorization performs synchronous SQLite work on request workers.** [pairing.rs:160](crates/forge-store/src/pairing.rs:160) locks a synchronous mutex and calls `refresh`, which queries SQLite. Any database commit invalidates the cached `data_version`, causing all device keys to reload—even when only thread content changed. Busy streaming increases request-path disk work. Publish authorization changes independently and perform refreshes on a blocking worker.

11. **Medium — Decoder compaction does not bound retained item count.** [decoder.rs:932](crates/forge-engines/src/claude/decoder.rs:932) retains tools indefinitely; [retained.rs:36](crates/forge-engines/src/retained.rs:36) compacts their content but does not evict records. A long-lived run with many completed tools grows continuously despite bounded frames. Retire settled records into a bounded late-update cache, retrieving durable state when necessary.

12. **Medium — Timestamp ties make displayed run state nondeterministic.** [fold.rs:527](crates/editor-conversation/src/conversation/fold.rs:527) chooses predecessors from HashMap iteration using only timestamps. [fold.rs:1192](crates/editor-conversation/src/conversation/fold.rs:1192) likewise sorts continuation members only by `started_at`, then takes the last member’s phase and activity. Equal timestamps can change grouping or displayed status across executions. Use explicit continuation identity and a total ordering.

13. **Medium — UI timing cannot be controlled by a test clock.** [thread.rs:29](crates/editor-conversation/src/conversation/thread.rs:29) uses `Instant::elapsed()` to decide freshness, while [root.rs:2161](crates/editor-aqua/src/shell/root.rs:2161) reads wall time to schedule work-clock refreshes. Replaying identical events under different scheduling changes reveal decisions and deadlines. Inject one clock throughout presentation and scheduling.

14. **Low — HashMap iteration reaches the SSE wire.** [lib.rs:730](crates/forge-runs/src/lib.rs:730), read from HEAD, emits live snapshots by iterating run, target, and suggestion maps without sorting. Identical state produces differently ordered reconnect streams, complicating reproducible transcript assertions. Sort snapshots by stable thread and target keys.

SQLite uses WAL `synchronous=NORMAL`, so a claim of **fsync on every SQLite commit is unsupported**. Image loading has decode limits, caching, and notifications; shimmer uses scene effects. A **512-item transcript freeze remains unverified** at this HEAD: virtualization and an 800-item window are present.

**What perfect looks like**

- **Performance:** Incremental folding and Markdown preparation; reusable transcript snapshots; CI budgets for streaming, scrolling, rebuild counts, and frame time.
- **Resource efficiency:** Byte budgets across every queue and cache, including metadata; event-driven idle operation; blocking IO confined to dedicated workers.
- **Scalability:** Indexed work-span summaries; bounded decoder retention; end-to-end scenarios at 10×/1000× data and across 511/512/513 items.
- **Determinism:** Explicit continuation relationships, total output ordering, injected clocks, and replay tests covering timestamp ties and scheduling differences.

| Score | Slice only |
|---|---:|
| Correctness | 6/10 |
| State handling | 5/10 |
| Code quality | 5/10 |
| Rule adherence | 5/10 |
| **Overall** | **5/10** |

### 3.2 Remaining findings and nits

15. **Medium — MCP replies can accumulate without backpressure.** [claudeai/mcp.rs:1143](crates/forge-engines/src/claudeai/mcp.rs:1143) creates an unbounded outgoing queue for both stdio and legacy SSE transports. The reader enqueues responses to server requests independently of writer progress. A server that floods requests while neglecting its input grows Forge’s memory indefinitely. Bound queued bytes and terminate an unresponsive transport when its budget is exhausted.

16. **Medium — Blob uploads occupy blocking workers while waiting for one mutex.** [blobs.rs:167](crates/forge-store/src/blobs.rs:167) starts a blocking task before acquiring `blob_lock`. Concurrent uploads therefore retain their complete buffers and consume blocking threads while waiting for serialized persistence. Slow storage amplifies both costs. Acquire asynchronous concurrency and byte permits before buffering or scheduling the blocking work.

17. **Medium — Tunneled requests spawn unlimited detached tasks.** [relay.rs:504](crates/forge-http/src/relay.rs:504) spawns every unary request without admission control or a task handle tied to the channel. A client can accumulate slow requests, and closing its channel does not cancel them. Their completed responses can then wait behind the shared response queue. Bound concurrent requests and retained bytes, and cancel channel-owned tasks on closure.

18. **Medium — Tunnel queues have envelope limits but no aggregate byte limit.** [relay.rs:543](crates/forge-http/src/relay.rs:543) buffers 256 owned envelopes per subscription; [relay.rs:147](crates/forge-http/src/relay.rs:147) adds another 256-message shared queue. Neither applies the direct SSE queue’s byte accounting. Slow tunneled consumers can retain hundreds of large payloads at these additional stages. Carry byte permits through serialization and forwarding.

19. **Medium — Thread pagination automatically downloads the complete active history.** [pages.rs:44](crates/editor-core/src/core/host/pages.rs:44) requests another older page whenever a cursor remains, and `finish_thread_page` calls it after every successful response. Page IDs accumulate and are explicitly retained by pruning. Connecting to a host with thousands of active threads eventually loads all of them without scrolling. Fetch older pages on viewport demand and enforce a retained window.

20. **Medium — Merging thread pages becomes quadratic.** [pages.rs:209](crates/editor-core/src/core/host/pages.rs:209) searches the growing ID vector for every incoming thread, then sorts the entire vector after each page. The automatic pagination above repeatedly performs this work on the UI thread. Maintain membership separately and merge already ordered pages without repeatedly sorting the complete history.

21. **Medium — Inspector refreshes reconstruct an unbounded prompt outline.** [read.rs:901](crates/forge-store/src/read.rs:901) reads every qualifying user message, then performs a content-range query for each stored message. The resulting outline is also returned in full with every inspector response. Long conversations turn routine inspector refreshes into history-wide queries and large responses. Persist compact outline entries and retrieve them incrementally.

22. **Medium — Message search has a row budget but no byte-work budget.** [text_search.rs:149](crates/forge-store/src/text_search.rs:149) scans up to 60,000 text rows, each searchable field potentially containing 64 KiB. Archived-thread and per-thread exclusions happen after reading and matching the text. An unsuccessful search can process gigabytes while producing no results. Filter eligible rows earlier and enforce a byte or execution-work budget, preferably using a suitable search index.

23. **Medium — Recall indexing hydrates complete collections before truncating their contribution.** [recall.rs:70](crates/forge-store/src/recall.rs:70) resolves stored collections into a complete vector. The question, plan, and edit loops continue visiting records after their searchable output reaches its 64 KiB ceiling. Updating an item with a large stored collection unnecessarily loads and processes the complete collection on the sole writer. Visit pages and stop when the relevant indexing budgets are exhausted.

24. **Medium — Building one source index can monopolize the store writer.** [source_index.rs:158](crates/forge-store/src/source_index.rs:158) inserts every derived leaf in one writer transaction. Literal sources with many short lines can produce a large number of inserts even though callers request only 64 descriptors. All message and state writes wait behind that transaction. Bound persistence work per writer turn and publish the completed immutable index after staged insertion.

25. **Medium — Cookie persistence synchronously fsyncs an async request worker.** [transport.rs:365](crates/forge-engines/src/claudeai/transport.rs:365) calls `Session::update` directly after receiving HTTP headers. That operation takes a process-wide mutex, reads and rewrites JSON, and calls `sync_all` at [session.rs:191](crates/forge-engines/src/claudeai/session.rs:191). Concurrent cookie-bearing responses serialize behind disk latency on runtime workers. Move persistence to a dedicated worker and avoid rewriting unchanged cookies.

26. **Medium — HTML entity decoding has quadratic worst-case work.** [tools.rs:1641](crates/forge-engines/src/claudeai/tools.rs:1641) searches the complete remaining suffix for `;` before checking the twelve-character entity limit. A fetched HTML body containing many `&` characters without semicolons repeatedly scans almost the entire remainder. Conversion runs synchronously inside `web_fetch`. Search only the bounded entity prefix and move substantial conversion work off runtime workers.

27. **Medium — Retaining code fences performs quadratic matching.** [presenter.rs:422](crates/editor-ui/src/ui/markdown/presenter.rs:422) searches the complete previous rich-entry vector for each code fence. Even unchanged fences late in a document repeatedly scan preceding empty slots. A response containing many fences pays quadratic matching on each changed preparation. Index reusable entries by fence identity or content fingerprint, preserving duplicate occurrences explicitly.

28. **Medium — Row heights survive transcript-window eviction.** [transcript.rs:665](crates/editor-conversation/src/conversation/view/transcript.rs:665) records heights by row key. The controller clears them on conversation reset or close, but ordinary page trimming does not retain only current rows; individual removal covers disclosure releases. Scrolling through a long conversation therefore grows this map beyond the bounded item window. Prune heights against retained rows and active disclosure requirements.

29. **Medium — Parked-thread accounting excludes much of the parked object.** [controller.rs:414](crates/editor-conversation/src/conversation/controller.rs:414) charges only `view.resident_bytes()` while parking the view, projections, and row-height map together. The view’s counter itself tracks item bodies rather than its context, live text, runs, and map allocations. The advertised 128 MiB shelf budget can substantially understate retained memory. Account for the complete parked state.

30. **Medium — Project icons hold strong image references outside the image-cache accounting.** [project_icons.rs:89](crates/editor-core/src/core/host/project_icons.rs:89) stores a strong image handle in each project entry. Up to 128 entries remain, independently of the host image cache’s 16 MiB accounting. Evicting an image from that cache therefore need not release it. Centralize decoded-image ownership or give icon retention its own byte budget and suitably sized thumbnails. The resulting process-memory peak is **suspected**, not measured.

31. **Medium — Publishing a resource can evict unfinished work or its own result.** [resource_cache.rs:395](crates/editor-ui/src/ui/media/resource_cache.rs:395) removes the oldest entry indiscriminately when decoded output exceeds the budget. That entry may still be loading; its task is aborted without waking its waiters. A sufficiently expensive result can also be immediately removed, so the next request starts the same work again. Protect pending entries, notify displaced consumers, and return a stable failure for an individually uncacheable result.

32. **Medium — Background shells have no aggregate process or retention budget.** [shells.rs:88](crates/forge-engines/src/claudeai/shells.rs:88) inserts every shell into the run’s map. Completed and killed shells are never removed, and starting another shell has no count limit. A long turn can retain many processes or hundreds of completed 256 KiB unread buffers. Bound active shells and retire completed records into a small, explicitly budgeted history.

33. **Medium — Terminal overflow turns small updates into full-preview publications.** [lib.rs:1106](crates/forge-runs/src/lib.rs:1106), at `10f84a7`, clones the retained terminal preview and publishes a replacement whenever an append crosses its limit. Once a preview is full, every small subsequent chunk can copy and transmit roughly 256 KiB. High-volume shell output causes disproportionate allocation, serialization, and network work. Represent prefix removal explicitly or coalesce bounded replacement snapshots.

34. **Medium — A slow Codex attachment blocks unrelated attachments.** [server.rs:57](crates/forge-engines/src/codex/server.rs:57) holds the global server-pool mutex across thread-release waits and server startup. The release timeout alone is sixty seconds. One stalled handoff can delay launches for unrelated sessions and launch configurations. Keep the pool lock around ownership changes and perform process operations outside it.

35. **Medium — Suspected bidirectional pipe deadlock in the Codex actor.** [server.rs:480](crates/forge-engines/src/codex/server.rs:480) awaits complete stdin writes in the same actor that drains stdout. `EngineProcess::send` has no write deadline. If Codex stops reading stdin while blocked writing stdout, Forge stops draining the pipe needed to make progress, and cancellation commands queue behind the write. Separate bounded writing from reading and make stalled writes cancellable. Codex’s behavior under this pressure was not exercised.

36. **Medium — Codex child-path metadata outlives its ownership.** [server.rs:711](crates/forge-engines/src/codex/server.rs:711) adds child IDs and paths to `agent_paths`. Neither thread closure nor run detachment removes these entries. A shared server kept alive by successive runs retains historical child paths until the entire process exits. Remove metadata with its owning thread or run and bound any history needed for late notifications.

37. **Medium — Warm MCP processes have a timeout but no global capacity.** [mcp.rs:239](crates/forge-engines/src/claudeai/mcp.rs:239) retains a complete client set for each thread in the global `WARM` map. Entries expire after ten minutes, but there is no count, process, or memory budget. Visiting many threads in that interval keeps all their stdio servers alive despite the run-concurrency bound. Apply a global warm-process budget with eviction.

38. **Medium — Config manifests are fully buffered without size admission.** [sync.rs:225](crates/forge-http/src/sync.rs:225) downloads the complete manifest with `.bytes()`, then deserializes all files and decodes their base64 contents. There are no manifest, file-count, or aggregate decoded-byte limits. An accidentally oversized published bundle can exhaust local memory or disk. Enforce streaming download and decoded-bundle budgets before installation.

39. **Medium — Config synchronization performs synchronous filesystem work on async workers.** [sync.rs:187](crates/forge-http/src/sync.rs:187) reads state synchronously; successful synchronization calls synchronous file writing and renaming at line 256. Bundle staging also creates and drops a filesystem-backed temporary directory inside the async operation. Slow filesystem operations block unrelated runtime work. Put synchronous state persistence and staging cleanup on blocking workers.

40. **Medium — One failed coordination receipt starves every later receipt.** [coordination.rs:225](crates/forge-runs/src/coordination.rs:225) propagates the first delivery error out of the loop. The query orders by `updated_at,id`, so the same failing oldest record leads every subsequent poll. Later deliverable receipts never get attempted. Process failures independently and maintain explicit delivery eligibility so one record cannot block the queue.

41. **Medium — The writer queue limits jobs rather than retained payload bytes.** [lib.rs:437](crates/forge-store/src/lib.rs:437) queues closures owning their complete inputs. Its 256-job capacity does not account for large items, configuration values, or prepared data; callers waiting to enqueue also retain their inputs. Slow writes can therefore accumulate far more memory than the queue count suggests. Reserve byte capacity before accepting large write jobs and retain the permit through execution.

42. **Medium — Aborting a software-model render does not stop its blocking computation.** [model/view.rs:237](crates/editor-ui/src/ui/rich/media/model/view.rs:237) stores the outer async task’s abort handle, while rasterization runs in a separately spawned blocking task. Removing the entry aborts the waiter, but an already-running rasterizer continues allocating and computing. Rapidly opening and dismissing software-rendered models can leave overlapping obsolete work. Bound raster workers and check cancellation during rendering.

43. **Medium — Filesystem traversal order changes tool results.** [tools.rs:883](crates/forge-engines/src/claudeai/tools.rs:883) configures no stable ordering before applying `MAX_WALKED`. Glob additionally truncates matches before sorting at line 927, and Grep returns matches in traversal order. Identical trees with different directory enumeration order can produce different subsets and therefore different agent context. Apply deterministic traversal ordering before either bound.

44. **Low — Closed relay channels leave receive-sequence entries behind.** [relay.rs:331](crates/forge-http/src/relay.rs:331) removes channel, stream, reassembly, and send-window state but leaves `relay_recv_seq` untouched. The local close helper also cannot access that map. A long-lived relay connection accumulates historical channel names. Remove sequence state on both closure paths.

45. **Low — Grant verification blocks the shared relay receive loop.** [relay.rs:408](crates/forge-http/src/relay.rs:408) awaits grant verification inline. A missing or expired JWKS can trigger a network request with a ten-second timeout while the connection stops processing all other channels, acknowledgements, and pings. Perform channel admission independently and return its result to the connection loop.

46. **Low — Registry lock files accumulate permanently.** [registry.rs:41](crates/forge-runs/src/runtime/registry.rs:41) removes lease metadata but leaves the created lock file. Generation and thread IDs continually introduce new files, and registry directory enumeration encounters those historical entries. Long-lived installations accumulate filesystem metadata and increasing discovery work. Retire obsolete registry artifacts safely or replace unnecessary per-identity files with bounded ownership state.

47. **Low — Search snippets allocate the complete preceding line.** [text_search.rs:93](crates/forge-store/src/text_search.rs:93) collects every preceding character and byte offset merely to keep the last 24 characters. A match near the end of a long line allocates a large temporary vector for a tiny snippet. Walk backward with `char_indices().rev().take(...)`.

48. **Low — Background output retention repeatedly shifts its entire buffer.** [shells.rs:240](crates/forge-engines/src/claudeai/shells.rs:240) drains the front of a `String` after each overflowing chunk. A continuously noisy shell repeatedly moves most of its 256 KiB retained output. Use a chunk deque or circular byte buffer with UTF-8-aware boundaries.

49. **Low — Every stored source is scanned as a diff.** [content.rs:503](crates/forge-store/src/content.rs:503) computes `diff_totals` for every source, including ordinary prose and derived index metadata, before discovering whether the content already exists. Repeated large non-diff sources pay an unnecessary full line scan on the writer. Compute diff statistics only where needed, and after determining that insertion is necessary.

50. **Low — Image upload constructs a second complete image buffer.** [transport.rs:507](crates/forge-engines/src/claudeai/transport.rs:507) copies the fully read image into a manually assembled multipart body. A maximum-size admitted image temporarily occupies both buffers before request transmission. Use streaming multipart upload or transfer ownership without retaining the original allocation.

51. **Low — Engine line handling discards reusable allocation capacity.** [process.rs:244](crates/forge-engines/src/process.rs:244) takes the accumulated byte vector, converts it, and returns another owned string, leaving an empty vector for the next frame. Valid UTF-8 lines commonly incur both fresh accumulation and a second allocation. Transfer valid UTF-8 buffers directly or retain reusable scratch capacity.

52. **Low — Chart paints rebuild unchanged path geometry.** [scene.rs:185](crates/editor-ui/src/ui/rich/charts/scene.rs:185) allocates transformed points and path commands, then makes four separate bounds passes for each path. The canvas paint callback invokes this conversion for unchanged chart shapes. Repainting during surrounding motion repeats preparation work. Cache local path geometry by shape and dimensions, and calculate bounds in one pass.

53. **Low — Search-hit highlighting finishes scanning after finding a match.** [reveal.rs:80](crates/editor-aqua/src/shell/root/reveal.rs:80) traverses the complete row subtree and computes matches for every text node before taking the first successful result. Each match operation also lowercases the needle again. The reveal poll repeats this work while selection waits. Stop traversal at the first match and prepare the needle once per reveal.

54. **Low — Local settings persistence blocks the UI path.** [store.rs:467](crates/editor-core/src/core/store.rs:467) synchronously saves settings during state mutation. `write_json` creates directories and a temporary file, serializes, flushes, and replaces the destination. Slow profile storage or filesystem scanning can stall a settings interaction. Persist ordered snapshots asynchronously while maintaining explicit pending and failed states.

55. **Low — Resource tests remain embedded in source files.** [process.rs:328](crates/forge-engines/src/process.rs:328), [content.rs:528](crates/forge-store/src/content.rs:528), and [events.rs:399](crates/forge-http/src/events.rs:399) contain complete test modules. This violates **“Tests live in `tests/`, mirroring the path in `src/`.”** Failure scenario: test discovery and maintenance require searching production modules as well as the test hierarchy. Move these tests into mirrored files.

56. **Low — Hot-path SQL remains embedded in Rust.** [read.rs:606](crates/forge-store/src/read.rs:606), [text_search.rs:149](crates/forge-store/src/text_search.rs:149), and [source_index.rs:158](crates/forge-store/src/source_index.rs:158) define queries inline. This violates **“Queries live in separate `.sql` files.”** Failure scenario: query changes remain mixed with control flow, making indexing and query-work review harder. Extract the queries into the existing SQL hierarchy.

57. **Low — Store queries lack the required SQLx checking.** [read.rs:83](crates/forge-store/src/read.rs:83) decodes rusqlite rows and serialized bodies at runtime; the audited queries use runtime SQL strings and positional conversions. This violates **“Typed queries: SQLx in Rust.”** Failure scenario: schema or selected-column drift reaches runtime instead of failing query validation. Introduce the required typed query checking.

58. **Nit — Explanatory comments violate the explicit code rule.** Examples include [trace.rs:87](crates/forge-engines/src/trace.rs:87), [resource_cache.rs:1](crates/editor-ui/src/ui/media/resource_cache.rs:1) through its surrounding implementation’s retained commentary elsewhere, and [sources.rs:41](crates/forge-store/src/sources.rs:41). The applicable rule is **“Never write code comments: no file headers, doc comments, inline explanations or TODOs.”** Cosmetic. Remove explanatory comments and express essential invariants through names and structure, preserving permitted tool directives.

59. **Nit — Transcript list construction is duplicated three times.** [transcript.rs:679](crates/editor-conversation/src/conversation/view/transcript.rs:679), line 750, and line 801 repeat row-vector construction, key formatting, height capture, virtualization, and viewport configuration. Cosmetic. Share the common preparation and list configuration so future virtualization changes cannot silently diverge.

60. **Nit — `row_destination` has no repository caller.** [scroll.rs:134](crates/editor-conversation/src/conversation/scroll.rs:134) exposes a helper unused anywhere in the repository; current scrolling uses other collection APIs. Cosmetic. Remove the unused surface unless an actual consumer requires it.

## 4. Reliability, recovery, data integrity, concurrency, network, compatibility, portability

Reviewer: Kristine (GPT 6.1 Sol).

### 4.1 Main review

| Category | Rating 0–3 | Justification |
|---|---:|---|
| Reliability | 2 | Deliberate lifecycle handling, but shared engine I/O can hang indefinitely: `forge-engines/src/codex/server.rs:480`. |
| Resilience and recovery | 2 | Restart recovery exists, but continuation consumption has a crash window: `forge-runs/src/run.rs:45`. |
| Data integrity | 2 | Transactions protect database mutations; durability and blob ownership still have gaps: `forge-store/src/lib.rs:249`, `blobs.rs:239`. |
| Concurrency | 2 | Ownership and admission are explicit; snapshot/publication boundaries remain inconsistent: `forge-runs/src/runtime/mod.rs:158`. |
| Network tolerance | 2 | Reconnect and replay exist, but stalled streams and regressed server cursors are unhandled: `editor-core/src/core/connection.rs:168`. |
| Compatibility | 2 | Schema versions and transactional migrations are checked; historical tests still construct databases using current migrations: `forge-store/tests/schema/model.rs:7`. |
| Portability | 2 | Platform adapters exist, but WSL path conversion discards the distribution: `forge-runs/src/resources.rs:236`. |

Reviewed committed `10f84a7ada6519109564cd433aed23c4e36676fa`; ignored concurrent edits. Findings are verified through code inspection. No builds or tests were run.

1. **High — Closing offline loses unsaved drafts.** [launch.rs:173](crates/editor-aqua/src/launch.rs:173) drains for five seconds after the window closes, then only logs failure and exits. Failed drafts have no local durable fallback. Typing while Forge is unreachable, then closing, loses that text. Persist pending drafts locally before shutdown.

2. **High — Initial draft loading can silently overwrite an existing draft.** [draft_session.rs:239](crates/editor-composer/src/composer/draft_session.rs:239) keeps locally typed text but adopts the fetched revision without detecting a conflict. Type before a slow load returns: the next save overwrites the unseen remote draft. Preserve both versions and require resolution.

3. **High — Blob collection can delete newly referenced images.** [blobs.rs:239](crates/forge-store/src/blobs.rs:239) reads ownership before taking the collection lock. Message/draft writes do not participate in that lock. Referencing an existing, hour-old blob after the snapshot can commit successfully before GC deletes it. Serialize reference admission with collection, or maintain transactional ownership.

4. **High — A crash can consume a continuation permanently.** [run.rs:45](crates/forge-runs/src/run.rs:45) commits deletion of the continuation before saving its replacement run at line 91. A crash between those operations leaves neither pending input nor a running run for recovery. Consume the continuation and create the run in one transaction.

5. **High — Storage errors can leave permanently running records on Windows.** [run.rs:1285](crates/forge-runs/src/run.rs:1285) writes an error item before settling the run; failure returns early. The caller removes its live ownership, while orphan recovery is generation-only. ENOSPC or sustained SQLite contention can leave a dead run marked running until restart. Retain explicit failed-persistence recovery state.

6. **High — Unix containment does not survive backend death.** [process.rs:123](crates/forge-engines/src/process.rs:123) establishes a process group; cleanup depends on destructors. SIGKILL/OOM bypasses them, and the isolation hook supplies no parent-death cleanup. Tools can survive while the gateway starts replacement work. Give the surviving supervisor ownership of engine cleanup across backend crashes.

7. **High — One blocked stdin can freeze every attached Codex run.** [server.rs:480](crates/forge-engines/src/codex/server.rs:480) awaits process writes inside the sole routing actor. A child that stops reading stdin blocks stdout draining, detach, and interrupt commands, without a write deadline. Separate reading from bounded writing and terminate an unresponsive server.

8. **High — Acknowledged database commits are not power-loss durable.** [lib.rs:249](crates/forge-store/src/lib.rs:249) explicitly selects WAL `synchronous=NORMAL`. Saved drafts, accepted messages, and sequence advances can disappear after power loss despite successful responses. Use a durability policy that synchronizes acknowledged user data.

9. **High — Reconnecting ahead of the database silently suppresses updates.** [events.rs:172](crates/forge-http/src/events.rs:172) accepts `since` above the current head; its cursor rejects subsequent sequences below that value. The Editor checks Hello’s protocol but ignores head regression. Restoring an older database can leave it “Live” while missing updates. Reset/resnapshot when the cursor exceeds the server head.

10. **Medium — Cross-generation reconnect can duplicate streaming text.** [runtime/mod.rs:158](crates/forge-runs/src/runtime/mod.rs:158) captures the local publication revision before fetching peer snapshots. A peer delta relayed during that fetch can appear in both the snapshot and queued delivery. The Editor blindly appends it. Deduplicate using coverage or establish one ordered snapshot boundary.

11. **Medium — An established SSE stream has no liveness deadline.** [connection.rs:168](crates/editor-core/src/core/connection.rs:168) waits for stream data indefinitely. A stalled peer or proxy that keeps TCP alive leaves the Editor marked Live and prevents reconnect. Enforce an inactivity deadline against Forge’s heartbeat interval.

12. **Medium — OpenCode launches race against an in-place configuration rewrite.** [opencode.rs:437](crates/forge-engines/src/opencode.rs:437) truncates the shared configuration directly. Concurrent initial launches or configuration changes can expose an empty/partial file to a starting engine. Publish complete configuration atomically, preferably per launch.

13. **Medium — WSL links can open the wrong distribution’s file.** [resources.rs:236](crates/forge-runs/src/resources.rs:236) discards the UNC distribution, then maps the remainder into the current Linux root. `\\wsl.localhost\OtherDistro\home\...` can resolve to unrelated local content. Validate the distribution and reject or explicitly translate foreign paths.

14. **Medium — Atomic configuration replacement is not durable replacement.** [hosts.rs:127](crates/editor-core/src/core/hosts.rs:127) flushes userspace buffers and persists the temporary file without synchronizing file contents or directory metadata. Power loss can lose pairing identity or host configuration after success. Synchronize the file and use platform-appropriate durable replacement.

15. **Medium — Permanent read-ack failures block acknowledgments for every thread.** [host.rs:762](crates/editor-core/src/core/host.rs:762) retries every error forever while occupying the single writer. Deleting a thread removes its queued acknowledgment but leaves this task retrying its 404. Cancel obsolete work and classify terminal failures. This also violates: “Retries around flaky paths are bandaids; make paths reliable by construction.”

To reach **3** in each category:

- **Reliability:** Bound engine writes; test blocked children and storage failures through the real run lifecycle.
- **Resilience and recovery:** Atomically claim continuations; retain offline drafts durably; verify recovery after abrupt backend death.
- **Data integrity:** Make acknowledgments durable; synchronize blob ownership with GC; exercise ENOSPC and power-loss boundaries.
- **Concurrency:** Fix snapshot/delta overlap and draft-load conflicts; enforce their invariants with deterministic interleavings.
- **Network tolerance:** Add heartbeat deadlines and head-regression resets; test reconnect during publication and restoration.
- **Compatibility:** Maintain frozen fixtures for every released schema and test previous-release client/server combinations.
- **Portability:** Preserve WSL distribution identity; exercise native Windows, Linux, macOS Forge, and WSL lifecycle/path behavior in CI.

| Score | This slice |
|---|---:|
| Correctness | 4/10 |
| State handling | 4/10 |
| Code quality | 6/10 |
| Rule adherence | 3/10 |
| **Overall** | **4/10** |

### 4.2 Remaining findings and nits

16. **High — Agent creation can leave a permanently working agent without any input.** [crates/forge-runs/src/agents.rs:292](crates/forge-runs/src/agents.rs:292), at `10f84a7`, commits the child thread, parent Agent item, and initial message separately. A crash or storage failure before the final write leaves a running Agent whose child has no run or pending message; `child_state()` classifies that state as Working indefinitely. Create all three records in one transaction, then dispatch.

17. **High — Retention deletes saved drafts on inactive threads.** [crates/forge-store/src/retention/threads.sql:5](crates/forge-store/src/retention/threads.sql:5) considers thread timestamps, runs, items, and continuations, but excludes drafts. Draft saves do not refresh thread activity, and `retention/remove.sql:5` deletes the draft. A recently autosaved draft on an old thread can disappear during collection. Protect threads with nonempty drafts or record draft activity for retention.

18. **High — Attached Codex output has no aggregate memory bound.** [crates/forge-engines/src/codex/server.rs:103](crates/forge-engines/src/codex/server.rs:103) creates an unbounded channel per run; `deliver()` enqueues frames without backpressure. The individual line limit does not bound queued lines. Fast engine output combined with slow decoding or persistence can exhaust Forge’s memory. Apply a byte budget and explicitly fail or detach an overloaded consumer.

19. **High — Offline revocation does not close a passive event subscriber.** [crates/forge-http/src/events.rs:343](crates/forge-http/src/events.rs:343) waits for in-process revocation announcements. CLI revocation changes SQLite, but its discovery depends on another request calling `is_authorized()`. A revoked client that only receives SSE can continue reading indefinitely. Make external revocations observable independently of subsequent requests.

20. **High — Offline administration can migrate a running older Forge’s database.** [crates/forge-store/src/admin.rs:31](crates/forge-store/src/admin.rs:31) runs migrations without acquiring the service’s exclusive data-directory lock. Running a newer CLI’s pairing or device command beside an older Forge can rewrite stored payloads and indexes beneath that process. Require exclusive ownership before migration; administration beside a running service should require an already compatible schema.

21. **Medium — Cross-generation cancellation can deadlock admission until RPC timeout.** [crates/forge-runs/src/lib.rs:604](crates/forge-runs/src/lib.rs:604) holds local admission while forwarding cancellation to another generation, whose `cancel_local()` takes its own admission lock. Simultaneous opposite-direction cancellations can each hold one lock and wait for the other. Both requests can time out with cancellation incomplete. Avoid holding local admission across peer RPCs.

22. **Medium — Background-agent completion and result delivery are separate durable decisions.** [crates/forge-runs/src/agents.rs:800](crates/forge-runs/src/agents.rs:800) calls `refresh_agent()`, which persists terminal status, before `deliver_result()` inserts the parent’s message. A crash or failed delivery leaves the Agent terminal, so startup’s working-agent reconciliation does not retry the notification. Commit completion and its parent input together, with dispatch following that commit.

23. **Medium — Failure of one input batch also freezes inputs outside that batch.** [crates/forge-store/src/read.rs:198](crates/forge-store/src/read.rs:198) computes the cutoff across all pending inputs but returns at most 32 rows within its byte budgets. That cutoff becomes the run’s failure boundary. If the first batch fails, `pending_threads()` excludes later messages that were never delivered. Keep observed and attempted boundaries separate, and give unattempted inputs an explicit disposition.

24. **Medium — An overdue parked turn causes dispatcher spinning when capacity is unavailable.** [crates/forge-runs/src/lib.rs:842](crates/forge-runs/src/lib.rs:842) immediately wakes again for an already-due continuation after failing to acquire a harness slot. The same happens while a generation is draining. It repeatedly queries SQLite instead of waiting for capacity or ownership to change. Make the parked deadline conditional on admission being possible.

25. **Medium — A parked-deadline read failure can disable automatic resumption.** [crates/forge-runs/src/park.rs:103](crates/forge-runs/src/park.rs:103) converts a database error into “no parked turn,” represented by a forever-pending future. In standalone mode there is no generation polling fallback. A transient read failure during an otherwise quiet period can leave due work asleep until another action wakes dispatch. Preserve a recoverable scheduling-error state.

26. **Medium — The disk guard can retain an unsafe threshold permanently.** [crates/forge-runs/src/disk.rs:91](crates/forge-runs/src/disk.rs:91) initializes `stop_below` once from the first sampled free space and never raises it after recovery. Starting below 1 GiB produces a zero threshold, making `free < stop_below` impossible. Recovering space also leaves the original reduced threshold in effect. Restore the normal threshold after recovery and retain an absolute exhaustion floor.

27. **Medium — A relay unary request can block every subsequent unary request indefinitely.** [crates/artisan-account/src/relay_client.rs:163](crates/artisan-account/src/relay_client.rs:163) holds the shared unary mutex through an exchange without a request deadline. Editor `call_relay()` adds none. A connected peer that never completes a response blocks draft saves, cancellation, and other calls behind it. Bound the complete exchange and discard an incomplete connection.

28. **Medium — Large relay responses overflow before acknowledgments can be processed.** [crates/forge-http/src/relay.rs:648](crates/forge-http/src/relay.rs:648) sends every chunk inside one actor turn. Once the send window fills, chunks accumulate in the 256-entry parked queue; the actor cannot return to its receive loop to process acknowledgments. A sufficiently large response, still below the supported transfer limit, necessarily closes the channel. Resume chunk production as acknowledgments release capacity.

29. **Medium — Relay size limits contradict supported binary operations.** [crates/editor-core/src/core/client.rs:889](crates/editor-core/src/core/client.rs:889) base64-encodes binary requests inside JSON, while tunnel reassembly limits the resulting message to 32 MiB. A valid 25 MiB blob therefore exceeds the transfer budget. Resource reads support 128 MiB directly, but relay dispatch caps raw responses at 32 MiB. Align operation limits with encoded transport sizes or transfer binary bodies separately.

30. **Medium — TLS handshakes have neither a deadline nor admission bound.** [crates/forge-http/src/lib.rs:416](crates/forge-http/src/lib.rs:416) waits until handshake completion or server shutdown. The accept loop keeps spawning connection tasks. On a configured LAN listener, clients can hold sockets open without completing TLS and exhaust descriptors or memory. Bound concurrent handshakes and their duration.

31. **Medium — Unreadable configuration can subsequently be replaced with defaults.** [crates/editor-core/src/core/store.rs:131](crates/editor-core/src/core/store.rs:131) constructs an empty hosts configuration after initialization failure; settings parsing independently falls back to an empty map at line 146. Later normal saves can overwrite the original files with that reduced state. Preserve unreadable files and prevent replacement until recovery is explicit.

32. **Medium — Concurrent identity creation can persist a different identity from the one returned.** [crates/editor-core/src/core/hosts.rs:78](crates/editor-core/src/core/hosts.rs:78) performs read, generate, and replace without exclusive creation. Two first callers can generate different identities, return both, and overwrite each other’s file. Pairing with the losing identity then breaks after restart. Publish once without clobbering and return the winning stored identity.

33. **Medium — Atomic configuration replacement does not prevent lost updates between owners.** [crates/editor-core/src/core/hosts.rs:91](crates/editor-core/src/core/hosts.rs:91) writes complete caller-owned snapshots without locking or checking a revision. Two owners of the same directory can independently add hosts or change settings, then erase each other’s changes. Serialize read-modify-write operations or detect stale revisions.

34. **Medium — Navigating away can lose an unfinished image attachment even while online.** [crates/editor-composer/src/composer/controller.rs:638](crates/editor-composer/src/composer/controller.rs:638) persists only attachments that already have blob hashes. Encoding and uploading attachments have none, and dropping their composer aborts their tasks. Paste an image and switch threads before upload completes: the saved draft contains no recoverable attachment. Let draft ownership retain pending attachment work and its source.

35. **Medium — Saving a draft can panic on a non-Unicode file path.** [crates/editor-composer/src/composer/controller.rs:661](crates/editor-composer/src/composer/controller.rs:661) assumes `DraftBody` serialization cannot fail, although `DraftFile` contains `PathBuf`. Serde rejects paths that cannot be represented as Unicode. Attaching such a file can terminate the Editor instead of reporting a draft error. Use an explicit path representation or propagate serialization failure.

36. **Medium — Config-bundle application can commit a partial bundle and lose ownership information.** [crates/forge-http/src/sync.rs:302](crates/forge-http/src/sync.rs:302) installs skills, enables them, saves the prompt, and mutates MCP entries through separate operations; the ownership state file is written afterward. Failure midway leaves mixed versions. A crash after adding an MCP server but before recording ownership can also prevent its removal by later bundles. Validate first, transact database changes with ownership, and make filesystem publication recoverable.

37. **Medium — Prompt-write failure does not prevent launching an engine with stale instructions.** [crates/forge-engines/src/customise.rs:110](crates/forge-engines/src/customise.rs:110) logs `PromptApplied::Failed` and continues successfully. Disk-full or permission failure can therefore launch a turn without the prompt the user saved. Surface this as a launch failure or an explicit user-visible degraded state.

38. **Medium — Skill replacement has an unrecovered crash window.** [crates/forge-engines/src/customise/skills.rs:1000](crates/forge-engines/src/customise/skills.rs:1000) moves the installed skill to backup before publishing staging. Its rollback only runs on an ordinary returned error. A crash between the renames leaves the canonical skill missing, and startup does not restore it. Add recoverable replacement state or a publication method that preserves the installed version until replacement succeeds.

39. **Medium — Windows skill copies are published incrementally.** [crates/forge-engines/src/customise.rs:265](crates/forge-engines/src/customise.rs:265) copies files directly into the engine’s visible skill directory when symlinks are unavailable. Updating first removes the old entry. A running engine can observe a missing or partial skill; copy failure leaves an incomplete directory without its ownership marker. Stage complete copies and publish them as one replacement.

40. **Medium — Quarantine replacement deletes the previous copy before securing the new one.** [crates/forge-engines/src/customise/skills.rs:147](crates/forge-engines/src/customise/skills.rs:147) removes an existing quarantine entry before moving the newly discovered copy. If that move or cross-device copy fails, the previously retained version is already gone. Preserve the old entry until the new copy is complete.

41. **Medium — Resource validation can race into a blocking special-file open.** [crates/forge-runs/src/resources.rs:285](crates/forge-runs/src/resources.rs:285) checks metadata by path, then opens that path separately. On Unix, replacing the regular file with a FIFO between those operations can block the worker indefinitely; byte limits do not bound opening time. Open with appropriate nonblocking semantics and validate the opened handle.

42. **Medium — Historical resource snapshots can capture a later file version.** [crates/forge-runs/src/resources.rs:345](crates/forge-runs/src/resources.rs:345) reads referenced files in detached work after the assistant item is committed. A subsequent turn can overwrite the same filename before that work runs, permanently associating the later artifact with the earlier reply. Capture immutable bytes before publishing the item’s snapshot association.

43. **Medium — Pending Editor calls disappear across reconnect.** [crates/forge-runs/src/gateway.rs:105](crates/forge-runs/src/gateway.rs:105) publishes calls only as memory-only events. The live snapshot includes streaming text and suggestions, but not pending calls. A connection interruption at publication loses the call even if the Editor reconnects within its 90-second answer window. Include outstanding calls in reconnect state until answered or cancelled.

44. **Medium — A delayed read acknowledgment can resurrect a deleted thread.** [crates/editor-core/src/core/host.rs:1151](crates/editor-core/src/core/host.rs:1151) accepts an acknowledgment response when the thread is absent and passes it to `remember_thread()`, which does not consult deletion tombstones. If deletion arrives before an already-successful acknowledgment response, the deleted thread returns to the cache and page. Apply the same tombstone check used by page and direct-lookup completions.

45. **Medium — Failed pairing persistence leaves an invisible connection that cannot be retried.** [crates/editor-core/src/core/store.rs:642](crates/editor-core/src/core/store.rs:642) reports `save_hosts()` failure but still inserts `HostData`. Public host enumeration requires the missing configuration entry, while `dial()` refuses to reopen an ID already present in `self.hosts`. A disk-full pairing can leave a hidden live connection until restart. Complete persistence before installing the host or retain an explicit recoverable pending state.

46. **Medium — Accepted stored state can exceed the client’s bootstrap limit.** [crates/forge-store/src/threads.rs:162](crates/forge-store/src/threads.rs:162) includes every setting in `/state`, without an aggregate response budget. Separate accepted settings, prompts, or MCP entries can collectively exceed the Editor’s 32 MiB response limit, preventing every fresh connection. Bound admitted aggregate state or bootstrap large values through separate endpoints.

47. **Medium — Scratch-directory identity is not unique for accepted thread IDs.** [crates/forge-runs/src/scratch.rs:75](crates/forge-runs/src/scratch.rs:75) retains only the last 12 alphanumeric characters of the thread ID. The HTTP creation boundary accepts arbitrary string IDs. Two distinct IDs with the same suffix share a working directory, and retention of one can delete the other’s files. Derive directory identity from the complete ID or validate a collision-safe canonical ID format.

48. **Medium — The download limit does not bound archive expansion.** [crates/forge-engines/src/install.rs:830](crates/forge-engines/src/install.rs:830) extracts regular archive entries without an aggregate unpacked-byte or entry-count limit. An archive within the compressed download allowance can fill Forge’s data volume during installation. Enforce extraction budgets before and during writes, including available-space requirements.

49. **Medium — An unreadable prompt ownership record is treated as first takeover.** [crates/forge-engines/src/customise/prompt.rs:137](crates/forge-engines/src/customise/prompt.rs:137) converts every record-read error to absence. Permission or I/O failure therefore authorizes replacing an externally edited prompt instead of preserving the drift decision. Distinguish a missing record from a failed read and propagate the latter.

50. **Low — Failed picker preferences are discarded.** [crates/editor-core/src/core/host/preferences.rs:99](crates/editor-core/src/core/host/preferences.rs:99) logs failure and then removes the matching queued preference anyway. Changing a model option or group while disconnected can appear successful but disappear after reload. Retain failed intent with a visible error and an explicit retry path.

51. **Low — Editor setting calls can acknowledge a write that failed.** [crates/editor-core/src/core/agent_calls.rs:57](crates/editor-core/src/core/agent_calls.rs:57) returns the requested value after calling `set_setting()`, whose failure path rolls back state and returns no error. On disk-full or permission failure, the agent receives success for an unapplied setting. Return and propagate the persistence result.

52. **Low — Run events can regress a newer cached revision.** [crates/editor-core/src/core/host.rs:673](crates/editor-core/src/core/host.rs:673) compares different runs by start time but does not reject an older revision of the same run. A newer HTTP page response interleaved with queued older SSE events can revert terminal status or usage temporarily. Compare durable sequence numbers before replacing cached run state.

53. **Low — Thread pagination automatically loads the entire active history.** [crates/editor-core/src/core/host/pages.rs:169](crates/editor-core/src/core/host/pages.rs:169) calls `fill_threads()` after each successful page; that immediately requests another page whenever a cursor remains. Large histories therefore produce continuous requests and retain every active thread despite pagination. Fetch additional pages according to actual display demand.

54. **Low — Closed relay channels leave receive-sequence entries behind.** [crates/forge-http/src/relay.rs:331](crates/forge-http/src/relay.rs:331) removes channel, stream, reassembly, and send-window state but never removes the channel from `relay_recv_seq`. Locally initiated closure has the same omission. Repeated subscriptions grow this map for the life of the Forge relay connection. Remove all channel state together.

55. **Low — Engine-status broadcast lag is silently ignored.** [crates/forge-runs/src/lib.rs:1180](crates/forge-runs/src/lib.rs:1180) drops `Lagged` without publishing replacement statuses. A terminal status for one engine can be displaced by another engine’s progress burst, leaving clients with stale installation or sign-in state. Republish current statuses after lag.

56. **Low — A repeatedly closing successful stream defeats exponential backoff.** [crates/editor-core/src/core/connection.rs:164](crates/editor-core/src/core/connection.rs:164) resets the attempt counter immediately after HTTP success; relay subscription does likewise. A server that repeatedly opens and closes the stream keeps reconnects at the first delay forever. Reset backoff after demonstrated stream health.

57. **Low — The event-opening deadline excludes reading an error response body.** [crates/editor-core/src/core/connection.rs:158](crates/editor-core/src/core/connection.rs:158) calls `check()` outside the opening timeout, using a request without a total deadline. A peer can send an error status and stall its body, preventing connection failure or retry before SSE decoding begins. Include status validation and bounded error-body reading in the opening deadline.

58. **Low — Cancelling an Editor-call future leaks its pending entry.** [crates/forge-runs/src/gateway.rs:104](crates/forge-runs/src/gateway.rs:104) inserts the reply sender and removes it only after the awaited timeout completes. Dropping the caller’s future skips removal, retaining an unusable entry indefinitely. Tie map cleanup to an ownership guard.

59. **Low — An applied config report is never retried after its response fails.** [crates/forge-http/src/sync.rs:256](crates/forge-http/src/sync.rs:256) saves local bundle state before reporting success to Core. A failed report is logged, but subsequent syncs return Current through the ETag or bundle comparison paths without reporting again. Core can remain permanently unaware of successful application. Persist pending report state and clear it after acknowledgment.

60. **Low — Checkpoint failure is recorded as completed work.** [crates/forge-store/src/lib.rs:554](crates/forge-store/src/lib.rs:554) logs checkpoint errors but still advances `checkpointed`; it also ignores the checkpoint result columns. If writes then stop, the checkpointer will not revisit the outstanding work. Retain pending work after failure and inspect reported progress.

61. **Low — Diagnostic trace failures omit their cause.** [crates/forge-engines/src/trace.rs:56](crates/forge-engines/src/trace.rs:56) discards the file-open error, and worker-spawn failure is silent. Permission denial, disk-full, and thread exhaustion become indistinguishable precisely when diagnostics are needed. This violates “Error logs carry the step, ids and cause … never a generic wrapper alone.” Preserve and log the underlying error.

62. **Low — Removing a host leaves stale navigation history.** [crates/editor-core/src/core/store.rs:481](crates/editor-core/src/core/store.rs:481) removes the host but does not purge its back-stack or pending-screen entries, unlike `drop_hosts()`. Back navigation can reopen a screen belonging to the removed host. Share the navigation cleanup between both removal paths.

63. **Low — Windows UNC repository paths lose their absolute prefix.** [crates/forge-engines/src/customise/skills.rs:205](crates/forge-engines/src/customise/skills.rs:205) blindly removes `\\?\`. For a canonical UNC path, this leaves `UNC\server\share\…` rather than `\\server\share\…`. Development builds on network shares can consequently construct invalid first-party skill targets. Handle verbatim disk and UNC prefixes separately.

64. **Low — Scratch folder names have no component-length bound.** [crates/forge-runs/src/scratch.rs:124](crates/forge-runs/src/scratch.rs:124) limits the number of words but not their length. A valid thread title containing one long alphanumeric word becomes an overlong directory component, causing first launch to fail. Bound the generated component independently of the stored title.

65. **Low — Runtime SQL bypasses the required typed-query boundary.** [crates/forge-store/src/read.rs:35](crates/forge-store/src/read.rs:35) prepares arbitrary SQL and decodes columns at runtime through rusqlite. This violates “Typed queries: SQLx in Rust.” Schema/query drift can compile successfully and fail only on the affected runtime path. Introduce the required typed query checking; no particular failing query is claimed here.

66. **Nit — Forbidden explanatory comments remain throughout the slice.** [crates/forge-runs/src/lib.rs:1](crates/forge-runs/src/lib.rs:1) contains a documentation header, with similar headers and inline explanations across storage, engines, HTTP, and connection code. This violates “Never write code comments: no file headers, doc comments, inline explanations or TODOs.” Scenario: cosmetic/rule adherence. Express necessary distinctions through names and structure, retaining only permitted directives or license text.

67. **Nit — Substantial test implementations remain under `src/`.** [crates/forge-engines/src/process.rs:330](crates/forge-engines/src/process.rs:330), `forge-http/src/events.rs:400`, and `forge-store/src/settings.rs:343` define inline test modules. This violates “Tests live in `tests/`, mirroring the path in `src/`.” Scenario: organizational. Move implementations into mirrored test files.

68. **Nit — SQL remains embedded in Rust files.** [crates/forge-store/src/writer.rs:256](crates/forge-store/src/writer.rs:256) embeds a mutation query; `schema.rs:16` embeds most migrations. This violates “Queries live in separate `.sql` files.” Scenario: organizational. Extract the SQL consistently, including migration statements.

69. **Nit — The global run-capacity constant is duplicated.** [crates/forge-runs/src/lib.rs:192](crates/forge-runs/src/lib.rs:192) constructs eight local slots, while `runtime/registry.rs:272` independently hardcodes eight cross-generation slots. Scenario: cosmetic today; changing one later silently changes admission behavior by mode. Use one named capacity shared by both paths.

70. **Nit — Runtime metadata serialization helpers are duplicated.** [crates/forge-http/src/upgrade/mod.rs:102](crates/forge-http/src/upgrade/mod.rs:102) repeats the temporary-file serialization and replacement logic in `forge-runs/src/runtime/registry.rs:66`, alongside another generic JSON reader. Scenario: maintenance duplication. Centralize the shared metadata I/O contract.

71. **Nit — Customisation documentation contradicts its implementation.** [crates/forge-engines/src/customise.rs:8](crates/forge-engines/src/customise.rs:8) says materialization never edits `config.toml`, but its Codex launch branch rewrites that file at line 391. Scenario: cosmetic, but misleading when investigating configuration ownership and races. Remove the stale claim in accordance with the no-comments rule.

## 5. Architecture and code: Editor

Reviewer: Iben (GPT 6.1 Sol).

### 5.1 Main review

Reviewed committed state `10f84a7ada6519109564cd433aed23c4e36676fa`, Editor scope only. No builds, tests, or edits performed. Supplementary reviewer spawns returned no reviews; these findings and scores are mine.

| Category | Rating 0–3 | Justification |
|---|---:|---|
| Architecture | 2 | Acyclic crate graph, but core owns renderer resources: `crates/editor-core/src/core/host/images.rs:2`. |
| State management | 1 | Request identity guards exist, but settings rollback ignores newer mutations: `crates/editor-settings/src/settings/controller.rs:564`. |
| Data modelling and persistence | 1 | Settings remain arbitrary JSON maps; unreadable storage becomes writable empty state: `crates/editor-core/src/core/store.rs:146`. |
| API design | 2 | Typed, bounded contracts exist, including Markdown validation; application APIs expose mutable implementation state: `crates/editor-core/src/core/host.rs:337`. |
| Error handling | 1 | Meaningful failures disappear into defaults or `None`: `crates/editor-ui/src/ui/media/audio.rs:109`. |
| Code quality | 1 | Several large responsibility clusters; shell root is 3,873 lines: `crates/editor-aqua/src/shell/root.rs:101`. |
| Type safety | 2 | Useful enums and validated ranges coexist with publicly mutable state flags: `crates/editor-composer/src/composer/draft_session.rs:44`. |
| Consistency | 1 | CI runs Clippy, but Editor crates replace workspace policy with copied, weaker policies: `crates/editor-core/Cargo.toml:36`. |
| Dependency hygiene | 2 | Most versions are pinned; unnecessary feature dependencies remain: `crates/editor-inspector/Cargo.toml:25`. |
| Technical debt | 1 | Cleanup is incremental; stale GPUI documentation remains: `crates/editor-assets/src/fonts.rs:109`. |
| Extensibility | 2 | Real extension points exist, but the central view interface spans unrelated responsibilities: `crates/editor-aqua/src/shell/application.rs:10`. |

**Findings, most severe first**

1. **High — Stale settings failures overwrite newer state.**  
   [`controller.rs:564`](crates/editor-settings/src/settings/controller.rs:564), [`preferences.rs:19`](crates/editor-settings/src/settings/preferences.rs:19). Writes run concurrently; rollback unconditionally restores the captured previous value. If request A fails after B succeeds or an SSE update arrives, A overwrites newer state. Serialize writes per key and make rollback conditional on mutation identity. Existing tests cover replaced hosts, not overlapping writes.

2. **High — Unreadable settings can be permanently overwritten.**  
   [`store.rs:146`](crates/editor-core/src/core/store.rs:146), [`store.rs:467`](crates/editor-core/src/core/store.rs:467). A decoding/read failure substitutes an empty map. Changing one preference then replaces the original file with that map plus one value. Preserve the unreadable file and represent recovery explicitly; block destructive saves until recovery succeeds.

3. **Medium — One permanently failing read acknowledgement blocks every later acknowledgement.**  
   [`host.rs:757`](crates/editor-core/src/core/host.rs:757). The single writer retries every error forever. Deleting its thread removes the queued entry at `host/pages.rs:379` but leaves the worker retrying HTTP 404; `read_writer` prevents other threads from progressing. Cancel deleted work and handle terminal errors. This violates “Retries around flaky paths are bandaids.”

4. **Medium — Marketplace search accepts another host’s completion.**  
   [`controller.rs:303`](crates/editor-marketplace/src/marketplace/controller.rs:303), [`mod.rs:103`](crates/editor-marketplace/src/marketplace/mod.rs:103). Switching A→B leaves the query serial and remote results intact. A’s completion is accepted while A remains connected, displaying A’s result/error on B. Include host, instance, and generation in search identity; invalidate search on every host change.

5. **Medium — Audio loading destroys actionable error information.**  
   [`audio.rs:109`](crates/editor-ui/src/ui/media/audio.rs:109). Missing files, disconnected hosts, and unreadable resources all become cached `None`; the view reports only “Couldn't load.” Preserve `Result` through the cache and expose the typed cause. Similarly, `editor-core/src/core/client.rs:949` discards error-response read failures through `unwrap_or_default()`.

6. **Medium — Settings are decoded repeatedly inside features instead of normalized once.**  
   [`marketplace/controller.rs:139`](crates/editor-marketplace/src/marketplace/controller.rs:139). Malformed MCP configuration silently becomes an empty server list, while Settings reports the same decoding failure at `mcp_view.rs:212`. The same data therefore means “none configured” on one screen and “invalid” on another. Introduce typed settings projections at ingestion. Violates “Normalize messy input data into clean structures at the boundary.”

7. **Medium — Draft lifecycle invariants are publicly writable.**  
   [`draft_session.rs:44`](crates/editor-composer/src/composer/draft_session.rs:44). Public `loaded`, `error`, and `conflict` coexist with private snapshot, busy, dirty, and outgoing state. A caller can mark an unloaded session loaded and permit a write using the default snapshot revision. Encapsulate transitions and model lifecycle phases as ADTs, following “Prefer pure functions and explicit state machines.”

8. **Medium — Oversized modules centralize unrelated responsibilities.**  
   [`root.rs:101`](crates/editor-aqua/src/shell/root.rs:101). `RootViews` owns navigation, transcript caches, browser lookup, media, dialogs, menus, and animation. Other large files are composer view **3,760**, Markdown presenter **3,004**, and conversation factory **2,652** lines. Changes to independent features converge on shared orchestration and invalidation state. Split cohesive owners with explicit lifecycle boundaries.

9. **Medium — Editor crates bypass the workspace’s stronger lint policy.**  
   [`editor-core/Cargo.toml:36`](crates/editor-core/Cargo.toml:36). Copied policies omit workspace bans including `unwrap_used`, `expect_used`, and ignored must-use results, while allowing argument/type complexity globally. CI can therefore pass code the workspace policy ostensibly prohibits. Inherit one policy and use narrow, justified exceptions; enforce formatting in CI too.

10. **Low — Duplicated loopback parsers already disagree.**  
    [`wsl.rs:175`](crates/editor-core/src/core/host/wsl.rs:175), [`client.rs:983`](crates/editor-core/src/core/client.rs:983). The client accepts case-insensitive `LOCALHOST`; WSL detection requires lowercase. A Windows local pairing using uppercase localhost misses WSL discovery/start support. Share one parsed-address helper and test casing and IPv6 boundaries.

11. **Low — Widespread comments directly violate the code rule.**  
    [`fonts.rs:1`](crates/editor-assets/src/fonts.rs:1), [`mentions/lib.rs:1`](crates/artisan-mentions/src/lib.rs:1), [`build.rs:57`](crates/artisan-markdown-index/src/build.rs:57). These are documentation/explanation comments, not license headers. Some already describe obsolete GPUI/WGPU integration. Remove them and clarify names/structure, as required by “Never write code comments.”

12. **Low — Verification support leaks into source and production APIs.**  
    [`application.rs:280`](crates/editor-aqua/src/shell/application.rs:280) exposes ungated `tap_render`, whose sole caller is capture code; enabling it accumulates cloned render commands until drained. Gate it with capture. Feature-gated fixtures also live in source (`spans/view.rs:553`), and inline tests at `editor-audio/src/state.rs:363` violate “Tests live in `tests/`, mirroring the path in `src/`.”

**What perfect looks like**

| Category | Changes needed to reach 3 |
|---|---|
| Architecture | Separate renderer ownership from core data; split shell orchestration; enforce allowed crate edges. |
| State management | Add per-key mutation identity; encode draft/load phases as ADTs; test stale completions across host changes. |
| Data modelling and persistence | Decode settings once; version persisted structures; preserve unreadable files during recovery. |
| API design | Make invariant-bearing fields private; use scoped request identities and typed operation outcomes. |
| Error handling | Preserve causes through caches; classify terminal failures; test failure paths without endless retries. |
| Code quality | Extract cohesive owners from the largest files; consolidate duplicated parsing and loading machinery. |
| Type safety | Replace parallel lifecycle flags and raw setting values with validated types. |
| Consistency | Inherit workspace lints; enforce formatting and the comment rule automatically. |
| Dependency hygiene | Remove unused inspector→sidebar dependency; centralize remaining version ranges; automate dependency/license checks. |
| Technical debt | Track concrete removals with owners and completion criteria; remove stale integration contracts. |
| Extensibility | Keep focused provider interfaces; replace the broad view interface with smaller capability boundaries. |

| Score | This slice |
|---|---:|
| Correctness | 6/10 |
| State handling | 4/10 |
| Code quality | 5/10 |
| Rule adherence | 3/10 |
| **Overall** | **5/10** |

### 5.2 Remaining findings and nits

13. **Medium — Composer outboxes survive target changes.** `crates/editor-composer/src/composer/controller.rs:183` changes the target without resetting `outbox`; `:705` subsequently restores whichever body that outbox holds. Switching from an in-flight submission on A to an already-failed session on B can restore A’s text and attachments into B. Associate the outbox with its draft session and test switching between failed and in-flight submissions.

14. **Medium — Parking a conversation preserves abandoned “sending” flags.** `crates/editor-conversation/src/conversation/controller.rs:428` parks the `ThreadView`, then `:433` cancels its answer tasks. The parked `ItemUi.sending` flags remain set, and `ThreadView::resume` does not clear them. Returning after an approval request was cancelled before reaching Forge can leave its controls permanently disabled. Preserve pending operations with their owner or clear their transient state when cancelling.

15. **Medium — MCP sign-ins outlive their host’s authority.** `crates/editor-core/src/core/mcp_sign_in.rs:230` keys jobs by host ID and server name, without `HostInstance`; `crates/editor-core/src/core/store.rs:481` removes hosts without cancelling these jobs. A pending callback can still finish through the captured client after host removal or account sign-out, and a replacement host can inherit the old authorization URL. Bind jobs to host lifetime and cancel them when that lifetime ends.

16. **Medium — Failed pairing persistence creates an inaccessible connected host.** `crates/editor-core/src/core/store.rs:642` reports a save failure but continues to insert the connected host at `:661`. `hosts()` at `:313` only exposes hosts joined to saved or relay configurations, so a new direct host becomes invisible while `dial()` refuses another attempt because it already exists. Make configuration admission and connection installation one explicit transition.

17. **Medium — Setting writes can report success after rollback.** `crates/editor-core/src/core/store.rs:462` returns `()` even when saving fails and the value is restored. `crates/editor-core/src/core/agent_calls.rs:57` nevertheless returns the requested value as a successful `SettingSet` result. An agent therefore believes a preference changed when it did not. Return a persistence result and propagate it to both the UI and editor-call reply.

18. **Medium — Source actions have no operation identity.** `crates/editor-sources/src/sources/controller.rs:295` accepts copy/export completions using only source equality. After a queued completion and an A→B→A source change, that old completion can clear the new action handle, abort its task, and publish the old copy result. Give actions generations and invalidate queued completions when changing source.

19. **Medium — Valid media can repeatedly evict its own result.** `crates/editor-ui/src/ui/media/resource_cache.rs:383` adds source and output sizes, then evicts until the entry fits. Audio from a 32 MiB data URI retains the same bytes as both source and output; counting both plus membership exceeds the 64 MiB limit. The completed entry is removed, its waiter wakes, and requesting it starts the load again. Account for shared allocations and return a stable budget failure when an output cannot be retained.

20. **Medium — Cache backpressure loses notification ownership.** `crates/editor-ui/src/ui/media/resource_cache.rs:194` returns `Pending` for the ninth load without registering that caller’s waiter. Publishing another result can also evict pending entries at `:395`, aborting their loaders without waking their waiters. Those callers have neither work nor a guaranteed completion notification. Represent waiting-for-admission explicitly and notify callers when admission or eviction changes their state.

21. **Medium — Revoked resource previews remain indefinitely loading.** `crates/editor-aqua/src/shell/resource_preview.rs:118` discards a completion when its host instance disappeared, leaving `content == None` after removing the request handle. The view interprets `None` as “Reading source…” at `resource_preview/view.rs:148`. Transition to an unavailable error or dismiss the preview when its host is revoked.

22. **Medium — Configuration replacement lacks durability synchronization.** `crates/editor-core/src/core/hosts.rs:127` flushes and renames the temporary file but never synchronizes the file or replacement directory. Successful identity, account, pairing, and preference writes can therefore disappear after power loss. Complete the platform-appropriate durable replacement sequence before reporting success.

23. **Medium — Windows drive paths are classified as unsupported URI schemes.** `crates/editor-ui/src/ui/markdown/inline.rs:328` treats the `C:` in `C:\src\file.rs` or `C:/src/file.rs` as a scheme, then rejects it at `:341`. Composer-generated links to such files render without their file action. Recognize drive and UNC paths before URI schemes and share that normalized path representation with link handling.

24. **Low — Removing a host leaves navigation history pointing at it.** `crates/editor-core/src/core/store.rs:494` navigates away through the normal history-pushing path, retaining the removed screen; it also leaves older history entries intact. Back navigation can reopen a thread belonging to the removed pairing. Reuse the history and pending-screen cleanup performed by `drop_hosts()`.

25. **Low — Project-icon retries omit blob failures.** `crates/editor-core/src/core/host/project_icons.rs:92` repeatedly calls `load_image(hash, false)`, which retains cached failures. `retry_project_icons()` at `:127` only expires failed metadata requests. A successful icon lookup followed by a transient blob failure can therefore remain broken without a working retry path. Track metadata and image failures separately and retry the failed stage.

26. **Low — Cancelled attachment previews never finish.** `crates/editor-composer/src/composer/attachment.rs:371` handles ready and failed image statuses, while `Cancelled` falls through the default arm at `:409`. Such an attachment remains `Encoding`, preventing submission indefinitely. Handle every terminal decode status and release the pending preview and encoded bytes consistently.

27. **Low — Avatar failures are permanent for an unchanged URL.** `crates/editor-aqua/src/frame/avatar.rs:74` returns whenever the URL matches, including `Phase::Failed`; failures at `:104` discard their cause. A temporary network failure leaves the avatar absent for the remaining session unless its URL changes. Preserve the failure and provide an explicit reload transition.

28. **Low — Archived duplicate titles suppress active thread mentions.** `crates/editor-composer/src/composer/mentions.rs:172` reserves a title in `seen` before checking whether its thread is archived at `:176`. If the newest duplicate is archived, a live thread with that title never reaches the mention menu. Deduplicate selectable threads after filtering archived entries; maintain highlighting titles separately.

29. **Low — File links interpolate unescaped paths into Markdown.** `crates/editor-composer/src/composer/controller.rs:987` inserts the path directly inside `<…>` and only strips brackets from the label. A path containing `>`, a newline, or Markdown-significant label characters can produce malformed links or unintended markup. Encode destinations and escape labels through one Markdown-link constructor.

30. **Low — Code-reference parsing accepts inconsistent ranges.** `crates/editor-ui/src/ui/rich/kit.rs:110` accepts empty paths, line zero, and reversed ranges without validation. A reference such as `src/a.rs:9-3` displays an inverted range, while `:7` attempts to open an empty path. Normalize references at deserialization into a nonempty path and validated line span, following “Normalize messy input data into clean structures at the boundary.”

31. **Low — Annotation rebasing uses unchecked arithmetic.** `crates/editor-ui/src/ui/rich/code/annotated.rs:105` evaluates `span.start + start - 1` with unrestricted `u32` inputs. Large `start_line` values can panic in builds with overflow checks or wrap annotation lines in unchecked builds. Validate the representable range and use checked offset arithmetic.

32. **Low — Chart series allow contradictory data representations.** `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:33` permits both `values` and `points`, or neither. `chart/model.rs:98` silently chooses `points` when both exist, and `prepare()` does not reject the ambiguity. A generated chart can silently discard one supplied dataset. Decode into an enum containing exactly one series representation.

33. **Low — Normalized media sources remain independently mutable.** `crates/editor-ui/src/ui/media/source.rs:218` publicly exposes both `raw` and `parsed`. Changing either leaves display text, cache identity, loading behavior, and memory accounting disagreeing. Make construction and replacement atomic, with read-only accessors for the normalized representation.

34. **Low — Each media load constructs fresh HTTP clients.** `crates/editor-core/src/core/media_http.rs:17` builds two clients per call; image, audio, model, and avatar loaders call it during individual jobs. Loading several resources repeatedly creates connection pools and forfeits reuse. Own a shared client in the media service and inject it into loaders.

35. **Low — Settings failure logs omit host identity.** `crates/editor-settings/src/settings/controller.rs:562` logs the setting key and cause but omits the host and instance already present in the completion. Concurrent failures on different Forges become indistinguishable. Include those identifiers, as required by “Error logs carry the step, ids and cause.”

36. **Low — Failure to initialize file logging is silently discarded.** `crates/editor-aqua/src/logging.rs:12` converts the log-opening error to `None`; `:35` also discards subscriber-initialization failure. A permissions or storage failure leaves no explanation for missing diagnostic files. Preserve and report initialization failures through stderr.

37. **Low — Static design-value tests violate the testing rule.** `crates/editor-ui/tests/ui/kit/row.rs:13` pins menu height and insets; `crates/editor-aqua/tests/markdown.rs:18` pins a prose-width constant. They fail on intentional design changes without demonstrating broken behavior. Remove the literal pins and retain meaningful geometry or interaction invariants. This violates “No tests of static values or markup.”

38. **Low — The settings scale test tolerates almost an entire viewport of drift.** `crates/editor-aqua/tests/settings/mod.rs:467` allows every coordinate and dimension to differ by 1000 logical pixels in a 900×700 viewport. Major layout regressions pass the test named “lay out identically.” Use a meaningful tolerance and assert the intended layout relationship.

39. **Low — Style enforcement passes when files cannot be inspected.** `crates/editor-ui/tests/ui/kit/enforcement.rs:68` flattens directory-read errors into an empty file list; `:98` replaces unreadable source files with empty strings. Missing or inaccessible source trees can therefore produce a clean enforcement result. Propagate filesystem errors and assert that every configured source root was inspected.

40. **Nit — `clear()` permanently closes the resource cache.** `crates/editor-ui/src/ui/media/resource_cache.rs:289` sets `closed = true`, and subsequent loads return `Pending` forever. The name suggests reusable cache eviction. Cosmetic/API fix: call this operation `close()` and reserve `clear()` for reusable clearing.

41. **Nit — The source-dialog API advertises an impossible synchronous result.** `crates/editor-aqua/src/shell/source.rs:45` returns an `Option` containing an export result, but `open()` always returns `None` at `:60`; results exclusively arrive through `drain()`. Cosmetic/API fix: return `()` or the request identity and remove impossible immediate-result branches.

42. **Nit — Markdown shape errors are duplicated across layers.** `crates/artisan-markdown-index/src/lib.rs:49` declares `InvalidShape`, although the index crate does not construct it; `crates/editor-ui/src/ui/markdown/source_index.rs:10` declares another variant with the same message for normalization failures. Cosmetic fix: keep the error in its owning layer or consistently reuse one type.

43. **Nit — Two glTF errors contain a missing space.** `crates/editor-ui/src/ui/media/model/formats/gltf.rs:44` and `:78` say “the64 MiB.” Cosmetic fix: correct both strings and consolidate the repeated budget message.

44. **Nit — Feature filenames repeat their hierarchy as prefixes.** `crates/editor-inspector/src/inspector/mod.rs:7` declares `terminal_view`; `crates/editor-sidebar/src/sidebar/mod.rs:13` declares `usage_controller` beside `usage_page`. Cosmetic organization issue under “Source in `src/`, with folders as the hierarchy” and “No name prefixes.” Move these into `terminal/view.rs` and `usage/{controller,view}.rs`.

45. **Nit — Redundant string conversions obscure simple ownership transfers.** `crates/editor-ui/src/ui/rich/kit.rs:119`, `:124`, and `crates/editor-ui/src/ui/rich/code/annotated.rs:111` wrap existing `String` values in `String::from(...)`. Cosmetic fix: use the owned string directly.

46. **Nit — Empty source previews display an inverted line interval.** `crates/editor-aqua/src/shell/resource_preview.rs:42` formats an empty file as “Lines 1–0 of 0.” Cosmetic fix: give empty content an explicit message or a consistent zero-line representation.

## 6. Architecture and code: Forge and shared crates

Reviewer: Erle (GPT 6.1 Sol).

### 6.1 Main review

Reviewed committed HEAD `10f84a7`, ignoring working-tree edits. Static review only; no builds, tests, or edits. Paths below are relative to the repository.

| Category | Rating 0–3 | Evidence |
|---|---:|---|
| Architecture | 2 | Deliberate crate boundaries, but `Hub` combines scheduling, OAuth, approvals, customization and agent coordination: `crates/forge-runs/src/lib.rs:59`. |
| State management | 1 | Run status, error and completion time remain independently writable: `crates/artisan-api/src/thread.rs:74`. |
| Data modelling and persistence | 2 | Transactional, versioned migrations exist; queries remain handwritten and unchecked: `crates/forge-store/src/schema.rs:543`. |
| API design | 2 | Protocol version 7 is advertised, pagination exists, but cross-generation calls erase typed failures: `crates/forge-runs/src/runtime/mod.rs:48`. |
| Error handling | 1 | Some contextual chains survive; account transport errors deliberately discard causes: `crates/artisan-account/src/network.rs:270`. |
| Code quality | 1 | Large orchestration units and routine complexity exemptions: `crates/forge-mcp/src/tools.rs:822`. |
| Type safety | 2 | Typed identities and events coexist with invalid outcome combinations and provider JSON interpreted downstream: `crates/forge-engines/src/harness.rs:171`. |
| Consistency | 2 | Denied Clippy lints run in CI; formatting and user-specific structural rules are unenforced: `.github/workflows/ci.yml:61`. |
| Dependency hygiene | 1 | Pins and vendor provenance exist, but archived native code remains and vendor qualification is excluded: `Cargo.toml:33`. |
| Technical debt | 1 | Legacy aliases remain without removal conditions: `crates/forge-mcp/src/names.rs:1`. |
| Extensibility | 2 | Useful `Harness` and `ToolSet` seams exist; their contracts still permit misuse: `crates/forge-engines/src/harness.rs:258`. |

**Findings, most severe first**

1. **High — Installer migration deletes data it never migrated.**  
   `crates/ae/src/legacy.rs:64`: every old Editor data directory is removed whenever Editor installation runs. If the current directory already exists, migration is skipped at line 23, yet old identities, hosts and settings are still deleted. Preserve unmigrated directories or archive them after explicit reconciliation.

2. **High — A leased pending thread starves unrelated work.**  
   `crates/forge-runs/src/lib.rs:884`: failed ownership acquisition executes `continue`, while candidate selection requests only the oldest pending thread and excludes only locally active threads. During overlapping generations, an old generation’s pending steer can therefore monopolize dispatch. Exclude unavailable candidates for the dispatch pass and wait for ownership changes.

3. **Medium — Steady-state runs pay for distributed upgrade coordination.**  
   `crates/forge-runs/src/runtime/registry.rs:234` introduces admission fencing and thread leases; line 271 adds slot leases. `crates/forge-http/src/generation.rs:163` polls every 50 ms even with one backend. This conflicts with the requested “plain ownership and cancellation” direction. Use ordinary ownership for steady-state execution and isolate any necessary upgrade overlap. These are actual cross-process upgrade leases; advisory messaging receipts are a separate feature.

4. **Medium — Terminal outcomes can describe running work.**  
   `crates/forge-engines/src/harness.rs:171`: `Outcome` permits `Running`, an error and retry metadata in arbitrary combinations. `crates/forge-runs/src/run.rs:1307` persists the supplied status alongside `ended_at`. An adapter returning `Running` produces a completed execution still advertised as live. Replace the product type with terminal outcome variants and validate persistence transitions. This breaks “Model data as ADTs.”

5. **Medium — Provider JSON escapes normalization into persistence logic.**  
   `crates/forge-store/src/read.rs:826` parses Claude workflow input JSON and searches script text for `meta` and `name`. The inspector also drops workflow names when input becomes stored content at line 731. Engine shape changes and large inputs therefore alter presentation downstream. Decode workflow metadata into a typed field at the engine boundary, as required by “Normalize messy input data … at the boundary.”

6. **Medium — SQL architecture directly violates both prescribed rules.**  
   `crates/forge-store/src/read.rs:165` embeds a substantial scheduling query in Rust; `crates/forge-store/src/schema.rs:17` embeds migrations. `Cargo.toml:135` selects rusqlite rather than SQLx. This violates “Queries live in separate `.sql` files” and “Typed queries: SQLx in Rust.” Query/schema mismatches reach runtime. Extract SQL and introduce checked query mappings.

7. **Medium — Account failures lose both step and cause.**  
   `crates/artisan-account/src/network.rs:240` discards `action`; line 270 maps every request failure to the source-less `Transport` variant. DNS failure, TLS rejection and malformed successful responses become indistinguishable. This violates “Error logs carry the step, ids and cause.” Preserve typed, sanitized causes and operation context while keeping credentials out.

8. **Medium — Cross-generation routing changes client error semantics.**  
   `crates/forge-runs/src/runtime/mod.rs:48` serializes failures as strings, then reconstructs generic anyhow errors at line 68. A refused answer routed to an older generation loses `Refused`; `crates/forge-http/src/routes.rs:200` consequently returns 500 instead of 409. Serialize typed rejection codes and retain diagnostic context separately.

9. **Medium — Location-file errors silently select another data root.**  
   `crates/artisan-paths/src/lib.rs:113` turns every read error into absence; line 99 falls back to the default installation. An unreadable relocation pointer can make Forge open an apparently empty database elsewhere. Treat only `NotFound` as absence; propagate permission, decoding and invalid-path failures with the pointer path.

10. **Medium — Highlighting depends on an ended native project.**  
    `vendor/highlighting/onig/Cargo.toml:47` pins `onig_sys`; `vendor/highlighting/README.md:49` confirms bundled native compilation. [Oniguruma upstream states that the project ended on April 24, 2025](https://github.com/kkos/oniguruma). A future native defect has no upstream maintenance commitment. Establish a maintained replacement or explicit fork ownership; this is a maintenance finding, not a verified vulnerability.

11. **Medium — Vendor regression checks do not participate in CI.**  
    `Cargo.toml:33` excludes the vendor packages, including highlighting validation. `.github/workflows/ci.yml:66` tests only the workspace. A regression in the modified regex implementation can pass CI without its dedicated fallback/parity checks. Run the standalone locked validation package and provenance verification explicitly.

12. **Medium — `Hub` is a shared mutable service container.**  
    `crates/forge-runs/src/lib.rs:59` places unrelated subsystems and numerous independently locked maps in one owner. Adding authentication or marketplace behavior consequently changes the run controller and expands its initialization surface. Split lifecycle, authentication, customization and coordination into owned services with narrower interfaces.

13. **Low — First-party comments violate the explicit coding rule.**  
    Examples include `crates/forge-runs/src/lib.rs:1`, `crates/artisan-api/src/item.rs:27` and `crates/forge-computer/src/runner.rs:101`. These are neither licenses nor tool directives. “Never write code comments” is systematically unenforced. Restructure explanatory code and express CLI help through attributes.

14. **Low — The workspace dependency catalogue contains stale declarations.**  
    `Cargo.toml:96` declares registry Mermaid, while `crates/editor-ui/Cargo.toml:38` uses the vendor path directly. Other unused workspace declarations include tokio-stream and zip. Dependency updates can therefore change declarations that control no consumer. Remove unused entries and centralize the actual dependency source.

Protocol versioning **is present**, including Editor mismatch checks. I found no `SELECT *` matches in the inspected Forge/shared persistence paths.

**What perfect looks like**

| Category | Changes needed to reach 3 |
|---|---|
| Architecture | Narrow `Hub`; enforce crate dependency direction. |
| State management | Terminal outcome ADTs; validated transitions; ownership-contention regressions. |
| Data modelling and persistence | Separate SQL, SQLx checks, frozen migration fixtures and nondestructive migration coverage. |
| API design | Typed cross-generation errors; previous-release client/server compatibility jobs. |
| Error handling | Preserve sanitized step, identities and source chains across every boundary. |
| Code quality | Split oversized dispatchers; remove stale declarations and prohibited comments. |
| Type safety | Normalize provider metadata before emitting events; reject invalid persisted combinations. |
| Consistency | Enforce formatting, SQL placement and applicable coding rules in CI. |
| Dependency hygiene | Qualify vendor patches automatically; resolve archived native dependency ownership. |
| Technical debt | Give compatibility aliases and legacy migration paths explicit retirement conditions. |
| Extensibility | Contract-test adapters against malformed events, cancellation and unsupported capabilities. |

| Score | Slice assessment |
|---|---:|
| Correctness | 5/10 |
| State handling | 4/10 |
| Code quality | 5/10 |
| Rule adherence | 2/10 |
| **Overall** | **4/10** |

### 6.2 Remaining findings and nits

15. **High — Offline account sync constructs asynchronous services outside a runtime.** `crates/ae/src/account.rs:456` (commit `10f84a7`) constructs `Hub` before entering `runtime().block_on`. `Hub::build` unconditionally calls `start_coordination`, whose `TaskTracker::spawn` requires a running Tokio runtime (`crates/forge-runs/src/coordination.rs:100`). Scenario: an enrolled user runs `ae account sync` while Forge is stopped; the command panics. Construct and shut down the hub inside the runtime, or separate configuration operations from background services.

16. **High — Disabling retries publishes durable events out of sequence.** `crates/forge-store/src/writer.rs:552` (commit `10f84a7`) calls `unpark_all` before emitting the already-numbered setting event. Unparking saves threads with higher sequence numbers, which are consequently broadcast first. Scenario: disable overload retries while a thread is parked; the SSE cursor advances past the setting’s sequence and rejects that setting event (`crates/forge-http/src/events.rs:322`). Emit events in sequence order and add a regression covering the actual subscriber output.

17. **High — Cancelling a thread does not cancel its outstanding Forge approval.** `crates/forge-runs/src/approvals.rs:130` (commit `10f84a7`) waits for an answer, timeout, or global shutdown, but never the requesting run’s cancellation. `cancel_owned` only cancels the run and pending inputs. Scenario: stop a thread while its destructive MCP operation awaits approval; the approval waiter can subsequently return permission to execute the operation. Bind approvals and their execution to the requesting run’s cancellation and identity.

18. **High — Account persistence can destroy the only machine private key.** `crates/ae/src/account.rs:96` (commit `10f84a7`) truncates `ast_account.json` before serializing the replacement. Scenario: a crash, interrupted write, or full disk during enrollment or confirmation leaves an empty or partial file containing no recoverable machine identity. Write a restricted temporary file, synchronize it, and atomically replace the account file.

19. **Medium — Failed configuration application leaves changes without ownership records.** `crates/forge-http/src/sync.rs:302` (commit `10f84a7`) applies skills, prompt changes, and MCP mutations incrementally; the new ownership state is written only after everything succeeds at line 256. Scenario: one MCP server is added, then a later server fails validation; a subsequent bundle removing the first server cannot identify it as previously synchronized. Preflight the complete bundle and commit configuration plus ownership bookkeeping together, with staged filesystem changes.

20. **Medium — Account sync overwrites locally created MCP servers with matching names.** `crates/forge-http/src/sync.rs:396` (commit `10f84a7`) matches every existing server by name, regardless of whether `synced_before` identifies it as bundle-owned. Scenario: a local server named `search` collides with a new account bundle entry; its command, arguments, and URL are replaced. Detect ownership conflicts and preserve local entries unless replacement is explicitly authorized.

21. **Medium — Concurrent launches can refresh the same rotating OAuth token twice.** `crates/forge-runs/src/mcp_auth.rs:341` (commit `10f84a7`) reads, refreshes, and replaces tokens without per-server serialization. Scenario: two threads start with an expired token; one refresh rotates it successfully, while the other receives rejection and deletes the successful replacement through `mcp_sign_out`. Give each server one refresh owner and re-read current credentials before changing them.

22. **Medium — In-flight OAuth work can undo sign-out.** `crates/forge-runs/src/mcp_auth.rs:224` (commit `10f84a7`) deletes credentials without cancelling pending authorization flows or refreshes. Both `mcp_sign_in_finish` and successful refresh later write credentials unconditionally. Scenario: sign out while either request is pending; completion silently signs the server back in. Serialize credential lifecycle changes and reject completions belonging to an obsolete sign-in.

23. **Medium — Temporary OAuth server failures permanently discard credentials.** `crates/forge-engines/src/mcp_oauth.rs:455` (commit `10f84a7`) wraps every unsuccessful HTTP response in `Rejected`. `fresh_token` interprets that type as a revoked grant and signs out. Scenario: the token endpoint returns 429 or 503; otherwise valid refresh credentials are deleted. Distinguish invalid-grant rejection from rate limiting, server failures, and transport failures.

24. **Medium — An unavailable MCP server is classified as requiring no authentication.** `crates/forge-engines/src/mcp_oauth.rs:134` (commit `10f84a7`) returns `Probe::Open` for every status except 401. Scenario: an authenticated server returns 403, 404, or 503; the auth check removes its sign-in state, and starting sign-in reports that authentication is unnecessary. Classify successful responses, authentication failures, and availability failures separately.

25. **Medium — Cached OAuth registrations are reused across different authorization servers.** `crates/forge-runs/src/mcp_auth.rs:142` (commit `10f84a7`) caches clients by MCP server name and checks only the redirect URI. Scenario: change a server’s URL while retaining its name; the new authorization server receives the old server’s client registration, and sign-in fails repeatedly. Bind cached registrations to the discovered issuer or registration endpoint as well as the redirect URI.

26. **Medium — Background authentication checks can overwrite newer sign-in results.** `crates/forge-runs/src/mcp_auth.rs:244` (commit `10f84a7`) snapshots all statuses, performs network probes, and replaces the entire setting without the lock used by `set_mcp_auth`. Scenario: a user completes sign-in while a probe is pending; the old probe subsequently writes `NeedsSignIn` over `SignedIn`. Merge updates against current state transactionally and discard stale probe results.

27. **Medium — Invalid stored preferences silently re-enable disabled skills.** `crates/forge-runs/src/extensions.rs:43` (commit `10f84a7`) converts deserialization errors into an empty disabled-skill set. Scenario: an incompatible or damaged `disabled_skills` value causes all skills to become enabled on the next reconciliation. This conflicts with “Normalize messy input data into clean structures at the boundary” and the requirement against silent error swallowing. Default only when absent; report malformed stored values explicitly.

28. **Medium — Download limits are checked after allocating the complete response.** `crates/forge-engines/src/mcp_oauth.rs:264` (commit `10f84a7`) buffers metadata before checking its 256 KiB limit; token responses do the same at line 450. Marketplace archives similarly buffer before checking their actual size (`crates/forge-engines/src/marketplace.rs:101`). Scenario: a chunked response without a trustworthy length consumes excessive memory before rejection. Enforce byte limits while reading the response stream.

29. **Medium — Marketplace extraction has no aggregate expansion limit.** `crates/forge-engines/src/marketplace.rs:139` (commit `10f84a7`) limits individual files but neither total extracted bytes nor entry count. Scenario: a small compressed repository contains thousands of individually admissible files and fills Forge’s disk during installation. Bound cumulative extracted bytes, files, and directories before writing them.

30. **Medium — Marketplace installations can share and delete each other’s staging directory.** `crates/forge-runs/src/extensions.rs:154` (commit `10f84a7`) uses the current millisecond as the staging identity. Scenario: two requests begin in the same millisecond, unpack into the same `repository` folder, and one removes the folder while the other installs from it. Use a uniquely created temporary directory owned by each operation.

31. **Medium — Release downloads can wait indefinitely.** `crates/ae/src/release.rs:154` (commit `10f84a7`) constructs the release HTTP client without request timeouts; manifest, signature, HEAD, and artifact requests inherit that behavior. Scenario: an endpoint accepts a connection but stops sending data, leaving installation permanently waiting. Apply operation deadlines and an idle-read timeout appropriate to downloads.

32. **Medium — Concurrent installers mutate the same staging directory.** `crates/ae/src/install.rs:515` (commit `10f84a7`) unconditionally removes and recreates the app’s shared `.incoming` directory. No installation ownership guard protects this path. Scenario: two installers run together; one deletes the other’s staged executable or installs files from the other release. Serialize installation per root and use operation-owned staging directories.

33. **Medium — Windows executable replacement has no rollback.** `crates/ae/src/install.rs:618` (commit `10f84a7`) moves the installed file aside before moving its replacement into place. Scenario: the second rename fails because of antivirus interference, permissions, or an I/O error; the expected executable path is left missing. Restore the previous file when installation fails and retain recovery information until replacement completes.

34. **Medium — Unsupported installer destinations silently become “do not install.”** `crates/ae/src/install.rs:93` (commit `10f84a7`) maps an Editor `Place::Wsl` to `Nowhere`; line 101 maps Forge `Place::Windows` to false. Both arguments use the same permissive parser. Scenario: `--editor wsl:Ubuntu` or `--forge windows` is accepted but silently skips the requested application. Use destination types specific to each application and reject unsupported combinations.

35. **Medium — Browser captures have no exclusive owner.** `crates/claude-ai-browser/src/lib.rs:281` (commit `10f84a7`) lets multiple captures attach to the same profile and browser, while every capture closes the entire browser at line 90. Scenario: renewal overlaps sign-in or another renewal; the first completion closes the browser underneath the other capture. Serialize captures per profile and give browser lifecycle management one owner.

36. **Medium — Cancelling browser capture bypasses browser cleanup.** `crates/claude-ai-browser/src/lib.rs:84` (commit `10f84a7`) closes the browser only after awaited capture work returns normally. The successful startup path moves the child into a detached waiter at line 314. Scenario: the caller drops the capture future during sign-in; the browser remains running indefinitely. Make capture ownership cancellation-safe and ensure its owner performs cleanup when abandoned.

37. **Medium — The declared computer-use setting cannot be changed through the settings HTTP endpoint.** `crates/forge-http/src/routes.rs:817` (commit `10f84a7`) maintains a separate switch allowlist that omits `computer_use`, although `artisan-api` declares it as a Forge boolean setting. Scenario: a client attempts to enable or disable computer use through `PUT /settings/computer_use` and receives an unsupported-setting error. Drive validation from the shared declarations, preserving the required approval behavior.

38. **Medium — Disabling computer use does not stop an already executing batch.** `crates/forge-computer/src/lib.rs:145` (commit `10f84a7`) checks the setting before execution, but `Batch::check` subsequently observes only `paused`, user activity, display changes, and elapsed time. Scenario: turn computer use off during a long batch; input continues until the batch ends. Propagate the setting transition to the batch’s cancellation or pause state.

39. **Medium — Stored question and plan collections disappear from MCP transcripts.** `crates/forge-mcp/src/transcript.rs:260` (commit `10f84a7`) renders questions only for `Collection::Inline`; plans have the same omission at line 282. Neither `read_thread` nor `read_thread_item` resolves these collections first. Scenario: a collection crosses the storage threshold and its question or plan text vanishes from both read tools. Render stored collections through bounded reads and expose continuation when necessary.

40. **Medium — Bounded transcript reads repeatedly resolve complete stored content.** `crates/forge-mcp/src/transcript.rs:30` (commit `10f84a7`) loads a complete text before clipping it. `read_thread_item` renders the complete item before selecting its requested byte range (`crates/forge-mcp/src/awareness.rs:342`). Scenario: paging through a large item repeatedly loads and formats tens of megabytes to return a small chunk. Read the needed source ranges and avoid rebuilding the entire transcript for each continuation.

41. **Medium — Grep pagination permanently skips remaining matches in an item.** `crates/forge-store/src/recall.rs:539` (commit `10f84a7`) stops at `max_matches` and returns only the current row ID as its cursor; the next query excludes that row. Scenario: one item contains more matching lines than the page limit; later matches and its remaining output field are never returned. Include field and line position in the continuation cursor.

42. **Medium — Output-search pagination repeats the first page indefinitely.** `crates/forge-store/src/recall.rs:605` (commit `10f84a7`) applies no offset or cursor to the output query, while returning `offset + limit` as continuation. Scenario: a search has only tool-output matches and more than one page; every continuation returns the same initial output hits. Paginate a unified result set or maintain separate cursors for text and output results.

43. **Medium — Historical search uses the thread’s current engine instead of the item’s engine.** `crates/forge-store/src/recall.rs:364` (commit `10f84a7`) filters on `threads.body.config.engine`. Scenario: a thread switches from Claude to Codex; all its old Claude items now appear in Codex-filtered searches and disappear from Claude-filtered searches. Filter using the item’s producing run, with an explicit policy for items without runs.

44. **Medium — Ordinary diagnostic logging includes raw engine payloads.** `crates/forge-engines/src/process.rs:148` (commit `10f84a7`) logs arbitrary engine stderr, and OpenCode logs complete stdout lines at `crates/forge-engines/src/opencode.rs:257`. Scenario: debugging is enabled during a failure and provider output containing source, prompts, tool output, or credentials enters persistent logs. This violates “keep payloads out of logs.” Keep raw content in the explicit trace facility and log bounded metadata ordinarily.

45. **Medium — Storage failures are incorrectly reported as client mistakes.** `crates/forge-http/src/routes.rs:617` (commit `10f84a7`) maps every source-index error to `bad_request`; content and collection routes repeat this pattern. Scenario: SQLite reports corruption, contention, or an I/O failure; the response is HTTP 400 and bypasses internal-error logging. Distinguish typed validation failures from operational failures and preserve internal errors as server failures.

46. **Medium — Editor-reply delivery failures are reported as expired calls.** `crates/forge-http/src/routes.rs:761` (commit `10f84a7`) converts every distributed reply error into false and HTTP 410. Scenario: contacting an owning generation fails while its approval or Editor call still waits; the client is told that nothing is waiting. Preserve transport failures and report expiration only when the call’s absence is established.

47. **Medium — Threads can be created under removed projects.** `crates/forge-store/src/writer.rs:140` (commit `10f84a7`) checks project existence but ignores `Project::removed`. Scenario: a stale client creates a thread after its project was removed; normal thread pagination hides it through the removed-project filter. Reject new threads for removed projects or define an explicit restoration operation.

48. **Medium — Administrative commands can migrate a database underneath an older running Forge.** `crates/forge-store/src/admin.rs:31` (commit `10f84a7`) always runs migrations despite advertising safe operation beside a running server. Scenario: a newer `ae forge pair` opens an older daemon’s database and advances its schema without the compatibility check used by generation joining. Separate offline migration from live administration and require schema compatibility when Forge owns the database.

49. **Medium — OpenCode stdout failures lose their actual cause.** `crates/forge-engines/src/opencode.rs:258` (commit `10f84a7`) handles EOF and every `next_line` error as “OpenCode exited mid-run.” Scenario: output exceeds the line limit or a pipe read fails while OpenCode remains alive; the diagnostic falsely reports process exit and discards the source error. Match EOF and read failures separately and retain the operation, thread identity, and cause.

50. **Low — Switching to Full access can miss an approval being registered concurrently.** `crates/forge-runs/src/approvals.rs:51` (commit `10f84a7`) reads permissions before inserting the approval into the pending map. Scenario: that read sees restricted access, the user enables Full access and drains the currently empty map, then the new card is inserted and waits unnecessarily. Coordinate permission observation with card registration and recheck before waiting.

51. **Low — Dropped approval and Editor-call futures leave pending entries behind.** `crates/forge-runs/src/approvals.rs:92` (commit `10f84a7`) removes its map entry only after awaited work completes. `call_editor` has the same structure at `crates/forge-runs/src/gateway.rs:104`. Scenario: the owning future is aborted while waiting; its sender and, for approvals, waiting card survive without an active waiter. Use cancellation-safe ownership guards and settle abandoned cards.

52. **Low — A failed “applied” report is never reconciled with Core.** `crates/forge-http/src/sync.rs:256` (commit `10f84a7`) records the bundle as current before reporting success. Later 304 or unchanged-bundle paths return without reporting again. Scenario: application succeeds but the report request fails; Core can continue showing an unapplied bundle indefinitely. Persist pending report state or reconcile acknowledgement on subsequent current-bundle checks.

53. **Low — Concurrent sign-in starts bypass the advertised flow reuse.** `crates/forge-runs/src/mcp_auth.rs:109` (commit `10f84a7`) checks for an existing flow before several network awaits and inserts the new flow only at line 161. Scenario: simultaneous starts both observe absence and create separate registrations and browser flows. Give each server/redirect pair one startup owner and share its result.

54. **Low — Thread ancestry is accepted without relational validation.** `crates/forge-store/src/writer.rs:158` (commit `10f84a7`) stores `parent_thread_id` and `spawned_by` without checking their existence, relationship, or self-reference. Scenario: a client creates a self-parented thread or references an agent item belonging to another parent, producing inconsistent handoff and retention behavior. Validate relationships at creation and model ordinary, handoff, and spawned threads explicitly, following “Model data as ADTs.”

55. **Low — Initial-state collections remain unbounded.** `crates/forge-store/src/threads.rs:132` (commit `10f84a7`) loads every project, and `crates/forge-store/src/read.rs:146` loads every waiting interaction. Scenario: a long-lived installation accumulates enough projects or pending questions that `/state` becomes a large allocation and response despite thread pagination. Add bounded pagination or a deliberate synchronization contract for these collections.

56. **Low — Credential-bearing types derive unrestricted `Debug`.** `crates/artisan-account/src/enrollment.rs:13` (commit `10f84a7`) derives `Debug` for the machine private key; OAuth `Client`, `Tokens`, and `Pkce` also expose secrets through derived debug output. Scenario: adding a routine debug field to diagnostics reveals credentials. Implement redacted debug formatting consistently with `ClaudeAiSession`.

57. **Low — Updating a run can make its indexed thread disagree with its serialized thread.** `crates/forge-store/src/writer.rs:485` (commit `10f84a7`) preserves the existing `runs.thread_id` column on conflict but writes the incoming run body without checking that its thread is unchanged. Scenario: an internal caller reuses a run ID with another thread and unchanged status; indexed queries and decoded responses disagree about ownership. Enforce immutable run identity and ownership before updating.

58. **Low — Thread retention leaves coordination data permanently orphaned.** `crates/forge-store/src/retention/remove.sql:1` (commit `10f84a7`) deletes thread-owned conversation data but leaves coordination records and events. Their schema has no ownership cascade. Scenario: retention removes a thread while its work declarations, messages, or watches remain indefinitely in the coordination store. Define and enforce retention for coordination ownership and references.

59. **Low — Removed threads retain valid gateway credentials and memory entries.** `crates/forge-runs/src/retention.rs:56` (commit `10f84a7`) clears selected hub maps but never removes the thread’s gateway token or reverse lookup. `gateway_thread` continues accepting that credential. Scenario: retention deletes many threads; their credentials and map entries survive until process restart. Revoke thread credentials and clear associated lifecycle state when the thread is removed.

60. **Low — Account state is a collection of independent optional fields.** `crates/ae/src/account.rs:60` (commit `10f84a7`) permits an enrolled machine without its private key or account binding. CLI status treats `machine_id` alone as enrolled, while daemon identity loading returns absence if the secret is missing. Scenario: a partial but valid JSON file makes the CLI report enrollment while account services remain off. Decode into validated enrollment variants, following “Model data as ADTs.”

61. **Low — Grep previews can omit the actual match.** `crates/forge-store/src/recall.rs:520` (commit `10f84a7`) clips every matching line to its first 400 characters. Scenario: the match occurs later in a long line; the result contains no matching text or indication of its position. Build the preview around the match and indicate omitted prefixes and suffixes.

62. **Low — A failed post-input screenshot becomes “screen did not change.”** `crates/forge-computer/src/runner.rs:311` (commit `10f84a7`) ignores capture errors and leaves `screen_changed` false. Scenario: clicks succeed but the subsequent capture fails; the caller receives a successful result falsely suggesting no visual change. Represent an unknown observation or return the capture failure with completed-step context.

63. **Low — Malformed MCP requests with IDs can receive no reply.** `crates/forge-mcp/src/server.rs:109` (commit `10f84a7`) returns `None` when `method` is absent or not a string, even when an ID is present. The gateway translates this into HTTP 202. Scenario: a malformed request is treated like a notification and the client waits for a response that never arrives. Parse and validate the request envelope before dispatch and return a structured invalid-request error.

64. **Low — Discovery output schemas do not enforce useful result contracts.** `crates/forge-mcp/src/awareness.rs:11` (commit `10f84a7`) shares an all-optional schema whose nested workers, records, and scope are unconstrained objects. Scenario: a result loses required identities or changes nested field types while still validating against the advertised schema. Define schemas from the typed result variants and require the fields each tool actually guarantees.

65. **Low — Tests remain embedded throughout production source directories.** `crates/forge-computer/src/lib.rs:192` (commit `10f84a7`) loads `src/tests.rs`; account, browser, content, HTTP, engine, and run modules also contain source-local test modules. This violates “Tests live in `tests/`, mirroring the path in `src/`.” Scenario: repository layout and test ownership continue diverging as modules grow. Move test implementations into the prescribed mirrored hierarchy.

66. **Low — Account persistence tests bypass account persistence.** `crates/ae/src/account.rs:495` (commit `10f84a7`) writes and reads a temporary JSON file directly instead of exercising `save_account` or `load_account`. Scenario: account-file writes become truncating, incorrectly permissioned, or otherwise broken while this round-trip test stays green. This is the prohibited “one-struct” testing pattern; replace it with behavioral coverage of the actual persistence boundary.

67. **Low — Vendored Mermaid advertises features whose implementation dependencies were removed.** `vendor/mermaid-rs-renderer/Cargo.toml:9` (commit `10f84a7`) declares `cli` and `png`; enabling them references the absent CLI module and undeclared `usvg`/`resvg` dependencies. Scenario: feature unification or an all-features check enables either feature and compilation fails. Remove unsupported feature surfaces and gated implementation paths, or restore their complete dependencies.

68. **Low — The Mermaid fork lacks reproducible upstream provenance.** `vendor/mermaid-rs-renderer/VENDORED.txt:1` (commit `10f84a7`) records only a version and a prose list of removed dependencies. It supplies no immutable source revision, archive checksum, or reproducible patch description. Scenario: an upgrade or audit cannot mechanically distinguish local modifications from the original package. Record the exact upstream artifact and the fork’s modifications.

69. **Low — Relative environment paths create working-directory-dependent installations.** `crates/artisan-paths/src/lib.rs:165` (commit `10f84a7`) accepts any nonempty environment value as a path, including relative `XDG_DATA_HOME`, `HOME`, or `LOCALAPPDATA`. Scenario: a malformed environment causes launches from different directories to open different data roots. Validate absolute paths at this boundary and return actionable configuration errors.

70. **Low — Failed Linux isolation attachment is invisible.** `crates/forge-isolation/src/linux.rs:225` (commit `10f84a7`) suppresses both opening and writing failures for cgroup membership and OOM priority. Scenario: permissions or cgroup state changes after startup; engines silently stay in Forge’s group despite the earlier applied-isolation announcement. Preserve an observable attachment result with the operation, process identity, and cause, as required by “Error logs carry the step, ids and cause.”

71. **Nit — The documented engine HTTP routes no longer exist.** `crates/artisan-api/src/request.rs:23` (commit `10f84a7`) documents separate `/install`, `/login`, `/logout`, and `/refresh` endpoints, while the router exposes `/engines/{engine}/commands`. Scenario: a client implementer follows the shared contract documentation and receives 404. Express the current route contract in a maintained source of truth.

72. **Nit — Recall renders compaction results as generic items.** `crates/forge-mcp/src/format.rs:24` (commit `10f84a7`) maps known recall kinds to labels but omits `compaction`, which the recall index explicitly produces. Scenario: cosmetic—checkpoint search results are labelled “Item.” Add the missing label and use a typed kind mapping to prevent omissions.

73. **Nit — Superseded thread-list dispatch code remains reachable only through an internal bypass.** `crates/forge-mcp/src/tools.rs:926` (commit `10f84a7`) retains a full `list_threads` implementation, although normal invocation is intercepted by `awareness::call` in `crates/forge-mcp/src/lib.rs:48`. Scenario: maintenance changes can land in an implementation that public calls never use. Remove the redundant branch and keep one implementation of thread listing.

## 7. Verification

Reviewer: Malene (GPT 6.1 Sol).

### 7.1 Main review

| Category | Rating | Evidence |
|---|---:|---|
| Test strategy | 2 | Substantial behavioral coverage, but the complete engine-to-screen journey remains untested ([tests.rs:1931](crates/forge-runs/src/tests.rs:1931)). |
| Performance testing | 1 | Rebuild-count checks exist; CI enforces no measured performance budgets ([gating.rs:215](crates/editor-aqua/tests/shell/gating.rs:215), [ci.yml:66](.github/workflows/ci.yml:66)). |
| Accessibility testing | 2 | Real Windows UIA probes check buttons and names, with limited scenario coverage ([uia.rs:27](crates/editor-aqua/tests/shell/uia.rs:27)). |
| Cross-platform testing | 2 | Windows/Linux workspace tests; macOS tests four backend crates ([ci.yml:127](.github/workflows/ci.yml:127)). |
| Test reliability | 1 | Unbounded polling remains; configured nextest timeouts are unused by CI ([tests.rs:709](crates/forge-runs/src/tests.rs:709)). |
| Static analysis | 2 | Strong denied Clippy lints; formatting and security analysis lack gates ([Cargo.toml:169](Cargo.toml:169), [ci.yml:61](.github/workflows/ci.yml:61)). |
| Code review | 1 | PR checks exist, but review and visual-validation practice is inconsistent ([ci.yml:5](.github/workflows/ci.yml:5), [PR #597](https://github.com/artisanstreet/editor/pull/597)). |

Reviewed committed HEAD `10f84a7`; ignored concurrent edits. No builds or tests were executed, as instructed. Ratings describe verification coverage, not a claim that the suite currently passes.

1. **High — No complete engine-to-screen regression path.** [forge-http/tests.rs:225](crates/forge-http/src/tests.rs:225) follows a run that fails because Claude is absent. [transport.rs:272](crates/editor-aqua/tests/markdown/transport.rs:272) injects an Editor event directly. Decoder, persistence, SSE and rendering can therefore disagree without these tests detecting it. Extend the existing process-backed harness through authenticated SSE into mounted, visible Editor text.

2. **High — Valuable integration and visual checks are outside the default gate.** [qualification.rs:1](crates/editor-aqua/tests/qualification.rs:1) and [markdown.rs:8](crates/editor-aqua/tests/markdown.rs:8) require `qualification`; CI enables neither qualification nor capture. The screenshot comparison is additionally ignored at [screens.rs:136](crates/editor-aqua/tests/qualification/screens.rs:136). Storage/rendering regressions can merge without these checks compiling. Add explicit feature jobs and an executed visual gate.

3. **High — Native Windows engine scenarios are underrepresented.** Claude cancellation and Codex server scenarios are Unix-gated ([claude.rs:594](crates/forge-engines/src/claude.rs:594), [codex.rs:32](crates/forge-engines/src/codex.rs:32)). Windows does have a native JobObject test, but that does not cover adapter delivery, approval and cancellation together. Use portable fake executables and run these scenarios on Windows; expand backend tests on macOS and shipped ARM targets.

4. **High — Performance regressions have no measured rejection criterion.** [gating.rs:215](crates/editor-aqua/tests/shell/gating.rs:215) checks rebuild behavior, while [ci.yml:66](.github/workflows/ci.yml:66) runs correctness tests only. An expensive accessibility update or repeated GPU work can preserve those assertions. Add frame, idle CPU, startup and long-thread memory budgets on controlled runners, including UIA-connected scrolling.

5. **Medium — Polling regressions can consume the entire CI job.** [tests.rs:709](crates/forge-runs/src/tests.rs:709) waits indefinitely for live output; [tests.rs:4073](crates/forge-runs/src/tests.rs:4073) indefinitely awaits a callback. A broken transition hangs instead of identifying its failure. Use bounded synchronization; execute nextest so [nextest.toml:4](.config/nextest.toml:4) actually applies.

6. **Medium — The DPI assertion tolerates a broken screen.** [settings/mod.rs:467](crates/editor-aqua/tests/settings/mod.rs:467) permits 1,000 logical pixels of drift in a 900×700 viewport. Controls can move outside the window while “lay out identically” passes. Assert bounded rounding differences and meaningful containment/alignment relationships.

7. **Medium — Capture failures return success.** [capture.sh:28](crates/editor-aqua/tests/qualification/capture.sh:28) converts a failed capture into a successful `echo`, then continues. Every screen can fail while the command exits zero. Accumulate failures, return nonzero and reject missing or stale output.

8. **Medium — Accessibility checks cover too little of the application.** [uia.rs:38](crates/editor-aqua/tests/shell/uia.rs:38) filters only buttons in the initial window. Missing textbox names, incorrect roles, inaccessible streamed replies and broken focus transitions remain undetected. Add populated/error/dialog scenarios with expected roles, values and focus, plus recorded Narrator/NVDA passes. Existing DPI/accessibility-bound tests should remain.

9. **Medium — Static and non-Rust checks are incomplete.** [ci.yml:58](.github/workflows/ci.yml:58) explicitly runs release Python tests, but omits [tests/nix/dev.py:67](tests/nix/dev.py:67); those tests also require WSL. [workspace.nix:290](nix/workspace.nix:290) defines only Clippy/Cargo-test checks. Formatting drift, advisory exposure and WSL launcher regressions have no corresponding gate. Add targeted checks in suitable environments.

10. **Medium — The proposed deletion strategy would remove useful protection.** [browser.rs:18](crates/artisan-account/tests/browser.rs:18) validates PKCE against an RFC vector; [glide.rs:27](crates/editor-aqua/tests/frame/glide.rs:27) checks interruption continuity; [chat_bubble.rs:55](crates/editor-aqua/tests/ui/chat_bubble.rs:55) checks joined/responsive geometry. Deleting entire categories loses interoperability and behavioral checks. Separate literal design pins from invariant assertions before deleting anything.

11. **Low — Genuine rule violations remain in the suite.** [markdown.rs:18](crates/editor-aqua/tests/markdown.rs:18) asserts a design constant, violating “Only test non-trivial behavior.” [settings/mod.rs:254](crates/editor-aqua/tests/settings/mod.rs:254) is explicitly a build-only smoke test, violating “No smoke tests.” Inline modules such as [schema.rs:937](crates/forge-store/src/schema.rs:937) also violate “Tests live in `tests/`.” Remove empty checks and relocate substantive tests without losing private-state coverage.

The earlier diagnosis needs these corrections:

- **Count:** 1,438 Rust test attributes plus 12 Python test methods supports “about 1,500.” This is a static count, not executed cases.
- **Noise:** “one third,” the subsystem percentages and “60 copies of the same race” remain unverified classifications. There are 36 kit tests, including meaningful interaction and geometry checks. Source/test co-changes do not establish low value; sampled history counts varied with traversal and path definitions.
- **Joined layers:** “never joined” is false. [tests.rs:1933](crates/forge-runs/src/tests.rs:1933) runs a fake Claude process through the real adapter/decoder into runs and storage. HTTP/TLS/SSE integration also exists. The missing piece is the complete journey.
- **Time/IO:** No paused Tokio clock is used, but explicit-time animation tests, injected free-space probes and injected transaction failures exist ([guard.rs:6](crates/forge-runs/tests/disk/guard.rs:6), [store.rs:742](crates/forge-store/tests/store.rs:742)). Actual fsync/ENOSPC/crash fault coverage remains missing.
- **Migrations:** “Old databases are built with today’s code” is overbroad: [store.rs:574](crates/forge-store/tests/store.rs:574) loads frozen schema-v3 SQL. Later migration tests do use current migrations.
- **Setup:** I counted 70 `Core::for_tests` and 87 `UiApp::new` test-file occurrences, versus 64/82 claimed. `Core::for_tests` is feature-gated ([store.rs:194](crates/editor-core/src/core/store.rs:194)).

The 30 merged PRs returned **zero recorded GitHub reviews**, which does not prove no review occurred: [#621](https://github.com/artisanstreet/editor/pull/621) documents five AI-review rounds. Several UI PRs explicitly lacked on-screen validation; [#605](https://github.com/artisanstreet/editor/pull/605) records an unresolved intermittent test failure.

To reach **3** in each category:

- **Test strategy:** extend existing integration harnesses; prioritize missing journeys before deletion. Retain protocol vectors and relational geometry checks.
- **Performance:** enforce absolute budgets alongside relative regressions; include native GPU and UIA costs.
- **Accessibility:** assert complete scenario trees and keyboard/focus behavior; record screen-reader validation.
- **Cross-platform:** test supported backend/platform combinations and native Windows adapters; defer unavailable Editor backends.
- **Reliability:** bound waits, control async time and inject targeted storage/network faults. Keep real-process tests with deadlines.
- **Static analysis:** gate formatting, advisory checks, relevant feature combinations and Python/WSL tests.
- **Code review:** record final-commit review and visual evidence consistently.

The proposed replay/scenario layers fit this project. A fully deterministic scheduler/filesystem simulator is a later investment; begin with targeted fault injection. Do not automatically delete mutation-surviving tests, and do not replace cheap protocol assertions with nondeterministic live-model checks.

| Score | Slice assessment |
|---|---:|
| Correctness | 6/10 |
| State handling | 6/10 |
| Code quality | 6/10 |
| Rule adherence | 4/10 |
| Overall | **5/10** |

### 7.2 Remaining findings and nits

12. **Medium — Stacked PRs bypass CI.** [.github/workflows/ci.yml:6](.github/workflows/ci.yml:6) restricts PR checks to a `master` base. PRs [#593](https://github.com/artisanstreet/editor/pull/593) and [#594](https://github.com/artisanstreet/editor/pull/594), targeting feature branches, had no repository CI checks. A broken intermediate branch can therefore merge unchecked. Run PR verification for supported stack bases too.

13. **Medium — UIA tests bypass the Editor’s native accessibility publication path.** [crates/editor-aqua/tests/shell/uia.rs:19](crates/editor-aqua/tests/shell/uia.rs:19) copies an extracted tree into `UiaProbe`, which creates its own provider window; the Editor fixture is headless. The tests can pass when the actual Editor window fails to attach or update its provider. Add a native-window check that observes the Editor’s own provider across updates.

14. **Medium — The “three remote reads” assertion counts test statements.** [crates/forge-store/tests/source_index.rs:125](crates/forge-store/tests/source_index.rs:125) checks a variable incremented manually before three Store calls. Additional internal reads or client requests cannot change that counter. Instrument the actual boundary whose request budget matters and assert its observed calls.

15. **Medium — The SSE test reader corrupts split UTF-8.** [crates/forge-http/src/tests.rs:60](crates/forge-http/src/tests.rs:60) applies `String::from_utf8_lossy` separately to each response chunk. Splitting a multibyte character between chunks inserts replacement characters into otherwise valid JSON text. Accumulate bytes until complete frames are available, or use the streaming decoder used by the client.

16. **Medium — Premature SSE closure can pass the resume test.** [crates/forge-http/src/tests.rs:59](crates/forge-http/src/tests.rs:59) treats EOF as successful completion even when the stop predicate never matched. At line 270, the resume assertion accepts an empty list or only an unsequenced Hello. A server that closes before engine status delivery can pass. Fail on EOF before the expected event and assert that event explicitly.

17. **Medium — Source-policy enforcement fails open on filesystem errors.** [crates/editor-ui/tests/ui/kit/enforcement.rs:68](crates/editor-ui/tests/ui/kit/enforcement.rs:68) flattens directory errors away; line 98 turns unreadable source into an empty string. Missing directories or unreadable files can produce a clean result without inspection. Propagate errors and verify that every intended source root was scanned.

18. **Medium — PR checks do not establish that the Editor builds without test support.** [crates/editor-aqua/Cargo.toml:74](crates/editor-aqua/Cargo.toml:74) enables `test-support` through a self dev-dependency. Workspace tests and Clippy with all targets therefore exercise that feature configuration, while [.github/workflows/ci.yml:95](.github/workflows/ci.yml:95) explicitly builds only `ae` and `forge`. A feature-dependent Editor compilation error can escape PR verification. Add a separate ordinary Editor build with the shipping feature set.

19. **Medium — MCP OAuth callback validation lacks focused regression coverage.** [crates/editor-core/src/core/mcp_sign_in.rs:90](crates/editor-core/src/core/mcp_sign_in.rs:90) independently handles callback paths, state, denial and authorization codes. The audited tests cover the separate Artisan account callback, not this implementation. Removing this state check or mishandling cancellation would lack a targeted failure. Test wrong and missing state, malformed requests, denial, empty codes and stale attempts.

20. **Medium — Client thread-page reconciliation lacks focused tests.** [crates/editor-core/src/core/host/pages.rs:172](crates/editor-core/src/core/host/pages.rs:172) merges responses against newer events, tombstones, archive state and paging direction; line 381 prunes several related maps. The tests exercise server thread pages but contain no client `ThreadsPage` scenarios. Stale page responses could resurrect threads or evict required state undetected. Test these transitions with interleaved page completions and events.

21. **Medium — Negative asynchronous assertions use elapsed time as completion evidence.** [crates/forge-runs/src/tests.rs:3573](crates/forge-runs/src/tests.rs:3573) sleeps 200 ms before asserting that disabled side queries asked nothing. A delayed erroneous task can run after the assertion. Similar short observation windows appear in retry checks. Wait for the relevant task lifecycle to finish before asserting absence.

22. **Low — External-input parsers have no focused coverage.** [crates/editor-core/src/core/screen.rs:236](crates/editor-core/src/core/screen.rs:236), [hosts.rs:210](crates/editor-core/src/core/hosts.rs:210) and [hosts.rs:275](crates/editor-core/src/core/hosts.rs:275) parse deep links, WSL listings and pairing text without dedicated tests. Empty identifiers, localized listings and malformed fingerprints can change behavior unnoticed. Add boundary cases and expected normalized results for each parser.

23. **Low — The detached-service fixture is Linux-specific despite its Unix gate.** [crates/ae/tests/detached_service.rs:40](crates/ae/tests/detached_service.rs:40) installs Forge under an XDG Linux path, while the file permits macOS. macOS resolves Forge beneath `~/Library/Application Support/Artisan Street/Forge`, so enabling this suite there fails before exercising service behavior. Construct the fixture using platform-correct paths.

24. **Low — Installer and self-update behavior is tested only on Linux.** [crates/ae/tests/ae.rs:1](crates/ae/tests/ae.rs:1) excludes the release installation, corrupted-release rejection and remote-feed update scenarios from Windows. Windows executable replacement and platform paths can regress while its workspace tests remain green. Port the behavioral scenarios using platform-specific fixture paths and replacement expectations.

25. **Low — The Codex process fixture assumes Linux tools under a Unix gate.** [crates/forge-engines/src/codex/server_tests.rs:27](crates/forge-engines/src/codex/server_tests.rs:27) requires Bash associative arrays, and line 31 requires `flock`. The enclosing module is enabled for all Unix systems. Stock macOS lacks this combination. Use a portable fixture executable or explicitly provision and validate these dependencies.

26. **Low — The Codex fake cannot faithfully round-trip arbitrary message text.** [crates/forge-engines/src/codex/server_tests.rs:68](crates/forge-engines/src/codex/server_tests.rs:68) extracts JSON text with `sed`, then interpolates it into response JSON without escaping. Quotes and backslashes in a new scenario can corrupt the fixture protocol. Parse and serialize JSON properly, with round-trip cases for escaped and Unicode input.

27. **Low — The source-policy scanner confuses source text with Rust syntax.** [crates/editor-ui/tests/ui/kit/enforcement.rs:35](crates/editor-ui/tests/ui/kit/enforcement.rs:35) searches raw substrings. A displayed code string can trigger a violation, while equivalent constructor syntax split across whitespace can evade the scanner. Enforce syntax-dependent policy through an AST-aware lint.

28. **Low — The blocked-sender test signals readiness before reaching the blocking operation.** [crates/editor-aqua/tests/core/ordering.rs:397](crates/editor-aqua/tests/core/ordering.rs:397) sends `started` before polling `send`; the second case repeats this at line 418. Another runtime worker can receive that signal before the operation has attempted admission. `is_finished() == false` then proves scheduling state rather than backpressure. Poll the send future to Pending directly before testing cancellation or receiver removal.

29. **Low — A test listener thread never shuts down.** [crates/editor-aqua/tests/shell/lifetime.rs:13](crates/editor-aqua/tests/shell/lifetime.rs:13) detaches an endless accept loop without a stop mechanism. It retains a thread and socket after the scenario completes. Return an owned server guard that stops and joins the worker.

30. **Low — Service tests release their port reservation before starting the service.** [crates/ae/tests/detached_service.rs:105](crates/ae/tests/detached_service.rs:105) returns an ephemeral address after dropping its listener. Another parallel test or process can claim it before Forge binds. Prefer binding port zero inside the service and reporting the chosen address, or transfer an owned listener.

31. **Low — The cross-device failure fixture assumes `/dev/shm` exists and is another filesystem.** [crates/forge-engines/src/customise/skills.rs:1312](crates/forge-engines/src/customise/skills.rs:1312) relies on host mount layout to reach the intended copy/delete path. Containers can omit that mount or place both temporary roots on the same filesystem. Inject the cross-device condition or explicitly validate the required filesystem arrangement.

32. **Low — Permission-failure tests report success when they skip their assertions.** [crates/forge-engines/src/customise/skills.rs:1241](crates/forge-engines/src/customise/skills.rs:1241) detects root and makes both tests return normally. A root-run suite reports rollback protection as passing without exercising it. Use deterministic failure injection or a separately reported environment-dependent test job.

33. **Low — Fake HTTP server shutdown depends on an exact request count.** [crates/artisan-account/tests/network.rs:17](crates/artisan-account/tests/network.rs:17) waits for two connections, then the test joins the worker at line 42. If client behavior returns successfully without the optional second request, the assertion is never reached because the join waits forever. Give the fixture explicit cancellation and bounded socket operations.

34. **Low — Transcript status assertions accept missing runs.** [crates/forge-store/tests/transcript.rs:330](crates/forge-store/tests/transcript.rs:330) and line 358 use `.all()` without requiring the expected run. Returning no runs passes both “working” and “finished” checks. Assert the expected run identity and status.

35. **Low — Animation assertions accept disappearing digit reels.** [crates/editor-aqua/tests/ui/phrase.rs:369](crates/editor-aqua/tests/ui/phrase.rs:369) checks intermediate reels with `.all()`, as does the landed-state assertion at line 378. Both pass if those frames contain no reels. Require the expected reel count and visible resulting text at each phase.

36. **Low — Schema compatibility coverage can disappear silently.** [crates/forge-engines/src/codex/schema_tests.rs:244](crates/forge-engines/src/codex/schema_tests.rs:244) skips anything the parser classifies as Unknown. Accidentally removing a supported method’s parse arm therefore removes its schema checks without failing them. Example generation also truncates at line 38. Track the expected supported methods and report omitted branches explicitly.

37. **Low — The case-insensitive search test preserves a Unicode failure.** [crates/editor-aqua/tests/shell/reveal.rs:9](crates/editor-aqua/tests/shell/reveal.rs:9) expects `"SMØR"` not to match `"smør"`, under a test named “whatever its case.” This guards the current ASCII limitation instead of the user-facing promise. Specify the intended case-folding behavior and test correct UTF-8 source ranges.

38. **Low — Screenshot tolerance values are not validated.** [crates/editor-aqua/tests/qualification/diff.rs:224](crates/editor-aqua/tests/qualification/diff.rs:224) deserializes `max_ratio` without checking its domain. A value above 1 silently permits every pixel to differ; a negative value rejects identical images. Reject invalid tolerances at the configuration boundary.

39. **Low — Removing a baseline silently removes its verification requirement.** [crates/editor-aqua/tests/qualification/diff.rs:244](crates/editor-aqua/tests/qualification/diff.rs:244) derives the entire screen inventory from existing reference PNGs. Deleting one baseline leaves the remaining reports eligible to pass. Validate references and candidates against an explicit required-screen manifest.

40. **Low — Screenshot directory enumeration discards errors.** [crates/editor-aqua/tests/qualification/diff.rs:246](crates/editor-aqua/tests/qualification/diff.rs:246) uses `filter_map(Result::ok)`. An unreadable directory entry can disappear from the comparison, allowing a partial set to pass. Propagate enumeration errors with the affected directory.

41. **Low — Screenshot reports discard decoding failures.** [crates/editor-aqua/tests/qualification/diff.rs:153](crates/editor-aqua/tests/qualification/diff.rs:153) replaces the image error with `Unreadable` and no cause or path. A missing permission, truncated PNG and unsupported image become indistinguishable. Preserve the underlying diagnostic in `ScreenReport`.

42. **Low — Heat-map output retains obsolete failures.** [crates/editor-aqua/tests/qualification/diff.rs:272](crates/editor-aqua/tests/qualification/diff.rs:272) writes failing comparisons but never removes an existing heat map when that screen later passes. Reusing the output directory leaves misleading review artifacts. Remove obsolete files or create a fresh artifact directory per comparison.

43. **Low — The explicitly selected real-database test can do nothing successfully.** [crates/forge-store/tests/recall.rs:249](crates/forge-store/tests/recall.rs:249) returns normally when `FORGE_RECALL_DB` is absent. Running ignored tests can therefore report this qualification as passing without opening a database. Require its input when explicitly selected.

44. **Low — The real-database fixture copies only the SQLite main file.** [crates/forge-store/tests/recall.rs:253](crates/forge-store/tests/recall.rs:253) omits WAL contents and snapshot coordination. If the supplied path belongs to an active WAL database, committed data can be missing from the measured copy. Use SQLite’s backup mechanism or require a verified, quiescent snapshot.

45. **Low — The real-database benchmark prints conversation content.** [crates/forge-store/tests/recall.rs:262](crates/forge-store/tests/recall.rs:262) debug-prints search hits, including titles and snippets from the supplied database. Running it with captured output can expose real thread content. This conflicts with “keep payloads out of logs.” Report counts, timings and redacted identifiers.

46. **Low — Workspace lint strength is not uniform across the application.** [crates/editor-core/Cargo.toml:36](crates/editor-core/Cargo.toml:36) and [editor-aqua/Cargo.toml:81](crates/editor-aqua/Cargo.toml:81) maintain independent lint lists that omit workspace restrictions such as `unwrap_used`, `expect_used` and `map_err_ignore`. Adding a workspace restriction does not protect these crates. Share a baseline and make intentional exceptions explicit.

47. **Low — Linux stops after the first failing test executable.** [.github/workflows/ci.yml:66](.github/workflows/ci.yml:66) omits the `--no-fail-fast` option used on Windows. An early failure prevents later test executables from running, leaving incomplete evidence for review. Run the complete selected suite before reporting failure.

48. **Low — Exact cache hits prevent the build cache from advancing with source changes.** [.github/workflows/ci.yml:56](.github/workflows/ci.yml:56) keys `target` only on dependency and configuration files; Windows and macOS use the same pattern. Source-only commits repeatedly restore the same immutable cache entry. Use a source-varying save key with dependency-based restore prefixes, or separate dependency caching from changing build outputs.

49. **Low — The diagnosis overstates the absence of range-state tests.** [crates/editor-aqua/tests/conversation/stored_rows.rs:196](crates/editor-aqua/tests/conversation/stored_rows.rs:196) exercises failure, stale completion rejection, retry and successful readiness; it also checks isolation between owners. Listing `RangeState` wholesale as untested misdirects replacement work. Document the specific uncovered transitions instead.

50. **Low — The proposed gap-free replay invariant contradicts the storage contract.** [crates/forge-store/src/read.rs:607](crates/forge-store/src/read.rs:607) reconstructs changes from current row revisions, rather than retaining every historical update. Updating one row repeatedly before reconnecting legitimately leaves sequence gaps. A simulator requiring contiguous replay sequences would reject correct behavior. Assert monotonic cursors and convergence to committed state.

51. **Low — The proposed restart invariant ignores continuation policy.** [crates/forge-store/src/writer.rs:621](crates/forge-store/src/writer.rs:621) deliberately declines continuation when disabled or ineligible; eligibility also excludes missing sessions and archived threads. Unconditionally asserting “restart resumes live turns” would misclassify these outcomes. Model policy and eligibility in the simulation oracle.

52. **Low — Test SQL violates the stated query rules.** [crates/forge-store/tests/source_index.rs:139](crates/forge-store/tests/source_index.rs:139) embeds substantive queries in Rust strings; [store.rs:581](crates/forge-store/tests/store.rs:581) embeds fixture statements too. The rules say “Queries live in separate `.sql` files” and “Typed queries: SQLx in Rust.” Move fixture SQL into assets and apply the required typed-query approach to query checks.

53. **Low — The screen comparison API accepts malformed buffers.** [crates/editor-aqua/tests/qualification/diff.rs:69](crates/editor-aqua/tests/qualification/diff.rs:69) zips complete RGBA chunks without validating either buffer against its dimensions. A candidate with an empty or shortened buffer can compare as unchanged. Current PNG-loading callers provide valid buffers, but the public helper permits this false pass. Validate construction and reject malformed or zero-sized screens.

54. **Nit — Test files retain prohibited explanatory comments.** [crates/ae/tests/account_json.rs:1](crates/ae/tests/account_json.rs:1), [forge-runs/src/tests.rs:60](crates/forge-runs/src/tests.rs:60) and [forge-engines/src/codex/schema_tests.rs:17](crates/forge-engines/src/codex/schema_tests.rs:17) contain headers, inline explanations and documentation comments. Cosmetic; these violate “Never write code comments,” with neither license nor tool-directive exemptions applying. Express the information through names and structure.

55. **Nit — Scanner line numbers are wrong for matches beginning a line.** [crates/editor-ui/tests/ui/kit/enforcement.rs:52](crates/editor-ui/tests/ui/kit/enforcement.rs:52) counts `.lines()` in the prefix. When that prefix ends immediately after a newline, the terminating empty line is omitted, so a column-one match is reported one line early. Cosmetic; count preceding newline bytes and add one.

56. **Nit — A cryptographic fixture test calculates an unused canonical request.** [crates/artisan-account/src/grant.rs:641](crates/artisan-account/src/grant.rs:641) computes `canonical`, then discards it at line 688. Cosmetic; it suggests an additional assertion that never occurs. Remove the calculation or assert the independently expected canonical bytes.

57. **Nit — Native qualification calculates an unused foreground PID.** [crates/editor-aqua/tests/qualification/native.ps1:290](crates/editor-aqua/tests/qualification/native.ps1:290) obtains `$foreground_pid`, then calls `AssertBackground`, which independently performs that check. Cosmetic; remove the unused lookup or share its result with the assertion.

## 8. Security and privacy

Reviewer: Kristina (GPT 6.1 Sol).

### 8.1 Main review

Reviewed commit `10f84a7ada6519109564cd433aed23c4e36676fa`. Read-only source review; no builds or tests executed. Line references refer to that commit.

| Category | Rating 0–3 | Justification |
|---|---:|---|
| Security | 2 | Deliberate TLS and route authorization, undermined by credential and media boundaries: [routes.rs:149](crates/forge-http/src/routes.rs:149). |
| Supply chain | 2 | Git revisions, exact dependency versions, Cargo.lock and vendor provenance; CI lacks vulnerability scanning: [Cargo.toml:49](Cargo.toml:49), [ci.yml:58](.github/workflows/ci.yml:58). |
| Privacy | 2 | Thirty-day retention and protected credential files exist; plaintext storage and automatic network requests remain: [settings.rs:163](crates/artisan-api/src/settings.rs:163). |
| Code signing | 2 | Updates verify Ed25519 signatures and SHA-256; bootstrap bypasses signatures, and native binaries lack platform signing: [release.rs:158](crates/ae/src/release.rs:158). |
| Vulnerability process | 0 | No committed SECURITY.md, disclosure instructions, patch SLA or advisory workflow found in the target tree; release gating checks CI only: [release.py:66](src/release.py:66). |

Verified boundaries: Forge defaults to `127.0.0.1:47900`; configuration can expose other interfaces. MCP gateway, generation controls, upgrade controls and OAuth callbacks bind loopback. Protected HTTPS routes share authorization middleware; pairing consumes an expiring code transactionally. Certificate pinning and handshake signatures are checked. Blob/content identifiers resist straightforward traversal. Engine launches use argument vectors; reviewed shell tools intentionally execute commands. Deep links select navigation targets. HTML renders as literal source, and image previews have resource limits. I found no first-party telemetry implementation; provider telemetry behavior remains unverified.

Findings, most severe first:

1. **High — Bootstrap executes an installer without authenticating its signature.** [install.sh:36](scripts/install.sh:36) downloads the manifest; line 54 compares its checksum, then line 59 executes the binary. It never fetches `release.json.sig`. An attacker replacing both release assets can supply matching malicious bytes despite lacking Artisan’s signing key. Verify the manifest against a pinned key before execution.

2. **High — Agent output bypasses file-access policy through resource snapshots.** [resources.rs:251](crates/forge-runs/src/resources.rs:251) accepts absolute paths, `..`, home paths and symlinks without checking tool permissions. [run.rs:1160](crates/forge-runs/src/run.rs:1160) automatically snapshots references from completed assistant messages. Output naming a denied `.env` or private key therefore copies it into Forge storage. Apply the same access policy before reading; require approval outside authorized roots.

3. **High — Forge’s credential database inherits permissive filesystem defaults.** [forge-store/lib.rs:185](crates/forge-store/src/lib.rs:185) opens ordinary SQLite without establishing private directory/file permissions. It contains Forge’s private identity and MCP refresh tokens. With umask `022` and traversable ancestors, another local account can read them. Establish private permissions before creation and repair existing databases, including journal files.

4. **High — MCP launch files expose credentials through default permissions.** [customise.rs:136](crates/forge-engines/src/customise.rs:136) uses `fs::write` for files containing headers and environment secrets, including [Codex configuration:348](crates/forge-engines/src/customise.rs:348). Under umask `022`, those files can be readable by other local users. Use private directories and atomically created `0600` credential files.

5. **High — OAuth client credentials survive a change of provider.** [mcp_auth.rs:142](crates/forge-runs/src/mcp_auth.rs:142) retrieves registrations by server name and validates only the redirect URI. Changing that server’s URL can reuse the old provider’s client secret, which [exchange:376](crates/forge-engines/src/mcp_oauth.rs:376) sends to the new token endpoint. Bind registrations to resource URL and issuer; invalidate them when either changes.

6. **Medium — OAuth discovery permits insecure endpoints without issuer validation.** [mcp_oauth.rs:203](crates/forge-engines/src/mcp_oauth.rs:203) accepts endpoint strings directly from metadata; [tokens:409](crates/forge-engines/src/mcp_oauth.rs:409) posts credentials there. A discovered non-loopback HTTP endpoint exposes codes, verifiers and refresh tokens to network interception. Require HTTPS, validate metadata issuer/resource identity, and constrain redirects.

7. **Medium — OAuth response limits apply after allocation.** [mcp_oauth.rs:450](crates/forge-engines/src/mcp_oauth.rs:450) buffers the entire response before checking `MAX_BODY`; metadata fetching does likewise at line 264. A hostile MCP authorization server can consume substantial memory before rejection. Enforce limits while streaming.

8. **Medium — Signing out does not cancel pending OAuth flows.** [mcp_auth.rs:223](crates/forge-runs/src/mcp_auth.rs:223) deletes saved tokens but leaves pending flows. A callback arriving afterward can complete and persist fresh credentials, reversing the user’s sign-out. Cancel matching flows and prevent in-flight exchanges from saving after sign-out.

9. **Medium — Unauthenticated connections have no TLS deadline or admission bound.** [forge-http/lib.rs:390](crates/forge-http/src/lib.rs:390) spawns every accepted socket; [line 418](crates/forge-http/src/lib.rs:418) waits indefinitely for TLS. When exposed on a network interface, idle connections can exhaust descriptors before authentication. Bound concurrent handshakes and enforce deadlines.

10. **Medium — Engine payloads enter logs and diagnostic traces.** [process.rs:148](crates/forge-engines/src/process.rs:148) logs arbitrary stderr; [line 181](crates/forge-engines/src/process.rs:181) records complete outbound JSON when tracing is enabled. Credentials echoed by an engine and confidential prompts can persist. This violates **“keep payloads out of logs.”** Log structured metadata and redact secrets before optional capture.

11. **Medium — Rendering remote images silently contacts arbitrary destinations.** [source.rs:338](crates/editor-ui/src/ui/media/source.rs:338) fetches image URLs automatically, following redirects. Agent output can track viewing through a unique URL or trigger GETs against loopback/private services. Require consent for external media and validate destinations across redirects.

12. **Medium — Forge isolation supplies no independent security sandbox.** [unix.rs:5](crates/forge-isolation/src/unix.rs:5) adjusts scheduling/cgroups; [lib.rs:53](crates/forge-isolation/src/lib.rs:53) makes Windows containment a no-op. A compromised engine or MCP subprocess retains host-user access regardless of model tool restrictions. Introduce OS-enforced filesystem/network restrictions for restricted modes and test credential isolation.

13. **Medium — Release controls omit native signing and dependency vulnerability gates.** [release.yml:208](.github/workflows/release.yml:208) builds/packages Windows binaries without Authenticode; [ci.yml:58](.github/workflows/ci.yml:58) runs tooling tests, lint and tests without cargo-audit/deny. A vulnerable dependency can pass release gating, while direct downloads lack OS-verifiable publisher identity. Add platform signing, advisory/license checks and release SBOMs.

What perfect looks like:

- **Security:** Document trust boundaries; enforce media/file permissions and subprocess isolation; add negative tests for OAuth provider changes, sign-out races and connection exhaustion.
- **Supply chain:** Gate releases on advisory/license checks and vendor provenance validation; publish SBOMs and provenance; verify reproducibility.
- **Privacy:** Protect credentials with OS-backed encryption; require consent for remote media; extend retention/deletion guarantees to logs, engine histories and credential copies.
- **Code signing:** Authenticate bootstrap downloads before execution; sign native binaries and enforce verification in release tooling.
- **Vulnerability process:** Publish SECURITY.md with private reporting instructions, supported versions and response/patch targets; maintain advisories and rehearse disclosure handling.

| Score | Security/privacy slice |
|---|---:|
| Correctness | 5/10 |
| State handling | 5/10 |
| Code quality | 6/10 |
| Rule adherence | 5/10 |
| **Overall** | **4/10** |

### 8.2 Remaining findings and nits

14. **High — Windows command checks use the wrong shell grammar.** `10f84a7:crates/forge-engines/src/claudeai/rules.rs:837` parses POSIX quoting, while `crates/forge-engines/src/claudeai/tools.rs:1733` executes through `cmd /C`. With an allow rule for `echo`, `echo 'ok & whoami & echo '` appears to the checker as one allowed command; Windows executes the additional commands because apostrophes do not quote them. Use the same shell and grammar for checking and execution, including background shells.

15. **High — Claude’s gateway credential is exposed in process arguments.** `10f84a7:crates/forge-engines/src/customise.rs:427` embeds the authenticated MCP URL in inline `--mcp-config` JSON. On systems exposing process arguments to other local accounts, another user can recover the token and call Forge’s gateway as that thread. Pass the configuration through a private, per-launch file or another protected channel.

16. **High — Config sync forwards existing credentials to replacement MCP destinations.** `10f84a7:crates/forge-http/src/sync.rs:397` replaces a server’s URL or command while preserving its previous headers and environment. A bundle changing an endpoint can therefore send an existing local `Authorization` header to a different provider, or give existing environment secrets to a replacement executable. Bind credentials to the destination and require explicit reassociation when it changes.

17. **High — Any connected Forge can trigger transfer of the local Claude browser session.** `10f84a7:crates/editor-core/src/core/host.rs:483` renews whenever that host reports `NeedsRenewal` and the device has a renewable browser profile. It does not establish that this device originally signed that host in, or that the accounts match, before sending captured cookies at line 507. A newly connected or compromised Forge can request renewal and receive the local account’s session. Record the authorized host/account binding and check it before capture and transfer.

18. **High — Read and Grep can consume unbounded special files.** `10f84a7:crates/forge-engines/src/claudeai/tools.rs:725` rejects directories and oversized metadata, then calls an unbounded filesystem read at line 741. `/dev/zero` reports a small size but produces unlimited bytes; a FIFO can block indefinitely. Grep has the same problem at line 1021. Reject non-regular files and enforce the byte limit during reading, with cancellation and appropriate open-time protections.

19. **Medium — Engine downloads have no independent publisher-signature verification.** `10f84a7:crates/forge-engines/src/install.rs:639` obtains OpenCode’s artifact digest from the same GitHub release that supplies the executable. Verification at line 790 checks hashes and sizes only. Replacing both assets bypasses the application updater’s separate release-signing protection. Authenticate first-party engine manifests with a pinned signing key and verify upstream signatures or attestations where supported.

20. **Medium — Unreadable or malformed permission files fail open.** `10f84a7:crates/forge-engines/src/claudeai/rules.rs:160` silently skips unreadable files and skips malformed or oversized files after a warning. Consequently, an invalid settings file containing intended deny rules becomes an empty policy, and tools such as Read run by default. Distinguish an absent policy from a policy that failed to load; refuse affected execution until the latter is repaired.

21. **Medium — Task calls bypass permission-rule evaluation.** `10f84a7:crates/forge-engines/src/claudeai/mod.rs:627` returns directly into `run_task` before reaching `tools::plan`. An explicit `deny: ["Task"]` or `ask: ["Task"]` rule therefore does not control actual subagent creation. Evaluate policy before dispatching Task, and test through the real call dispatcher.

22. **Medium — Unicode command names can panic the permission checker.** `10f84a7:crates/forge-engines/src/claudeai/rules.rs:726` slices a string at `name.len() - 4` without checking a UTF-8 boundary. A program name such as `éaaa` makes that index fall inside `é`. Parsing model-supplied Bash input can therefore panic before execution. Use checked suffix access or byte comparisons that do not slice arbitrary Unicode boundaries.

23. **Medium — CLI revocation does not promptly close an existing SSE stream.** `10f84a7:crates/forge-http/src/events.rs:342` waits for in-process revocation announcements. The CLI deletes directly through `Admin`, and `Devices::refresh` notices that deletion only when another authorization check occurs. A revoked device can keep receiving events without making another request. Add a database-change watcher or bounded authorization recheck for open streams.

24. **Medium — Tunneled event streams do not independently enforce grant expiry.** `10f84a7:crates/forge-http/src/relay.rs:550` starts an event pump without an expiry deadline. Expiry is checked on incoming application data at line 426, but outgoing frames pass through `handle_stream_msg` and `send_payload` without that check. If Relay leaves the connection open, a subscription can continue beyond its inner grant’s expiry. Close the channel at expiry and check authorization before sending queued data.

25. **Medium — Concurrent launches race rotating refresh tokens.** `10f84a7:crates/forge-runs/src/mcp_auth.rs:341` reads saved credentials, refreshes remotely, and writes the replacement without per-server serialization. Two launches can use the same refresh token; one succeeds while the other receives a rejection and deletes the newly saved credentials. Serialize refresh ownership per server and re-read credentials after acquiring it.

26. **Medium — Provider outages destroy valid OAuth credentials.** `10f84a7:crates/forge-engines/src/mcp_oauth.rs:455` turns every unsuccessful HTTP response into `Rejected`. `crates/forge-runs/src/mcp_auth.rs:370` treats that type as a revoked grant and signs out. A token endpoint returning 429 or 503 therefore deletes a usable refresh token. Separate transient HTTP failures from actual OAuth grant rejection.

27. **Medium — OAuth probing classifies server failures as unauthenticated access.** `10f84a7:crates/forge-engines/src/mcp_oauth.rs:134` returns `Probe::Open` for every status other than 401, including 403, 404 and 500. `check_mcp_auth_inner` then removes the recorded sign-in requirement. During an outage, the UI can incorrectly stop offering sign-in. Classify successful responses, authentication failures and other failures separately.

28. **Medium — Background auth checks overwrite newer sign-in state.** `10f84a7:crates/forge-runs/src/mcp_auth.rs:244` snapshots the whole status map, performs network requests, then replaces it at line 279 without using `mcp_auth_writes`. A sign-in completed during that interval can be overwritten by the older probe result. Apply observations under the existing write lock against current configuration and state.

29. **Medium — Deleted threads retain valid gateway credentials.** `10f84a7:crates/forge-runs/src/gateway.rs:43` authorizes solely from the token map, whose entries have no removal path. `crates/forge-runs/src/retention.rs:56` forgets other thread state but leaves these credentials. A retained token for a deleted thread can still reach MCP operations that do not look up the caller, such as listing projects. Revoke gateway credentials when deleting their owning thread and validate ownership at admission.

30. **Medium — Removing an MCP server leaves its OAuth secrets behind.** `10f84a7:crates/forge-store/src/settings.rs:93` removes only the settings entry. Neither that operation nor its HTTP caller deletes the corresponding token and client-registration secrets. Removing and later re-adding the same name and URL can silently restore the previous account. Delete associated secrets and pending flows when removing the connection.

31. **Medium — MCP listing sends credential-bearing URLs and arguments to the model.** `10f84a7:crates/forge-mcp/src/tools.rs:1567` prints the complete server URL or command arguments into tool output. Although environment values are hidden, query-string API keys, URL passwords and `--api-key` arguments are exposed to the engine/provider and retained in conversation output. Render a sanitized destination and redact credential-bearing arguments.

32. **Medium — Background Claude renewal can undo sign-out.** `10f84a7:crates/editor-core/src/core/host.rs:502` launches renewal without retaining a cancellation handle or generation check. If the user signs out while capture is pending, the task can subsequently submit `ClaudeAiSignIn` and save the session again. Own renewal as cancellable host state and reject a result invalidated by sign-out.

33. **Medium — Release fetching has neither a size limit nor an overall deadline.** `10f84a7:crates/ae/src/release.rs:154` builds the client without a timeout, and `fetch` accumulates every response into a `Vec` at line 201. An endpoint can hang an update indefinitely or exhaust memory while supplying an unauthenticated manifest, signature, or artifact. Apply small metadata limits, explicit artifact limits, deadlines, and streaming verification.

34. **Medium — Uninstall with purge leaves the authenticated browser profile behind.** `10f84a7:crates/claude-ai-browser/src/lib.rs:58` stores the profile directly under the installation root, outside every application’s `data` directory. `crates/ae/src/install.rs:823` removes only selected application directories. A full purge can therefore leave Claude login cookies and browser history behind, and later uninstall attempts return “Nothing to uninstall.” Include the owned browser profile in explicit purge handling.

35. **Low — Concurrent sign-in starts defeat the advertised flow reuse.** `10f84a7:crates/forge-runs/src/mcp_auth.rs:109` checks for an existing flow, releases the lock, performs discovery and registration, and inserts later at line 161. Concurrent starts can both register clients and create flows for the same server and redirect URI. Represent discovery/registration as an owned starting state so concurrent callers join it.

36. **Low — Provider-controlled expiry arithmetic can overflow.** `10f84a7:crates/forge-engines/src/mcp_oauth.rs:430` computes `now + seconds * 1000` directly from an arbitrary JSON integer. Large values panic with overflow checks or wrap into incorrect timestamps; `Tokens::stale` also subtracts without protection. Validate a sensible expiry range and use checked arithmetic at the decoding boundary.

37. **Low — OAuth token responses are accepted without validating their usable token contract.** `10f84a7:crates/forge-engines/src/mcp_oauth.rs:418` accepts an empty access token and ignores `token_type`, while `crates/forge-runs/src/mcp_auth.rs:330` always sends it as Bearer. A malformed response or unsupported token type is stored as signed-in and fails only when launching tools. Decode into a validated, nonempty supported token representation.

38. **Low — Secret-bearing public types derive unrestricted Debug.** `10f84a7:crates/artisan-api/src/tls.rs:17` exposes the private identity key through derived formatting. `crates/forge-engines/src/mcp_oauth.rs:49`, line 57 and line 76 similarly expose client secrets, tokens and PKCE verifiers. A diagnostic print of these values leaks credentials. Implement redacted formatting consistently; no actual logging of these particular values was verified.

39. **Low — Cancelling browser capture skips browser cleanup.** `10f84a7:crates/claude-ai-browser/src/lib.rs:84` closes the browser only after the awaited capture operation returns normally. Dropping the capture future bypasses that cleanup, while the child is detached at line 314. An interrupted sign-in can leave the authenticated debugging-enabled browser running. Keep an owned browser-session guard with cancellation-safe teardown.

40. **Low — Private blobs are explicitly marked publicly cacheable.** `10f84a7:crates/forge-http/src/routes.rs:973` returns `public, max-age=31536000, immutable` for authenticated user images. A cache permitted to store these responses can retain private material for a year, beyond thread deletion or device revocation. Use private caching semantics and an explicit retention policy. Existing native-client cache exposure was not demonstrated.

41. **Low — Deep links admit invalid identifiers without URL normalization.** `10f84a7:crates/editor-core/src/core/screen.rs:237` splits raw text and accepts empty host/thread components, query strings and fragments as identifier text. For example, `artisan://thread//id` becomes a navigable screen with an empty host. This violates **“Normalize messy input data into clean structures at the boundary.”** Parse the URL and validate each identifier before constructing a screen.

42. **Low — Pairing completion is inferred from an unrelated device-count change.** `10f84a7:crates/ae/src/forge.rs:70` records the device count after issuing the invitation and later treats any increase as success. Another invitation being consumed can produce false success; consuming this invitation before the baseline read, or re-pairing an existing certificate, can instead produce a timeout. Observe consumption of the specific invitation and its resulting device.

43. **Low — Expired pairing invitations are never collected.** `10f84a7:crates/forge-store/src/pairing.rs:147` inserts invitations, while the only deletion at line 83 removes successfully consumed, unexpired codes. Repeated abandoned pairing attempts leave an indefinitely growing table of expired authentication records. Delete expired invitations during creation or maintenance.

44. **Low — The MCP callback’s state-checking behavior has no direct regression coverage.** `10f84a7:crates/editor-core/src/core/mcp_sign_in.rs:90` implements callback-path, state, error and code handling, but the target tree contains no tests exercising this parser. A change accepting a mismatched state could pass the suite. Add behavioral cases for wrong/missing state, wrong paths, encoded parameters, denial and successful completion.

45. **Low — The PKCE test reproduces its own implementation.** `10f84a7:crates/forge-engines/src/mcp_oauth.rs:486` recalculates the expected challenge with the same hashing and encoding helpers used by `Pkce::new`. A shared encoding mistake passes both sides. This conflicts with **“Tests must fail for user-visible reasons.”** Use an independent published vector and exercise verifier/challenge interoperability.

46. **Low, suspected — Session-token erasure may be optimized away.** `10f84a7:crates/artisan-account/src/network.rs:49` uses ordinary `Vec::fill` immediately before deallocation. That does not establish compiler-resistant zeroization, despite the type’s stated guarantee. Optimized machine code was not inspected. Use a dedicated zeroization primitive whose erasure guarantees survive optimization.

47. **Low — Archive limits cover compressed downloads but not extracted contents.** `10f84a7:crates/forge-engines/src/install.rs:830` extracts every accepted regular member without bounding aggregate extracted bytes or entry count. A small, highly compressible published archive can exhaust disk space despite the download cap. Enforce extraction budgets and clean staging on rejection.

48. **Low — Version-pinned installation does not verify the manifest’s version matches the pin.** `10f84a7:crates/ae/src/release.rs:70` turns `--version` into a feed URL but discards the expected version. Resolution at line 162 accepts any signed manifest with a usable version string. A misplaced signed release can make an explicit version request install a different version. Preserve the requested version and compare it before downloading or executing the carrier.

49. **Low — Security-sensitive database access violates the typed-query rule.** `10f84a7:crates/forge-store/src/pairing.rs:89` and `crates/forge-store/src/settings.rs:295` use runtime SQL strings and positional row decoding through rusqlite. This violates **“Typed queries: SQLx in Rust.”** A schema/query mismatch in pairing or secret retrieval is discovered only at runtime. Move these operations to the required checked query mechanism.

50. **Nit — Authentication SQL is embedded in Rust.** `10f84a7:crates/forge-store/src/pairing.rs:83` and `crates/forge-store/src/settings.rs:310` embed pairing and secret-storage queries inline. This violates **“Queries live in separate `.sql` files.”** Scenario: cosmetic/rule adherence. Extract these queries into the established SQL-file hierarchy.

51. **Nit — Security tests live inside production source files.** `10f84a7:crates/forge-engines/src/mcp_oauth.rs:465`, `crates/forge-http/src/account_auth.rs:147` and `crates/artisan-account/src/signin.rs:108` embed test modules in `src/`. This violates **“Tests live in `tests/`, mirroring the path in `src/`.”** Scenario: cosmetic/rule adherence. Move the tests into the required mirrored locations.

52. **Nit — The reviewed security code contains extensive prohibited comments.** `10f84a7:crates/forge-engines/src/mcp_oauth.rs:1`, `crates/forge-http/src/routes.rs:146` and `crates/artisan-account/src/signin.rs:49` contain module explanations, inline commentary and an ownership note. This violates **“Never write code comments: no file headers, doc comments, inline explanations or TODOs.”** Scenario: cosmetic/rule adherence. Remove the comments and express necessary distinctions through names, types and structure.

53. **Nit — Browser callback HTTP parsing is duplicated.** `10f84a7:crates/editor-core/src/core/mcp_sign_in.rs:61` and `crates/artisan-account/src/browser.rs:223` independently implement bounded request-head reading, GET-target extraction and response handling, with different limits and parsing behavior. Scenario: cosmetic today; future boundary fixes can diverge. Share the bounded callback transport while keeping each authentication protocol’s state validation separate.

## 9. Delivery and operations

Reviewer: Oda (GPT 6.1 Sol).

### 9.1 Main review

| Category | Rating 0–3 | Justification at `10f84a7` |
|---|---:|---|
| Build system | 1 | Pinned inputs, but Nix excludes required embedded files: [nix/workspace.nix:25](nix/workspace.nix:25). |
| CI | 2 | Linux/Windows suites enforced; macOS tests only four crates: [ci.yml:127](.github/workflows/ci.yml:127). |
| Release engineering | 2 | Exact-SHA gating, version stamping and immutable nightlies; rolling publication remains unsafe: [release.yml:304](.github/workflows/release.yml:304). |
| Packaging and distribution | 1 | Install/uninstall implemented; interrupted upgrades can defeat subsequent updates: [install.rs:713](crates/ae/src/install.rs:713). |
| Environments | 2 | Deliberate pinned development shells; Windows development uses MinGW while releases use MSVC: [workspace.nix:116](nix/workspace.nix:116). |
| Configuration | 2 | Shared declarations, validation and migrations; persistence errors disappear at callers: [store.rs:462](crates/editor-core/src/core/store.rs:462). |
| Infrastructure as code | 2 | Declarative NixOS service; cache infrastructure remains externally configured: [forge-service.nix:82](nix/forge-service.nix:82). |
| Observability | 1 | Contextual local logs; incomplete retention and release crash diagnostics: [main.rs:178](crates/forge/src/main.rs:178). |
| Monitoring and alerting | 0 | Local health checking exists; no committed SLO/dashboard/alert configuration found: [controller.rs:37](crates/forge-http/src/upgrade/controller.rs:37). |
| Diagnostics | 1 | Status and diagnostic primitives exist; production turns disable raw traces: [run.rs:699](crates/forge-runs/src/run.rs:699). |
| Incident response | 0 | Recent-log recovery hints, but no committed runbooks/postmortems found: [service.rs:212](crates/ae/src/service.rs:212). |
| Telemetry | 0 | Local opt-in profiling, without product usage collection or consent infrastructure: [rebuild-stats.rs:71](crates/editor-core/src/core/diagnostics/rebuild_stats.rs:71). |
| Cost | 1 | Some local retention; immutable nightly releases accumulate without repository-defined pruning: [release.yml:282](.github/workflows/release.yml:282). |

Reviewed the pinned commit, without builds, tests or edits. The requested `gh` sample contained **8 successful CI runs, 16 failed CI runs, 4 cancellations and 2 failed releases**. Successful runs’ creation-to-update duration was **12.5–39.8 minutes; median 23.9 minutes**.

`s3` is an ignored local file, absent from the target commit. External R2 policies remain unverified. No exposed CI secret was verified. Workspace version `0.0.0` is stamped during release; that value alone is not a defect.

Findings, most severe first:

1. **High — Partial updates falsely become “up to date.”** [install.rs:713](crates/ae/src/install.rs:713). Installation replaces ae first, then Forge/Editor, but update eligibility checks only ae’s version. If Forge installation fails after ae succeeds, subsequent updates refuse to repair it. Compare every component and resume incomplete installation.

2. **High — Running Unix Forge cannot accept schema-changing updates.** [install.rs:536](crates/ae/src/install.rs:536), [lib.rs:220](crates/forge-store/src/lib.rs:220). Online replacement uses `Store::join`, which requires identical schemas; its error propagates before the stopped-install path. Every schema-changing release requires manual intervention. Detect incompatibility and provide a controlled migration/restart path.

3. **High — A diagnostic command can migrate a live database.** [status.rs:69](crates/ae/src/status.rs:69), [admin.rs:31](crates/forge-store/src/admin.rs:31). `ae status` opens `Admin`, which applies migrations. After a partial update, newer ae can change the database beneath older running Forge. Make diagnostic connections read-only; migrate only under explicit exclusive ownership.

4. **High — Nightly publication exposes mixed generations.** [release.yml:304](.github/workflows/release.yml:304). Public assets are individually clobbered while clients separately download manifest, signature and binaries. Concurrent updates fail verification; interrupted publication leaves the feed inconsistent. Publish immutable assets and switch a single channel pointer last. This violates the instruction to “make paths reliable by construction.”

5. **High — Bootstrap execution bypasses manifest authentication.** [install.sh:36](scripts/install.sh:36). The script trusts a checksum from an unverified manifest and executes the downloaded ae before Rust signature verification can help. A modified feed can replace both checksum and executable; HTTP feeds are accepted. Verify the signature against an embedded trust anchor before execution.

6. **High — Advertised Nix build/release paths contain deterministic missing inputs.** [workspace.nix:25](nix/workspace.nix:25) excludes `LICENSE.md`, unconditionally embedded by [install.rs:163](crates/ae/src/install.rs:163). Separately, [release.nix:91](nix/release.nix:91) references deleted `scripts/install.ps1`. Include required inputs, remove obsolete packaging assumptions, and validate actual derivations in CI.

7. **Medium — Binary rollback does not recover migrated data.** [schema.rs:547](crates/forge-store/src/schema.rs:547). Although installation supports choosing older versions, older Forge refuses newer database schemas. No corresponding backup/restore step exists in installation. A bad schema-changing release therefore cannot be reversed by reinstalling its predecessor. Provide tested compatibility or transactional backup/restore.

8. **Medium — Forge file logs remain unbounded during operation.** [main.rs:190](crates/forge/src/main.rs:190), [service.rs:187](crates/ae/src/service.rs:187). Rotation occurs only before ae’s detached launch; Forge itself appends indefinitely, including direct Windows launches. Long-running verbose instances can exhaust the database volume. Rotate inside the writer. Editor logs already have bounded, tested retention.

9. **Medium — Release downloads can hang indefinitely.** [release.rs:154](crates/ae/src/release.rs:154). The client sets neither request nor connection deadlines; body reads also await without progress limits. A server that accepts and stalls leaves installation stuck. Add connection, overall and stalled-transfer deadlines with cancellation.

10. **Medium — Released crashes lack usable symbolication artifacts.** [Cargo.toml:165](Cargo.toml:165). Releases abort on panic, disable debug information and strip symbols; release uploads contain packaged binaries without archived symbols. No application crash-report integration was found. User crashes cannot reliably produce actionable stacks. Retain matching symbols and add consent-based crash capture.

11. **Medium — Green CI excludes important supported behavior.** [ci.yml:96](.github/workflows/ci.yml:96), [ci.yml:127](.github/workflows/ci.yml:127). Default-feature tests omit qualification/capture scenarios; macOS omits ae and Forge runtime suites. Formatting and `tests/nix/dev.py` are also unenforced. Platform/process regressions can pass the release gate. Run the relevant suites and development-path checks.

12. **Medium — Exact cache hits preserve stale workspace artifacts.** [ci.yml:56](.github/workflows/ci.yml:56). `target/` shares a dependency-only immutable cache key. Source changes restore the original cache, but exact hits cannot save newly compiled workspace artifacts. Repeated runs rebuild those changes. Separate dependency caches from revision-keyed build artifacts and measure cache effectiveness.

13. **Medium — Settings calls acknowledge changes that failed.** [store.rs:467](crates/editor-core/src/core/store.rs:467), [agent-calls.rs:57](crates/editor-core/src/core/agent_calls.rs:57). Save failure rolls back memory and returns `()`, while the caller reports success. On disk-full/permission failure, agents receive false confirmation. Return and propagate a persistence result.

14. **Low — Trace initialization discards the actual failure cause.** [trace.rs:56](crates/forge-engines/src/trace.rs:56). Permission failures and disk exhaustion produce the same warning without the IO error. This breaks “Error logs carry the step, ids and cause.” Preserve the cause and report trace-worker startup failures too.

15. **Low — Explicit repository rules remain unenforced.** [install.rs:10](crates/ae/src/install.rs:10) contains prohibited documentation comments; [settings.rs:220](crates/forge-store/src/settings.rs:220) embeds SQL; [schema.rs:762](crates/forge-store/src/schema.rs:762) embeds tests. These violate “Never write code comments,” “Queries live in separate .sql files,” and “Tests live in tests/.” Restructure and enforce these conventions.

What perfect looks like:

| Category | Concrete changes needed for 3 |
|---|---|
| Build system | Repair Nix inputs; test clean release derivations; pin native release tools. |
| CI | Cover supported backend platforms and qualification; enforce formatting; maintain updating caches. |
| Release engineering | Atomic channel publication; interruption tests; exercised schema-aware rollback. |
| Packaging and distribution | Authenticate bootstrap; recover partial updates; test component failures and uninstall. |
| Environments | Exercise development commands and verify development/release toolchain parity. |
| Configuration | Propagate persistence errors; validate loaded settings; preserve historical migration fixtures. |
| Infrastructure as code | Version external storage/cache policies and validate service deployment. |
| Observability | Bound every log; preserve failure chains; archive symbols and capture crashes. |
| Monitoring and alerting | Define service/release health objectives and test actionable alerts. |
| Diagnostics | Add a redacted support bundle and explicitly enabled, bounded reproduction traces. |
| Incident response | Exercise failed-update, corruption and rollback procedures; retain incident records. |
| Telemetry | Decide required insight; enforce consent, minimization and retention if collecting. |
| Cost | Enforce artifact/cache retention and measure storage, egress and CI growth. |

Excluded reviewer suggestions: Nix toolchain drift is contradicted by its shared toolchain-file input; Windows ARM64 is explicitly experimental.

| Score | This review | Opus 5.5 | GPT 6.1 Sol |
|---|---:|---:|---:|
| Correctness | 4/10 | 5/10 | 4/10 |
| State handling | 3/10 | 4/10 | 4/10 |
| Code quality | 5/10 | 6/10 | 5/10 |
| Rule adherence | 4/10 | 6/10 | 4/10 |
| **Overall** | **4/10** | **4/10** | **4/10** |

### 9.2 Remaining findings and nits

16. **High — Legacy cleanup can delete Editor data it never migrated.** [crates/ae/src/legacy.rs:65](crates/ae/src/legacy.rs:65). Migration skips copying when the destination data directory exists, but subsequent cleanup deletes every old Editor data directory. A destination containing only logs can therefore cause settings, identity and pairings to be discarded. Delete sources only after verifying their successful migration.

17. **High — Interrupted legacy copies are mistaken for completed migrations.** [crates/ae/src/legacy.rs:239](crates/ae/src/legacy.rs:239). Cross-volume copies write directly into the final destination. After an interrupted copy, destination existence—or a copied `forge.sqlite3`—suppresses migration on the next attempt. Editor cleanup can then delete the complete source. Stage, verify and commit the migration before treating the destination as authoritative.

18. **High — Component migration removes programs outside the selected component.** [crates/ae/src/legacy.rs:70](crates/ae/src/legacy.rs:70). Every migration stops old programs and removes the complete old program list, including when only Forge or only Editor was requested. Installing Forge alone can remove the old Editor without replacing it. This violates “Do exactly what was asked.” Scope discovery, stopping and deletion to the selected apps.

19. **High — Legacy PID files can kill unrelated processes.** [crates/ae/src/legacy.rs:203](crates/ae/src/legacy.rs:203). Migration signals whatever PID an old file contains without checking executable identity or process start time. A stale PID reused by another application causes that application—and its children on Windows—to be terminated. Verify ownership before signalling, as the current service-management path already attempts.

20. **Medium — Concurrent installers delete each other’s staging files.** [crates/ae/src/install.rs:368](crates/ae/src/install.rs:368). Handoffs share `.installer`, and component installation shares `.incoming`; both are recursively removed before use. Two overlapping updates can delete another installer’s downloaded or unpacked executable. Use unique staging directories and serialize replacement across the installer handoff.

21. **Medium — Windows autostart bypasses lifecycle bookkeeping.** [crates/ae/src/service.rs:120](crates/ae/src/service.rs:120). The Run entry starts `forge.exe` directly, but Windows Forge does not write the PID file required by `running_pid()`. After sign-in, status can report Forge stopped, and stop/update operations cannot identify the running instance. Make autostart use the managed launcher or have Forge register its own process identity.

22. **Medium — Detached Unix Forge loses backend and replacement-gateway logs.** [crates/forge-http/src/upgrade/process.rs:55](crates/forge-http/src/upgrade/process.rs:55). Children inherit stderr without inheriting the parent’s `--log` setting. The detached launcher redirects stderr to null. Backend failures therefore disappear immediately, and replacement-gateway logs disappear after an update. Explicitly propagate a working log destination to every Forge process.

23. **Medium — The advertised follow-up purge cannot remove retained data.** [crates/ae/src/install.rs:747](crates/ae/src/install.rs:747). Purge targets require an existing binary. A normal uninstall removes the binaries, then tells users to run `ae uninstall --purge` later; that command subsequently selects nothing. Include retained data directories when resolving purge targets.

24. **Medium — Removing ae alone breaks the remaining Windows uninstallers.** [crates/ae/src/platform.rs:216](crates/ae/src/platform.rs:216). Every registered uninstall command invokes the installed ae, while ae can be removed independently. Removing “Artisan command line” first leaves Forge and Editor entries pointing to a missing executable. Retain an independent uninstaller while other components remain, or reject that removal order.

25. **Medium — “Keep” silently omits WSL Forge when native Forge exists.** [crates/ae/src/install.rs:99](crates/ae/src/install.rs:99). The native-install branch returns an empty distro list without discovering WSL installations. On a Windows machine with both installations, `ae update` updates native Forge and leaves WSL Forge behind. Resolve all installed placements instead of choosing one mutually exclusive location.

26. **Medium — WSL discovery ignores supported custom install locations.** [crates/ae/src/wsl.rs:56](crates/ae/src/wsl.rs:56). Discovery probes a hard-coded default path instead of following the shared location pointer. A Forge installed with `--location` inside WSL disappears from automatic forwarding, status and update discovery. Query WSL’s ae through the shared path resolver.

27. **Medium — Declining installation still changes the persistent install location.** [crates/ae/src/install.rs:244](crates/ae/src/install.rs:244). `relocate()` writes the root pointer before plan validation and confirmation. Answering “No,” cancelling a prompt or failing validation leaves subsequent commands redirected to the new location. Persist the location only when committing an accepted installation.

28. **Medium — Local builds retain an earlier release feed.** [crates/ae/src/install.rs:561](crates/ae/src/install.rs:561). A local source supplies no feed, but installation leaves an existing `release-feed` untouched. Installing a local build over a release can therefore make a later update unexpectedly replace it from the old channel. Persist an explicit source state and clear obsolete feed information.

29. **Medium — Damaged location pointers silently select another installation.** [crates/artisan-paths/src/lib.rs:112](crates/artisan-paths/src/lib.rs:112). Unreadable, malformed or relative pointers become `None`, causing fallback to the default root. The pointer is also overwritten non-atomically. A damaged pointer can make existing threads and settings appear missing or create a second installation. Distinguish absence from corruption and replace pointers atomically.

30. **Medium — Unreadable settings are overwritten by the next successful preference change.** [crates/editor-core/src/core/store.rs:146](crates/editor-core/src/core/store.rs:146). Loading errors substitute an empty settings map without preserving the original file or preventing writes. After malformed JSON, changing one preference replaces the entire original configuration. Preserve the unreadable file and expose a recoverable configuration error.

31. **Medium — Multiple Editor instances overwrite each other’s configuration changes.** [crates/editor-core/src/core/hosts.rs:99](crates/editor-core/src/core/hosts.rs:99). Each instance saves its complete in-memory snapshot without rereading or coordinating with other processes. Two clients changing different preferences can erase each other’s changes; hosts have the same persistence pattern. Serialize mutations against the latest file contents or use transactional shared storage.

32. **Medium — Concurrent first launches can persist a different identity from the one in use.** [crates/editor-core/src/core/hosts.rs:78](crates/editor-core/src/core/hosts.rs:78). Identity creation performs an unlocked read-then-write. Two instances can generate different identities, overwrite the same file and continue using different keys. Pairings established with the overwritten identity fail after restart. Make identity initialization exclusive and return the identity that won persistence.

33. **Medium — Older setting failures can roll back newer successful changes.** [crates/editor-settings/src/settings/preferences.rs:19](crates/editor-settings/src/settings/preferences.rs:19). Requests run concurrently, but rollback unconditionally restores the older request’s previous value. For example, a failed retention change to seven days can arrive after a successful change to fourteen days and restore ninety days locally. Sequence requests per setting and reject stale completions; test reordered successes and failures.

34. **Medium — Unauthenticated release responses can exhaust installer memory.** [crates/ae/src/release.rs:192](crates/ae/src/release.rs:192). Manifest, signature and binary responses accumulate without size limits in a `Vec`. A broken or hostile feed can stream an enormous manifest before authentication occurs. Bound metadata bodies and stream bounded binary downloads to temporary files while hashing.

35. **Medium — Nix release artifacts use an incompatible distribution format.** [nix/release.nix:44](nix/release.nix:44). This packaging path produces tarballs and ZIPs, while both bootstrap and Rust handoff paths execute downloaded artifacts directly. Even after its previously reported missing-input problems are repaired, its output remains incompatible with current installers. Produce standalone carriers or implement consistent archive extraction.

36. **Medium — Forge can be reported ready before its backend starts.** [crates/ae/src/service.rs:214](crates/ae/src/service.rs:214). Readiness means only that a TCP connection succeeds. The Unix gateway binds its public listener before spawning and checking its backend. Startup can therefore succeed while backend initialization is still pending or about to fail. Wait for authenticated application readiness instead of an open socket.

37. **Medium — Service startup has no deadline or cancellation path.** [crates/ae/src/service.rs:209](crates/ae/src/service.rs:209). The loop continues for as long as the process exists, even if startup is stalled. This can hang installation indefinitely; the setup window also refuses to close during installation. Add cancellable readiness waiting with progress and a bounded stall deadline.

38. **Medium — Windows updates immediately force-kill running engine trees.** [crates/ae/src/service.rs:260](crates/ae/src/service.rs:260). Stop always uses `taskkill /T /F`, giving Forge and engine children no opportunity to settle writes or shut down normally. Every update, restart or uninstall takes this path. Request graceful shutdown first and reserve forced termination for an expired deadline.

39. **Medium — Setup and top-level launch failures bypass persistent diagnostics.** [crates/editor-aqua/src/launch.rs:56](crates/editor-aqua/src/launch.rs:56). Setup runs before logging initialization and returns early for installer launches. Other launch errors propagate through `main()` without explicit file logging. With the Windows console suppressed, startup/setup failures can leave no useful diagnostic. Initialize diagnostics early and record the complete top-level failure chain.

40. **Medium — Failed Codex rollout retargeting is never resumed.** [crates/ae/src/legacy.rs:49](crates/ae/src/legacy.rs:49). Retargeting occurs after moving Forge’s data. If a state database is locked or unreadable, the install fails, but the next attempt skips retargeting because the destination Forge database now exists. Earlier Codex threads remain pointed at the old location. Make retargeting an independently resumable migration step.

41. **Medium — Symlink repair can destroy a link and still report successful migration.** [crates/ae/src/legacy.rs:262](crates/ae/src/legacy.rs:262). Repair removes each old link before creating its replacement, then discards both errors. Disk exhaustion during recreation leaves the link missing without failing migration. Create replacement links before swapping them into place and propagate repair failures.

42. **Medium — Legacy migration silently merges into non-empty destinations.** [crates/ae/src/legacy.rs:237](crates/ae/src/legacy.rs:237). Failed destination removal and rename fall through to recursive copying over existing files. A destination containing newer engine configuration but no Forge database can become a mixture of old and new state, after which the source is deleted. Reject conflicts or preserve the destination before migration.

43. **Medium — Publishing an older stable version moves the latest feed backwards.** [.github/workflows/release.yml:321](.github/workflows/release.yml:321). Every stable publication uses `--latest` without comparing versions. Publishing a maintenance release such as `1.9.1` after `2.0.0` redirects new installations and update discovery to the older line. Promote only the intended stable-channel head.

44. **Medium — WSL discovery and diagnostics can block indefinitely.** [crates/ae/src/wsl.rs:49](crates/ae/src/wsl.rs:49). Distribution probes synchronously start each distro without a timeout; status subsequently runs another unbounded command per discovered installation. One stalled distro blocks status and automatic update planning for every installation. Bound subprocess execution and preserve per-distro timeout results.

45. **Low — Cross-volume migration does not preserve symlinks.** [crates/ae/src/legacy.rs:317](crates/ae/src/legacy.rs:317). The copier distinguishes only directories from other entries. File symlinks are dereferenced, while directory symlinks are passed to `fs::copy` and fail. Engine homes containing linked skill directories cannot migrate reliably across volumes. Copy symlinks explicitly and retain their intended targets.

46. **Low — Loaded Editor settings bypass schema validation.** [crates/editor-core/src/core/hosts.rs:95](crates/editor-core/src/core/hosts.rs:95). Loading accepts arbitrary JSON values into an untyped map. A stored `"dark_mode":"false"` is returned literally to agents while the UI silently uses its boolean default. This violates “Normalize messy input data into clean structures at the boundary.” Validate and normalize individual settings during loading.

47. **Low — The shared setting declarations omit existing preferences.** [crates/editor-settings/src/settings/view.rs:229](crates/editor-settings/src/settings/view.rs:229). `streaming_mode` and `graphics_vsync` are editable in the UI but absent from `SETTINGS`. Agent setting tools therefore cannot discover or change them through the declared schema. Declare every supported preference and derive validation and discovery from that source.

48. **Low — Font validation accepts values the UI cannot apply.** [crates/artisan-api/src/settings.rs:230](crates/artisan-api/src/settings.rs:230). `font_set` is declared as unrestricted text although its supported values are `spline` and `system`. An agent can successfully save an arbitrary string while rendering silently falls back to a supported font. Model string choices explicitly and reject unsupported values.

49. **Low — Failed Forge preference saves have no visible error state.** [crates/editor-settings/src/settings/controller.rs:561](crates/editor-settings/src/settings/controller.rs:561). Failure produces only a warning and rollback. A user changing retention or another switch sees it revert without an explanation or recovery action. Preserve a visible error associated with the setting and clear it after a successful save.

50. **Low — Service address errors become plausible but incorrect defaults.** [crates/ae/src/service.rs:24](crates/ae/src/service.rs:24). Configuration and path errors are swallowed and replaced with loopback port 47900. Diagnostics can consequently report or probe the wrong address while hiding the actual configuration failure. Return the error and let callers represent address availability explicitly.

51. **Low — Database failures are reported as zero paired devices.** [crates/ae/src/status.rs:69](crates/ae/src/status.rs:69). Database-open and device-query errors become `None`, then zero, without adding a reported problem. A corrupt database appears to have no pairings. Represent the count as unavailable and include its failure cause.

52. **Low — Failed WSL diagnostics disappear from status output.** [crates/ae/src/status.rs:14](crates/ae/src/status.rs:14). Subprocess failures and malformed JSON are discarded through `filter_map`; exit status is not checked. A discovered but broken installation simply vanishes from both human and JSON reports. Keep a diagnostic result for every discovered distro, including failed steps and causes.

53. **Low — Editor logging initialization silently discards failures.** [crates/editor-aqua/src/logging.rs:12](crates/editor-aqua/src/logging.rs:12). File-opening errors become `None`, subscriber initialization errors are ignored, and fallback stderr is normally invisible on Windows. This violates “Error logs carry the step, ids and cause.” Preserve initialization failures and provide an actionable fallback diagnostic.

54. **Low — Open-timing diagnostics bypass the file logger.** [crates/editor-core/src/core/diagnostics/open_timing.rs:27](crates/editor-core/src/core/diagnostics/open_timing.rs:27). These diagnostics use `eprintln!` rather than tracing. Enabling `AQUA_OPEN_TIMING` in the normal Windows GUI launch therefore does not put timing evidence in `editor.log`. Route diagnostic events through the configured logging pipeline.

55. **Low — Gateway connection errors are discarded by incorrect nested-result handling.** [crates/forge-http/src/upgrade/controller.rs:278](crates/forge-http/src/upgrade/controller.rs:278). The `JoinSet` returns a task result containing an application result, but only the outer error is inspected. Backend connection failures and forwarding errors arrive as `Ok(Err(...))` and disappear. Handle both result layers and attach gateway/backend identifiers.

56. **Low — Atomic configuration replacement is not durable persistence.** [crates/editor-core/src/core/hosts.rs:127](crates/editor-core/src/core/hosts.rs:127). `flush()` is followed by rename without syncing file contents or the containing directory. A power loss can undo an acknowledged settings, hosts or identity write. Complete the durable write protocol before reporting persistence success.

57. **Low — User services retain the installer’s transient PATH.** [crates/ae/src/service.rs:86](crates/ae/src/service.rs:86). The complete installer environment is embedded in the systemd unit. Installing from a temporary Nix shell can leave references to tools subsequently garbage-collected; later PATH changes are also ignored. Give the service a deliberate, maintainable tool environment.

58. **Low — Offline uninstall leaves systemd autostart configuration behind.** [crates/ae/src/service.rs:133](crates/ae/src/service.rs:133). Unit removal depends on `uses_systemd()`, which requires a currently reachable user manager. Uninstalling while that manager is unavailable leaves the unit and enablement behind. Separate ownership detection and filesystem cleanup from live manager operations.

59. **Low — Uninstall deletes protocol registration without checking ownership.** [crates/ae/src/platform.rs:166](crates/ae/src/platform.rs:166). The complete `artisan` registry handler is removed whenever present. If another installation or application has subsequently taken ownership, uninstalling this copy breaks its links. Compare the registered command with this installation before deleting shared integration.

60. **Low — Installer staging leaks when launching a child fails.** [crates/ae/src/install.rs:358](crates/ae/src/install.rs:358). Cleanup occurs after fallible `.status()` calls, so launch errors return before deleting staged carriers. The Windows and WSL handoff paths repeat this pattern. Use owned temporary directories or scope guards covering every exit path.

61. **Low — Client-only development launches accumulate executable copies.** [nix/dev.sh:129](nix/dev.sh:129). Each launch creates a GUID-named temporary directory and copies the complete Editor executable, with no cleanup after exit or retention sweep. Repeated launches consume increasing disk space. Arrange cleanup after process termination and reclaim abandoned launch directories.

62. **Low — ae and Forge can report stale Git revisions in development builds.** [crates/ae/build.rs:6](crates/ae/build.rs:6), [crates/forge/build.rs:5](crates/forge/build.rs:5). Their build scripts watch the commit environment variable but not Git metadata used by the fallback. Rebuilding after another commit can retain an earlier `--version` revision. Track Git references as Editor’s build script does, or always supply the revision explicitly.

63. **Low — Configuration-path failures silently switch Editor to shared temporary storage.** [crates/editor-core/src/core/hosts.rs:63](crates/editor-core/src/core/hosts.rs:63). Failure to resolve the normal data root selects a fixed temporary directory. A broken environment therefore changes the location of identity, pairings and settings instead of reporting the cause. Return a configuration error or require an explicit temporary-data mode.

64. **Low — Unreadable local version files become fabricated versions.** [crates/ae/src/release.rs:332](crates/ae/src/release.rs:332). Every read error becomes `0.0.0-local`, including permission and other filesystem failures. Installation can succeed with misleading version metadata. Default only when the file is absent and propagate other failures with the path.

65. **Low — Remembered release feeds are not validated at the boundary.** [crates/ae/src/install.rs:407](crates/ae/src/install.rs:407). Empty or malformed file contents are passed to general source parsing, where they become local paths or unrelated resolution errors. This violates “Normalize messy input data into clean structures at the boundary.” Decode a typed feed value and report corrupt persisted configuration directly.

66. **Low — Unsupported placement combinations are silently accepted.** [crates/ae/src/install.rs:93](crates/ae/src/install.rs:93). The shared parser accepts values such as `--editor wsl:Ubuntu` and `--forge windows`, but planning silently translates them into no installation. A mistyped request can complete successfully while omitting the requested component. Use component-specific placement types or reject unsupported combinations.

67. **Low — The version rollback option cannot resolve published nightlies.** [crates/ae/src/release.rs:70](crates/ae/src/release.rs:70). `--version` always constructs a `v<version>` release tag, whereas nightly publication uses `nightly-<SHA>`. Supplying an installed nightly’s displayed version therefore targets a nonexistent release. Resolve nightly versions through their immutable commit tags.

68. **Low — Windows self-uninstall never completes directory and location cleanup.** [crates/ae/src/install.rs:842](crates/ae/src/install.rs:842). Executable deletion is deferred, but directory removal and `forget_root()` are attempted immediately and fail while ae still exists. No later cleanup runs. Move final directory and location cleanup into the deferred removal operation.

69. **Low — Trace truncation and dropped records are indistinguishable from complete traces.** [crates/forge-engines/src/trace.rs:84](crates/forge-engines/src/trace.rs:84). Oversized records, exhausted budgets and full queues silently discard events. A diagnostic replay can consequently show apparent protocol omissions without revealing that capture lost evidence. Record dropped-event counts and explicit truncation markers while retaining bounded capture.

70. **Low — Release verification tests do not exercise invalid signatures or signed-manifest tampering.** [tests/release.py:55](tests/release.py:55). The test verifies a valid signature, but its corruption cases alter artifacts or supplied metadata and fail before cryptographic verification. A regression in the Python verifier’s rejection path can escape. Add negative tests modifying signature bytes and signed manifest bytes.

71. **Low — File removal treats metadata failures as successful absence.** [crates/ae/src/sys.rs:110](crates/ae/src/sys.rs:110). Every `symlink_metadata()` error returns success, including access failures. Cleanup callers can therefore claim removal without inspecting or deleting the path. Ignore only `NotFound`; propagate other errors with their operation and path.

72. **Low — Registry failures lose the file and generation needed to diagnose them.** [crates/forge-runs/src/runtime/registry.rs:58](crates/forge-runs/src/runtime/registry.rs:58). Reads return raw IO or JSON errors without path context, and maintenance logs only a generic wrapper. With several registry files, a parse error does not identify the damaged state. This violates “Error logs carry the step, ids and cause.” Add path context and gateway/generation identifiers.

73. **Low — Migration queries do not follow the required typed-query approach.** [crates/ae/src/legacy.rs:294](crates/ae/src/legacy.rs:294). The Codex retarget query is checked only when executed through rusqlite, contrary to “Typed queries: SQLx in Rust.” A schema mismatch is discovered after the data move. Use the required typed-query approach and validate supported external schema versions before committing migration.

74. **Nit — Windows PATH normalization is inconsistent.** [crates/ae/src/platform.rs:25](crates/ae/src/platform.rs:25). Installation compares raw strings while removal trims trailing separators. An existing equivalent `...\ae\` entry survives deduplication and receives another entry. Cosmetic; normalize entries once and use the same comparison for installation and removal.

75. **Nit — Process-close deadlines are duplicated across implementations.** [crates/ae/src/platform.rs:280](crates/ae/src/platform.rs:280). The ten-second grace period is embedded separately in PowerShell and Rust, while installation also contains an unnamed `0..100` replacement-name bound. Cosmetic; give operational limits descriptive names and share values across platform implementations.

76. **Nit — Workspace repository metadata points at a different repository.** [Cargo.toml:44](Cargo.toml:44). Package metadata names `sandersonstabo/artisan-editor`, while release tooling uses `artisanstreet/editor`. Cosmetic; update inherited repository metadata to the canonical project location.

## 10. People, process, business and legal

Reviewer: Celine (GPT 6.1 Sol).

### 10.1 Main review

| Category | Rating 0–3 | Justification with evidence |
|---|---:|---|
| Developer experience | 2 | Deliberate Nix tooling and command help; onboarding and an obsolete release path remain problematic (`nix/dev.sh:15`, `nix/release.nix:91`). |
| Documentation | 1 | Detailed agent-facing skill documentation, but no contributor setup or general user guide found (`skills/artisan-editor/SKILL.md:20`, `README.md:25`). |
| Project management | 1 | Issues are welcomed; no committed roadmap, triage policy or definition of done found (`README.md:26`). Requested issue query returned no open issues. |
| Collaboration | 2 | Extensive PR practice and descriptive commits, but direct master commits bypass that practice (`.github/workflows/ci.yml:3`, `README.md:25`; history). |
| Knowledge continuity | 1 | Provenance records exist, but important contracts reference unavailable paths and informal ownership (`crates/artisan-account/src/network.rs:3`). History overwhelmingly has one author. |
| Support and feedback | 1 | Issue intake is advertised; the app’s feedback action does nothing (`crates/editor-ui/src/ui/chrome/menu.rs:118`). |
| Licensing | 1 | Root licence, workspace SPDX and Aqua agree on BSD-3-Clause; vendored MIT notices exist, but font clearance and dependency attribution are incomplete (`Cargo.toml:43`, `crates/editor-aqua/build.rs:35`). |
| Legal and compliance | 1 | Sign-in exposes terms/privacy links; repository evidence does not establish policy contents or completed asset clearance (`crates/editor-aqua/src/frame/sign_in/card.rs:653`). |
| Brand | 2 | Company, application, toolkit and server names generally have coherent roles; obsolete “AST” copy remains (`crates/editor-settings/src/settings/page.rs:49`). |
| Discoverability | 1 | Account/download links and a GitHub homepage exist; repository description is empty (`README.md:17`). External SEO/store presence was not verified. |
| Monetization | 1 | “Upgrade to Go” is presented, but has no action; no billing/entitlement flow found in the inspected account/API/UI paths (`crates/editor-ui/src/ui/chrome/account.rs:334`). |
| Sustainability | 1 | BSD licensing and automated distribution support continuity; no funding, maintenance commitment or end-of-life plan found (`LICENSE.md:6`, `.github/workflows/release.yml:5`). |
| Trust | 1 | Build identification and legal links exist, but platform claims and dead commercial/support controls undermine accuracy (`README.md:3`, `crates/editor-ui/src/ui/chrome/menu.rs:118`). |

Reviewed committed state **`10f84a7`**, despite the checkout advancing. Read the requested 200 commits and 50 PRs. No builds, tests, edits or writes were performed. External policy contents and privately held font permissions remain unverified.

**Findings, most severe first**

1. **High — Evaluation-only fonts enter release binaries.**  
   [fonts.rs:96](crates/editor-assets/src/fonts.rs:96) unconditionally embeds both Founders Grotesk test fonts. [Their notice:3](crates/editor-assets/licenses/founders-grotesk-test.txt:3) explicitly says it grants no redistribution or production rights. Release builds use this catalog. **Scenario:** publishing the normal Windows build distributes evaluation fonts without recorded production clearance. **Fix:** replace them or record appropriate embedding rights and enforce release eligibility.

2. **High — Sigurd ships despite an explicit unresolved release prerequisite.**  
   [fonts.rs:55](crates/editor-assets/src/fonts.rs:55) embeds Sigurd; [sigurd-NOTES.md:8](crates/editor-assets/licenses/sigurd-NOTES.md:8) says a desktop embedding licence is required before release. **Scenario:** nightly publishing proceeds without satisfying the repository’s stated prerequisite. **Fix:** obtain and record clearance or substitute a cleared face. Actual ownership of a suitable licence is **unverified**.

3. **High — Shipped notices omit runtime dependency attribution.**  
   [build.rs:39](crates/editor-aqua/build.rs:39) collects selected asset/vendor notices, not the dependency graph. Tokio is a runtime dependency ([editor-core/Cargo.toml:19](crates/editor-core/Cargo.toml:19)); its checked local licence requires its copyright/permission notice, absent from these inputs. The CLI installer writes only the root licence at [install.rs:557](crates/ae/src/install.rs:557). **Scenario:** Editor/Forge/CLI distributions carry incomplete notices. **Fix:** generate complete, platform-specific notices from locked dependencies and verify every distribution includes them.

4. **Medium — “Help & Feedback” silently dismisses itself.**  
   [menu.rs:118](crates/editor-ui/src/ui/chrome/menu.rs:118) closes the menu and returns no effect. The visible entry is wired at [account.rs:195](crates/editor-ui/src/ui/chrome/account.rs:195). **Scenario:** a user encountering a bug clicks the advertised support route and gets nothing. **Fix:** open documented help/issue intake and test the resulting navigation. Existing tests explicitly expect the no-op.

5. **Medium — “Upgrade to Go” has no commercial destination.**  
   [account.rs:334](crates/editor-ui/src/ui/chrome/account.rs:334) renders an active upgrade button, consumed by the same no-op branch. **Scenario:** a user attempting to purchase cannot discover pricing or proceed. **Fix:** connect a real plan/billing destination with clear entitlement behavior, or remove the unsupported offer.

6. **Medium — Public platform claims exceed delivered Editor support.**  
   [README.md:3](README.md:3) advertises a native Windows, Mac and Linux app. [release.py:14](src/release.py:14) only expects Windows Editor artifacts; [dev.sh:109](nix/dev.sh:109) explicitly requires Windows. **Scenario:** Mac/Linux users sign up expecting the advertised desktop application. **Fix:** state current Editor availability separately from Forge/CLI support and planned backends.

7. **Medium — Released-product PR discipline is bypassable and bypassed.**  
   [.github/workflows/ci.yml:3](.github/workflows/ci.yml:3) runs CI after master pushes, but GitHub reports **“Branch not protected”** and no repository rulesets. First-parent history contains direct commits including `4200e7738`, modifying [README.md:25](README.md:25). This breaks **“Released products: work in PRs.”** **Scenario:** changes reach master without a PR or successful pre-merge checks. **Fix:** require scoped PRs and CI through branch rules; define review ownership.

8. **Medium — Contributor onboarding does not explain the real development workflow.**  
   [README.md:25](README.md:25) only explains contribution eligibility. The useful instructions live inside [dev.sh:15](nix/dev.sh:15); its default command installs into the user’s environment with `--yes` at line 186. **Scenario:** new staff cannot distinguish building, launching a client and replacing their installed Forge. **Fix:** provide maintained setup, prerequisites, command semantics and data-isolation instructions. The deficiency is missing content, **not README count**.

9. **Medium — An exposed release-maintenance route references a deleted installer.**  
   [scripts/release:47](scripts/release:47) invokes `nix build .#release`; [release.nix:91](nix/release.nix:91) references `scripts/install.ps1`, absent from the pinned tree. **Scenario:** a maintainer follows the supplied release script and cannot complete packaging. **Fix:** retire that route or reconcile it with the current release pipeline. Verified statically; execution was prohibited.

10. **Medium — Distributed artwork notes contradict the actual licence.**  
    [artisan-first-party-NOTES.md:38](crates/editor-assets/licenses/artisan-first-party-NOTES.md:38) claims `LICENSE.md` contains “Artisan Street Terms (AST)”; it actually contains BSD-3-Clause. These notes enter the notice bundle. **Scenario:** downstream users receive conflicting licensing statements. **Fix:** reconcile the notice with the BSD relicensing and explicitly identify third-party exceptions.

11. **Low — Obsolete identity and repository metadata remain.**  
    [page.rs:49](crates/editor-settings/src/settings/page.rs:49) calls it an “AST account”; [Cargo.toml:44](Cargo.toml:44) identifies the old personal repository. **Scenario:** users encounter an unexplained account name and tooling directs contributors toward obsolete metadata. **Fix:** use the current account name and canonical repository consistently.

**What perfect looks like — changes required for rating 3**

| Category | Concrete changes |
|---|---|
| Developer experience | Verified clean-machine setup; explicit build/client/install workflows; one maintained release route. |
| Documentation | Owned user/contributor guides and contract references, with executable examples checked in CI. |
| Project management | Prioritized roadmap, triage ownership and enforced completion criteria. |
| Collaboration | Required PR/check rules, ownership assignments and reviewable logical changes. |
| Knowledge continuity | Durable decisions, accessible contracts, backup maintainers and exercised handover procedures. |
| Support and feedback | Working intake, reproducible diagnostic attachments and tracked responses. |
| Licensing | Complete dependency notices, recorded asset permissions and release-blocking clearance checks. |
| Legal and compliance | Reviewed product-specific policies, data-rights procedures and recorded applicability decisions. |
| Brand | Canonical terminology/metadata with checks for obsolete public identifiers. |
| Discoverability | Owned landing/store metadata, accurate platform availability and automated link checks. |
| Monetization | Real pricing/billing/entitlements with tested purchase, cancellation and refund paths. |
| Sustainability | Explicit funding/support commitments, backup ownership and an exercised sunset/export plan. |
| Trust | Accurate capability/status claims, transparent data-use explanations and functional advertised actions. |

| Score — this slice only | /10 |
|---|---:|
| Correctness | 5 |
| State handling | 4 |
| Code quality | 5 |
| Rule adherence | 5 |
| **Overall** | **4** |

### 10.2 Remaining findings and nits

12. **Medium — Nix source filtering excludes a required licence file.** `nix/workspace.nix:25` at `10f84a7` omits `LICENSE.md` from the filtered source. `ae` requires it through `include_str!` at `crates/ae/src/install.rs:163`; the Editor’s build script also reads it. **Scenario:** building the Nix `ae` or Windows Editor package fails because the source input lacks the file. **Fix:** include the root licence in the source filter. Verified statically; no build was run.

13. **Medium — Concurrent development installs share destructive staging state.** `nix/dev.sh:93` assigns every invocation the same staging directory, which line 162 recursively deletes. **Scenario:** two developers or agents run the command concurrently; one deletes or overwrites files while the other installer reads them. **Fix:** allocate a staging directory per invocation and clean it up afterward.

14. **Medium — Brand-asset notes promise review flags that no longer exist.** `crates/editor-assets/licenses/svgl-brand-marks-NOTES.md:31` says every relevant entry has `needs_review = true`. The current `Asset` structure at `crates/editor-assets/src/lib.rs:154` and catalog contain no such field, and notice generation simply includes the assets. **Scenario:** a release proceeds without exposing the outstanding brand-clearance decisions described by the shipped notes. **Fix:** restore explicit clearance records and a release check, or revise the notes to accurately describe the process. Actual brand permissions remain unverified.

15. **Medium — The credential-sharing promise exceeds the native sync contract.** `README.md:7` promises shared credentials across enrolled machines. `crates/forge-http/src/sync.rs:77` accepts only MCP definitions, skills and a global prompt; MCP definitions omit credentials, and line 381 explicitly preserves local environment variables and headers. **Scenario:** a user enrolls another machine expecting authenticated MCP services to work, but its required credentials are absent. **Fix:** describe the implemented sharing boundaries accurately or implement the promised credential transfer.

16. **Medium — Following the documented callout envelope breaks rendering.** `skills/artisan-editor/SKILL.md:63` and `skills/artisan-editor/references/blocks.md:5` advertise `subtitle` and `caption` for every kind. `crates/editor-ui/src/ui/markdown/presenter.rs:1957` rejects callouts containing either field. **Scenario:** an agent follows the reference and produces a callout that the presenter refuses to draw. **Fix:** support these fields or document the exception, with a rendering regression covering the published contract.

17. **Medium, suspected — Artisan Neo source redistribution lacks a resolved attribution record.** `crates/editor-assets/fonts/FONTS.md:30` explicitly leaves the design’s upstream attribution unresolved; the accompanying licence names the Inter Project Authors. `artisan-neo-600.ttf` remains in the committed source tree, although it is absent from the active bundled-font catalog. **Scenario:** someone redistributes the repository assuming the supplied notice establishes the font’s provenance. **Fix:** verify and record the font’s origin and applicable licence, or remove it. Whether the existing attribution is legally sufficient was not verified.

18. **Low — Incremental development builds can report an old commit.** `crates/forge/build.rs:5` and `crates/ae/build.rs:6` watch environment variables but obtain their fallback commit from Git without watching Git metadata. Under [Cargo’s change-detection rules](https://doc.rust-lang.org/cargo/reference/build-scripts.html#change-detection), advancing HEAD alone need not rerun these scripts. **Scenario:** a locally rebuilt binary reports the previous checkout’s commit, misleading a bug investigation. **Fix:** track HEAD and its resolved reference, as the Editor build script already does, or consistently supply the commit from the development runner.

19. **Low — Signed-in users lose access to the legal-policy links.** `crates/editor-settings/src/settings/account_view.rs:351` provides refresh and sign-out controls but no account-management or policy destination. The terms/privacy links exist only on the sign-in card at `crates/editor-aqua/src/frame/sign_in/card.rs:653`. **Scenario:** a signed-in user wants to revisit the applicable policies and must discover the URLs independently or sign out. **Fix:** retain policy links in an accessible signed-in surface.

20. **Low — Installed third-party notices have no in-app discovery route.** `crates/editor-settings/src/settings/view.rs:401` renders only build-information rows on About. Notices are written to an installation-directory file by `crates/editor-aqua/src/setup/payload.rs:45`, but no settings action opens or identifies that file. **Scenario:** a user looking for third-party acknowledgements cannot find them through the application. **Fix:** expose an About action that opens the installed notices.

21. **Low — The account identity advertises navigation but performs none.** `crates/editor-ui/src/ui/chrome/account.rs:238` renders a chevron, pointer cursor and activatable menu item; `crates/editor-ui/src/ui/chrome/menu.rs:112` returns no effect for its activation. **Scenario:** clicking the account header appears to offer profile navigation but leaves the user in place. **Fix:** navigate to account settings or render a noninteractive identity header.

22. **Low — Development-client launches permanently accumulate executable copies.** `nix/dev.sh:129` creates a new GUID-named Windows temporary directory and copies the Editor into it. The command exits without arranging cleanup after the client closes. **Scenario:** repeated edit-run cycles leave many large executable copies in the user’s temporary directory. **Fix:** arrange cleanup after process exit or prune completed development-client directories.

23. **Low — Mermaid vendoring lacks enough provenance to reproduce the fork.** `vendor/mermaid-rs-renderer/VENDORED.txt:1` records a version and removed dependencies, but no original archive checksum, immutable source revision or reproducible patch inventory. **Scenario:** a maintainer upgrading the renderer cannot reliably distinguish upstream code from local alterations. **Fix:** record the immutable upstream input and local modifications, following the existing highlighting provenance approach.

24. **Low — The rich-code reference documents an ignored field.** `skills/artisan-editor/references/blocks.md:53` and line 62 advertise `language` for code references and annotated code. Their deserialization structures at `crates/editor-ui/src/ui/rich/code/code_ref.rs:9` and `crates/editor-ui/src/ui/rich/code/annotated.rs:19` do not retain it. **Scenario:** an agent supplies a language expecting language-dependent presentation, but the field silently has no effect. **Fix:** implement the documented behavior or remove the unsupported field from the reference.

25. **Low — Font-maintenance instructions describe obsolete rendering APIs.** `crates/editor-assets/fonts/FONTS.md:40` describes the active backend as WGPU/Cosmic Text, while `crates/editor-assets/src/fonts.rs:109` documents registration through `gpui::TextSystem::add_fonts`. The actual registration at `crates/editor-ui/src/ui/fonts.rs:3` uses Aqua’s `SharedFonts`. **Scenario:** a contributor investigates font matching using the wrong runtime and API. **Fix:** update the maintained font instructions to match Aqua and remove obsolete API documentation.

26. **Low — The account persistence test bypasses the persistence implementation.** `crates/ae/src/account.rs:495` manually serializes, writes, reads and deserializes a fixture instead of invoking `save_account` or `load_account`. **Scenario:** the real helpers stop writing the correct location or preserving required fields, while this test remains green. It also conflicts with **“Only test non-trivial behavior”** by largely exercising dependency round-tripping. **Fix:** test the actual persistence boundary through an injectable location, or remove the redundant round-trip test.

27. **Low — Account tests remain embedded in production-source files.** `crates/artisan-account/src/network.rs:635`, `crates/artisan-account/src/enrollment.rs:118` and `crates/ae/src/account.rs:484` contain inline test modules. This violates **“Tests live in `tests/`, mirroring the path in `src/`.”** **Scenario:** maintaining account coverage requires navigating a different layout from the prescribed test hierarchy. **Fix:** move the tests into mirrored test files, retaining private access through an explicit module path where necessary.

28. **Nit — The same sign-in schema cases are tested twice.** `crates/artisan-account/src/network.rs:639` repeats the `MagicRequest` and `MagicStatusRequest` cases in `crates/artisan-account/src/signin.rs:114`; forward-compatible response cases are duplicated too. **Scenario:** cosmetic maintenance duplication; the same contract change requires updating multiple equivalent fixtures. **Fix:** retain one authoritative set and use network tests for transport behavior.

29. **Nit — Asset provenance scopes reference removed directories and stale inventories.** `crates/editor-assets/licenses/svgl-brand-marks-NOTES.md:3` identifies `modules/assets/...` and twelve SVGL entries; the current directory is under `crates/editor-assets/` and contains ten. `crates/editor-assets/licenses/simple-icons-NOTES.md:3` uses the same obsolete root. **Scenario:** cosmetic documentation drift that complicates mapping notices to distributed assets. **Fix:** distinguish historical extraction inventories from current scopes and name the current paths.

30. **Nit — The font document repeats its evaluation section.** `crates/editor-assets/fonts/FONTS.md:69` repeats the “Founders Grotesk header evaluation” heading from line 61, with overlapping descriptions. **Scenario:** cosmetic; readers must reconcile two adjacent accounts of the same font set. **Fix:** consolidate them into one maintained section.

31. **Nit — The bundled skill repeats navigation instructions.** `skills/artisan-editor/SKILL.md:160` repeats the `navigate_user` instruction already stated at line 149; the neighboring sign-in guidance also substantially overlaps. **Scenario:** cosmetic duplication that consumes agent context and invites future divergence. **Fix:** consolidate each instruction into one authoritative paragraph.

32. **Nit — Development-script branches repeat conditions already established by their enclosing branch.** `nix/dev.sh:150` checks `want_editor` inside a branch that already requires it; line 172 repeats the same pattern. **Scenario:** cosmetic dead conditional structure. **Fix:** flatten these branches.

33. **Nit — The public licence name differs from the repository’s precise identifier.** `README.md:30` labels the licence “BSD-3”, while `Cargo.toml:43` uses `BSD-3-Clause`. **Scenario:** cosmetic terminology inconsistency. **Fix:** use `BSD-3-Clause` consistently.

34. **Nit — Contribution copy repeats “issues” unnecessarily.** `README.md:26` invites “issues for bugs and issues, or feature requests.” **Scenario:** cosmetic. **Fix:** simplify it to bugs and feature requests.

35. **Nit — First-party code still contains prohibited explanatory comments.** `crates/editor-assets/src/fonts.rs:15` contains documentation comments throughout its declarations, and `scripts/release:2` begins an explanatory comment block. These exceed the exceptions in **“Never write code comments: no file headers, doc comments, inline explanations or TODOs.”** **Scenario:** cosmetic rule noncompliance. **Fix:** remove explanatory code comments, preserving licence headers and tool directives; put necessary operational instructions in an appropriate existing documentation surface.

36. **Nit — Mermaid has two conflicting dependency declarations.** `Cargo.toml:96` declares a registry workspace dependency, while its actual consumer at `crates/editor-ui/Cargo.toml:38` directly selects the vendored path. **Scenario:** cosmetic configuration duplication that makes dependency and licence audits harder to follow. **Fix:** define the selected vendored dependency once and inherit it from the workspace.

## 11. Dead code and assets

Reviewer: dead-code crawl (GPT 6.1 Sol).

### 11.1 Main review

Audited **all 1,620 tracked files at HEAD `10f84a7`**, using committed blobs rather than the shared working tree. No files changed; no builds ran.

These are conservative static findings. Name searches prove the listed unused identifiers; they cannot rule out additional dead methods whose names collide with live methods.

| Category | Findings | Bytes |
|---|---:|---:|
| Fonts, icons, fixture candidates | 43 files | 660,452 |
| Rust declarations without callers | 97 items | 1,451,926 in containing files¹ |
| Unused dependency declarations | 9 | 576 declaration bytes |
| Dormant features/import code | 4 features; 8 associated files | 98,442 + partial-file branches |
| Repository/tooling candidates | 9 files | 52,633 |

¹ **Not reclaimable bytes:** these source files also contain live code.

Evidence searches below mean `git grep … 10f84a7 -- …`, supplemented by token searches over every committed text blob. Sizes are committed blob bytes.

**Assets**

Under `crates/editor-assets/`:

| Path | Bytes | Confidence and evidence |
|---|---:|---|
| `fonts/artisan-neo-600.ttf` | 343,516 | **DEAD**. Filename appears only in provenance documentation; no embedding or family selection. |
| `fonts/cal-sans-700.ttf` | 219,644 | **DEAD**. Same: documentation only, no embedding or selection. |
| `fonts/test-founders-grotesk-semibold.otf` | 14,320 | **DEAD**. Embedded at `src/fonts.rs:103`; no UI selection of `Test Founders Grotesk`. |
| `fonts/test-founders-grotesk-bold.otf` | 13,928 | **DEAD**. Embedded at `src/fonts.rs:96`; family search outside this crate returns nothing. |
| `licenses/cal-sans-OFL.txt` | 4,381 | **LIKELY**. Notice survives its unused font. |
| `licenses/artisan-neo-OFL.txt` | 4,380 | **LIKELY**. Same. |
| `licenses/founders-grotesk-test.txt` | 402 | **LIKELY**. Referenced only by the unused font catalog entry and automatic notices collection. |

Founders font last touched: `a290f3168`, **2026-10-13**, “reform: replace the codebase with the radical reform”. Its continued registration does not establish a UI consumer.

The following **34 SVGs, 37,056 bytes, are DEAD** under `crates/editor-assets/svg/`. For each catalog row in `src/lib.rs`, searches for **both its constant and string ID outside that file return zero**. None has another filename consumer. Entries below are `filename(bytes)`:

- `artisan/`: `app-icon.svg(10821)`, `logo-gradient.svg(4249)`, `star.svg(726)`, `success-check.svg(216)`.
- `jetbrains/`: `ts-test.svg(2297)`, `typescript.svg(2147)`, `svelte.svg(1891)`, `text.svg(497)`.
- `jetbrains-2023/`: `protobuf.svg(427)`.
- `svgl/`: `gemini.svg(3680)`.
- `lobe/`: `xiaomi.svg(2536)`.
- `tabler/`: `palette.svg(514)`, `bug.svg(480)`, `chart-column.svg(451)`, `star-filled.svg(409)`, `arrows-minimize.svg(395)`, `shopping-bag.svg(382)`, `message-plus.svg(374)`, `test-pipe.svg(368)`, `edit.svg(367)`, `checklist.svg(342)`, `maximize.svg(341)`, `login.svg(337)`, `star.svg(321)`, `circle-x.svg(273)`, `code.svg(269)`, `arrow-narrow-left.svg(260)`, `player-track-prev.svg(255)`, `player-track-next.svg(254)`, `brand-visual-studio.svg(251)`, `corner-down-left.svg(244)`, `rotate-clockwise.svg(237)`, `player-play.svg(223)`, `loader-2.svg(222)`.

Under `crates/editor-aqua/tests/qualification/fixtures/`:

- `capture-light.json` — **14,523 bytes, LIKELY**. No filename references or configured screen selection. Last touched `8767c0cc4`, **2026-11-06**, “Import Aqua Editor port with pinned runtime dependencies”.
- `capture-workflows.json` — **8,302 bytes, LIKELY**. Same. Last touched `a6712f8e0`, **2026-11-06**, “Qualify native Windows workflows with isolated profiles and physical UIA input”.

Both remain manually loadable through the fixture environment variable; that prevents a stronger classification.

**Rust**

All entries in the following list are **DEAD declarations**: their exact identifier has no occurrence beyond its definition, except `launch_env`, which additionally appears in documentation. These are unused APIs, constants, wrappers and qualification hooks—not whole-file deletion recommendations.

Paths below are relative to `crates/`; `name@line` identifies each item. Parentheses give the containing file’s size.

| Source file | Uncalled declarations |
|---|---|
| `artisan-account/src/network.rs` (22631) | `magic_consume@331`, `list_sessions@430`, `totp_enroll@464`, `totp_verify@477`, `totp_disable@495`, `list_machines@529`, `latest_bundle_head@585` |
| `artisan-api/src/engine.rs` (15441) | `option_str@104` |
| `artisan-api/src/tunnel.rs` (7585) | `STREAM_START_KEY@209`, `EVENTS_PATH@227` |
| `editor-aqua/src/frame/sidebar.rs` (10525) | `lay_out@239` |
| `editor-aqua/src/qualification/bootstrap.rs` (3424) | `core_snapshot_ready@69` |
| `editor-aqua/src/qualification/fixture.rs` (33860) | `InitialPayload@60`, `initial_items@75`, `initial_content_ready@107` |
| `editor-aqua/src/shell/host.rs` (16135) | `close_window@350` |
| `editor-aqua/src/shell/layout.rs` (4071) | `entry_layout@70` |
| `editor-aqua/src/shell/resource_preview.rs` (4198) | `visible_line@35` |
| `editor-aqua/src/shell/titlebar.rs` (22622) | `thread_header@250` |
| `editor-aqua/src/shell/workspace.rs` (66919) | `inspector_elements@635`, `acknowledge_painted@1708` |
| `editor-assets/src/fonts.rs` (5468) | `lookup_family@126` |
| `editor-assets/src/lib.rs` (56020) | generated `CONSTANTS@199` |
| `editor-composer/src/composer/controller.rs` (37265) | `remembered_harness_config@1025` |
| `editor-composer/src/composer/goal_strip.rs` (7951) | `set_parked_time@217` |
| `editor-composer/src/composer/view.rs` (137014) | `open_animated_popup@1459`, `open_project_popup@1622`, `refresh_model_popup@1789` |
| `editor-conversation/src/conversation/adapter.rs` (14128) | `changed_disclosures@299`, `composer_goal@362`, `restore_anchor@404` |
| `editor-conversation/src/conversation/admission.rs` (6153) | `speculative_frontier@16`, `revealed_rows@65`, `changed_from@70`, `sync_frontier@75`, `publish_frontier@95` |
| `editor-conversation/src/conversation/controller.rs` (54712) | `claim_prose_arrival@784` |
| `editor-conversation/src/conversation/presentation.rs` (32945) | `prepare_collection_row@153`, `rendered_current@473` |
| `editor-conversation/src/conversation/scroll.rs` (6304) | `row_destination@134` |
| `editor-conversation/src/conversation/view/factory.rs` (106423) | `prepare_prose@1130`, `with_body_versions@1517`, `sync_reveal_geometry@1630` |
| `editor-conversation/src/conversation/view/mod.rs` (13377) | `observed_row_frame@319`, `thread_element@346`, `try_thread_element@373`, `batched_thread_element@400` |
| `editor-conversation/src/conversation/view/resources.rs` (8987) | `from_prepared@105`, `stored_source@230` |
| `editor-conversation/src/conversation/view/transcript.rs` (31788) | `prefetch_stored@117` |
| `editor-core/src/core/host.rs` (56153) | `threads_by_activity@620`, `remember_skill_catalog@704` |
| `editor-core/src/core/host/pages.rs` (14372) | `thread_load_error@50` |
| `editor-core/src/core/hosts.rs` (9452) | `parse_pairing@275` |
| `editor-inspector/src/inspector/controller.rs` (7036) | `open_step@198`, `terminal_id@222` |
| `editor-marketplace/src/marketplace/mod.rs` (10525) | `MARKETPLACE_SCREEN_SELECTOR@10`, `MARKETPLACE_SEARCH_SELECTOR@11` |
| `editor-settings/src/settings/extensions.rs` (5453) | `MANAGE_SKILLS_SELECTOR@5` |
| `editor-settings/src/settings/page.rs` (2941) | `engine_label@65` |
| `editor-sidebar/src/sidebar/geometry.rs` (7988) | `splices@205` |
| `editor-sidebar/src/sidebar/row.rs` (16929) | `opens_keyboard_menu@35` |
| `editor-ui/src/ui/asset.rs` (2724) | `source_asset_glyph_factory@13` |
| `editor-ui/src/ui/button.rs` (12069) | `icon_text@86` |
| `editor-ui/src/ui/chrome/account.rs` (18651) | `menu_radius@173` |
| `editor-ui/src/ui/dialog.rs` (12317) | `dismiss_callback@162`, `is_soft_backdrop@170` |
| `editor-ui/src/ui/glass.rs` (4778) | `KEY_RADIUS@123` |
| `editor-ui/src/ui/markdown/literal.rs` (15901) | `NATIVE_BODY_WINDOW@25` |
| `editor-ui/src/ui/markdown/reveal.rs` (9175) | `has_presented@71` |
| `editor-ui/src/ui/markdown/terminal/screen.rs` (15612) | `take_dirty_line_indices@475`, `last_nonempty_line@520` |
| `editor-ui/src/ui/markdown/terminal/style.rs` (4389) | `is_default@31` |
| `editor-ui/src/ui/markdown/visual.rs` (26789) | `file_icon_paints@205`, `swatch_paints@225`, `interactive_prose_leaf@486`, `interactive_heading_leaf@501` |
| `editor-ui/src/ui/motion.rs` (5758) | `SHIMMER_CYCLE@98`, `SHIMMER_DELAY@99`, `FADE_ARC_ROTATION@100` |
| `editor-ui/src/ui/phrase.rs` (20812) | `phrase_or_text@196` |
| `editor-ui/src/ui/segmented_control.rs` (8142) | `focus_host@90` |
| `editor-ui/src/ui/status_chip.rs` (4804) | `chips_row@68`, `prompt_status@113` |
| `editor-ui/src/ui/text/metrics.rs` (4077) | `prompt_max_height@111` |
| `editor-ui/src/ui/text/mount.rs` (29354) | `element_with_notice@526` |
| `editor-ui/src/ui/theme.rs` (37220) | `in_hue_of@233`, `toggled@338`, `BODY_TRACKING_PX@856`, `body_tracking_px@859`, `CODE_PAD_PX@867` |
| `forge-engines/src/manager.rs` (39206) | `launch_env@424`, `check_update@480` |
| `forge-engines/src/process.rs` (14362) | `stdin_open@193` |
| `forge-runs/src/lib.rs` (44278) | `set_skills_sh_url@218` |
| `forge-store/src/sources.rs` (3934) | `resolve_item_text@88` |

Additionally, **DEAD**: `vendor/mermaid-rs-renderer/src/parser.rs:4617`, `parse_xy_series_line` (containing file **260,779 bytes**). Exact-word search finds only its definition; parsing uses `parse_xy_series_line_v2`. Suppressed by `#[allow(dead_code)]`.

No orphaned first-party Rust source files surfaced in the `mod`/`#[path]` scan. Other dead-code suppressions have real Windows or test consumers. No convincing commented-out implementation blocks surfaced.

**Dependencies and dormant features**

**DEAD declarations**, with declaration byte sizes:

- `Cargo.toml`: `tokio-stream:79` (60), `native-dialog:97` (110), `symphonia:118` (172), `unicode-segmentation:145` (33), `zip:147` (79). No crate inherits them. This does **not** make transitive Symphonia dead.
- `crates/claude-ai-browser/Cargo.toml`: `tracing:16` (25), `tempfile:19` (26).
- `crates/editor-inspector/Cargo.toml:25`: `editor-sidebar` (48), including redundant feature forwarding at line 8.
- `crates/forge-computer/Cargo.toml:15`: `serde` (23).

The latter four have no dependency-name token in their crate’s Rust sources, tests, examples or build script.

**CHECK**: `editor-aqua/Cargo.toml:12–13`, `heap-probe` and `profile-heap`: forwarding declarations only; no tracked invocation enables either.

**LIKELY**: `forge-engines/Cargo.toml:10` and `forge-store/Cargo.toml:9`, `legacy-import`: neither enabled by repository manifests, scripts or CI. Associated files:

- `forge-engines/src/customise/legacy.rs` (1,272), `src/install/legacy.rs` (9,304).
- `forge-store/src/legacy.rs` (18,787), `src/legacy/config.rs` (4,208), `repair.rs` (7,192), `source.rs` (39,792).
- `forge-store/tests/legacy.rs` (15,854), `legacy_prompt.rs` (2,033).

Remaining references are gated integration code and tests of those importers. Existing database migrations remain live.

**Repository leftovers**

- **CHECK** `vendor/highlighting/validation/`: six files, **38,653 bytes**—`Cargo.lock` (9555), `Cargo.toml` (574), `fixtures/corpus.txt` (23532), `src/bin/check_patterns.rs` (2437), `src/lib.rs` (188), `tests/parity.rs` (2367). Standalone excluded package; no CI/script invocation. Last touched `5ce28034b`, **2026-10-15**, “reform: second pass with the GPUI main pin and restored UI”. Could remain intentional manual regression tooling.
- **CHECK** `crates/editor-assets/fonts/FONTS.md` (**7,492 bytes**): provenance for unused Artisan Neo/Cal Sans and references to deleted `modules/frontend/...` sources.
- **CHECK** `nix/release.nix:91,98` (**3,620 bytes**): reachable flake output requiring deleted `scripts/install.ps1`; packaging needs repair or retirement.
- **CHECK** `scripts/release` (**2,868 bytes**): older standalone Nix signing workflow; no repository caller, while CI uses `src/release.py`. Last touched `ed394cba3`, **2026-10-23**, “Integrate editor and Forge updates”.

No tracked ignored junk or symlinks found. Local `evidence/`, `s3`, and `result` are outside HEAD. Root skills, tests, README images, production vendor packages and largest tracked image (`mountain.png`, **2,063,832 bytes**) have consumers. Transcript and baseline filenames are dynamically constructed; those collections are live.

Safe deletion order, one logical commit each:

1. Unused fonts, their catalog entries, obsolete provenance and corresponding notices.
2. Unrequested SVGs and their catalog rows.
3. Uncalled Rust declarations and newly orphaned helpers/types.
4. Unused dependency declarations and redundant feature forwarding.
5. Dormant importers with their features/tests, after confirming retirement.
6. Unused fixtures; separately resolve the old release pipeline and detached validation harness.

## 12. Duplication (jscpd)

jscpd 5.4.3 at commit `10f84a7`, over the 919 tracked source, config and script files outside `vendor/`, `target/` and `.dist/`, with `--min-lines 8 --min-tokens 70`. Run on 2026-11-17.

### 12.1 Totals

| Format | Files | Clones | Duplicated lines | Share |
| --- | ---: | ---: | ---: | ---: |
| Rust | 811 | 370 | 5,209 | 2.1% |
| TOML | 33 | 21 | 997 | 47.8% |
| YAML | 2 | 2 | 58 | 12.8% |
| HLSL | 2 | 1 | 11 | 5.4% |
| Bash, Nix, Python, PowerShell | 13 | 0 | 0 | 0% |
| **All** | **861** | **394** | **6,275** | **2.4%** |

By location: 3,513 duplicated lines are between production files, 2,692 between test files, and 70 between production and test.

### 12.2 Findings

1. **medium** — `crates/editor-aqua/clippy.toml:1-78` is copied verbatim into `editor-composer`, `editor-conversation`, `editor-inspector`, `editor-marketplace`, `editor-settings`, `editor-sidebar`, `editor-sources` and `editor-ui/feature-clippy.toml`. Nine copies of one lint policy drift independently (slice 5 already found they are weaker than the workspace policy). Use one workspace `clippy.toml` and `[lints] workspace = true`.
2. **medium** — The editor crates' `Cargo.toml` files share 34–40-line identical blocks: `editor-composer/Cargo.toml:31-66` matches `editor-conversation`, `editor-core`, `editor-inspector`, `editor-marketplace` and `editor-ui`; `editor-aqua/Cargo.toml:78-111` matches `editor-composer` and `editor-sidebar`; `editor-marketplace/Cargo.toml:18-57` matches `editor-sources` and `editor-settings`. Lint tables and dependency lists drift per crate. Move them to `[workspace.lints]` and `workspace = true` dependencies.
3. **medium** — `.github/workflows/release.yml:85-121` duplicates `release.yml:153-189`. Fixes to one build job miss the other. Use a matrix or a composite action.
4. **low** — `crates/artisan-api/src/tls.rs:122-145` duplicates `crates/forge-http/src/tls.rs:46-69`. Two copies of TLS setup can diverge on security-relevant settings. Keep one in `artisan-api`.
5. **low** — `crates/ae/build.rs:6-27` duplicates `crates/forge/build.rs:5-26`. Share the build script logic.
6. **low** — `crates/editor-ui/src/ui/media/audio.rs:16-36` and `:110-134` duplicate `crates/editor-ui/src/ui/media/model/load.rs:88-109` and `:205-229`. The audio loader re-implements the shared load cache, which is also where slice 5 found errors collapsed to `None`. Build audio on `model/load.rs`.
7. **low** — Button and control code is triplicated: `editor-ui/src/ui/kit/button/mod.rs:133-175`, `kit/control/icon_button.rs:107-158` and `:197-214`, and `kit/control/key.rs:80-103` and `:147-164`. Interaction-state fixes land in one control only. Extract the shared press and hover handling.
8. **low** — `crates/editor-aqua/src/shell/view.rs:166-198` duplicates `:270-302`.
9. **low** — `crates/editor-ui/src/ui/markdown/presenter.rs` has internal clones at `734-760` and `962-988`, and at `1615-1634` and `1738-1757`. It has the second-most duplicated lines of any file (582).
10. **low** — `crates/editor-conversation/src/conversation/view/transcript.rs:682-699` is repeated at `804-821` and `840-857`.
11. **low** — `crates/editor-conversation/src/conversation/view/factory.rs:294-314` duplicates `:322-342`.
12. **low** — `crates/editor-composer/src/composer/view.rs:1478-1499` duplicates `:1530-1551`.
13. **low** — `crates/editor-aqua/src/shell/root.rs:2863-2882` duplicates `:2900-2918`.
14. **low** — `crates/forge-engines/src/claude.rs:792-813` duplicates `:890-911`.
15. **low** — `crates/artisan-markdown-index/src/build.rs:102-124` duplicates `:252-274`.
16. **low** — `editor-ui/src/ui/rich/charts/chart/view.rs:453-475` duplicates `charts/concentration/view.rs:273-294`. Shared chart scaffolding belongs in the chart kit.
17. **low** — `crates/editor-ui/src/ui/media/preview.rs:503-520` duplicates `:565-582`.
18. **low** — Test setup is copied rather than shared. The most duplicated file is `crates/editor-aqua/tests/conversation/presentation.rs` (760 lines in clones). Others: `tests/shell/root.rs` (285-329 with 422-466, 253-282 with 391-420, 184-221 with `tests/shell/read.rs:12-50`), `tests/conversation/motion.rs:260-302` with `tests/shell/wheel.rs:11-53`, `tests/conversation/request_rows.rs:254-284` with `tests/conversation/stored_rows.rs:374-478`, `tests/markdown/transport.rs:198-231` with `editor-ui/tests/markdown/stored.rs:481-514`, `tests/text/mod.rs:158-182` with `206-230`, `tests/ui/color_copy_menu.rs:22-48` with `83-108`, and `forge-runs/tests/coordination/notifications.rs:113-138` with `199-226`. This matches the shared `tests/support` gap in `quality.md`.

Rust duplication at 2.1% is low overall; most of the value is in the configuration and test-setup clones.

## 13. Unused dependencies (cargo-shear, cargo-machete)

cargo-shear 1.14.1 (`--frozen --check-test-targets`) and cargo-machete 0.10.1 at commit `10f84a7`. Neither compiles anything; both match declared dependencies against `use` paths, so macro-only uses can be false positives. Run on 2026-11-17.

### 13.1 Unused crate dependencies

1. **low** — `crates/claude-ai-browser/Cargo.toml:16` `tracing` is never used (both tools).
2. **low** — `crates/claude-ai-browser/Cargo.toml:19` dev-dependency `tempfile` is never used (shear).
3. **low** — `crates/editor-aqua/Cargo.toml:70` `color` is never used (shear).
4. **low** — `crates/editor-conversation/Cargo.toml:27` `image`, with seven codec features, is never used (both tools). Each codec adds compile time.
5. **low** — `crates/editor-conversation` dev-dependency `tempfile` is never used (machete).
6. **low** — `crates/editor-sidebar/Cargo.toml:21` `image` is never used (both tools).
7. **low** — `crates/forge-computer/Cargo.toml:15` `serde` is never used (both tools).
8. **low** — `crates/editor-inspector/Cargo.toml:25` depends on `editor-sidebar` only for a feature, and no code uses it (both tools; slice 5 found the same).
9. **nit** — `vendor/highlighting/validation/Cargo.toml` declares `once_cell`, `onig`, `serde` and `serde_json` without using them (machete). This package sits outside the workspace and CI.

Not findings: the self dev-dependencies flagged in `editor-composer/Cargo.toml:31`, `editor-conversation/Cargo.toml:34` and `editor-core/Cargo.toml:31` (`<crate> = { path = ".", features = ["test-support"] }`) only exist to turn on `test-support` for tests. They go away when test hooks leave `src/`, as `quality.md` proposes.

### 13.2 Unused workspace dependencies

10. **low** — `Cargo.toml:79` `tokio-stream` is declared in `[workspace.dependencies]` but no crate uses it.
11. **low** — `Cargo.toml:97` `native-dialog`, same.
12. **low** — `Cargo.toml:118` `symphonia`, same.
13. **low** — `Cargo.toml:145` `unicode-segmentation`, same.
14. **low** — `Cargo.toml:147` `zip`, same.

### 13.3 Test target flags

15. **nit** — 27 library targets keep `doctest = true` with no doc tests, so `cargo test` compiles and runs an empty doctest binary for each. Set `doctest = false` once for the workspace.
16. **nit** — 6 targets keep `test = true` with no tests (`test_enabled_without_tests`):
    - `test = true` on lib target `editor_inspector` but source contains no tests
    - `test = true` on lib target `editor_marketplace` but source contains no tests
    - `test = true` on lib target `editor_settings` but source contains no tests
    - `test = true` on lib target `editor_sidebar` but source contains no tests
    - `test = true` on lib target `editor_sources` but source contains no tests
    - `test = true` on lib target `forge_isolation` but source contains no tests

### 13.4 What the tools cannot see

rustc's `dead_code` lint only covers items that aren't exported from their crate. A `pub` item that no other crate uses is invisible to it, to cargo-shear and to cargo-machete. Two ways to cover that:
- the grep-based crawl in section 11;
- building once with `-W unreachable_pub` (reports `pub` items that could be `pub(crate)`; after narrowing them, `dead_code` reports the unused ones). That needs a full workspace build, so it was not run while the Editor is in use.

## 14. Dead and over-exposed public API (Hawk)

cargo-hawk 0.1.18 (Astral; the tool Bun used for its 39k-line dead-code removal) on Rust 1.100.0, run on a clean export of commit `10f84a7`. Production entry points: the `editor-aqua`, `forge` and `ae` binaries, with default (shipped) features. It was run twice, once for `x86_64-unknown-linux-gnu` and once for `x86_64-pc-windows-gnu` (through the `cargo-windows` dev shell); this section lists only findings reported on **both** targets, so platform-only code is not counted as dead. Hawk treats the workspace as the whole world and separates production reachability from tests: an item used only by tests counts as dead. Hawk is experimental, so verify each item before deleting it. Counts are deduplicated by source location; Hawk printed 2,588 (Linux) and 2,587 (Windows) raw warnings because some locations carry several, such as macro-generated items. Run on 2026-11-17.

| Lint | Both targets | Linux only | Windows only |
| --- | ---: | ---: | ---: |
| Dead public items (`hawk::dead_public`) | 250 | 2 | 3 |
| Public items that can be `pub(crate)` (`hawk::unnecessary_public`) | 2065 | 8 | 6 |
| Restricted items that can be private (`hawk::unnecessary_restricted_visibility`) | 201 | 2 | 2 |

### 14.1 Dead public items


**editor-ui** (127)

1. **low** — `crates/editor-ui/src/ui/asset.rs:5:1` `ui::asset::SourceSvg`
2. **low** — `crates/editor-ui/src/ui/asset.rs:6:5` `ui::asset::SourceSvg::source`
3. **low** — `crates/editor-ui/src/ui/asset.rs:7:5` `ui::asset::SourceSvg::presentation`
4. **low** — `crates/editor-ui/src/ui/asset.rs:8:5` `ui::asset::SourceSvg::intrinsic_dimensions`
5. **low** — `crates/editor-ui/src/ui/asset.rs:9:5` `ui::asset::SourceSvg::edge`
6. **low** — `crates/editor-ui/src/ui/asset.rs:10:5` `ui::asset::SourceSvg::tint`
7. **low** — `crates/editor-ui/src/ui/asset.rs:13:1` `ui::asset::source_asset_glyph_factory`
8. **low** — `crates/editor-ui/src/ui/asset.rs:50:1` `ui::asset::asset_glyph`
9. **low** — `crates/editor-ui/src/ui/button.rs:75:5` `ui::button::Button::text`
10. **low** — `crates/editor-ui/src/ui/button.rs:86:5` `ui::button::Button::icon_text`
11. **low** — `crates/editor-ui/src/ui/button.rs:94:5` `ui::button::Button::CORNER_INSET`
12. **low** — `crates/editor-ui/src/ui/button.rs:95:5` `ui::button::Button::corner_close`
13. **low** — `crates/editor-ui/src/ui/button.rs:112:5` `ui::button::Button::label_overflow`
14. **low** — `crates/editor-ui/src/ui/button.rs:116:5` `ui::button::Button::disabled`
15. **low** — `crates/editor-ui/src/ui/button.rs:120:5` `ui::button::Button::muted`
16. **low** — `crates/editor-ui/src/ui/button.rs:124:5` `ui::button::Button::nested_radius`
17. **low** — `crates/editor-ui/src/ui/button.rs:128:5` `ui::button::Button::glass`
18. **low** — `crates/editor-ui/src/ui/button.rs:132:5` `ui::button::Button::compact`
19. **low** — `crates/editor-ui/src/ui/button.rs:144:5` `ui::button::Button::debug_selector`
20. **low** — `crates/editor-ui/src/ui/button.rs:148:5` `ui::button::Button::motion_policy`
21. **low** — `crates/editor-ui/src/ui/card.rs:22:5` `ui::card::CardStyle::resolve`
22. **low** — `crates/editor-ui/src/ui/card.rs:37:5` `ui::card::CardStyle::ring`
23. **low** — `crates/editor-ui/src/ui/card.rs:48:1` `ui::card::compact_card`
24. **low** — `crates/editor-ui/src/ui/card.rs:68:1` `ui::card::compact_card_content`
25. **low** — `crates/editor-ui/src/ui/card.rs:76:1` `ui::card::Card`
26. **low** — `crates/editor-ui/src/ui/card.rs:82:5` `ui::card::Card::new`
27. **low** — `crates/editor-ui/src/ui/card.rs:89:5` `ui::card::Card::bands`
28. **low** — `crates/editor-ui/src/ui/card.rs:96:5` `ui::card::Card::build`
29. **low** — `crates/editor-ui/src/ui/chat_bubble.rs:84:5` `ui::chat_bubble::ChatBubbleSegment::debug_selector`
30. **low** — `crates/editor-ui/src/ui/chrome/account.rs:173:1` `ui::chrome::account::menu_radius`
31. **low** — `crates/editor-ui/src/ui/chrome/account.rs:177:1` `ui::chrome::account::hover_style`
32. **low** — `crates/editor-ui/src/ui/chrome/card.rs:10:1` `ui::chrome::card::INSET_RADIUS`
33. **low** — `crates/editor-ui/src/ui/chrome/card.rs:72:1` `ui::chrome::card::inset`
34. **low** — `crates/editor-ui/src/ui/chrome/card.rs:89:1` `ui::chrome::card::card`
35. **low** — `crates/editor-ui/src/ui/dialog.rs:85:5` `ui::dialog::Dialog::description`
36. **low** — `crates/editor-ui/src/ui/dialog.rs:162:5` `ui::dialog::Dialog::dismiss_callback`
37. **low** — `crates/editor-ui/src/ui/dialog.rs:170:5` `ui::dialog::Dialog::is_soft_backdrop`
38. **low** — `crates/editor-ui/src/ui/dialog.rs:284:5` `ui::dialog::DialogHost::handle_input`
39. **low** — `crates/editor-ui/src/ui/glass.rs:123:1` `ui::glass::KEY_RADIUS`
40. **low** — `crates/editor-ui/src/ui/hover_tooltip.rs:136:5` `ui::hover_tooltip::TooltipHost::root`
41. **low** — `crates/editor-ui/src/ui/kit/button/mod.rs:124:5` `ui::kit::button::Button::destructive`
42. **low** — `crates/editor-ui/src/ui/kit/control/icon_button.rs:131:5` `ui::kit::control::icon_button::IconButton::destructive`
43. **low** — `crates/editor-ui/src/ui/kit/control/icon_button.rs:149:5` `ui::kit::control::icon_button::IconButton::motion_policy`
44. **low** — `crates/editor-ui/src/ui/kit/control/key.rs:91:5` `ui::kit::control::key::Key::motion_policy`
45. **low** — `crates/editor-ui/src/ui/kit/control/tabs.rs:26:5` `ui::kit::control::tabs::Tabs::new`
46. **low** — `crates/editor-ui/src/ui/kit/control/tabs.rs:44:5` `ui::kit::control::tabs::Tabs::on_select`
47. **low** — `crates/editor-ui/src/ui/kit/control/tabs.rs:50:5` `ui::kit::control::tabs::Tabs::build`
48. **low** — `crates/editor-ui/src/ui/kit/control/trigger.rs:93:5` `ui::kit::control::trigger::Trigger::disabled`
49. **low** — `crates/editor-ui/src/ui/kit/divider.rs:60:5` `ui::kit::divider::Divider::vertical`
50. **low** — `crates/editor-ui/src/ui/kit/hover/mod.rs:37:5` `ui::kit::hover::Material::animate_to`
51. **low** — `crates/editor-ui/src/ui/kit/hover/mod.rs:125:5` `ui::kit::hover::Hover::layer`
52. **low** — `crates/editor-ui/src/ui/markdown/literal.rs:25:1` `ui::markdown::literal::NATIVE_BODY_WINDOW`
53. **low** — `crates/editor-ui/src/ui/markdown/literal.rs:68:5` `ui::markdown::literal::LiteralSelection::leaf`
54. **low** — `crates/editor-ui/src/ui/markdown/literal.rs:196:5` `ui::markdown::literal::LiteralOwner::release`
55. **low** — `crates/editor-ui/src/ui/markdown/literal.rs:208:5` `ui::markdown::literal::LiteralOwner::retry`
56. **low** — `crates/editor-ui/src/ui/markdown/literal.rs:221:5` `ui::markdown::literal::LiteralOwner::is_current`
57. **low** — `crates/editor-ui/src/ui/markdown/mod.rs:167:5` `ui::markdown::Block::text_content`
58. **low** — `crates/editor-ui/src/ui/markdown/mod.rs:195:5` `ui::markdown::ListItem::text_content`
59. **low** — `crates/editor-ui/src/ui/markdown/reveal.rs:71:5` `ui::markdown::reveal::PreparedReveal::has_presented`
60. **low** — `crates/editor-ui/src/ui/markdown/source_index.rs:35:5` `ui::markdown::source_index::MarkdownIndex::source_bytes`
61. **low** — `crates/editor-ui/src/ui/markdown/source_index.rs:38:5` `ui::markdown::source_index::MarkdownIndex::resident_bytes`
62. **low** — `crates/editor-ui/src/ui/markdown/table.rs:23:5` `ui::markdown::table::Table::text_content`
63. **low** — `crates/editor-ui/src/ui/markdown/terminal/screen.rs:475:5` `ui::markdown::terminal::screen::TerminalScreen::take_dirty_line_indices`
64. **low** — `crates/editor-ui/src/ui/markdown/terminal/screen.rs:520:5` `ui::markdown::terminal::screen::TerminalScreen::last_nonempty_line`
65. **low** — `crates/editor-ui/src/ui/markdown/terminal/style.rs:31:5` `ui::markdown::terminal::style::TerminalStyle::is_default`
66. **low** — `crates/editor-ui/src/ui/markdown/visual.rs:128:5` `ui::markdown::visual::PreparedInline::styled_spans`
67. **low** — `crates/editor-ui/src/ui/markdown/visual.rs:205:5` `ui::markdown::visual::PreparedInline::file_icon_paints`
68. **low** — `crates/editor-ui/src/ui/markdown/visual.rs:225:5` `ui::markdown::visual::PreparedInline::swatch_paints`
69. **low** — `crates/editor-ui/src/ui/markdown/visual.rs:486:1` `ui::markdown::visual::interactive_prose_leaf`
70. **low** — `crates/editor-ui/src/ui/markdown/visual.rs:501:1` `ui::markdown::visual::interactive_heading_leaf`
71. **low** — `crates/editor-ui/src/ui/media/model/formats/mod.rs:66:5` `ui::media::model::formats::Model::resident_bytes`
72. **low** — `crates/editor-ui/src/ui/motion.rs:9:5` `ui::motion::WorkingPulse::reset`
73. **low** — `crates/editor-ui/src/ui/motion.rs:69:5` `ui::motion::MotionPolicy::resolve`
74. **low** — `crates/editor-ui/src/ui/motion.rs:98:1` `ui::motion::SHIMMER_CYCLE`
75. **low** — `crates/editor-ui/src/ui/motion.rs:99:1` `ui::motion::SHIMMER_DELAY`
76. **low** — `crates/editor-ui/src/ui/motion.rs:100:1` `ui::motion::FADE_ARC_ROTATION`
77. **low** — `crates/editor-ui/src/ui/motion.rs:173:5` `ui::motion::MotionAnimation::duration`
78. **low** — `crates/editor-ui/src/ui/motion.rs:176:5` `ui::motion::MotionAnimation::curve`
79. **low** — `crates/editor-ui/src/ui/page.rs:31:5` `ui::page::PageBlock::debug_selector`
80. **low** — `crates/editor-ui/src/ui/page.rs:50:5` `ui::page::Notice::debug_selector`
81. **low** — `crates/editor-ui/src/ui/phrase.rs:196:1` `ui::phrase::phrase_or_text`
82. **low** — `crates/editor-ui/src/ui/phrase.rs:497:1` `ui::phrase::phrase_element`
83. **low** — `crates/editor-ui/src/ui/picker_motion.rs:76:5` `ui::picker_motion::MenuMotion::painted`
84. **low** — `crates/editor-ui/src/ui/popover.rs:115:1` `ui::popover::PopoverHost`
85. **low** — `crates/editor-ui/src/ui/popover.rs:444:5` `ui::popover::AnimatedPickerHost::step`
86. **low** — `crates/editor-ui/src/ui/popover.rs:503:5` `ui::popover::PopoverHost::open`
87. **low** — `crates/editor-ui/src/ui/popover.rs:520:5` `ui::popover::PopoverHost::sync`
88. **low** — `crates/editor-ui/src/ui/popover.rs:526:5` `ui::popover::PopoverHost::handle_input`
89. **low** — `crates/editor-ui/src/ui/popover.rs:532:5` `ui::popover::PopoverHost::is_open`
90. **low** — `crates/editor-ui/src/ui/segmented_control.rs:77:5` `ui::segmented_control::SegmentedControl::<T>::disabled`
91. **low** — `crates/editor-ui/src/ui/segmented_control.rs:85:5` `ui::segmented_control::SegmentedControl::<T>::on_focus_change`
92. **low** — `crates/editor-ui/src/ui/segmented_control.rs:90:5` `ui::segmented_control::SegmentedControl::<T>::focus_host`
93. **low** — `crates/editor-ui/src/ui/segmented_control.rs:217:5` `ui::segmented_control::SegmentedFocus::sync`
94. **low** — `crates/editor-ui/src/ui/segmented_control.rs:231:5` `ui::segmented_control::SegmentedFocus::handle_input`
95. **low** — `crates/editor-ui/src/ui/select.rs:5:1` `ui::select::MENU_MOTION`
96. **low** — `crates/editor-ui/src/ui/select.rs:30:5` `ui::select::SelectEntry::<V>::disabled`
97. **low** — `crates/editor-ui/src/ui/select.rs:108:5` `ui::select::SelectState::<V>::finish`
98. **low** — `crates/editor-ui/src/ui/select.rs:284:5` `ui::select::Select::<V>::disabled`
99. **low** — `crates/editor-ui/src/ui/select.rs:558:1` `ui::select::SelectPopupHost`
100. **low** — `crates/editor-ui/src/ui/select.rs:570:5` `ui::select::SelectPopupHost::<V>::open`
101. **low** — `crates/editor-ui/src/ui/select.rs:620:5` `ui::select::SelectPopupHost::<V>::advance`
102. **low** — `crates/editor-ui/src/ui/select.rs:693:5` `ui::select::SelectPopupHost::<V>::handle_input`
103. **low** — `crates/editor-ui/src/ui/select.rs:718:5` `ui::select::SelectPopupHost::<V>::step`
104. **low** — `crates/editor-ui/src/ui/select.rs:756:5` `ui::select::SelectPopupHost::<V>::is_open`
105. **low** — `crates/editor-ui/src/ui/select.rs:760:5` `ui::select::SelectPopupHost::<V>::is_animating`
106. **low** — `crates/editor-ui/src/ui/slider.rs:63:5` `ui::slider::SliderCommand::from_key`
107. **low** — `crates/editor-ui/src/ui/slider.rs:172:5` `ui::slider::Slider::disabled`
108. **low** — `crates/editor-ui/src/ui/solar.rs:20:1` `ui::solar::artwork`
109. **low** — `crates/editor-ui/src/ui/status_chip.rs:33:5` `ui::status_chip::EngineChip::new`
110. **low** — `crates/editor-ui/src/ui/status_chip.rs:68:1` `ui::status_chip::chips_row`
111. **low** — `crates/editor-ui/src/ui/status_chip.rs:113:1` `ui::status_chip::prompt_status`
112. **low** — `crates/editor-ui/src/ui/switch.rs:46:5` `ui::switch::Switch::focus_visible`
113. **low** — `crates/editor-ui/src/ui/text/metrics.rs:111:5` `ui::text::metrics::EditorMetrics::prompt_max_height`
114. **low** — `crates/editor-ui/src/ui/text/mount.rs:526:5` `ui::text::mount::EditorController::element_with_notice`
115. **low** — `crates/editor-ui/src/ui/text/truncated.rs:8:1` `ui::text::truncated::TruncatedText`
116. **low** — `crates/editor-ui/src/ui/text/truncated.rs:16:5` `ui::text::truncated::TruncatedText::new`
117. **low** — `crates/editor-ui/src/ui/text/truncated.rs:25:5` `ui::text::truncated::TruncatedText::debug_selector`
118. **low** — `crates/editor-ui/src/ui/text/truncated.rs:30:5` `ui::text::truncated::TruncatedText::build`
119. **low** — `crates/editor-ui/src/ui/theme.rs:233:5` `ui::theme::Oklch::in_hue_of`
120. **low** — `crates/editor-ui/src/ui/theme.rs:338:5` `ui::theme::ThemeMode::toggled`
121. **low** — `crates/editor-ui/src/ui/theme.rs:773:5` `ui::theme::TypographyTokens::size`
122. **low** — `crates/editor-ui/src/ui/theme.rs:814:5` `ui::theme::FontSet::label`
123. **low** — `crates/editor-ui/src/ui/theme.rs:856:5` `ui::theme::ProseTypography::BODY_TRACKING_PX`
124. **low** — `crates/editor-ui/src/ui/theme.rs:859:5` `ui::theme::ProseTypography::body_tracking_px`
125. **low** — `crates/editor-ui/src/ui/theme.rs:867:5` `ui::theme::ProseTypography::CODE_PAD_PX`
126. **low** — `crates/editor-ui/src/ui/theme.rs:944:5` `ui::theme::RadiusTokens::BASE_PX`
127. **low** — `crates/editor-ui/src/ui/theme.rs:965:5` `ui::theme::RadiusTokens::nested`

**editor-conversation** (48)

128. **low** — `crates/editor-conversation/src/conversation/adapter.rs:299:1` `conversation::adapter::changed_disclosures`
129. **low** — `crates/editor-conversation/src/conversation/adapter.rs:313:1` `conversation::adapter::row_key`
130. **low** — `crates/editor-conversation/src/conversation/adapter.rs:362:1` `conversation::adapter::composer_goal`
131. **low** — `crates/editor-conversation/src/conversation/adapter.rs:404:1` `conversation::adapter::restore_anchor`
132. **low** — `crates/editor-conversation/src/conversation/admission.rs:16:1` `conversation::admission::speculative_frontier`
133. **low** — `crates/editor-conversation/src/conversation/admission.rs:65:1` `conversation::admission::revealed_rows`
134. **low** — `crates/editor-conversation/src/conversation/admission.rs:70:1` `conversation::admission::changed_from`
135. **low** — `crates/editor-conversation/src/conversation/admission.rs:75:1` `conversation::admission::sync_frontier`
136. **low** — `crates/editor-conversation/src/conversation/admission.rs:95:1` `conversation::admission::publish_frontier`
137. **low** — `crates/editor-conversation/src/conversation/admission.rs:150:5` `conversation::admission::RevealQueue::remove`
138. **low** — `crates/editor-conversation/src/conversation/admission.rs:173:5` `conversation::admission::LiveBlockArrivals::is_fresh`
139. **low** — `crates/editor-conversation/src/conversation/content.rs:417:5` `conversation::content::SourceOwners::drain`
140. **low** — `crates/editor-conversation/src/conversation/controller.rs:20:5` `conversation::controller::ConversationEvent::NewProject`
141. **low** — `crates/editor-conversation/src/conversation/controller.rs:483:5` `conversation::controller::Conversation::items`
142. **low** — `crates/editor-conversation/src/conversation/controller.rs:562:5` `conversation::controller::Conversation::events`
143. **low** — `crates/editor-conversation/src/conversation/controller.rs:784:5` `conversation::controller::Conversation::claim_prose_arrival`
144. **low** — `crates/editor-conversation/src/conversation/controller.rs:821:5` `conversation::controller::Conversation::publish_projection`
145. **low** — `crates/editor-conversation/src/conversation/controller.rs:1092:5` `conversation::controller::Conversation::agent_context`
146. **low** — `crates/editor-conversation/src/conversation/presentation.rs:153:5` `conversation::presentation::SelectionOwners::prepare_collection_row`
147. **low** — `crates/editor-conversation/src/conversation/presentation.rs:333:5` `conversation::presentation::MarkdownOwners::set_media_services`
148. **low** — `crates/editor-conversation/src/conversation/presentation.rs:473:5` `conversation::presentation::MarkdownOwners::rendered_current`
149. **low** — `crates/editor-conversation/src/conversation/presentation.rs:484:5` `conversation::presentation::MarkdownOwners::prepare_projected`
150. **low** — `crates/editor-conversation/src/conversation/presentation.rs:596:5` `conversation::presentation::MarkdownOwners::render_projected`
151. **low** — `crates/editor-conversation/src/conversation/presentation.rs:611:5` `conversation::presentation::MarkdownOwners::prepare_projected_row`
152. **low** — `crates/editor-conversation/src/conversation/preview.rs:7:1` `conversation::preview::markdown`
153. **low** — `crates/editor-conversation/src/conversation/questions.rs:396:5` `conversation::questions::QuestionCollection::open`
154. **low** — `crates/editor-conversation/src/conversation/scroll.rs:134:1` `conversation::scroll::row_destination`
155. **low** — `crates/editor-conversation/src/conversation/stored.rs:180:5` `conversation::stored::StoredConversation::wake_mailbox`
156. **low** — `crates/editor-conversation/src/conversation/stored.rs:518:5` `conversation::stored::StoredConversation::owner`
157. **low** — `crates/editor-conversation/src/conversation/view/factory.rs:1130:5` `conversation::view::factory::RowFactory::prepare_prose`
158. **low** — `crates/editor-conversation/src/conversation/view/factory.rs:1455:5` `conversation::view::factory::RowFactory::set_media_services`
159. **low** — `crates/editor-conversation/src/conversation/view/factory.rs:1517:5` `conversation::view::factory::RowFactory::with_body_versions`
160. **low** — `crates/editor-conversation/src/conversation/view/factory.rs:1539:5` `conversation::view::factory::RowFactory::retire`
161. **low** — `crates/editor-conversation/src/conversation/view/factory.rs:1630:5` `conversation::view::factory::RowFactory::sync_reveal_geometry`
162. **low** — `crates/editor-conversation/src/conversation/view/mod.rs:319:1` `conversation::view::observed_row_frame`
163. **low** — `crates/editor-conversation/src/conversation/view/mod.rs:346:1` `conversation::view::thread_element`
164. **low** — `crates/editor-conversation/src/conversation/view/mod.rs:373:1` `conversation::view::try_thread_element`
165. **low** — `crates/editor-conversation/src/conversation/view/mod.rs:400:1` `conversation::view::batched_thread_element`
166. **low** — `crates/editor-conversation/src/conversation/view/motion.rs:317:5` `conversation::view::motion::RowMotion::retain`
167. **low** — `crates/editor-conversation/src/conversation/view/resources.rs:105:5` `conversation::view::resources::ResourceBatches::from_prepared`
168. **low** — `crates/editor-conversation/src/conversation/view/resources.rs:230:1` `conversation::view::resources::stored_source`
169. **low** — `crates/editor-conversation/src/conversation/view/resources.rs:256:1` `conversation::view::resources::stored_questions`
170. **low** — `crates/editor-conversation/src/conversation/view/row.rs:83:1` `conversation::view::row::render`
171. **low** — `crates/editor-conversation/src/conversation/view/transcript.rs:117:5` `conversation::view::transcript::TranscriptObservations::prefetch_stored`
172. **low** — `crates/editor-conversation/src/conversation/view/transcript.rs:679:1` `conversation::view::transcript::list`
173. **low** — `crates/editor-conversation/src/conversation/view/transcript.rs:715:1` `conversation::view::transcript::PreparedRowFactory`
174. **low** — `crates/editor-conversation/src/conversation/view/transcript.rs:801:1` `conversation::view::transcript::batched_list`
175. **low** — `crates/editor-conversation/src/conversation/view/transcript.rs:837:1` `conversation::view::transcript::try_list`

**editor-aqua** (18)

176. **low** — `crates/editor-aqua/src/frame/sidebar.rs:206:5` `frame::sidebar::Sidebar::fades`
177. **low** — `crates/editor-aqua/src/frame/sidebar.rs:235:5` `frame::sidebar::Sidebar::pinned`
178. **low** — `crates/editor-aqua/src/frame/sidebar.rs:239:5` `frame::sidebar::Sidebar::lay_out`
179. **low** — `crates/editor-aqua/src/shell/application.rs:280:5` `shell::application::ShellApplication::<V>::tap_render`
180. **low** — `crates/editor-aqua/src/shell/context_meter.rs:23:5` `shell::context_meter::ContextReading::tone`
181. **low** — `crates/editor-aqua/src/shell/context_meter.rs:92:1` `shell::context_meter::ring`
182. **low** — `crates/editor-aqua/src/shell/host.rs:329:5` `shell::host::Shell::activate_palette`
183. **low** — `crates/editor-aqua/src/shell/host.rs:347:5` `shell::host::Shell::quit`
184. **low** — `crates/editor-aqua/src/shell/host.rs:350:5` `shell::host::Shell::close_window`
185. **low** — `crates/editor-aqua/src/shell/layout.rs:70:1` `shell::layout::entry_layout`
186. **low** — `crates/editor-aqua/src/shell/palette/view.rs:228:5` `shell::palette::view::PaletteView::dialog`
187. **low** — `crates/editor-aqua/src/shell/resource_preview.rs:35:5` `shell::resource_preview::PreviewText::visible_line`
188. **low** — `crates/editor-aqua/src/shell/titlebar.rs:58:5` `shell::titlebar::ContextMeterTooltip::ring`
189. **low** — `crates/editor-aqua/src/shell/titlebar.rs:250:5` `shell::titlebar::Titlebar::thread_header`
190. **low** — `crates/editor-aqua/src/shell/view.rs:157:1` `shell::view::conversation`
191. **low** — `crates/editor-aqua/src/shell/workspace.rs:635:5` `shell::workspace::Workspace::inspector_elements`
192. **low** — `crates/editor-aqua/src/shell/workspace.rs:1660:5` `shell::workspace::Workspace::activate_palette`
193. **low** — `crates/editor-aqua/src/shell/workspace.rs:1708:5` `shell::workspace::Workspace::acknowledge_painted`

**editor-core** (13)

194. **low** — `crates/editor-core/src/core/account/mod.rs:305:5` `core::account::Account::base_url`
195. **low** — `crates/editor-core/src/core/client.rs:245:5` `core::client::ForgeClient::items`
196. **low** — `crates/editor-core/src/core/client.rs:345:5` `core::client::ForgeClient::set_default_model`
197. **low** — `crates/editor-core/src/core/client.rs:393:5` `core::client::ForgeClient::remove_project`
198. **low** — `crates/editor-core/src/core/client.rs:749:5` `core::client::ForgeClient::engines`
199. **low** — `crates/editor-core/src/core/client.rs:785:5` `core::client::ForgeClient::skills`
200. **low** — `crates/editor-core/src/core/host.rs:127:5` `core::host::HostEvent::thread`
201. **low** — `crates/editor-core/src/core/host.rs:604:5` `core::host::HostData::live`
202. **low** — `crates/editor-core/src/core/host.rs:620:5` `core::host::HostData::threads_by_activity`
203. **low** — `crates/editor-core/src/core/host.rs:704:5` `core::host::HostData::remember_skill_catalog`
204. **low** — `crates/editor-core/src/core/host/pages.rs:50:5` `core::host::pages::<impl core::host::HostData>::thread_load_error`
205. **low** — `crates/editor-core/src/core/hosts.rs:275:1` `core::hosts::parse_pairing`
206. **low** — `crates/editor-core/src/core/range_window.rs:198:5` `core::range_window::RangeWindow::<R>::reset`

**artisan-account** (8)

207. **low** — `crates/artisan-account/src/network.rs:331:5` `network::CoreSessionClient::magic_consume`
208. **low** — `crates/artisan-account/src/network.rs:430:5` `network::CoreSessionClient::list_sessions`
209. **low** — `crates/artisan-account/src/network.rs:464:5` `network::CoreSessionClient::totp_enroll`
210. **low** — `crates/artisan-account/src/network.rs:477:5` `network::CoreSessionClient::totp_verify`
211. **low** — `crates/artisan-account/src/network.rs:495:5` `network::CoreSessionClient::totp_disable`
212. **low** — `crates/artisan-account/src/network.rs:529:5` `network::CoreSessionClient::list_machines`
213. **low** — `crates/artisan-account/src/network.rs:585:5` `network::CoreSessionClient::latest_bundle_head`
214. **low** — `crates/artisan-account/src/network.rs:600:5` `network::CoreSessionClient::jwks`

**editor-composer** (7)

215. **low** — `crates/editor-composer/src/composer/controller.rs:1025:1` `composer::controller::remembered_harness_config`
216. **low** — `crates/editor-composer/src/composer/goal_strip.rs:217:5` `composer::goal_strip::StripState::set_parked_time`
217. **low** — `crates/editor-composer/src/composer/view.rs:1344:5` `composer::view::ComposerOwner::tooltip_target`
218. **low** — `crates/editor-composer/src/composer/view.rs:1459:5` `composer::view::ComposerOwner::open_animated_popup`
219. **low** — `crates/editor-composer/src/composer/view.rs:1622:5` `composer::view::ComposerOwner::open_project_popup`
220. **low** — `crates/editor-composer/src/composer/view.rs:1789:5` `composer::view::ComposerOwner::refresh_model_popup`
221. **low** — `crates/editor-composer/src/composer/view.rs:2817:1` `composer::view::open_popup`

**artisan-api** (5)

222. **low** — `crates/artisan-api/src/engine.rs:104:5` `engine::EngineConfig::option_str`
223. **low** — `crates/artisan-api/src/item.rs:335:5` `item::Output::is_empty`
224. **low** — `crates/artisan-api/src/transport.rs:121:5` `transport::ClaudeAiState::usable`
225. **low** — `crates/artisan-api/src/tunnel.rs:209:1` `tunnel::STREAM_START_KEY`
226. **low** — `crates/artisan-api/src/tunnel.rs:227:1` `tunnel::EVENTS_PATH`

**forge-engines** (5)

227. **low** — `crates/forge-engines/src/claudeai/mod.rs:1365:1` `claudeai::tool`
228. **low** — `crates/forge-engines/src/manager.rs:424:5` `manager::Engines::launch_env`
229. **low** — `crates/forge-engines/src/manager.rs:436:5` `manager::Engines::login_args`
230. **low** — `crates/forge-engines/src/manager.rs:480:5` `manager::Engines::check_update`
231. **low** — `crates/forge-engines/src/process.rs:193:5` `process::EngineProcess::stdin_open`

**editor-assets** (4)

232. **low** — `crates/editor-assets/src/fonts.rs:126:1` `fonts::lookup_family`
233. **low** — `crates/editor-assets/src/lib.rs:107:5` `Family::ALL`
234. **low** — `crates/editor-assets/src/lib.rs:603:1` `LICENSE_FILES`
235. **low** — `crates/editor-assets/src/lib.rs:664:1` `license`

**editor-inspector** (3)

236. **low** — `crates/editor-inspector/src/inspector/controller.rs:120:5` `inspector::controller::Inspector::open_agent`
237. **low** — `crates/editor-inspector/src/inspector/controller.rs:198:5` `inspector::controller::Inspector::open_step`
238. **low** — `crates/editor-inspector/src/inspector/controller.rs:222:5` `inspector::controller::Inspector::terminal_id`

**editor-settings** (3)

239. **low** — `crates/editor-settings/src/settings/extensions.rs:5:1` `settings::extensions::MANAGE_SKILLS_SELECTOR`
240. **low** — `crates/editor-settings/src/settings/page.rs:4:1` `settings::page::label`
241. **low** — `crates/editor-settings/src/settings/page.rs:65:1` `settings::page::engine_label`

**editor-sidebar** (3)

242. **low** — `crates/editor-sidebar/src/sidebar/geometry.rs:205:1` `sidebar::geometry::splices`
243. **low** — `crates/editor-sidebar/src/sidebar/row.rs:35:1` `sidebar::row::opens_keyboard_menu`
244. **low** — `crates/editor-sidebar/src/sidebar/view.rs:717:1` `sidebar::view::thread_list`

**editor-marketplace** (2)

245. **low** — `crates/editor-marketplace/src/marketplace/mod.rs:10:1` `marketplace::MARKETPLACE_SCREEN_SELECTOR`
246. **low** — `crates/editor-marketplace/src/marketplace/mod.rs:11:1` `marketplace::MARKETPLACE_SEARCH_SELECTOR`

**editor-audio** (1)

247. **low** — `crates/editor-audio/src/lib.rs:106:5` `Player::stop`

**editor-sources** (1)

248. **low** — `crates/editor-sources/src/sources/controller.rs:90:5` `sources::controller::ContentText::completion_for_tests`

**forge-runs** (1)

249. **low** — `crates/forge-runs/src/lib.rs:218:5` `Hub::set_skills_sh_url`

**forge-store** (1)

250. **low** — `crates/forge-store/src/sources.rs:88:5` `sources::<impl Store>::resolve_item_text`

### 14.2 Public items that can be `pub(crate)`

<details><summary>2065 findings</summary>


**editor-aqua** (498)

251. **nit** — `crates/editor-aqua/src/frame/avatar.rs:61:5` `frame::avatar::Avatar::mailbox` — it can be `pub(crate)`
252. **nit** — `crates/editor-aqua/src/frame/avatar.rs:66:5` `frame::avatar::Avatar::image` — it can be `pub(crate)`
253. **nit** — `crates/editor-aqua/src/frame/avatar.rs:73:5` `frame::avatar::Avatar::want` — it can be `pub(crate)`
254. **nit** — `crates/editor-aqua/src/frame/avatar.rs:92:5` `frame::avatar::Avatar::drain` — it can be `pub(crate)`
255. **nit** — `crates/editor-aqua/src/frame/inbox.rs:12:1` `frame::inbox::view` — it can be `pub(crate)`
256. **nit** — `crates/editor-aqua/src/frame/overview.rs:24:5` `frame::overview::Subagent::id` — it can be `pub(crate)`
257. **nit** — `crates/editor-aqua/src/frame/overview.rs:25:5` `frame::overview::Subagent::thread` — it can be `pub(crate)`
258. **nit** — `crates/editor-aqua/src/frame/overview.rs:26:5` `frame::overview::Subagent::label` — it can be `pub(crate)`
259. **nit** — `crates/editor-aqua/src/frame/overview.rs:27:5` `frame::overview::Subagent::task` — it can be `pub(crate)`
260. **nit** — `crates/editor-aqua/src/frame/overview.rs:28:5` `frame::overview::Subagent::status` — it can be `pub(crate)`
261. **nit** — `crates/editor-aqua/src/frame/overview.rs:29:5` `frame::overview::Subagent::engine` — it can be `pub(crate)`
262. **nit** — `crates/editor-aqua/src/frame/overview.rs:30:5` `frame::overview::Subagent::card` — it can be `pub(crate)`
263. **nit** — `crates/editor-aqua/src/frame/overview.rs:35:5` `frame::overview::Overview::project` — it can be `pub(crate)`
264. **nit** — `crates/editor-aqua/src/frame/overview.rs:36:5` `frame::overview::Overview::machine` — it can be `pub(crate)`
265. **nit** — `crates/editor-aqua/src/frame/overview.rs:37:5` `frame::overview::Overview::branch` — it can be `pub(crate)`
266. **nit** — `crates/editor-aqua/src/frame/overview.rs:38:5` `frame::overview::Overview::context` — it can be `pub(crate)`
267. **nit** — `crates/editor-aqua/src/frame/overview.rs:39:5` `frame::overview::Overview::agents` — it can be `pub(crate)`
268. **nit** — `crates/editor-aqua/src/frame/overview.rs:44:5` `frame::overview::Overview::of` — it can be `pub(crate)`
269. **nit** — `crates/editor-aqua/src/frame/overview.rs:86:1` `frame::overview::card` — it can be `pub(crate)`
270. **nit** — `crates/editor-aqua/src/frame/popup.rs:16:5` `frame::popup::Popup::sync` — it can be `pub(crate)`
271. **nit** — `crates/editor-aqua/src/frame/popup.rs:59:5` `frame::popup::Popup::advance` — it can be `pub(crate)`
272. **nit** — `crates/editor-aqua/src/frame/popup.rs:71:5` `frame::popup::Popup::dismiss` — it can be `pub(crate)`
273. **nit** — `crates/editor-aqua/src/frame/popup.rs:82:5` `frame::popup::Popup::dispose` — it can be `pub(crate)`
274. **nit** — `crates/editor-aqua/src/frame/projects.rs:21:1` `frame::projects::title_of` — it can be `pub(crate)`
275. **nit** — `crates/editor-aqua/src/frame/projects.rs:83:1` `frame::projects::ROW_HEIGHT` — it can be `pub(crate)`
276. **nit** — `crates/editor-aqua/src/frame/projects.rs:84:1` `frame::projects::ROW_GAP` — it can be `pub(crate)`
277. **nit** — `crates/editor-aqua/src/frame/projects.rs:88:5` `frame::projects::Section::project` — it can be `pub(crate)`
278. **nit** — `crates/editor-aqua/src/frame/projects.rs:115:5` `frame::projects::Metrics::body_height` — it can be `pub(crate)`
279. **nit** — `crates/editor-aqua/src/frame/scramble.rs:1:1` `frame::scramble::GLYPHS` — it can be `pub(crate)`
280. **nit** — `crates/editor-aqua/src/frame/scramble.rs:4:1` `frame::scramble::STAGGER_MS` — it can be `pub(crate)`
281. **nit** — `crates/editor-aqua/src/frame/scramble.rs:5:1` `frame::scramble::SETTLE_MS` — it can be `pub(crate)`
282. **nit** — `crates/editor-aqua/src/frame/scramble.rs:6:1` `frame::scramble::TICK_MS` — it can be `pub(crate)`
283. **nit** — `crates/editor-aqua/src/frame/scramble.rs:13:5` `frame::scramble::Cell::from` — it can be `pub(crate)`
284. **nit** — `crates/editor-aqua/src/frame/scramble.rs:14:5` `frame::scramble::Cell::to` — it can be `pub(crate)`
285. **nit** — `crates/editor-aqua/src/frame/scramble.rs:55:1` `frame::scramble::glyph_at` — it can be `pub(crate)`
286. **nit** — `crates/editor-aqua/src/frame/scramble.rs:64:1` `frame::scramble::cell_char` — it can be `pub(crate)`
287. **nit** — `crates/editor-aqua/src/frame/seam.rs:17:1` `frame::seam::ACTIVE` — it can be `pub(crate)`
288. **nit** — `crates/editor-aqua/src/frame/seam.rs:18:1` `frame::seam::SHEET` — it can be `pub(crate)`
289. **nit** — `crates/editor-aqua/src/frame/seam.rs:20:1` `frame::seam::surface` — it can be `pub(crate)`
290. **nit** — `crates/editor-aqua/src/frame/seam/motion.rs:78:5` `frame::seam::motion::Motion::destination` — it can be `pub(crate)`
291. **nit** — `crates/editor-aqua/src/frame/search.rs:17:1` `frame::search::OVERLAY` — it can be `pub(crate)`
292. **nit** — `crates/editor-aqua/src/frame/search.rs:18:1` `frame::search::QUERY` — it can be `pub(crate)`
293. **nit** — `crates/editor-aqua/src/frame/search.rs:19:1` `frame::search::RAIL_KEY` — it can be `pub(crate)`
294. **nit** — `crates/editor-aqua/src/frame/search.rs:28:1` `frame::search::bar` — it can be `pub(crate)`
295. **nit** — `crates/editor-aqua/src/frame/search.rs:70:1` `frame::search::rail` — it can be `pub(crate)`
296. **nit** — `crates/editor-aqua/src/frame/search.rs:125:1` `frame::search::rail_width` — it can be `pub(crate)`
297. **nit** — `crates/editor-aqua/src/frame/search.rs:131:5` `frame::search::Anchor::left` — it can be `pub(crate)`
298. **nit** — `crates/editor-aqua/src/frame/search.rs:132:5` `frame::search::Anchor::top` — it can be `pub(crate)`
299. **nit** — `crates/editor-aqua/src/frame/search.rs:133:5` `frame::search::Anchor::width` — it can be `pub(crate)`
300. **nit** — `crates/editor-aqua/src/frame/search.rs:134:5` `frame::search::Anchor::bottom` — it can be `pub(crate)`
301. **nit** — `crates/editor-aqua/src/frame/search.rs:137:1` `frame::search::Overlay` — it can be `pub(crate)`
302. **nit** — `crates/editor-aqua/src/frame/search.rs:138:5` `frame::search::Overlay::progress` — it can be `pub(crate)`
303. **nit** — `crates/editor-aqua/src/frame/search.rs:139:5` `frame::search::Overlay::anchor` — it can be `pub(crate)`
304. **nit** — `crates/editor-aqua/src/frame/search.rs:140:5` `frame::search::Overlay::bar` — it can be `pub(crate)`
305. **nit** — `crates/editor-aqua/src/frame/search.rs:141:5` `frame::search::Overlay::results` — it can be `pub(crate)`
306. **nit** — `crates/editor-aqua/src/frame/search.rs:142:5` `frame::search::Overlay::emit` — it can be `pub(crate)`
307. **nit** — `crates/editor-aqua/src/frame/search.rs:150:1` `frame::search::overlay` — it can be `pub(crate)`
308. **nit** — `crates/editor-aqua/src/frame/sidebar.rs:22:1` `frame::sidebar::ACCORDION` — it can be `pub(crate)`
309. **nit** — `crates/editor-aqua/src/frame/sidebar.rs:153:5` `frame::sidebar::Sidebar::search` — it can be `pub(crate)`
310. **nit** — `crates/editor-aqua/src/frame/sidebar.rs:158:5` `frame::sidebar::Sidebar::searching` — it can be `pub(crate)`
311. **nit** — `crates/editor-aqua/src/frame/sidebar.rs:162:5` `frame::sidebar::Sidebar::take_search_focus` — it can be `pub(crate)`
312. **nit** — `crates/editor-aqua/src/frame/sidebar.rs:182:5` `frame::sidebar::Sidebar::wordmark` — it can be `pub(crate)`
313. **nit** — `crates/editor-aqua/src/frame/sidebar.rs:187:5` `frame::sidebar::Sidebar::menu` — it can be `pub(crate)`
314. **nit** — `crates/editor-aqua/src/frame/sidebar.rs:191:5` `frame::sidebar::Sidebar::take_effects` — it can be `pub(crate)`
315. **nit** — `crates/editor-aqua/src/frame/sidebar.rs:195:5` `frame::sidebar::Sidebar::refresh_due` — it can be `pub(crate)`
316. **nit** — `crates/editor-aqua/src/frame/sidebar.rs:211:5` `frame::sidebar::Sidebar::edges` — it can be `pub(crate)`
317. **nit** — `crates/editor-aqua/src/frame/sidebar.rs:223:5` `frame::sidebar::Sidebar::take_commands` — it can be `pub(crate)`
318. **nit** — `crates/editor-aqua/src/frame/sidebar.rs:245:5` `frame::sidebar::Sidebar::scrolled` — it can be `pub(crate)`
319. **nit** — `crates/editor-aqua/src/frame/sidebar.rs:299:5` `frame::sidebar::Sidebar::resizing` — it can be `pub(crate)`
320. **nit** — `crates/editor-aqua/src/frame/sidebar.rs:304:5` `frame::sidebar::Sidebar::deadline` — it can be `pub(crate)`
321. **nit** — `crates/editor-aqua/src/frame/sign_in/card.rs:24:1` `frame::sign_in::card::WIDTH` — it can be `pub(crate)`
322. **nit** — `crates/editor-aqua/src/frame/sign_in/card.rs:44:1` `frame::sign_in::card::Props` — it can be `pub(crate)`
323. **nit** — `crates/editor-aqua/src/frame/sign_in/card.rs:45:5` `frame::sign_in::card::Props::view` — it can be `pub(crate)`
324. **nit** — `crates/editor-aqua/src/frame/sign_in/card.rs:46:5` `frame::sign_in::card::Props::sign_in` — it can be `pub(crate)`
325. **nit** — `crates/editor-aqua/src/frame/sign_in/card.rs:47:5` `frame::sign_in::card::Props::email` — it can be `pub(crate)`
326. **nit** — `crates/editor-aqua/src/frame/sign_in/card.rs:48:5` `frame::sign_in::card::Props::code` — it can be `pub(crate)`
327. **nit** — `crates/editor-aqua/src/frame/sign_in/card.rs:49:5` `frame::sign_in::card::Props::busy` — it can be `pub(crate)`
328. **nit** — `crates/editor-aqua/src/frame/sign_in/card.rs:50:5` `frame::sign_in::card::Props::width` — it can be `pub(crate)`
329. **nit** — `crates/editor-aqua/src/frame/sign_in/card.rs:51:5` `frame::sign_in::card::Props::now` — it can be `pub(crate)`
330. **nit** — `crates/editor-aqua/src/frame/sign_in/card.rs:52:5` `frame::sign_in::card::Props::emit` — it can be `pub(crate)`
331. **nit** — `crates/editor-aqua/src/frame/sign_in/card.rs:56:1` `frame::sign_in::card::card` — it can be `pub(crate)`
332. **nit** — `crates/editor-aqua/src/frame/sign_in/card.rs:68:1` `frame::sign_in::card::header` — it can be `pub(crate)`
333. **nit** — `crates/editor-aqua/src/frame/sign_in/card.rs:83:1` `frame::sign_in::card::framed` — it can be `pub(crate)`
334. **nit** — `crates/editor-aqua/src/frame/sign_in/card.rs:94:1` `frame::sign_in::card::body` — it can be `pub(crate)`
335. **nit** — `crates/editor-aqua/src/frame/sign_in/card.rs:102:1` `frame::sign_in::card::wordmark` — it can be `pub(crate)`
336. **nit** — `crates/editor-aqua/src/frame/sign_in/card.rs:373:1` `frame::sign_in::card::keyed_field` — it can be `pub(crate)`
337. **nit** — `crates/editor-aqua/src/frame/sign_in/card.rs:468:1` `frame::sign_in::card::key_lip` — it can be `pub(crate)`
338. **nit** — `crates/editor-aqua/src/frame/sign_in/card.rs:473:1` `frame::sign_in::card::field_key` — it can be `pub(crate)`
339. **nit** — `crates/editor-aqua/src/frame/sign_in/card.rs:719:1` `frame::sign_in::card::screen` — it can be `pub(crate)`
340. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:11:1` `frame::sign_in::EMAIL_PAGE` — it can be `pub(crate)`
341. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:12:1` `frame::sign_in::CODE_PAGE` — it can be `pub(crate)`
342. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:13:1` `frame::sign_in::EMAIL_FEEDBACK` — it can be `pub(crate)`
343. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:14:1` `frame::sign_in::CODE_FEEDBACK` — it can be `pub(crate)`
344. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:15:1` `frame::sign_in::ART_BOX` — it can be `pub(crate)`
345. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:16:1` `frame::sign_in::ART_PARAMS` — it can be `pub(crate)`
346. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:24:1` `frame::sign_in::ART_PAINT` — it can be `pub(crate)`
347. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:31:1` `frame::sign_in::PAGE` — it can be `pub(crate)`
348. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:35:1` `frame::sign_in::RESIZE` — it can be `pub(crate)`
349. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:39:1` `frame::sign_in::SWAP` — it can be `pub(crate)`
350. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:43:1` `frame::sign_in::REVEAL` — it can be `pub(crate)`
351. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:65:5` `frame::sign_in::Motion::page` — it can be `pub(crate)`
352. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:66:5` `frame::sign_in::Motion::height` — it can be `pub(crate)`
353. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:67:5` `frame::sign_in::Motion::busy` — it can be `pub(crate)`
354. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:68:5` `frame::sign_in::Motion::email_feedback` — it can be `pub(crate)`
355. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:69:5` `frame::sign_in::Motion::code_feedback` — it can be `pub(crate)`
356. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:70:5` `frame::sign_in::Motion::spin` — it can be `pub(crate)`
357. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:73:1` `frame::sign_in::SPIN_STEPS` — it can be `pub(crate)`
358. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:77:1` `frame::sign_in::spin_step` — it can be `pub(crate)`
359. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:83:5` `frame::sign_in::Sizes::email_page` — it can be `pub(crate)`
360. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:84:5` `frame::sign_in::Sizes::code_page` — it can be `pub(crate)`
361. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:85:5` `frame::sign_in::Sizes::email_feedback` — it can be `pub(crate)`
362. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:86:5` `frame::sign_in::Sizes::code_feedback` — it can be `pub(crate)`
363. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:90:5` `frame::sign_in::SignIn::problem` — it can be `pub(crate)`
364. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:91:5` `frame::sign_in::SignIn::reduced` — it can be `pub(crate)`
365. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:92:5` `frame::sign_in::SignIn::motion` — it can be `pub(crate)`
366. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:129:5` `frame::sign_in::SignIn::take_events` — it can be `pub(crate)`
367. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:134:5` `frame::sign_in::SignIn::art` — it can be `pub(crate)`
368. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:138:5` `frame::sign_in::SignIn::ensure_art` — it can be `pub(crate)`
369. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:160:5` `frame::sign_in::SignIn::height` — it can be `pub(crate)`
370. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:165:5` `frame::sign_in::SignIn::last` — it can be `pub(crate)`
371. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:173:5` `frame::sign_in::SignIn::feedback_height` — it can be `pub(crate)`
372. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:180:5` `frame::sign_in::SignIn::present` — it can be `pub(crate)`
373. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:223:5` `frame::sign_in::SignIn::measure` — it can be `pub(crate)`
374. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:255:5` `frame::sign_in::SignIn::animating` — it can be `pub(crate)`
375. **nit** — `crates/editor-aqua/src/frame/sign_in/mod.rs:269:5` `frame::sign_in::SignIn::tick` — it can be `pub(crate)`
376. **nit** — `crates/editor-aqua/src/frame/sign_in/model.rs:52:5` `frame::sign_in::model::Link::path` — it can be `pub(crate)`
377. **nit** — `crates/editor-aqua/src/frame/sign_in/pattern.rs:1:1` `frame::sign_in::pattern::TILE` — it can be `pub(crate)`
378. **nit** — `crates/editor-aqua/src/frame/sign_in/pattern.rs:3:1` `frame::sign_in::pattern::COVERAGE` — it can be `pub(crate)`
379. **nit** — `crates/editor-aqua/src/frame/strip.rs:17:1` `frame::strip::ROW` — it can be `pub(crate)`
380. **nit** — `crates/editor-aqua/src/frame/strip.rs:18:1` `frame::strip::HEIGHT` — it can be `pub(crate)`
381. **nit** — `crates/editor-aqua/src/frame/strip.rs:22:1` `frame::strip::BAR_HEIGHT` — it can be `pub(crate)`
382. **nit** — `crates/editor-aqua/src/frame/strip.rs:28:1` `frame::strip::OUTLINE_EDGE` — it can be `pub(crate)`
383. **nit** — `crates/editor-aqua/src/frame/strip.rs:263:1` `frame::strip::flip` — it can be `pub(crate)`
384. **nit** — `crates/editor-aqua/src/frame/view.rs:15:1` `frame::view::SHEET_PADDING` — it can be `pub(crate)`
385. **nit** — `crates/editor-aqua/src/frame/view.rs:16:1` `frame::view::SIDEBAR` — it can be `pub(crate)`
386. **nit** — `crates/editor-aqua/src/frame/view.rs:18:1` `frame::view::WORDMARK_FAMILY` — it can be `pub(crate)`
387. **nit** — `crates/editor-aqua/src/frame/view.rs:19:1` `frame::view::WORDMARK_SIZE` — it can be `pub(crate)`
388. **nit** — `crates/editor-aqua/src/frame/view.rs:20:1` `frame::view::WORDMARK_WIDTH` — it can be `pub(crate)`
389. **nit** — `crates/editor-aqua/src/frame/view.rs:21:1` `frame::view::WORDMARK_TRACKING` — it can be `pub(crate)`
390. **nit** — `crates/editor-aqua/src/frame/view.rs:24:1` `frame::view::INK_X` — it can be `pub(crate)`
391. **nit** — `crates/editor-aqua/src/frame/view.rs:25:1` `frame::view::FOOTER_LEFT` — it can be `pub(crate)`
392. **nit** — `crates/editor-aqua/src/frame/view.rs:29:1` `frame::view::WORDMARK_INK_LEFT` — it can be `pub(crate)`
393. **nit** — `crates/editor-aqua/src/frame/view.rs:33:1` `frame::view::Sidebar` — it can be `pub(crate)`
394. **nit** — `crates/editor-aqua/src/frame/view.rs:34:5` `frame::view::Sidebar::width` — it can be `pub(crate)`
395. **nit** — `crates/editor-aqua/src/frame/view.rs:35:5` `frame::view::Sidebar::open` — it can be `pub(crate)`
396. **nit** — `crates/editor-aqua/src/frame/view.rs:36:5` `frame::view::Sidebar::wordmark` — it can be `pub(crate)`
397. **nit** — `crates/editor-aqua/src/frame/view.rs:37:5` `frame::view::Sidebar::emit` — it can be `pub(crate)`
398. **nit** — `crates/editor-aqua/src/frame/view.rs:38:5` `frame::view::Sidebar::footer` — it can be `pub(crate)`
399. **nit** — `crates/editor-aqua/src/frame/view.rs:39:5` `frame::view::Sidebar::search` — it can be `pub(crate)`
400. **nit** — `crates/editor-aqua/src/frame/view.rs:40:5` `frame::view::Sidebar::tree` — it can be `pub(crate)`
401. **nit** — `crates/editor-aqua/src/frame/view.rs:44:1` `frame::view::body` — it can be `pub(crate)`
402. **nit** — `crates/editor-aqua/src/frame/view.rs:82:1` `frame::view::search_anchor` — it can be `pub(crate)`
403. **nit** — `crates/editor-aqua/src/frame/view.rs:108:1` `frame::view::sidebar_view` — it can be `pub(crate)`
404. **nit** — `crates/editor-aqua/src/frame/wordmark.rs:4:1` `frame::wordmark::REST` — it can be `pub(crate)`
405. **nit** — `crates/editor-aqua/src/frame/wordmark.rs:5:1` `frame::wordmark::SWAP` — it can be `pub(crate)`
406. **nit** — `crates/editor-aqua/src/frame/wordmark.rs:6:1` `frame::wordmark::REST_SQUEEZED` — it can be `pub(crate)`
407. **nit** — `crates/editor-aqua/src/frame/wordmark.rs:7:1` `frame::wordmark::LINE_HEIGHTS` — it can be `pub(crate)`
408. **nit** — `crates/editor-aqua/src/logging.rs:8:1` `logging::file` — it can be `pub(crate)`
409. **nit** — `crates/editor-aqua/src/logging.rs:28:1` `logging::init` — it can be `pub(crate)`
410. **nit** — `crates/editor-aqua/src/setup/mod.rs:11:1` `setup::Outcome` — it can be `pub(crate)`
411. **nit** — `crates/editor-aqua/src/setup/mod.rs:34:1` `setup::begin` — it can be `pub(crate)`
412. **nit** — `crates/editor-aqua/src/setup/model.rs:3:1` `setup::model::FOLDER` — it can be `pub(crate)`
413. **nit** — `crates/editor-aqua/src/setup/model.rs:24:1` `setup::model::same_program` — it can be `pub(crate)`
414. **nit** — `crates/editor-aqua/src/setup/model.rs:65:5` `setup::model::Machine::title` — it can be `pub(crate)`
415. **nit** — `crates/editor-aqua/src/setup/model.rs:73:5` `setup::model::Machine::detail` — it can be `pub(crate)`
416. **nit** — `crates/editor-aqua/src/setup/model.rs:123:5` `setup::model::Setup::version` — it can be `pub(crate)`
417. **nit** — `crates/editor-aqua/src/setup/model.rs:124:5` `setup::model::Setup::default_location` — it can be `pub(crate)`
418. **nit** — `crates/editor-aqua/src/setup/model.rs:156:5` `setup::model::Setup::arguments` — it can be `pub(crate)`
419. **nit** — `crates/editor-aqua/src/setup/view.rs:18:1` `setup::view::SPIN_STEPS` — it can be `pub(crate)`
420. **nit** — `crates/editor-aqua/src/shell/application.rs:238:5` `shell::application::ShellApplication::<V>::opening` — it can be `pub(crate)`
421. **nit** — `crates/editor-aqua/src/shell/application.rs:286:5` `shell::application::ShellApplication::<V>::fitting_launch_to_display` — it can be `pub(crate)`
422. **nit** — `crates/editor-aqua/src/shell/browser.rs:82:5` `shell::browser::Browser::search` — it can be `pub(crate)`
423. **nit** — `crates/editor-aqua/src/shell/browser.rs:103:1` `shell::browser::icon` — it can be `pub(crate)`
424. **nit** — `crates/editor-aqua/src/shell/composer_glide.rs:9:1` `shell::composer_glide::DOCK_KEY` — it can be `pub(crate)`
425. **nit** — `crates/editor-aqua/src/shell/composer_glide.rs:10:1` `shell::composer_glide::STAGE_KEY` — it can be `pub(crate)`
426. **nit** — `crates/editor-aqua/src/shell/composer_glide.rs:52:1` `shell::composer_glide::Route` — it can be `pub(crate)`
427. **nit** — `crates/editor-aqua/src/shell/composer_glide.rs:63:5` `shell::composer_glide::Synced::moving` — it can be `pub(crate)`
428. **nit** — `crates/editor-aqua/src/shell/composer_glide.rs:64:5` `shell::composer_glide::Synced::sky_gone` — it can be `pub(crate)`
429. **nit** — `crates/editor-aqua/src/shell/composer_glide.rs:68:5` `shell::composer_glide::ComposerGlide::follow` — it can be `pub(crate)`
430. **nit** — `crates/editor-aqua/src/shell/composer_glide.rs:90:5` `shell::composer_glide::ComposerGlide::new_thread` — it can be `pub(crate)`
431. **nit** — `crates/editor-aqua/src/shell/composer_glide.rs:102:5` `shell::composer_glide::ComposerGlide::awaits` — it can be `pub(crate)`
432. **nit** — `crates/editor-aqua/src/shell/composer_glide.rs:157:5` `shell::composer_glide::ComposerGlide::leave` — it can be `pub(crate)`
433. **nit** — `crates/editor-aqua/src/shell/composer_glide.rs:168:5` `shell::composer_glide::ComposerGlide::launched` — it can be `pub(crate)`
434. **nit** — `crates/editor-aqua/src/shell/composer_glide.rs:173:5` `shell::composer_glide::ComposerGlide::shift` — it can be `pub(crate)`
435. **nit** — `crates/editor-aqua/src/shell/composer_glide.rs:178:5` `shell::composer_glide::ComposerGlide::transform` — it can be `pub(crate)`
436. **nit** — `crates/editor-aqua/src/shell/composer_glide.rs:183:5` `shell::composer_glide::ComposerGlide::dusk` — it can be `pub(crate)`
437. **nit** — `crates/editor-aqua/src/shell/composer_glide.rs:188:5` `shell::composer_glide::ComposerGlide::sky_shown` — it can be `pub(crate)`
438. **nit** — `crates/editor-aqua/src/shell/composer_glide.rs:193:5` `shell::composer_glide::ComposerGlide::sky_setting` — it can be `pub(crate)`
439. **nit** — `crates/editor-aqua/src/shell/composer_glide.rs:198:5` `shell::composer_glide::ComposerGlide::sky_opacity` — it can be `pub(crate)`
440. **nit** — `crates/editor-aqua/src/shell/composer_glide.rs:203:5` `shell::composer_glide::ComposerGlide::sky_transform` — it can be `pub(crate)`
441. **nit** — `crates/editor-aqua/src/shell/composer_glide.rs:208:5` `shell::composer_glide::ComposerGlide::sync` — it can be `pub(crate)`
442. **nit** — `crates/editor-aqua/src/shell/connection.rs:29:5` `shell::connection::WordmarkLoader::reset` — it can be `pub(crate)`
443. **nit** — `crates/editor-aqua/src/shell/connection.rs:33:5` `shell::connection::WordmarkLoader::build` — it can be `pub(crate)`
444. **nit** — `crates/editor-aqua/src/shell/connection.rs:98:5` `shell::connection::WordmarkLoader::deadline` — it can be `pub(crate)`
445. **nit** — `crates/editor-aqua/src/shell/connection.rs:107:5` `shell::connection::WordmarkLoader::animating` — it can be `pub(crate)`
446. **nit** — `crates/editor-aqua/src/shell/connection.rs:124:5` `shell::connection::ConnectionPresenter::reset` — it can be `pub(crate)`
447. **nit** — `crates/editor-aqua/src/shell/connection.rs:131:5` `shell::connection::ConnectionPresenter::waiting` — it can be `pub(crate)`
448. **nit** — `crates/editor-aqua/src/shell/connection.rs:136:5` `shell::connection::ConnectionPresenter::dispatch` — it can be `pub(crate)`
449. **nit** — `crates/editor-aqua/src/shell/connection.rs:146:5` `shell::connection::ConnectionPresenter::build` — it can be `pub(crate)`
450. **nit** — `crates/editor-aqua/src/shell/connection.rs:297:5` `shell::connection::ConnectionPresenter::deadline` — it can be `pub(crate)`
451. **nit** — `crates/editor-aqua/src/shell/connection.rs:308:5` `shell::connection::ConnectionPresenter::animating` — it can be `pub(crate)`
452. **nit** — `crates/editor-aqua/src/shell/context_meter.rs:3:5` `shell::context_meter::ContextReading::model_name` — it can be `pub(crate)`
453. **nit** — `crates/editor-aqua/src/shell/context_meter.rs:4:5` `shell::context_meter::ContextReading::context_tokens` — it can be `pub(crate)`
454. **nit** — `crates/editor-aqua/src/shell/context_meter.rs:5:5` `shell::context_meter::ContextReading::window_tokens` — it can be `pub(crate)`
455. **nit** — `crates/editor-aqua/src/shell/context_meter.rs:11:5` `shell::context_meter::ContextReading::new` — it can be `pub(crate)`
456. **nit** — `crates/editor-aqua/src/shell/context_meter.rs:19:5` `shell::context_meter::ContextReading::percent` — it can be `pub(crate)`
457. **nit** — `crates/editor-aqua/src/shell/context_meter.rs:27:5` `shell::context_meter::ContextReading::percent_sentence` — it can be `pub(crate)`
458. **nit** — `crates/editor-aqua/src/shell/context_meter.rs:35:5` `shell::context_meter::ContextReading::capacity_sentence` — it can be `pub(crate)`
459. **nit** — `crates/editor-aqua/src/shell/context_meter.rs:44:1` `shell::context_meter::compact_tokens` — it can be `pub(crate)`
460. **nit** — `crates/editor-aqua/src/shell/context_meter.rs:61:1` `shell::context_meter::reading` — it can be `pub(crate)`
461. **nit** — `crates/editor-aqua/src/shell/entry.rs:33:5` `shell::entry::Entry::emit` — it can be `pub(crate)`
462. **nit** — `crates/editor-aqua/src/shell/entry.rs:38:5` `shell::entry::Entry::bind` — it can be `pub(crate)`
463. **nit** — `crates/editor-aqua/src/shell/entry.rs:152:5` `shell::entry::Entry::drain` — it can be `pub(crate)`
464. **nit** — `crates/editor-aqua/src/shell/entry.rs:156:5` `shell::entry::Entry::view` — it can be `pub(crate)`
465. **nit** — `crates/editor-aqua/src/shell/entry.rs:227:5` `shell::entry::Entry::sync_editors` — it can be `pub(crate)`
466. **nit** — `crates/editor-aqua/src/shell/entry.rs:265:5` `shell::entry::Entry::land` — it can be `pub(crate)`
467. **nit** — `crates/editor-aqua/src/shell/entry.rs:283:5` `shell::entry::Entry::finish_events` — it can be `pub(crate)`
468. **nit** — `crates/editor-aqua/src/shell/entry.rs:289:5` `shell::entry::Entry::handle_event` — it can be `pub(crate)`
469. **nit** — `crates/editor-aqua/src/shell/host.rs:11:5` `shell::host::Shell::titlebar` — it can be `pub(crate)`
470. **nit** — `crates/editor-aqua/src/shell/host.rs:15:5` `shell::host::Shell::audio` — it can be `pub(crate)`
471. **nit** — `crates/editor-aqua/src/shell/host.rs:16:5` `shell::host::Shell::audio_assets` — it can be `pub(crate)`
472. **nit** — `crates/editor-aqua/src/shell/host.rs:17:5` `shell::host::Shell::models` — it can be `pub(crate)`
473. **nit** — `crates/editor-aqua/src/shell/host.rs:18:5` `shell::host::Shell::meshes_supported` — it can be `pub(crate)`
474. **nit** — `crates/editor-aqua/src/shell/host.rs:19:5` `shell::host::Shell::meshes_changed` — it can be `pub(crate)`
475. **nit** — `crates/editor-aqua/src/shell/host.rs:21:5` `shell::host::Shell::live` — it can be `pub(crate)`
476. **nit** — `crates/editor-aqua/src/shell/host.rs:26:5` `shell::host::ShellUpdates::notifications` — it can be `pub(crate)`
477. **nit** — `crates/editor-aqua/src/shell/host.rs:27:5` `shell::host::ShellUpdates::preferences_changed` — it can be `pub(crate)`
478. **nit** — `crates/editor-aqua/src/shell/host.rs:28:5` `shell::host::ShellUpdates::theme_transition` — it can be `pub(crate)`
479. **nit** — `crates/editor-aqua/src/shell/host.rs:29:5` `shell::host::ShellUpdates::settings_platform_effects` — it can be `pub(crate)`
480. **nit** — `crates/editor-aqua/src/shell/host.rs:30:5` `shell::host::ShellUpdates::changed` — it can be `pub(crate)`
481. **nit** — `crates/editor-aqua/src/shell/host.rs:31:5` `shell::host::ShellUpdates::live_only` — it can be `pub(crate)`
482. **nit** — `crates/editor-aqua/src/shell/host.rs:39:5` `shell::host::Shell::opening` — it can be `pub(crate)`
483. **nit** — `crates/editor-aqua/src/shell/host.rs:103:5` `shell::host::Shell::bind` — it can be `pub(crate)`
484. **nit** — `crates/editor-aqua/src/shell/host.rs:177:5` `shell::host::Shell::apply_preferences` — it can be `pub(crate)`
485. **nit** — `crates/editor-aqua/src/shell/host.rs:185:5` `shell::host::Shell::drain` — it can be `pub(crate)`
486. **nit** — `crates/editor-aqua/src/shell/host.rs:296:5` `shell::host::Shell::open_palette` — it can be `pub(crate)`
487. **nit** — `crates/editor-aqua/src/shell/host.rs:303:5` `shell::host::Shell::open_reference` — it can be `pub(crate)`
488. **nit** — `crates/editor-aqua/src/shell/host.rs:317:5` `shell::host::Shell::dismiss_reference` — it can be `pub(crate)`
489. **nit** — `crates/editor-aqua/src/shell/host.rs:323:5` `shell::host::Shell::dismiss_palette` — it can be `pub(crate)`
490. **nit** — `crates/editor-aqua/src/shell/host.rs:335:5` `shell::host::Shell::new_thread` — it can be `pub(crate)`
491. **nit** — `crates/editor-aqua/src/shell/host.rs:338:5` `shell::host::Shell::go_back` — it can be `pub(crate)`
492. **nit** — `crates/editor-aqua/src/shell/host.rs:341:5` `shell::host::Shell::open_settings` — it can be `pub(crate)`
493. **nit** — `crates/editor-aqua/src/shell/host.rs:344:5` `shell::host::Shell::navigate` — it can be `pub(crate)`
494. **nit** — `crates/editor-aqua/src/shell/host.rs:370:5` `shell::host::ShellResources::for_runtime` — it can be `pub(crate)`
495. **nit** — `crates/editor-aqua/src/shell/host.rs:389:5` `shell::host::ShellResources::with_color_profile` — it can be `pub(crate)`
496. **nit** — `crates/editor-aqua/src/shell/host.rs:396:5` `shell::host::ShellResources::into_managed` — it can be `pub(crate)`
497. **nit** — `crates/editor-aqua/src/shell/host.rs:401:1` `shell::host::set_window_scale` — it can be `pub(crate)`
498. **nit** — `crates/editor-aqua/src/shell/layout.rs:2:1` `shell::layout::COLUMN_WIDTH` — it can be `pub(crate)`
499. **nit** — `crates/editor-aqua/src/shell/layout.rs:3:1` `shell::layout::COLUMN_MIN_WIDTH` — it can be `pub(crate)`
500. **nit** — `crates/editor-aqua/src/shell/layout.rs:4:1` `shell::layout::CHAT_MIN_WIDTH` — it can be `pub(crate)`
501. **nit** — `crates/editor-aqua/src/shell/layout.rs:5:1` `shell::layout::MIN_WINDOW_WIDTH` — it can be `pub(crate)`
502. **nit** — `crates/editor-aqua/src/shell/layout.rs:6:1` `shell::layout::MIN_WINDOW_HEIGHT` — it can be `pub(crate)`
503. **nit** — `crates/editor-aqua/src/shell/layout.rs:14:1` `shell::layout::launch_size` — it can be `pub(crate)`
504. **nit** — `crates/editor-aqua/src/shell/layout.rs:26:1` `shell::layout::sidebar_width` — it can be `pub(crate)`
505. **nit** — `crates/editor-aqua/src/shell/layout.rs:31:1` `shell::layout::inspector_width` — it can be `pub(crate)`
506. **nit** — `crates/editor-aqua/src/shell/layout.rs:36:1` `shell::layout::settings_sidebar_width` — it can be `pub(crate)`
507. **nit** — `crates/editor-aqua/src/shell/layout.rs:42:5` `shell::layout::SettingsLayout::size` — it can be `pub(crate)`
508. **nit** — `crates/editor-aqua/src/shell/layout.rs:43:5` `shell::layout::SettingsLayout::navigation_width` — it can be `pub(crate)`
509. **nit** — `crates/editor-aqua/src/shell/layout.rs:46:1` `shell::layout::settings_layout` — it can be `pub(crate)`
510. **nit** — `crates/editor-aqua/src/shell/layout.rs:66:5` `shell::layout::EntryLayout::width` — it can be `pub(crate)`
511. **nit** — `crates/editor-aqua/src/shell/layout.rs:67:5` `shell::layout::EntryLayout::max_height` — it can be `pub(crate)`
512. **nit** — `crates/editor-aqua/src/shell/layout.rs:81:1` `shell::layout::RAIL_WIDTH` — it can be `pub(crate)`
513. **nit** — `crates/editor-aqua/src/shell/layout.rs:82:1` `shell::layout::CONTROL_WIDTH` — it can be `pub(crate)`
514. **nit** — `crates/editor-aqua/src/shell/layout.rs:83:1` `shell::layout::CONTROL_GLYPH` — it can be `pub(crate)`
515. **nit** — `crates/editor-aqua/src/shell/layout.rs:84:1` `shell::layout::CONTROLS_WIDTH` — it can be `pub(crate)`
516. **nit** — `crates/editor-aqua/src/shell/layout.rs:85:1` `shell::layout::CONTROL_INSET_X` — it can be `pub(crate)`
517. **nit** — `crates/editor-aqua/src/shell/layout.rs:86:1` `shell::layout::CONTROL_INK_INSET` — it can be `pub(crate)`
518. **nit** — `crates/editor-aqua/src/shell/layout.rs:90:5` `shell::layout::WorkspaceLayout::sidebar` — it can be `pub(crate)`
519. **nit** — `crates/editor-aqua/src/shell/layout.rs:91:5` `shell::layout::WorkspaceLayout::frame_width` — it can be `pub(crate)`
520. **nit** — `crates/editor-aqua/src/shell/layout.rs:92:5` `shell::layout::WorkspaceLayout::inspector` — it can be `pub(crate)`
521. **nit** — `crates/editor-aqua/src/shell/layout.rs:93:5` `shell::layout::WorkspaceLayout::meter_right` — it can be `pub(crate)`
522. **nit** — `crates/editor-aqua/src/shell/layout.rs:94:5` `shell::layout::WorkspaceLayout::header_right_inset` — it can be `pub(crate)`
523. **nit** — `crates/editor-aqua/src/shell/layout.rs:97:1` `shell::layout::workspace_layout` — it can be `pub(crate)`
524. **nit** — `crates/editor-aqua/src/shell/layout.rs:128:1` `shell::layout::meter_right` — it can be `pub(crate)`
525. **nit** — `crates/editor-aqua/src/shell/layout.rs:132:1` `shell::layout::header_right_inset` — it can be `pub(crate)`
526. **nit** — `crates/editor-aqua/src/shell/native_notifications.rs:51:5` `shell::native_notifications::NativeNotifications::new` — it can be `pub(crate)`
527. **nit** — `crates/editor-aqua/src/shell/native_notifications.rs:73:5` `shell::native_notifications::NativeNotifications::has_pending` — it can be `pub(crate)`
528. **nit** — `crates/editor-aqua/src/shell/native_notifications.rs:81:5` `shell::native_notifications::NativeNotifications::bind` — it can be `pub(crate)`
529. **nit** — `crates/editor-aqua/src/shell/native_notifications.rs:85:5` `shell::native_notifications::NativeNotifications::apply` — it can be `pub(crate)`
530. **nit** — `crates/editor-aqua/src/shell/native_notifications.rs:124:5` `shell::native_notifications::NativeNotifications::drain` — it can be `pub(crate)`
531. **nit** — `crates/editor-aqua/src/shell/notifications.rs:29:5` `shell::notifications::Notifications::reset` — it can be `pub(crate)`
532. **nit** — `crates/editor-aqua/src/shell/notifications.rs:42:5` `shell::notifications::Notifications::sync_connection` — it can be `pub(crate)`
533. **nit** — `crates/editor-aqua/src/shell/notifications.rs:48:5` `shell::notifications::Notifications::retain_hosts` — it can be `pub(crate)`
534. **nit** — `crates/editor-aqua/src/shell/notifications.rs:61:5` `shell::notifications::Notifications::withdraw` — it can be `pub(crate)`
535. **nit** — `crates/editor-aqua/src/shell/notifications.rs:78:5` `shell::notifications::Notifications::observe` — it can be `pub(crate)`
536. **nit** — `crates/editor-aqua/src/shell/notifications.rs:147:1` `shell::notifications::tag` — it can be `pub(crate)`
537. **nit** — `crates/editor-aqua/src/shell/notifications.rs:151:1` `shell::notifications::parse_tag` — it can be `pub(crate)`
538. **nit** — `crates/editor-aqua/src/shell/notifications.rs:156:1` `shell::notifications::NotificationChanges` — it can be `pub(crate)`
539. **nit** — `crates/editor-aqua/src/shell/notifications.rs:157:5` `shell::notifications::NotificationChanges::withdraw` — it can be `pub(crate)`
540. **nit** — `crates/editor-aqua/src/shell/notifications.rs:158:5` `shell::notifications::NotificationChanges::show` — it can be `pub(crate)`
541. **nit** — `crates/editor-aqua/src/shell/notifications.rs:162:5` `shell::notifications::Notifications::thread_changed` — it can be `pub(crate)`
542. **nit** — `crates/editor-aqua/src/shell/notifications.rs:177:5` `shell::notifications::Notifications::host_reset` — it can be `pub(crate)`
543. **nit** — `crates/editor-aqua/src/shell/palette.rs:17:5` `shell::palette::Entry::label` — it can be `pub(crate)`
544. **nit** — `crates/editor-aqua/src/shell/palette.rs:18:5` `shell::palette::Entry::detail` — it can be `pub(crate)`
545. **nit** — `crates/editor-aqua/src/shell/palette.rs:19:5` `shell::palette::Entry::screen` — it can be `pub(crate)`
546. **nit** — `crates/editor-aqua/src/shell/palette.rs:22:1` `shell::palette::HostSnapshot` — it can be `pub(crate)`
547. **nit** — `crates/editor-aqua/src/shell/palette.rs:23:5` `shell::palette::HostSnapshot::id` — it can be `pub(crate)`
548. **nit** — `crates/editor-aqua/src/shell/palette.rs:24:5` `shell::palette::HostSnapshot::name` — it can be `pub(crate)`
549. **nit** — `crates/editor-aqua/src/shell/palette.rs:25:5` `shell::palette::HostSnapshot::instance` — it can be `pub(crate)`
550. **nit** — `crates/editor-aqua/src/shell/palette.rs:26:5` `shell::palette::HostSnapshot::threads` — it can be `pub(crate)`
551. **nit** — `crates/editor-aqua/src/shell/palette.rs:27:5` `shell::palette::HostSnapshot::projects` — it can be `pub(crate)`
552. **nit** — `crates/editor-aqua/src/shell/palette.rs:30:1` `shell::palette::SearchHost` — it can be `pub(crate)`
553. **nit** — `crates/editor-aqua/src/shell/palette.rs:31:5` `shell::palette::SearchHost::id` — it can be `pub(crate)`
554. **nit** — `crates/editor-aqua/src/shell/palette.rs:32:5` `shell::palette::SearchHost::name` — it can be `pub(crate)`
555. **nit** — `crates/editor-aqua/src/shell/palette.rs:33:5` `shell::palette::SearchHost::client` — it can be `pub(crate)`
556. **nit** — `crates/editor-aqua/src/shell/palette.rs:34:5` `shell::palette::SearchHost::instance` — it can be `pub(crate)`
557. **nit** — `crates/editor-aqua/src/shell/palette.rs:53:5` `shell::palette::Palette::query` — it can be `pub(crate)`
558. **nit** — `crates/editor-aqua/src/shell/palette.rs:54:5` `shell::palette::Palette::highlighted` — it can be `pub(crate)`
559. **nit** — `crates/editor-aqua/src/shell/palette.rs:55:5` `shell::palette::Palette::entries` — it can be `pub(crate)`
560. **nit** — `crates/editor-aqua/src/shell/palette.rs:56:5` `shell::palette::Palette::search_error` — it can be `pub(crate)`
561. **nit** — `crates/editor-aqua/src/shell/palette.rs:111:5` `shell::palette::Palette::wake_mailbox` — it can be `pub(crate)`
562. **nit** — `crates/editor-aqua/src/shell/palette.rs:115:5` `shell::palette::Palette::start_search` — it can be `pub(crate)`
563. **nit** — `crates/editor-aqua/src/shell/palette.rs:165:5` `shell::palette::Palette::drain` — it can be `pub(crate)`
564. **nit** — `crates/editor-aqua/src/shell/palette.rs:187:5` `shell::palette::Palette::refresh` — it can be `pub(crate)`
565. **nit** — `crates/editor-aqua/src/shell/palette.rs:321:5` `shell::palette::Palette::up` — it can be `pub(crate)`
566. **nit** — `crates/editor-aqua/src/shell/palette.rs:324:5` `shell::palette::Palette::down` — it can be `pub(crate)`
567. **nit** — `crates/editor-aqua/src/shell/palette.rs:327:5` `shell::palette::Palette::selected` — it can be `pub(crate)`
568. **nit** — `crates/editor-aqua/src/shell/palette/view.rs:43:5` `shell::palette::view::Event::session` — it can be `pub(crate)`
569. **nit** — `crates/editor-aqua/src/shell/palette/view.rs:44:5` `shell::palette::view::Event::action` — it can be `pub(crate)`
570. **nit** — `crates/editor-aqua/src/shell/palette/view.rs:56:5` `shell::palette::view::Outcome::changed` — it can be `pub(crate)`
571. **nit** — `crates/editor-aqua/src/shell/palette/view.rs:57:5` `shell::palette::view::Outcome::effect` — it can be `pub(crate)`
572. **nit** — `crates/editor-aqua/src/shell/palette/view.rs:67:5` `shell::palette::view::PaletteView::new` — it can be `pub(crate)`
573. **nit** — `crates/editor-aqua/src/shell/palette/view.rs:99:5` `shell::palette::view::PaletteView::session` — it can be `pub(crate)`
574. **nit** — `crates/editor-aqua/src/shell/palette/view.rs:103:5` `shell::palette::view::PaletteView::close` — it can be `pub(crate)`
575. **nit** — `crates/editor-aqua/src/shell/palette/view.rs:107:5` `shell::palette::view::PaletteView::focus_query` — it can be `pub(crate)`
576. **nit** — `crates/editor-aqua/src/shell/palette/view.rs:111:5` `shell::palette::view::PaletteView::sync_query` — it can be `pub(crate)`
577. **nit** — `crates/editor-aqua/src/shell/palette/view.rs:121:5` `shell::palette::view::PaletteView::dispatch` — it can be `pub(crate)`
578. **nit** — `crates/editor-aqua/src/shell/palette/view.rs:177:5` `shell::palette::view::PaletteView::content` — it can be `pub(crate)`
579. **nit** — `crates/editor-aqua/src/shell/refresh.rs:14:5` `shell::refresh::Refresh::merge` — it can be `pub(crate)`
580. **nit** — `crates/editor-aqua/src/shell/repository.rs:20:5` `shell::repository::RepositoryContext::reset` — it can be `pub(crate)`
581. **nit** — `crates/editor-aqua/src/shell/repository.rs:24:5` `shell::repository::RepositoryContext::sync` — it can be `pub(crate)`
582. **nit** — `crates/editor-aqua/src/shell/resource_preview.rs:5:5` `shell::resource_preview::PreviewText::text` — it can be `pub(crate)`
583. **nit** — `crates/editor-aqua/src/shell/resource_preview.rs:6:5` `shell::resource_preview::PreviewText::start_line` — it can be `pub(crate)`
584. **nit** — `crates/editor-aqua/src/shell/resource_preview.rs:7:5` `shell::resource_preview::PreviewText::offsets` — it can be `pub(crate)`
585. **nit** — `crates/editor-aqua/src/shell/resource_preview.rs:8:5` `shell::resource_preview::PreviewText::total_lines` — it can be `pub(crate)`
586. **nit** — `crates/editor-aqua/src/shell/resource_preview.rs:12:5` `shell::resource_preview::PreviewText::new` — it can be `pub(crate)`
587. **nit** — `crates/editor-aqua/src/shell/resource_preview.rs:40:5` `shell::resource_preview::PreviewText::note` — it can be `pub(crate)`
588. **nit** — `crates/editor-aqua/src/shell/resource_preview.rs:51:5` `shell::resource_preview::ResourcePreview::path` — it can be `pub(crate)`
589. **nit** — `crates/editor-aqua/src/shell/resource_preview.rs:52:5` `shell::resource_preview::ResourcePreview::target_line` — it can be `pub(crate)`
590. **nit** — `crates/editor-aqua/src/shell/resource_preview/view.rs:48:5` `shell::resource_preview::view::ViewState::new` — it can be `pub(crate)`
591. **nit** — `crates/editor-aqua/src/shell/resource_preview/view.rs:58:5` `shell::resource_preview::view::ViewState::prepare` — it can be `pub(crate)`
592. **nit** — `crates/editor-aqua/src/shell/resource_preview/view.rs:90:5` `shell::resource_preview::view::ViewState::pending_jump` — it can be `pub(crate)`
593. **nit** — `crates/editor-aqua/src/shell/resource_preview/view.rs:94:5` `shell::resource_preview::view::ViewState::sync_jump` — it can be `pub(crate)`
594. **nit** — `crates/editor-aqua/src/shell/resource_preview/view.rs:109:5` `shell::resource_preview::view::ViewState::jumped` — it can be `pub(crate)`
595. **nit** — `crates/editor-aqua/src/shell/resource_preview/view.rs:115:5` `shell::resource_preview::view::ViewState::release` — it can be `pub(crate)`
596. **nit** — `crates/editor-aqua/src/shell/resource_preview/view.rs:122:5` `shell::resource_preview::view::ViewState::content` — it can be `pub(crate)`
597. **nit** — `crates/editor-aqua/src/shell/resource_preview/view.rs:185:5` `shell::resource_preview::view::ViewState::dialog` — it can be `pub(crate)`
598. **nit** — `crates/editor-aqua/src/shell/root.rs:1074:5` `shell::root::RootViews::conversation_callbacks` — it can be `pub(crate)`
599. **nit** — `crates/editor-aqua/src/shell/root.rs:1518:5` `shell::root::RootViews::replace_rows` — it can be `pub(crate)`
600. **nit** — `crates/editor-aqua/src/shell/selection.rs:61:5` `shell::selection::SelectionScope::key` — it can be `pub(crate)`
601. **nit** — `crates/editor-aqua/src/shell/selection.rs:62:5` `shell::selection::SelectionScope::inspector` — it can be `pub(crate)`
602. **nit** — `crates/editor-aqua/src/shell/selection.rs:64:5` `shell::selection::SelectionScope::question` — it can be `pub(crate)`
603. **nit** — `crates/editor-aqua/src/shell/selection.rs:65:5` `shell::selection::SelectionScope::question_options` — it can be `pub(crate)`
604. **nit** — `crates/editor-aqua/src/shell/selection.rs:66:5` `shell::selection::SelectionScope::question_option_detail` — it can be `pub(crate)`
605. **nit** — `crates/editor-aqua/src/shell/selection.rs:71:5` `shell::selection::SelectionScope::source_allocator` — it can be `pub(crate)`
606. **nit** — `crates/editor-aqua/src/shell/selection.rs:76:5` `shell::selection::SelectionScope::next_source` — it can be `pub(crate)`
607. **nit** — `crates/editor-aqua/src/shell/selection.rs:80:5` `shell::selection::SelectionScope::next_document` — it can be `pub(crate)`
608. **nit** — `crates/editor-aqua/src/shell/source.rs:26:5` `shell::source::RowSourceDialogs::bind` — it can be `pub(crate)`
609. **nit** — `crates/editor-aqua/src/shell/source.rs:32:5` `shell::source::RowSourceDialogs::retain` — it can be `pub(crate)`
610. **nit** — `crates/editor-aqua/src/shell/source.rs:39:5` `shell::source::RowSourceDialogs::open` — it can be `pub(crate)`
611. **nit** — `crates/editor-aqua/src/shell/source.rs:63:5` `shell::source::RowSourceDialogs::drain` — it can be `pub(crate)`
612. **nit** — `crates/editor-aqua/src/shell/thread_navigation.rs:38:1` `shell::thread_navigation::active_marker` — it can be `pub(crate)`
613. **nit** — `crates/editor-aqua/src/shell/thread_navigation.rs:87:1` `shell::thread_navigation::LATEST_KEY` — it can be `pub(crate)`
614. **nit** — `crates/editor-aqua/src/shell/thread_navigation.rs:103:5` `shell::thread_navigation::LatestPresence::show` — it can be `pub(crate)`
615. **nit** — `crates/editor-aqua/src/shell/thread_navigation.rs:114:5` `shell::thread_navigation::LatestPresence::value` — it can be `pub(crate)`
616. **nit** — `crates/editor-aqua/src/shell/thread_navigation.rs:119:5` `shell::thread_navigation::LatestPresence::shown` — it can be `pub(crate)`
617. **nit** — `crates/editor-aqua/src/shell/thread_navigation.rs:123:5` `shell::thread_navigation::LatestPresence::sync` — it can be `pub(crate)`
618. **nit** — `crates/editor-aqua/src/shell/thread_navigation.rs:148:1` `shell::thread_navigation::jump_to_latest` — it can be `pub(crate)`
619. **nit** — `crates/editor-aqua/src/shell/thread_navigation.rs:214:1` `shell::thread_navigation::step` — it can be `pub(crate)`
620. **nit** — `crates/editor-aqua/src/shell/thread_navigation.rs:229:1` `shell::thread_navigation::outside` — it can be `pub(crate)`
621. **nit** — `crates/editor-aqua/src/shell/thread_navigation.rs:312:5` `shell::thread_navigation::Frame::changed` — it can be `pub(crate)`
622. **nit** — `crates/editor-aqua/src/shell/thread_navigation.rs:313:5` `shell::thread_navigation::Frame::animating` — it can be `pub(crate)`
623. **nit** — `crates/editor-aqua/src/shell/thread_navigation.rs:328:1` `shell::thread_navigation::OVERLAY_KEY` — it can be `pub(crate)`
624. **nit** — `crates/editor-aqua/src/shell/thread_navigation.rs:337:5` `shell::thread_navigation::ThreadNavigation::toggle` — it can be `pub(crate)`
625. **nit** — `crates/editor-aqua/src/shell/thread_navigation.rs:347:5` `shell::thread_navigation::ThreadNavigation::close` — it can be `pub(crate)`
626. **nit** — `crates/editor-aqua/src/shell/thread_navigation.rs:357:5` `shell::thread_navigation::ThreadNavigation::key` — it can be `pub(crate)`
627. **nit** — `crates/editor-aqua/src/shell/thread_navigation.rs:373:5` `shell::thread_navigation::ThreadNavigation::dismiss` — it can be `pub(crate)`
628. **nit** — `crates/editor-aqua/src/shell/thread_navigation.rs:392:5` `shell::thread_navigation::ThreadNavigation::chevron` — it can be `pub(crate)`
629. **nit** — `crates/editor-aqua/src/shell/thread_navigation.rs:398:5` `shell::thread_navigation::ThreadNavigation::build` — it can be `pub(crate)`
630. **nit** — `crates/editor-aqua/src/shell/thread_navigation.rs:414:5` `shell::thread_navigation::ThreadNavigation::refresh` — it can be `pub(crate)`
631. **nit** — `crates/editor-aqua/src/shell/thread_navigation.rs:504:5` `shell::thread_navigation::ThreadNavigation::sync` — it can be `pub(crate)`
632. **nit** — `crates/editor-aqua/src/shell/thread_navigation.rs:537:5` `shell::thread_navigation::ThreadNavigation::dispose` — it can be `pub(crate)`
633. **nit** — `crates/editor-aqua/src/shell/titlebar.rs:22:1` `shell::titlebar::LEAD` — it can be `pub(crate)`
634. **nit** — `crates/editor-aqua/src/shell/titlebar.rs:23:1` `shell::titlebar::WORDMARK` — it can be `pub(crate)`
635. **nit** — `crates/editor-aqua/src/shell/titlebar.rs:67:5` `shell::titlebar::ContextMeterTooltip::sync` — it can be `pub(crate)`
636. **nit** — `crates/editor-aqua/src/shell/titlebar.rs:155:5` `shell::titlebar::ContextMeterTooltip::dispose` — it can be `pub(crate)`
637. **nit** — `crates/editor-aqua/src/shell/titlebar.rs:166:5` `shell::titlebar::ContextMeterTooltip::reset` — it can be `pub(crate)`
638. **nit** — `crates/editor-aqua/src/shell/titlebar.rs:388:5` `shell::titlebar::Titlebar::regions` — it can be `pub(crate)`
639. **nit** — `crates/editor-aqua/src/shell/titlebar.rs:392:5` `shell::titlebar::Titlebar::take_actions` — it can be `pub(crate)`
640. **nit** — `crates/editor-aqua/src/shell/titlebar.rs:396:5` `shell::titlebar::Titlebar::take_changed` — it can be `pub(crate)`
641. **nit** — `crates/editor-aqua/src/shell/titlebar.rs:400:5` `shell::titlebar::Titlebar::entry` — it can be `pub(crate)`
642. **nit** — `crates/editor-aqua/src/shell/titlebar.rs:412:5` `shell::titlebar::Titlebar::workspace` — it can be `pub(crate)`
643. **nit** — `crates/editor-aqua/src/shell/view.rs:19:1` `shell::view::WorkspaceElements` — it can be `pub(crate)`
644. **nit** — `crates/editor-aqua/src/shell/view.rs:20:5` `shell::view::WorkspaceElements::body` — it can be `pub(crate)`
645. **nit** — `crates/editor-aqua/src/shell/view.rs:21:5` `shell::view::WorkspaceElements::settings_dialog` — it can be `pub(crate)`
646. **nit** — `crates/editor-aqua/src/shell/view.rs:22:5` `shell::view::WorkspaceElements::palette` — it can be `pub(crate)`
647. **nit** — `crates/editor-aqua/src/shell/view.rs:23:5` `shell::view::WorkspaceElements::resource_preview` — it can be `pub(crate)`
648. **nit** — `crates/editor-aqua/src/shell/view.rs:24:5` `shell::view::WorkspaceElements::copy_menu` — it can be `pub(crate)`
649. **nit** — `crates/editor-aqua/src/shell/view.rs:25:5` `shell::view::WorkspaceElements::image_preview` — it can be `pub(crate)`
650. **nit** — `crates/editor-aqua/src/shell/view.rs:122:1` `shell::view::workspace` — it can be `pub(crate)`
651. **nit** — `crates/editor-aqua/src/shell/view.rs:147:1` `shell::view::ComposerParts` — it can be `pub(crate)`
652. **nit** — `crates/editor-aqua/src/shell/view.rs:148:5` `shell::view::ComposerParts::nested` — it can be `pub(crate)`
653. **nit** — `crates/editor-aqua/src/shell/view.rs:149:5` `shell::view::ComposerParts::placeholder` — it can be `pub(crate)`
654. **nit** — `crates/editor-aqua/src/shell/view.rs:150:5` `shell::view::ComposerParts::questionnaires` — it can be `pub(crate)`
655. **nit** — `crates/editor-aqua/src/shell/view.rs:151:5` `shell::view::ComposerParts::attachments` — it can be `pub(crate)`
656. **nit** — `crates/editor-aqua/src/shell/view.rs:152:5` `shell::view::ComposerParts::mention_overlay` — it can be `pub(crate)`
657. **nit** — `crates/editor-aqua/src/shell/view.rs:153:5` `shell::view::ComposerParts::thread_error` — it can be `pub(crate)`
658. **nit** — `crates/editor-aqua/src/shell/view.rs:154:5` `shell::view::ComposerParts::shift` — it can be `pub(crate)`
659. **nit** — `crates/editor-aqua/src/shell/view.rs:209:1` `shell::view::conversation_virtual` — it can be `pub(crate)`
660. **nit** — `crates/editor-aqua/src/shell/view.rs:337:1` `shell::view::conversation_body` — it can be `pub(crate)`
661. **nit** — `crates/editor-aqua/src/shell/view.rs:370:1` `shell::view::centered_body` — it can be `pub(crate)`
662. **nit** — `crates/editor-aqua/src/shell/view.rs:384:1` `shell::view::conversation_edge_style` — it can be `pub(crate)`
663. **nit** — `crates/editor-aqua/src/shell/view.rs:396:1` `shell::view::conversation_load_error` — it can be `pub(crate)`
664. **nit** — `crates/editor-aqua/src/shell/view.rs:413:1` `shell::view::conversation_notice` — it can be `pub(crate)`
665. **nit** — `crates/editor-aqua/src/shell/view.rs:427:1` `shell::view::Sky` — it can be `pub(crate)`
666. **nit** — `crates/editor-aqua/src/shell/view.rs:428:5` `shell::view::Sky::opacity` — it can be `pub(crate)`
667. **nit** — `crates/editor-aqua/src/shell/view.rs:429:5` `shell::view::Sky::transform` — it can be `pub(crate)`
668. **nit** — `crates/editor-aqua/src/shell/view.rs:442:1` `shell::view::new_conversation` — it can be `pub(crate)`
669. **nit** — `crates/editor-aqua/src/shell/view.rs:475:1` `shell::view::under_setting_sky` — it can be `pub(crate)`
670. **nit** — `crates/editor-aqua/src/shell/view.rs:499:1` `shell::view::centered_conversation` — it can be `pub(crate)`
671. **nit** — `crates/editor-aqua/src/shell/view.rs:520:1` `shell::view::home` — it can be `pub(crate)`
672. **nit** — `crates/editor-aqua/src/shell/wheel.rs:6:1` `shell::wheel::THREADS` — it can be `pub(crate)`
673. **nit** — `crates/editor-aqua/src/shell/wheel.rs:8:1` `shell::wheel::PROJECT_TREE` — it can be `pub(crate)`
674. **nit** — `crates/editor-aqua/src/shell/wheel.rs:27:5` `shell::wheel::Advance::moved` — it can be `pub(crate)`
675. **nit** — `crates/editor-aqua/src/shell/wheel.rs:29:5` `shell::wheel::Advance::threads_moved` — it can be `pub(crate)`
676. **nit** — `crates/editor-aqua/src/shell/wheel.rs:30:5` `shell::wheel::Advance::transcript_moved` — it can be `pub(crate)`
677. **nit** — `crates/editor-aqua/src/shell/wheel.rs:59:1` `shell::wheel::hidden_edges` — it can be `pub(crate)`
678. **nit** — `crates/editor-aqua/src/shell/wheel.rs:73:1` `shell::wheel::viewport_height` — it can be `pub(crate)`
679. **nit** — `crates/editor-aqua/src/shell/wheel.rs:207:5` `shell::wheel::WheelGlides::cancel_transcript` — it can be `pub(crate)`
680. **nit** — `crates/editor-aqua/src/shell/workspace.rs:13:5` `shell::workspace::NotificationEffects::withdraw` — it can be `pub(crate)`
681. **nit** — `crates/editor-aqua/src/shell/workspace.rs:14:5` `shell::workspace::NotificationEffects::show` — it can be `pub(crate)`
682. **nit** — `crates/editor-aqua/src/shell/workspace.rs:15:5` `shell::workspace::NotificationEffects::badge` — it can be `pub(crate)`
683. **nit** — `crates/editor-aqua/src/shell/workspace.rs:37:5` `shell::workspace::Workspace::inspector` — it can be `pub(crate)`
684. **nit** — `crates/editor-aqua/src/shell/workspace.rs:38:5` `shell::workspace::Workspace::conversation_selection` — it can be `pub(crate)`
685. **nit** — `crates/editor-aqua/src/shell/workspace.rs:40:5` `shell::workspace::Workspace::status_shimmers` — it can be `pub(crate)`
686. **nit** — `crates/editor-aqua/src/shell/workspace.rs:41:5` `shell::workspace::Workspace::images` — it can be `pub(crate)`
687. **nit** — `crates/editor-aqua/src/shell/workspace.rs:49:5` `shell::workspace::Workspace::sidebar` — it can be `pub(crate)`
688. **nit** — `crates/editor-aqua/src/shell/workspace.rs:50:5` `shell::workspace::Workspace::settings_sidebar` — it can be `pub(crate)`
689. **nit** — `crates/editor-aqua/src/shell/workspace.rs:51:5` `shell::workspace::Workspace::usage` — it can be `pub(crate)`
690. **nit** — `crates/editor-aqua/src/shell/workspace.rs:80:5` `shell::workspace::Workspace::marketplace` — it can be `pub(crate)`
691. **nit** — `crates/editor-aqua/src/shell/workspace.rs:83:5` `shell::workspace::Workspace::settings` — it can be `pub(crate)`
692. **nit** — `crates/editor-aqua/src/shell/workspace.rs:84:5` `shell::workspace::Workspace::home` — it can be `pub(crate)`
693. **nit** — `crates/editor-aqua/src/shell/workspace.rs:92:5` `shell::workspace::Workspace::palette` — it can be `pub(crate)`
694. **nit** — `crates/editor-aqua/src/shell/workspace.rs:93:5` `shell::workspace::Workspace::resource_preview` — it can be `pub(crate)`
695. **nit** — `crates/editor-aqua/src/shell/workspace.rs:96:5` `shell::workspace::Workspace::show_inspector` — it can be `pub(crate)`
696. **nit** — `crates/editor-aqua/src/shell/workspace.rs:97:5` `shell::workspace::Workspace::failure` — it can be `pub(crate)`
697. **nit** — `crates/editor-aqua/src/shell/workspace.rs:320:5` `shell::workspace::Workspace::handle_domain_event` — it can be `pub(crate)`
698. **nit** — `crates/editor-aqua/src/shell/workspace.rs:361:5` `shell::workspace::Workspace::dispose_composer_popups` — it can be `pub(crate)`
699. **nit** — `crates/editor-aqua/src/shell/workspace.rs:375:5` `shell::workspace::Workspace::sync_composer_tooltip` — it can be `pub(crate)`
700. **nit** — `crates/editor-aqua/src/shell/workspace.rs:449:5` `shell::workspace::Workspace::code_action_emit` — it can be `pub(crate)`
701. **nit** — `crates/editor-aqua/src/shell/workspace.rs:456:5` `shell::workspace::Workspace::drain_code_actions` — it can be `pub(crate)`
702. **nit** — `crates/editor-aqua/src/shell/workspace.rs:500:5` `shell::workspace::Workspace::take_code_frame` — it can be `pub(crate)`
703. **nit** — `crates/editor-aqua/src/shell/workspace.rs:508:5` `shell::workspace::Workspace::sync_composer_popups` — it can be `pub(crate)`
704. **nit** — `crates/editor-aqua/src/shell/workspace.rs:625:5` `shell::workspace::Workspace::inspector_emit` — it can be `pub(crate)`
705. **nit** — `crates/editor-aqua/src/shell/workspace.rs:658:5` `shell::workspace::Workspace::sync_terminal_dialog` — it can be `pub(crate)`
706. **nit** — `crates/editor-aqua/src/shell/workspace.rs:703:5` `shell::workspace::Workspace::sync_terminal_input` — it can be `pub(crate)`
707. **nit** — `crates/editor-aqua/src/shell/workspace.rs:714:5` `shell::workspace::Workspace::dispose_terminal_dialog` — it can be `pub(crate)`
708. **nit** — `crates/editor-aqua/src/shell/workspace.rs:723:5` `shell::workspace::Workspace::drain_inspector` — it can be `pub(crate)`
709. **nit** — `crates/editor-aqua/src/shell/workspace.rs:800:5` `shell::workspace::Workspace::take_composer_actions` — it can be `pub(crate)`
710. **nit** — `crates/editor-aqua/src/shell/workspace.rs:804:5` `shell::workspace::Workspace::refresh_composer_mentions` — it can be `pub(crate)`
711. **nit** — `crates/editor-aqua/src/shell/workspace.rs:827:5` `shell::workspace::Workspace::sync_question_pages` — it can be `pub(crate)`
712. **nit** — `crates/editor-aqua/src/shell/workspace.rs:890:5` `shell::workspace::Workspace::composer_image` — it can be `pub(crate)`
713. **nit** — `crates/editor-aqua/src/shell/workspace.rs:917:5` `shell::workspace::Workspace::take_composer_effects` — it can be `pub(crate)`
714. **nit** — `crates/editor-aqua/src/shell/workspace.rs:921:5` `shell::workspace::Workspace::dispatch_composer_actions` — it can be `pub(crate)`
715. **nit** — `crates/editor-aqua/src/shell/workspace.rs:943:5` `shell::workspace::Workspace::handle_question_option` — it can be `pub(crate)`
716. **nit** — `crates/editor-aqua/src/shell/workspace.rs:976:5` `shell::workspace::Workspace::question_source_view` — it can be `pub(crate)`
717. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1038:5` `shell::workspace::Workspace::take_sent_new_thread` — it can be `pub(crate)`
718. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1043:5` `shell::workspace::Workspace::new_thread_posting` — it can be `pub(crate)`
719. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1060:5` `shell::workspace::Workspace::composer_content` — it can be `pub(crate)`
720. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1168:5` `shell::workspace::Workspace::bind_question_source` — it can be `pub(crate)`
721. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1196:5` `shell::workspace::Workspace::drain_question_source` — it can be `pub(crate)`
722. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1238:5` `shell::workspace::Workspace::release_question_source` — it can be `pub(crate)`
723. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1245:5` `shell::workspace::Workspace::release_question_options` — it can be `pub(crate)`
724. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1256:5` `shell::workspace::Workspace::question_option_emit` — it can be `pub(crate)`
725. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1270:5` `shell::workspace::Workspace::usage_emit` — it can be `pub(crate)`
726. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1275:5` `shell::workspace::Workspace::sidebar_view` — it can be `pub(crate)`
727. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1303:5` `shell::workspace::Workspace::usage_view` — it can be `pub(crate)`
728. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1324:5` `shell::workspace::Workspace::home_emit` — it can be `pub(crate)`
729. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1329:5` `shell::workspace::Workspace::home_view` — it can be `pub(crate)`
730. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1340:5` `shell::workspace::Workspace::settings_emit` — it can be `pub(crate)`
731. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1345:5` `shell::workspace::Workspace::dispatch_sidebar_actions` — it can be `pub(crate)`
732. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1420:5` `shell::workspace::Workspace::take_settings_platform_effects` — it can be `pub(crate)`
733. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1428:5` `shell::workspace::Workspace::drain_domains` — it can be `pub(crate)`
734. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1505:5` `shell::workspace::Workspace::sync_editor_ui` — it can be `pub(crate)`
735. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1529:5` `shell::workspace::Workspace::sync_marketplace_ui` — it can be `pub(crate)`
736. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1545:5` `shell::workspace::Workspace::marketplace_view` — it can be `pub(crate)`
737. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1553:5` `shell::workspace::Workspace::settings_view` — it can be `pub(crate)`
738. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1572:5` `shell::workspace::Workspace::open_reference` — it can be `pub(crate)`
739. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1589:5` `shell::workspace::Workspace::dismiss_reference` — it can be `pub(crate)`
740. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1594:5` `shell::workspace::Workspace::resource_preview_generation` — it can be `pub(crate)`
741. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1598:5` `shell::workspace::Workspace::drain_reference` — it can be `pub(crate)`
742. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1604:5` `shell::workspace::Workspace::open_palette` — it can be `pub(crate)`
743. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1611:5` `shell::workspace::Workspace::dismiss_palette` — it can be `pub(crate)`
744. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1615:5` `shell::workspace::Workspace::search_palette` — it can be `pub(crate)`
745. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1632:5` `shell::workspace::Workspace::drain_palette` — it can be `pub(crate)`
746. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1643:5` `shell::workspace::Workspace::refresh_palette` — it can be `pub(crate)`
747. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1668:5` `shell::workspace::Workspace::new_thread` — it can be `pub(crate)`
748. **nit** — `crates/editor-aqua/src/shell/workspace.rs:1679:5` `shell::workspace::Workspace::open_settings` — it can be `pub(crate)`

**editor-ui** (488)

749. **nit** — `crates/editor-ui/src/ui/asset.rs:28:1` `ui::asset::source_asset_glyph` — it can be `pub(crate)`
750. **nit** — `crates/editor-ui/src/ui/badge.rs:23:1` `ui::badge::Badge` — it can be `pub(crate)`
751. **nit** — `crates/editor-ui/src/ui/badge.rs:30:5` `ui::badge::Badge::new` — it can be `pub(crate)`
752. **nit** — `crates/editor-ui/src/ui/badge.rs:39:5` `ui::badge::Badge::variant` — it can be `pub(crate)`
753. **nit** — `crates/editor-ui/src/ui/badge.rs:44:5` `ui::badge::Badge::build` — it can be `pub(crate)`
754. **nit** — `crates/editor-ui/src/ui/button.rs:40:1` `ui::button::Button` — it can be `pub(crate)`
755. **nit** — `crates/editor-ui/src/ui/button.rs:78:5` `ui::button::Button::icon` — it can be `pub(crate)`
756. **nit** — `crates/editor-ui/src/ui/button.rs:108:5` `ui::button::Button::variant` — it can be `pub(crate)`
757. **nit** — `crates/editor-ui/src/ui/button.rs:136:5` `ui::button::Button::on_activate` — it can be `pub(crate)`
758. **nit** — `crates/editor-ui/src/ui/button.rs:140:5` `ui::button::Button::icon_slot` — it can be `pub(crate)`
759. **nit** — `crates/editor-ui/src/ui/button.rs:183:5` `ui::button::Button::build` — it can be `pub(crate)`
760. **nit** — `crates/editor-ui/src/ui/card.rs:10:5` `ui::card::CardStyle::corner_radius` — it can be `pub(crate)`
761. **nit** — `crates/editor-ui/src/ui/card.rs:11:5` `ui::card::CardStyle::vertical_padding` — it can be `pub(crate)`
762. **nit** — `crates/editor-ui/src/ui/card.rs:12:5` `ui::card::CardStyle::content_horizontal_padding` — it can be `pub(crate)`
763. **nit** — `crates/editor-ui/src/ui/card.rs:13:5` `ui::card::CardStyle::vertical_gap` — it can be `pub(crate)`
764. **nit** — `crates/editor-ui/src/ui/card.rs:14:5` `ui::card::CardStyle::text_size` — it can be `pub(crate)`
765. **nit** — `crates/editor-ui/src/ui/card.rs:15:5` `ui::card::CardStyle::background` — it can be `pub(crate)`
766. **nit** — `crates/editor-ui/src/ui/card.rs:16:5` `ui::card::CardStyle::foreground` — it can be `pub(crate)`
767. **nit** — `crates/editor-ui/src/ui/card.rs:17:5` `ui::card::CardStyle::source_foreground` — it can be `pub(crate)`
768. **nit** — `crates/editor-ui/src/ui/card.rs:18:5` `ui::card::CardStyle::ring_color` — it can be `pub(crate)`
769. **nit** — `crates/editor-ui/src/ui/card.rs:19:5` `ui::card::CardStyle::ring_spread` — it can be `pub(crate)`
770. **nit** — `crates/editor-ui/src/ui/chrome/account.rs:20:1` `ui::chrome::account::AVATAR` — it can be `pub(crate)`
771. **nit** — `crates/editor-ui/src/ui/chrome/account.rs:73:5` `ui::chrome::account::Identity::label` — it can be `pub(crate)`
772. **nit** — `crates/editor-ui/src/ui/chrome/account.rs:169:1` `ui::chrome::account::menu_surface` — it can be `pub(crate)`
773. **nit** — `crates/editor-ui/src/ui/chrome/card.rs:9:1` `ui::chrome::card::RADIUS` — it can be `pub(crate)`
774. **nit** — `crates/editor-ui/src/ui/chrome/card.rs:38:1` `ui::chrome::card::Surfaces` — it can be `pub(crate)`
775. **nit** — `crates/editor-ui/src/ui/chrome/card.rs:39:5` `ui::chrome::card::Surfaces::bg` — it can be `pub(crate)`
776. **nit** — `crates/editor-ui/src/ui/chrome/card.rs:40:5` `ui::chrome::card::Surfaces::inner` — it can be `pub(crate)`
777. **nit** — `crates/editor-ui/src/ui/chrome/card.rs:41:5` `ui::chrome::card::Surfaces::ink` — it can be `pub(crate)`
778. **nit** — `crates/editor-ui/src/ui/chrome/card.rs:42:5` `ui::chrome::card::Surfaces::ink_dim` — it can be `pub(crate)`
779. **nit** — `crates/editor-ui/src/ui/chrome/card.rs:47:5` `ui::chrome::card::Surfaces::of` — it can be `pub(crate)`
780. **nit** — `crates/editor-ui/src/ui/chrome/card.rs:84:1` `ui::chrome::card::double_rule` — it can be `pub(crate)`
781. **nit** — `crates/editor-ui/src/ui/chrome/card.rs:100:1` `ui::chrome::card::outlined` — it can be `pub(crate)`
782. **nit** — `crates/editor-ui/src/ui/chrome/card.rs:162:1` `ui::chrome::card::soft` — it can be `pub(crate)`
783. **nit** — `crates/editor-ui/src/ui/chrome/card.rs:182:1` `ui::chrome::card::key_shadows` — it can be `pub(crate)`
784. **nit** — `crates/editor-ui/src/ui/chrome/card.rs:213:1` `ui::chrome::card::polar_face` — it can be `pub(crate)`
785. **nit** — `crates/editor-ui/src/ui/chrome/card.rs:243:1` `ui::chrome::card::radial_glow` — it can be `pub(crate)`
786. **nit** — `crates/editor-ui/src/ui/chrome/limits.rs:139:1` `ui::chrome::limits::title` — it can be `pub(crate)`
787. **nit** — `crates/editor-ui/src/ui/chrome/menu.rs:16:5` `ui::chrome::menu::Spot::ALL` — it can be `pub(crate)`
788. **nit** — `crates/editor-ui/src/ui/code_copy.rs:75:1` `ui::code_copy::CodeCopyButton` — it can be `pub(crate)`
789. **nit** — `crates/editor-ui/src/ui/code_copy.rs:85:5` `ui::code_copy::CodeCopyButton::new` — it can be `pub(crate)`
790. **nit** — `crates/editor-ui/src/ui/code_copy.rs:103:5` `ui::code_copy::CodeCopyButton::build` — it can be `pub(crate)`
791. **nit** — `crates/editor-ui/src/ui/color_copy_menu.rs:181:5` `ui::color_copy_menu::ColorCopyMenuHost::root` — it can be `pub(crate)`
792. **nit** — `crates/editor-ui/src/ui/glass.rs:12:5` `ui::glass::Gradient::angle_degrees` — it can be `pub(crate)`
793. **nit** — `crates/editor-ui/src/ui/glass.rs:13:5` `ui::glass::Gradient::start` — it can be `pub(crate)`
794. **nit** — `crates/editor-ui/src/ui/glass.rs:14:5` `ui::glass::Gradient::end` — it can be `pub(crate)`
795. **nit** — `crates/editor-ui/src/ui/glass.rs:15:5` `ui::glass::Gradient::end_position` — it can be `pub(crate)`
796. **nit** — `crates/editor-ui/src/ui/glass.rs:20:5` `ui::glass::GlassShadow::color` — it can be `pub(crate)`
797. **nit** — `crates/editor-ui/src/ui/glass.rs:21:5` `ui::glass::GlassShadow::offset_x` — it can be `pub(crate)`
798. **nit** — `crates/editor-ui/src/ui/glass.rs:22:5` `ui::glass::GlassShadow::offset_y` — it can be `pub(crate)`
799. **nit** — `crates/editor-ui/src/ui/glass.rs:23:5` `ui::glass::GlassShadow::css_blur` — it can be `pub(crate)`
800. **nit** — `crates/editor-ui/src/ui/glass.rs:24:5` `ui::glass::GlassShadow::spread` — it can be `pub(crate)`
801. **nit** — `crates/editor-ui/src/ui/glass.rs:25:5` `ui::glass::GlassShadow::inset` — it can be `pub(crate)`
802. **nit** — `crates/editor-ui/src/ui/glass.rs:54:1` `ui::glass::glass_control_fill` — it can be `pub(crate)`
803. **nit** — `crates/editor-ui/src/ui/glass.rs:79:1` `ui::glass::card_shadows` — it can be `pub(crate)`
804. **nit** — `crates/editor-ui/src/ui/glass.rs:86:1` `ui::glass::glass_rims` — it can be `pub(crate)`
805. **nit** — `crates/editor-ui/src/ui/glass.rs:109:1` `ui::glass::glass_card_shadows` — it can be `pub(crate)`
806. **nit** — `crates/editor-ui/src/ui/glass.rs:124:1` `ui::glass::COMPOSER_PADDING` — it can be `pub(crate)`
807. **nit** — `crates/editor-ui/src/ui/glass.rs:125:1` `ui::glass::COMPOSER_RADIUS` — it can be `pub(crate)`
808. **nit** — `crates/editor-ui/src/ui/hint_card.rs:62:1` `ui::hint_card::presence` — it can be `pub(crate)`
809. **nit** — `crates/editor-ui/src/ui/hover.rs:4:1` `ui::hover::hover_fill_stops` — it can be `pub(crate)`
810. **nit** — `crates/editor-ui/src/ui/icon.rs:28:1` `ui::icon::file_icon` — it can be `pub(crate)`
811. **nit** — `crates/editor-ui/src/ui/kit/button/mod.rs:47:5` `ui::kit::button::ButtonSize::pad` — it can be `pub(crate)`
812. **nit** — `crates/editor-ui/src/ui/kit/button/mod.rs:151:5` `ui::kit::button::Button::ellipsis` — it can be `pub(crate)`
813. **nit** — `crates/editor-ui/src/ui/kit/button/mod.rs:163:5` `ui::kit::button::Button::motion_policy` — it can be `pub(crate)`
814. **nit** — `crates/editor-ui/src/ui/kit/control/icon_button.rs:15:1` `ui::kit::control::icon_button::CORNER_INSET` — it can be `pub(crate)`
815. **nit** — `crates/editor-ui/src/ui/kit/control/icon_button.rs:35:5` `ui::kit::control::icon_button::IconButtonSize::glyph` — it can be `pub(crate)`
816. **nit** — `crates/editor-ui/src/ui/kit/control/icon_button.rs:91:5` `ui::kit::control::icon_button::IconButton::corner_close` — it can be `pub(crate)`
817. **nit** — `crates/editor-ui/src/ui/kit/control/key.rs:14:1` `ui::kit::control::key::PRESS` — it can be `pub(crate)`
818. **nit** — `crates/editor-ui/src/ui/kit/control/mod.rs:14:1` `ui::kit::control::focus_ring` — it can be `pub(crate)`
819. **nit** — `crates/editor-ui/src/ui/kit/control/trigger.rs:21:5` `ui::kit::control::trigger::TriggerSize::height` — it can be `pub(crate)`
820. **nit** — `crates/editor-ui/src/ui/kit/hover/mod.rs:46:5` `ui::kit::hover::Material::fill` — it can be `pub(crate)`
821. **nit** — `crates/editor-ui/src/ui/kit/hover/mod.rs:64:5` `ui::kit::hover::Material::ring` — it can be `pub(crate)`
822. **nit** — `crates/editor-ui/src/ui/kit/hover/mod.rs:76:5` `ui::kit::hover::Material::style` — it can be `pub(crate)`
823. **nit** — `crates/editor-ui/src/ui/kit/hover/mod.rs:84:1` `ui::kit::hover::neutral_stops` — it can be `pub(crate)`
824. **nit** — `crates/editor-ui/src/ui/kit/pane.rs:16:5` `ui::kit::pane::Seam::insets` — it can be `pub(crate)`
825. **nit** — `crates/editor-ui/src/ui/kit/row.rs:51:5` `ui::kit::row::Row::title` — it can be `pub(crate)`
826. **nit** — `crates/editor-ui/src/ui/kit/surface/band.rs:44:5` `ui::kit::surface::band::Band::opens` — it can be `pub(crate)`
827. **nit** — `crates/editor-ui/src/ui/kit/surface/band.rs:49:5` `ui::kit::surface::band::Band::closes` — it can be `pub(crate)`
828. **nit** — `crates/editor-ui/src/ui/kit/surface/glass.rs:8:1` `ui::kit::surface::glass::BLUR` — it can be `pub(crate)`
829. **nit** — `crates/editor-ui/src/ui/kit/surface/glass.rs:78:1` `ui::kit::surface::glass::fills` — it can be `pub(crate)`
830. **nit** — `crates/editor-ui/src/ui/kit/surface/glass.rs:88:1` `ui::kit::surface::glass::rims` — it can be `pub(crate)`
831. **nit** — `crates/editor-ui/src/ui/kit/surface/glass.rs:111:1` `ui::kit::surface::glass::paint` — it can be `pub(crate)`
832. **nit** — `crates/editor-ui/src/ui/kit/surface/mod.rs:70:5` `ui::kit::surface::Surface::corner` — it can be `pub(crate)`
833. **nit** — `crates/editor-ui/src/ui/kit/surface/mod.rs:108:5` `ui::kit::surface::Surface::material` — it can be `pub(crate)`
834. **nit** — `crates/editor-ui/src/ui/kit/surface/mod.rs:127:5` `ui::kit::surface::Surface::elevation` — it can be `pub(crate)`
835. **nit** — `crates/editor-ui/src/ui/kit/tokens/elevation.rs:17:5` `ui::kit::tokens::elevation::Elevation::shadows` — it can be `pub(crate)`
836. **nit** — `crates/editor-ui/src/ui/kit/tokens/radius.rs:12:5` `ui::kit::tokens::radius::Inset::ALL` — it can be `pub(crate)`
837. **nit** — `crates/editor-ui/src/ui/kit/tokens/radius.rs:15:5` `ui::kit::tokens::radius::Inset::of` — it can be `pub(crate)`
838. **nit** — `crates/editor-ui/src/ui/kit/tokens/radius.rs:108:5` `ui::kit::tokens::radius::Corners::radii` — it can be `pub(crate)`
839. **nit** — `crates/editor-ui/src/ui/kit/tokens/space.rs:17:5` `ui::kit::tokens::space::Space::ALL` — it can be `pub(crate)`
840. **nit** — `crates/editor-ui/src/ui/local_time.rs:17:1` `ui::local_time::occasion` — it can be `pub(crate)`
841. **nit** — `crates/editor-ui/src/ui/markdown/literal.rs:24:1` `ui::markdown::literal::MAX_NATIVE_SOURCE` — it can be `pub(crate)`
842. **nit** — `crates/editor-ui/src/ui/markdown/literal.rs:46:5` `ui::markdown::literal::LiteralLeaf::text` — it can be `pub(crate)`
843. **nit** — `crates/editor-ui/src/ui/markdown/literal.rs:47:5` `ui::markdown::literal::LiteralLeaf::source_ref` — it can be `pub(crate)`
844. **nit** — `crates/editor-ui/src/ui/markdown/literal.rs:48:5` `ui::markdown::literal::LiteralLeaf::ordinal` — it can be `pub(crate)`
845. **nit** — `crates/editor-ui/src/ui/markdown/literal.rs:49:5` `ui::markdown::literal::LiteralLeaf::source_range` — it can be `pub(crate)`
846. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:76:5` `ui::markdown::CodeToken::kind` — it can be `pub(crate)`
847. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:77:5` `ui::markdown::CodeToken::range` — it can be `pub(crate)`
848. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:82:5` `ui::markdown::DiagramSvg::source` — it can be `pub(crate)`
849. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:83:5` `ui::markdown::DiagramSvg::light_source` — it can be `pub(crate)`
850. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:84:5` `ui::markdown::DiagramSvg::width_bits` — it can be `pub(crate)`
851. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:85:5` `ui::markdown::DiagramSvg::height_bits` — it can be `pub(crate)`
852. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:89:5` `ui::markdown::DiagramSvg::width` — it can be `pub(crate)`
853. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:93:5` `ui::markdown::DiagramSvg::height` — it can be `pub(crate)`
854. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:100:5` `ui::markdown::CodeFence::language` — it can be `pub(crate)`
855. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:101:5` `ui::markdown::CodeFence::closed` — it can be `pub(crate)`
856. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:102:5` `ui::markdown::CodeFence::source` — it can be `pub(crate)`
857. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:103:5` `ui::markdown::CodeFence::tokens` — it can be `pub(crate)`
858. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:104:5` `ui::markdown::CodeFence::diagram` — it can be `pub(crate)`
859. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:123:5` `ui::markdown::Span::text_content` — it can be `pub(crate)`
860. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:137:1` `ui::markdown::spans_text` — it can be `pub(crate)`
861. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:189:5` `ui::markdown::ListItem::blocks` — it can be `pub(crate)`
862. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:190:5` `ui::markdown::ListItem::task` — it can be `pub(crate)`
863. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:292:5` `ui::markdown::MarkdownDocument::blocks` — it can be `pub(crate)`
864. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:337:5` `ui::markdown::MarkdownEngine::parse_document` — it can be `pub(crate)`
865. **nit** — `crates/editor-ui/src/ui/markdown/presenter.rs:49:5` `ui::markdown::presenter::ProjectedTextLeaf::text` — it can be `pub(crate)`
866. **nit** — `crates/editor-ui/src/ui/markdown/presenter.rs:60:5` `ui::markdown::presenter::ProjectedLeafInfo::source_range` — it can be `pub(crate)`
867. **nit** — `crates/editor-ui/src/ui/markdown/presenter.rs:62:5` `ui::markdown::presenter::ProjectedLeafInfo::proof` — it can be `pub(crate)`
868. **nit** — `crates/editor-ui/src/ui/markdown/presenter.rs:128:5` `ui::markdown::presenter::StoredProjectedMarkdown::source_range` — it can be `pub(crate)`
869. **nit** — `crates/editor-ui/src/ui/markdown/presenter.rs:129:5` `ui::markdown::presenter::StoredProjectedMarkdown::list_depth` — it can be `pub(crate)`
870. **nit** — `crates/editor-ui/src/ui/markdown/presenter.rs:130:5` `ui::markdown::presenter::StoredProjectedMarkdown::proof` — it can be `pub(crate)`
871. **nit** — `crates/editor-ui/src/ui/markdown/presenter.rs:189:5` `ui::markdown::presenter::HeatmapServices::empty` — it can be `pub(crate)`
872. **nit** — `crates/editor-ui/src/ui/markdown/presenter.rs:190:5` `ui::markdown::presenter::HeatmapServices::readout` — it can be `pub(crate)`
873. **nit** — `crates/editor-ui/src/ui/markdown/presenter.rs:191:5` `ui::markdown::presenter::HeatmapServices::navigation_frame` — it can be `pub(crate)`
874. **nit** — `crates/editor-ui/src/ui/markdown/presenter.rs:192:5` `ui::markdown::presenter::HeatmapServices::concentration` — it can be `pub(crate)`
875. **nit** — `crates/editor-ui/src/ui/markdown/presenter.rs:196:5` `ui::markdown::presenter::ConcentrationServices::frame` — it can be `pub(crate)`
876. **nit** — `crates/editor-ui/src/ui/markdown/presenter.rs:197:5` `ui::markdown::presenter::ConcentrationServices::plot_label` — it can be `pub(crate)`
877. **nit** — `crates/editor-ui/src/ui/markdown/presenter.rs:198:5` `ui::markdown::presenter::ConcentrationServices::readout` — it can be `pub(crate)`
878. **nit** — `crates/editor-ui/src/ui/markdown/presenter.rs:202:5` `ui::markdown::presenter::ConcentrationServices::new` — it can be `pub(crate)`
879. **nit** — `crates/editor-ui/src/ui/markdown/presenter.rs:802:5` `ui::markdown::presenter::MarkdownPresenter::projected_range` — it can be `pub(crate)`
880. **nit** — `crates/editor-ui/src/ui/markdown/presenter.rs:1338:5` `ui::markdown::presenter::MarkdownPresenter::render_with_actions` — it can be `pub(crate)`
881. **nit** — `crates/editor-ui/src/ui/markdown/presenter.rs:1364:5` `ui::markdown::presenter::MarkdownPresenter::render_with_context` — it can be `pub(crate)`
882. **nit** — `crates/editor-ui/src/ui/markdown/reveal.rs:203:5` `ui::markdown::reveal::PreparedReveal::block_growth` — it can be `pub(crate)`
883. **nit** — `crates/editor-ui/src/ui/markdown/source_index.rs:26:5` `ui::markdown::source_index::MarkdownIndex::build` — it can be `pub(crate)`
884. **nit** — `crates/editor-ui/src/ui/markdown/source_index.rs:29:5` `ui::markdown::source_index::MarkdownIndex::leaves` — it can be `pub(crate)`
885. **nit** — `crates/editor-ui/src/ui/markdown/source_index.rs:32:5` `ui::markdown::source_index::MarkdownIndex::context` — it can be `pub(crate)`
886. **nit** — `crates/editor-ui/src/ui/markdown/source_index.rs:41:5` `ui::markdown::source_index::MarkdownIndex::normalize_leaf` — it can be `pub(crate)`
887. **nit** — `crates/editor-ui/src/ui/markdown/stored.rs:5:1` `ui::markdown::stored::MAX_SOURCE_BYTES` — it can be `pub(crate)`
888. **nit** — `crates/editor-ui/src/ui/markdown/stored.rs:6:1` `ui::markdown::stored::RANGE_BYTES` — it can be `pub(crate)`
889. **nit** — `crates/editor-ui/src/ui/markdown/stored.rs:7:1` `ui::markdown::stored::MAX_INDEX_ENTRIES` — it can be `pub(crate)`
890. **nit** — `crates/editor-ui/src/ui/markdown/stored.rs:8:1` `ui::markdown::stored::MAX_PREPARED_ENTRIES` — it can be `pub(crate)`
891. **nit** — `crates/editor-ui/src/ui/markdown/stored.rs:9:1` `ui::markdown::stored::MAX_PENDING_JOBS` — it can be `pub(crate)`
892. **nit** — `crates/editor-ui/src/ui/markdown/stored.rs:10:1` `ui::markdown::stored::PREPARED_BYTES` — it can be `pub(crate)`
893. **nit** — `crates/editor-ui/src/ui/markdown/stored.rs:11:1` `ui::markdown::stored::QUEUE_DEPTH` — it can be `pub(crate)`
894. **nit** — `crates/editor-ui/src/ui/markdown/stored.rs:13:1` `ui::markdown::stored::validate_page` — it can be `pub(crate)`
895. **nit** — `crates/editor-ui/src/ui/markdown/stored.rs:60:5` `ui::markdown::stored::RangeCollector::new` — it can be `pub(crate)`
896. **nit** — `crates/editor-ui/src/ui/markdown/stored.rs:76:5` `ui::markdown::stored::RangeCollector::next_request` — it can be `pub(crate)`
897. **nit** — `crates/editor-ui/src/ui/markdown/stored.rs:86:5` `ui::markdown::stored::RangeCollector::push` — it can be `pub(crate)`
898. **nit** — `crates/editor-ui/src/ui/markdown/stored.rs:112:5` `ui::markdown::stored::RangeCollector::finish` — it can be `pub(crate)`
899. **nit** — `crates/editor-ui/src/ui/markdown/stored.rs:140:5` `ui::markdown::stored::StoredSource::literal_content_range` — it can be `pub(crate)`
900. **nit** — `crates/editor-ui/src/ui/markdown/stored.rs:202:5` `ui::markdown::stored::StoredLeaf::descriptor` — it can be `pub(crate)`
901. **nit** — `crates/editor-ui/src/ui/markdown/stored.rs:203:5` `ui::markdown::stored::StoredLeaf::source` — it can be `pub(crate)`
902. **nit** — `crates/editor-ui/src/ui/markdown/stored.rs:205:5` `ui::markdown::stored::StoredLeaf::table_continues` — it can be `pub(crate)`
903. **nit** — `crates/editor-ui/src/ui/markdown/stored.rs:206:5` `ui::markdown::stored::StoredLeaf::content` — it can be `pub(crate)`
904. **nit** — `crates/editor-ui/src/ui/markdown/stored.rs:221:5` `ui::markdown::stored::LeafProof::owner` — it can be `pub(crate)`
905. **nit** — `crates/editor-ui/src/ui/markdown/table.rs:10:5` `ui::markdown::table::TableCell::spans` — it can be `pub(crate)`
906. **nit** — `crates/editor-ui/src/ui/markdown/table.rs:15:5` `ui::markdown::table::Table::alignments` — it can be `pub(crate)`
907. **nit** — `crates/editor-ui/src/ui/markdown/table.rs:16:5` `ui::markdown::table::Table::header` — it can be `pub(crate)`
908. **nit** — `crates/editor-ui/src/ui/markdown/table.rs:17:5` `ui::markdown::table::Table::rows` — it can be `pub(crate)`
909. **nit** — `crates/editor-ui/src/ui/markdown/table.rs:18:5` `ui::markdown::table::Table::range` — it can be `pub(crate)`
910. **nit** — `crates/editor-ui/src/ui/markdown/terminal/screen.rs:456:5` `ui::markdown::terminal::screen::TerminalScreen::with_max_lines` — it can be `pub(crate)`
911. **nit** — `crates/editor-ui/src/ui/markdown/terminal/screen.rs:487:5` `ui::markdown::terminal::screen::TerminalScreen::lines` — it can be `pub(crate)`
912. **nit** — `crates/editor-ui/src/ui/markdown/visual.rs:23:5` `ui::markdown::visual::InlineStyle::slant` — it can be `pub(crate)`
913. **nit** — `crates/editor-ui/src/ui/markdown/visual.rs:35:5` `ui::markdown::visual::InlineLink::range` — it can be `pub(crate)`
914. **nit** — `crates/editor-ui/src/ui/markdown/visual.rs:44:5` `ui::markdown::visual::PreparedInline::code_ranges` — it can be `pub(crate)`
915. **nit** — `crates/editor-ui/src/ui/markdown/visual.rs:45:5` `ui::markdown::visual::PreparedInline::file_icons` — it can be `pub(crate)`
916. **nit** — `crates/editor-ui/src/ui/markdown/visual.rs:52:5` `ui::markdown::visual::InlineSwatch::slot` — it can be `pub(crate)`
917. **nit** — `crates/editor-ui/src/ui/markdown/visual.rs:53:5` `ui::markdown::visual::InlineSwatch::value` — it can be `pub(crate)`
918. **nit** — `crates/editor-ui/src/ui/markdown/visual.rs:54:5` `ui::markdown::visual::InlineSwatch::alone` — it can be `pub(crate)`
919. **nit** — `crates/editor-ui/src/ui/markdown/visual.rs:65:5` `ui::markdown::visual::SlotBounds::x` — it can be `pub(crate)`
920. **nit** — `crates/editor-ui/src/ui/markdown/visual.rs:66:5` `ui::markdown::visual::SlotBounds::y` — it can be `pub(crate)`
921. **nit** — `crates/editor-ui/src/ui/markdown/visual.rs:67:5` `ui::markdown::visual::SlotBounds::side` — it can be `pub(crate)`
922. **nit** — `crates/editor-ui/src/ui/markdown/visual.rs:72:5` `ui::markdown::visual::SwatchPaint::bounds` — it can be `pub(crate)`
923. **nit** — `crates/editor-ui/src/ui/markdown/visual.rs:73:5` `ui::markdown::visual::SwatchPaint::color` — it can be `pub(crate)`
924. **nit** — `crates/editor-ui/src/ui/markdown/visual.rs:74:5` `ui::markdown::visual::SwatchPaint::edge` — it can be `pub(crate)`
925. **nit** — `crates/editor-ui/src/ui/markdown/visual.rs:75:5` `ui::markdown::visual::SwatchPaint::radius` — it can be `pub(crate)`
926. **nit** — `crates/editor-ui/src/ui/markdown/visual.rs:76:5` `ui::markdown::visual::SwatchPaint::border_width` — it can be `pub(crate)`
927. **nit** — `crates/editor-ui/src/ui/markdown/visual.rs:81:5` `ui::markdown::visual::FileIconPaint::bounds` — it can be `pub(crate)`
928. **nit** — `crates/editor-ui/src/ui/markdown/visual.rs:82:5` `ui::markdown::visual::FileIconPaint::asset` — it can be `pub(crate)`
929. **nit** — `crates/editor-ui/src/ui/markdown/visual.rs:435:5` `ui::markdown::visual::ProseLeaf::element` — it can be `pub(crate)`
930. **nit** — `crates/editor-ui/src/ui/markdown/visual.rs:439:1` `ui::markdown::visual::raw_leaf` — it can be `pub(crate)`
931. **nit** — `crates/editor-ui/src/ui/markdown/visual.rs:526:1` `ui::markdown::visual::interactive_leaf` — it can be `pub(crate)`
932. **nit** — `crates/editor-ui/src/ui/markdown/visual.rs:587:1` `ui::markdown::visual::interactive_table_leaf` — it can be `pub(crate)`
933. **nit** — `crates/editor-ui/src/ui/media/color.rs:8:5` `ui::media::color::Color::g` — it can be `pub(crate)`
934. **nit** — `crates/editor-ui/src/ui/media/color.rs:9:5` `ui::media::color::Color::b` — it can be `pub(crate)`
935. **nit** — `crates/editor-ui/src/ui/media/color.rs:10:5` `ui::media::color::Color::alpha` — it can be `pub(crate)`
936. **nit** — `crates/editor-ui/src/ui/media/color.rs:24:5` `ui::media::color::Notation::ALL` — it can be `pub(crate)`
937. **nit** — `crates/editor-ui/src/ui/media/color.rs:33:5` `ui::media::color::Notation::label` — it can be `pub(crate)`
938. **nit** — `crates/editor-ui/src/ui/media/color.rs:52:5` `ui::media::color::Color::reference_paint` — it can be `pub(crate)`
939. **nit** — `crates/editor-ui/src/ui/media/color.rs:68:5` `ui::media::color::Color::clipped` — it can be `pub(crate)`
940. **nit** — `crates/editor-ui/src/ui/media/color.rs:99:5` `ui::media::color::Color::to_oklab` — it can be `pub(crate)`
941. **nit** — `crates/editor-ui/src/ui/media/color.rs:102:5` `ui::media::color::Color::to_oklch` — it can be `pub(crate)`
942. **nit** — `crates/editor-ui/src/ui/media/color.rs:105:5` `ui::media::color::Color::to_hsl` — it can be `pub(crate)`
943. **nit** — `crates/editor-ui/src/ui/media/image.rs:239:5` `ui::media::image::Images::retain_owners` — it can be `pub(crate)`
944. **nit** — `crates/editor-ui/src/ui/media/model/formats/mod.rs:24:5` `ui::media::model::formats::Format::label` — it can be `pub(crate)`
945. **nit** — `crates/editor-ui/src/ui/media/model/formats/mod.rs:41:5` `ui::media::model::formats::Format::sniff` — it can be `pub(crate)`
946. **nit** — `crates/editor-ui/src/ui/media/model/formats/mod.rs:60:5` `ui::media::model::formats::Model::mesh` — it can be `pub(crate)`
947. **nit** — `crates/editor-ui/src/ui/media/model/formats/mod.rs:61:5` `ui::media::model::formats::Model::triangles` — it can be `pub(crate)`
948. **nit** — `crates/editor-ui/src/ui/media/model/formats/mod.rs:62:5` `ui::media::model::formats::Model::format` — it can be `pub(crate)`
949. **nit** — `crates/editor-ui/src/ui/media/model/load.rs:32:5` `ui::media::model::load::LoadedModel::geometry` — it can be `pub(crate)`
950. **nit** — `crates/editor-ui/src/ui/media/model/load.rs:33:5` `ui::media::model::load::LoadedModel::triangles` — it can be `pub(crate)`
951. **nit** — `crates/editor-ui/src/ui/media/model/load.rs:34:5` `ui::media::model::load::LoadedModel::format` — it can be `pub(crate)`
952. **nit** — `crates/editor-ui/src/ui/media/model/load.rs:136:5` `ui::media::model::load::ModelAssets::request` — it can be `pub(crate)`
953. **nit** — `crates/editor-ui/src/ui/media/model/mesh.rs:42:5` `ui::media::model::mesh::Mesh::positions` — it can be `pub(crate)`
954. **nit** — `crates/editor-ui/src/ui/media/model/mesh.rs:43:5` `ui::media::model::mesh::Mesh::normals` — it can be `pub(crate)`
955. **nit** — `crates/editor-ui/src/ui/media/model/mesh.rs:44:5` `ui::media::model::mesh::Mesh::colors` — it can be `pub(crate)`
956. **nit** — `crates/editor-ui/src/ui/media/model/mesh.rs:48:5` `ui::media::model::mesh::Mesh::triangles` — it can be `pub(crate)`
957. **nit** — `crates/editor-ui/src/ui/media/model/raster.rs:11:5` `ui::media::model::raster::Camera::yaw` — it can be `pub(crate)`
958. **nit** — `crates/editor-ui/src/ui/media/model/raster.rs:12:5` `ui::media::model::raster::Camera::pitch` — it can be `pub(crate)`
959. **nit** — `crates/editor-ui/src/ui/media/model/raster.rs:13:5` `ui::media::model::raster::Camera::distance` — it can be `pub(crate)`
960. **nit** — `crates/editor-ui/src/ui/media/model/raster.rs:14:5` `ui::media::model::raster::Camera::pan` — it can be `pub(crate)`
961. **nit** — `crates/editor-ui/src/ui/media/model/raster.rs:18:5` `ui::media::model::raster::Camera::DEFAULT_YAW` — it can be `pub(crate)`
962. **nit** — `crates/editor-ui/src/ui/media/model/raster.rs:19:5` `ui::media::model::raster::Camera::DEFAULT_PITCH` — it can be `pub(crate)`
963. **nit** — `crates/editor-ui/src/ui/media/model/raster.rs:20:5` `ui::media::model::raster::Camera::NEAREST` — it can be `pub(crate)`
964. **nit** — `crates/editor-ui/src/ui/media/model/raster.rs:21:5` `ui::media::model::raster::Camera::FARTHEST` — it can be `pub(crate)`
965. **nit** — `crates/editor-ui/src/ui/media/model/raster.rs:23:5` `ui::media::model::raster::Camera::fitted` — it can be `pub(crate)`
966. **nit** — `crates/editor-ui/src/ui/media/model/raster.rs:27:5` `ui::media::model::raster::Camera::new` — it can be `pub(crate)`
967. **nit** — `crates/editor-ui/src/ui/media/model/raster.rs:36:5` `ui::media::model::raster::Camera::focal` — it can be `pub(crate)`
968. **nit** — `crates/editor-ui/src/ui/media/model/raster.rs:40:5` `ui::media::model::raster::Camera::view_matrix` — it can be `pub(crate)`
969. **nit** — `crates/editor-ui/src/ui/media/model/raster.rs:50:5` `ui::media::model::raster::Camera::projection_matrix` — it can be `pub(crate)`
970. **nit** — `crates/editor-ui/src/ui/media/model/raster.rs:81:5` `ui::media::model::raster::Look::base` — it can be `pub(crate)`
971. **nit** — `crates/editor-ui/src/ui/media/model/raster.rs:82:5` `ui::media::model::raster::Look::edges` — it can be `pub(crate)`
972. **nit** — `crates/editor-ui/src/ui/media/model/raster.rs:86:5` `ui::media::model::raster::Frame::width` — it can be `pub(crate)`
973. **nit** — `crates/editor-ui/src/ui/media/model/raster.rs:87:5` `ui::media::model::raster::Frame::height` — it can be `pub(crate)`
974. **nit** — `crates/editor-ui/src/ui/media/model/raster.rs:88:5` `ui::media::model::raster::Frame::pixels` — it can be `pub(crate)`
975. **nit** — `crates/editor-ui/src/ui/media/preview.rs:40:5` `ui::media::preview::OpenPreview::sources` — it can be `pub(crate)`
976. **nit** — `crates/editor-ui/src/ui/media/preview.rs:41:5` `ui::media::preview::OpenPreview::index` — it can be `pub(crate)`
977. **nit** — `crates/editor-ui/src/ui/media/preview.rs:42:5` `ui::media::preview::OpenPreview::image` — it can be `pub(crate)`
978. **nit** — `crates/editor-ui/src/ui/media/preview.rs:43:5` `ui::media::preview::OpenPreview::opener_key` — it can be `pub(crate)`
979. **nit** — `crates/editor-ui/src/ui/media/preview.rs:107:5` `ui::media::preview::Preview::new` — it can be `pub(crate)`
980. **nit** — `crates/editor-ui/src/ui/media/resource_cache.rs:123:5` `ui::media::resource_cache::ResourceCache::id` — it can be `pub(crate)`
981. **nit** — `crates/editor-ui/src/ui/media/source.rs:37:5` `ui::media::source::MediaBytes::permit` — it can be `pub(crate)`
982. **nit** — `crates/editor-ui/src/ui/media/source.rs:74:5` `ui::media::source::MediaFetcher::fetch` — it can be `pub(crate)`
983. **nit** — `crates/editor-ui/src/ui/media/source.rs:93:5` `ui::media::source::MediaFetcher::scope` — it can be `pub(crate)`
984. **nit** — `crates/editor-ui/src/ui/media/source.rs:162:5` `ui::media::source::MediaProvider::scope` — it can be `pub(crate)`
985. **nit** — `crates/editor-ui/src/ui/media/source.rs:185:5` `ui::media::source::Owned::owner` — it can be `pub(crate)`
986. **nit** — `crates/editor-ui/src/ui/media/source.rs:186:5` `ui::media::source::Owned::source` — it can be `pub(crate)`
987. **nit** — `crates/editor-ui/src/ui/media/source.rs:250:5` `ui::media::source::MediaSource::is_empty` — it can be `pub(crate)`
988. **nit** — `crates/editor-ui/src/ui/media_frame.rs:66:5` `ui::media_frame::MediaFrame::attachment` — it can be `pub(crate)`
989. **nit** — `crates/editor-ui/src/ui/media_frame.rs:134:5` `ui::media_frame::MediaFrame::error` — it can be `pub(crate)`
990. **nit** — `crates/editor-ui/src/ui/media_frame.rs:144:5` `ui::media_frame::MediaFrame::thumb_radius` — it can be `pub(crate)`
991. **nit** — `crates/editor-ui/src/ui/media_frame.rs:148:5` `ui::media_frame::MediaFrame::thumb_content_style` — it can be `pub(crate)`
992. **nit** — `crates/editor-ui/src/ui/menu_keys.rs:4:1` `ui::menu_keys::POPOVER_MENU` — it can be `pub(crate)`
993. **nit** — `crates/editor-ui/src/ui/page.rs:28:5` `ui::page::PageBlock::new` — it can be `pub(crate)`
994. **nit** — `crates/editor-ui/src/ui/page.rs:143:1` `ui::page::note` — it can be `pub(crate)`
995. **nit** — `crates/editor-ui/src/ui/phrase.rs:29:5` `ui::phrase::PhraseWord::text` — it can be `pub(crate)`
996. **nit** — `crates/editor-ui/src/ui/phrase.rs:31:5` `ui::phrase::PhraseWord::elapsed` — it can be `pub(crate)`
997. **nit** — `crates/editor-ui/src/ui/phrase.rs:44:1` `ui::phrase::phrase_fit` — it can be `pub(crate)`
998. **nit** — `crates/editor-ui/src/ui/picker_motion.rs:25:1` `ui::picker_motion::travel_scale` — it can be `pub(crate)`
999. **nit** — `crates/editor-ui/src/ui/picker_motion.rs:68:5` `ui::picker_motion::MenuMotion::frame` — it can be `pub(crate)`
1000. **nit** — `crates/editor-ui/src/ui/picker_motion.rs:72:5` `ui::picker_motion::MenuMotion::phase` — it can be `pub(crate)`
1001. **nit** — `crates/editor-ui/src/ui/picker_motion.rs:80:5` `ui::picker_motion::MenuMotion::transition` — it can be `pub(crate)`
1002. **nit** — `crates/editor-ui/src/ui/picker_motion.rs:92:5` `ui::picker_motion::MenuMotion::begin_open` — it can be `pub(crate)`
1003. **nit** — `crates/editor-ui/src/ui/picker_motion.rs:101:5` `ui::picker_motion::MenuMotion::begin_close` — it can be `pub(crate)`
1004. **nit** — `crates/editor-ui/src/ui/picker_motion.rs:111:5` `ui::picker_motion::MenuMotion::apply_progress` — it can be `pub(crate)`
1005. **nit** — `crates/editor-ui/src/ui/picker_motion.rs:121:5` `ui::picker_motion::MenuMotion::finish_open` — it can be `pub(crate)`
1006. **nit** — `crates/editor-ui/src/ui/picker_motion.rs:130:5` `ui::picker_motion::MenuMotion::finish_close` — it can be `pub(crate)`
1007. **nit** — `crates/editor-ui/src/ui/picker_motion.rs:139:5` `ui::picker_motion::MenuMotion::hide` — it can be `pub(crate)`
1008. **nit** — `crates/editor-ui/src/ui/popover.rs:10:1` `ui::popover::PopoverPanel` — it can be `pub(crate)`
1009. **nit** — `crates/editor-ui/src/ui/popover.rs:19:5` `ui::popover::PopoverPanel::new` — it can be `pub(crate)`
1010. **nit** — `crates/editor-ui/src/ui/popover.rs:35:5` `ui::popover::PopoverPanel::build` — it can be `pub(crate)`
1011. **nit** — `crates/editor-ui/src/ui/popover.rs:125:5` `ui::popover::PickerPopupHost::root` — it can be `pub(crate)`
1012. **nit** — `crates/editor-ui/src/ui/popover.rs:152:5` `ui::popover::PickerPopupHost::refresh` — it can be `pub(crate)`
1013. **nit** — `crates/editor-ui/src/ui/popover.rs:157:5` `ui::popover::PickerPopupHost::close` — it can be `pub(crate)`
1014. **nit** — `crates/editor-ui/src/ui/popover.rs:161:5` `ui::popover::PickerPopupHost::is_open` — it can be `pub(crate)`
1015. **nit** — `crates/editor-ui/src/ui/popover.rs:186:5` `ui::popover::AnimatedPickerHost::set_reduced_motion` — it can be `pub(crate)`
1016. **nit** — `crates/editor-ui/src/ui/popover.rs:199:5` `ui::popover::AnimatedPickerHost::overlay` — it can be `pub(crate)`
1017. **nit** — `crates/editor-ui/src/ui/rich/actions.rs:18:5` `ui::rich::actions::CopyEntry::kind` — it can be `pub(crate)`
1018. **nit** — `crates/editor-ui/src/ui/scroll_edge.rs:10:1` `ui::scroll_edge::SCROLL_EDGE_HEIGHT` — it can be `pub(crate)`
1019. **nit** — `crates/editor-ui/src/ui/scroll_edge.rs:289:1` `ui::scroll_edge::hidden_in` — it can be `pub(crate)`
1020. **nit** — `crates/editor-ui/src/ui/scroll_edge.rs:330:5` `ui::scroll_edge::EdgeBands::bands` — it can be `pub(crate)`
1021. **nit** — `crates/editor-ui/src/ui/segmented_control.rs:18:1` `ui::segmented_control::Segment` — it can be `pub(crate)`
1022. **nit** — `crates/editor-ui/src/ui/segmented_control.rs:23:5` `ui::segmented_control::Segment::<T>::new` — it can be `pub(crate)`
1023. **nit** — `crates/editor-ui/src/ui/segmented_control.rs:33:1` `ui::segmented_control::SegmentedControl` — it can be `pub(crate)`
1024. **nit** — `crates/editor-ui/src/ui/segmented_control.rs:46:5` `ui::segmented_control::SegmentedControl::<T>::new` — it can be `pub(crate)`
1025. **nit** — `crates/editor-ui/src/ui/segmented_control.rs:65:5` `ui::segmented_control::SegmentedControl::<T>::options` — it can be `pub(crate)`
1026. **nit** — `crates/editor-ui/src/ui/segmented_control.rs:81:5` `ui::segmented_control::SegmentedControl::<T>::on_change` — it can be `pub(crate)`
1027. **nit** — `crates/editor-ui/src/ui/segmented_control.rs:95:5` `ui::segmented_control::SegmentedControl::<T>::build` — it can be `pub(crate)`
1028. **nit** — `crates/editor-ui/src/ui/select.rs:6:1` `ui::select::OPTION_HEIGHT` — it can be `pub(crate)`
1029. **nit** — `crates/editor-ui/src/ui/select.rs:10:1` `ui::select::MENU_MIN_WIDTH` — it can be `pub(crate)`
1030. **nit** — `crates/editor-ui/src/ui/select.rs:16:5` `ui::select::SelectEntry::value` — it can be `pub(crate)`
1031. **nit** — `crates/editor-ui/src/ui/select.rs:17:5` `ui::select::SelectEntry::label` — it can be `pub(crate)`
1032. **nit** — `crates/editor-ui/src/ui/select.rs:18:5` `ui::select::SelectEntry::disabled` — it can be `pub(crate)`
1033. **nit** — `crates/editor-ui/src/ui/select.rs:39:5` `ui::select::SelectState::shown` — it can be `pub(crate)`
1034. **nit** — `crates/editor-ui/src/ui/select.rs:40:5` `ui::select::SelectState::motion` — it can be `pub(crate)`
1035. **nit** — `crates/editor-ui/src/ui/select.rs:41:5` `ui::select::SelectState::highlighted` — it can be `pub(crate)`
1036. **nit** — `crates/editor-ui/src/ui/select.rs:42:5` `ui::select::SelectState::keyed` — it can be `pub(crate)`
1037. **nit** — `crates/editor-ui/src/ui/select.rs:49:5` `ui::select::SelectKeyResult::handled` — it can be `pub(crate)`
1038. **nit** — `crates/editor-ui/src/ui/select.rs:50:5` `ui::select::SelectKeyResult::commit` — it can be `pub(crate)`
1039. **nit** — `crates/editor-ui/src/ui/select.rs:51:5` `ui::select::SelectKeyResult::reveal` — it can be `pub(crate)`
1040. **nit** — `crates/editor-ui/src/ui/select.rs:67:5` `ui::select::SelectState::<V>::reconcile` — it can be `pub(crate)`
1041. **nit** — `crates/editor-ui/src/ui/select.rs:89:5` `ui::select::SelectState::<V>::set_open` — it can be `pub(crate)`
1042. **nit** — `crates/editor-ui/src/ui/select.rs:120:5` `ui::select::SelectState::<V>::close_now` — it can be `pub(crate)`
1043. **nit** — `crates/editor-ui/src/ui/select.rs:127:5` `ui::select::SelectState::<V>::key` — it can be `pub(crate)`
1044. **nit** — `crates/editor-ui/src/ui/select.rs:490:5` `ui::select::SelectMenu::<V>::build` — it can be `pub(crate)`
1045. **nit** — `crates/editor-ui/src/ui/slider.rs:8:5` `ui::slider::SliderRange::new` — it can be `pub(crate)`
1046. **nit** — `crates/editor-ui/src/ui/slider.rs:29:5` `ui::slider::SliderRange::snap` — it can be `pub(crate)`
1047. **nit** — `crates/editor-ui/src/ui/slider.rs:46:5` `ui::slider::SliderRange::command` — it can be `pub(crate)`
1048. **nit** — `crates/editor-ui/src/ui/slider.rs:98:5` `ui::slider::SliderState::new` — it can be `pub(crate)`
1049. **nit** — `crates/editor-ui/src/ui/slider.rs:106:5` `ui::slider::SliderState::value` — it can be `pub(crate)`
1050. **nit** — `crates/editor-ui/src/ui/slider.rs:110:5` `ui::slider::SliderState::reset` — it can be `pub(crate)`
1051. **nit** — `crates/editor-ui/src/ui/slider.rs:143:1` `ui::slider::Slider` — it can be `pub(crate)`
1052. **nit** — `crates/editor-ui/src/ui/slider.rs:154:5` `ui::slider::Slider::new` — it can be `pub(crate)`
1053. **nit** — `crates/editor-ui/src/ui/slider.rs:176:5` `ui::slider::Slider::on_change` — it can be `pub(crate)`
1054. **nit** — `crates/editor-ui/src/ui/slider.rs:181:5` `ui::slider::Slider::build` — it can be `pub(crate)`
1055. **nit** — `crates/editor-ui/src/ui/status_chip.rs:25:5` `ui::status_chip::EngineChip::key` — it can be `pub(crate)`
1056. **nit** — `crates/editor-ui/src/ui/status_chip.rs:26:5` `ui::status_chip::EngineChip::engine` — it can be `pub(crate)`
1057. **nit** — `crates/editor-ui/src/ui/status_chip.rs:27:5` `ui::status_chip::EngineChip::label` — it can be `pub(crate)`
1058. **nit** — `crates/editor-ui/src/ui/status_chip.rs:28:5` `ui::status_chip::EngineChip::tone` — it can be `pub(crate)`
1059. **nit** — `crates/editor-ui/src/ui/status_chip.rs:29:5` `ui::status_chip::EngineChip::icon` — it can be `pub(crate)`
1060. **nit** — `crates/editor-ui/src/ui/status_chip.rs:30:5` `ui::status_chip::EngineChip::detail` — it can be `pub(crate)`
1061. **nit** — `crates/editor-ui/src/ui/status_chip.rs:54:1` `ui::status_chip::tone_colors` — it can be `pub(crate)`
1062. **nit** — `crates/editor-ui/src/ui/sticker.rs:37:5` `ui::sticker::StickerFrame::shift` — it can be `pub(crate)`
1063. **nit** — `crates/editor-ui/src/ui/sticker.rs:38:5` `ui::sticker::StickerFrame::scale` — it can be `pub(crate)`
1064. **nit** — `crates/editor-ui/src/ui/sticker.rs:39:5` `ui::sticker::StickerFrame::glue` — it can be `pub(crate)`
1065. **nit** — `crates/editor-ui/src/ui/sticker.rs:40:5` `ui::sticker::StickerFrame::curl` — it can be `pub(crate)`
1066. **nit** — `crates/editor-ui/src/ui/sticker.rs:99:5` `ui::sticker::Sticker::style` — it can be `pub(crate)`
1067. **nit** — `crates/editor-ui/src/ui/sticker.rs:110:5` `ui::sticker::Sticker::layers` — it can be `pub(crate)`
1068. **nit** — `crates/editor-ui/src/ui/tab_card.rs:128:1` `ui::tab_card::CARD_RADIUS` — it can be `pub(crate)`
1069. **nit** — `crates/editor-ui/src/ui/tab_card.rs:134:5` `ui::tab_card::GeometryKey::size` — it can be `pub(crate)`
1070. **nit** — `crates/editor-ui/src/ui/tab_card.rs:135:5` `ui::tab_card::GeometryKey::tab_width` — it can be `pub(crate)`
1071. **nit** — `crates/editor-ui/src/ui/tab_card.rs:136:5` `ui::tab_card::GeometryKey::radius` — it can be `pub(crate)`
1072. **nit** — `crates/editor-ui/src/ui/tab_card.rs:137:5` `ui::tab_card::GeometryKey::reveal` — it can be `pub(crate)`
1073. **nit** — `crates/editor-ui/src/ui/tab_card.rs:147:5` `ui::tab_card::GeometryCache::get` — it can be `pub(crate)`
1074. **nit** — `crates/editor-ui/src/ui/tab_card.rs:158:5` `ui::tab_card::Geometry::shell` — it can be `pub(crate)`
1075. **nit** — `crates/editor-ui/src/ui/tab_card.rs:159:5` `ui::tab_card::Geometry::edge` — it can be `pub(crate)`
1076. **nit** — `crates/editor-ui/src/ui/tab_card.rs:163:5` `ui::tab_card::Geometry::new` — it can be `pub(crate)`
1077. **nit** — `crates/editor-ui/src/ui/tab_card.rs:280:5` `ui::tab_card::ShadowPass::bounds` — it can be `pub(crate)`
1078. **nit** — `crates/editor-ui/src/ui/tab_card.rs:281:5` `ui::tab_card::ShadowPass::corners` — it can be `pub(crate)`
1079. **nit** — `crates/editor-ui/src/ui/tab_card.rs:282:5` `ui::tab_card::ShadowPass::layer` — it can be `pub(crate)`
1080. **nit** — `crates/editor-ui/src/ui/tab_card.rs:283:5` `ui::tab_card::ShadowPass::clip` — it can be `pub(crate)`
1081. **nit** — `crates/editor-ui/src/ui/tab_card.rs:287:5` `ui::tab_card::PathPass::transformed_window_logical_bounds` — it can be `pub(crate)`
1082. **nit** — `crates/editor-ui/src/ui/tab_card.rs:288:5` `ui::tab_card::PathPass::path` — it can be `pub(crate)`
1083. **nit** — `crates/editor-ui/src/ui/tab_card.rs:292:5` `ui::tab_card::PreparedTabCard::shadows` — it can be `pub(crate)`
1084. **nit** — `crates/editor-ui/src/ui/tab_card.rs:293:5` `ui::tab_card::PreparedTabCard::backdrop` — it can be `pub(crate)`
1085. **nit** — `crates/editor-ui/src/ui/tab_card.rs:294:5` `ui::tab_card::PreparedTabCard::paths` — it can be `pub(crate)`
1086. **nit** — `crates/editor-ui/src/ui/tab_card.rs:340:5` `ui::tab_card::PreparedTabCard::display_items` — it can be `pub(crate)`
1087. **nit** — `crates/editor-ui/src/ui/tab_card.rs:364:5` `ui::tab_card::PreparedTabCard::new` — it can be `pub(crate)`
1088. **nit** — `crates/editor-ui/src/ui/text/editor.rs:269:5` `ui::text::editor::Editor::replacement_range` — it can be `pub(crate)`
1089. **nit** — `crates/editor-ui/src/ui/text/editor.rs:382:5` `ui::text::editor::Editor::text_for_utf16_range` — it can be `pub(crate)`
1090. **nit** — `crates/editor-ui/src/ui/text/editor.rs:414:5` `ui::text::editor::Editor::copy` — it can be `pub(crate)`
1091. **nit** — `crates/editor-ui/src/ui/text/editor.rs:419:5` `ui::text::editor::Editor::cut` — it can be `pub(crate)`
1092. **nit** — `crates/editor-ui/src/ui/text/editor.rs:427:5` `ui::text::editor::Editor::token_target` — it can be `pub(crate)`
1093. **nit** — `crates/editor-ui/src/ui/text/editor.rs:439:5` `ui::text::editor::Editor::token_deletion` — it can be `pub(crate)`
1094. **nit** — `crates/editor-ui/src/ui/text/editor.rs:488:5` `ui::text::editor::Editor::list_vertical` — it can be `pub(crate)`
1095. **nit** — `crates/editor-ui/src/ui/text/editor.rs:492:5` `ui::text::editor::Editor::dismiss` — it can be `pub(crate)`
1096. **nit** — `crates/editor-ui/src/ui/text/editor.rs:618:5` `ui::text::editor::Editor::move_line_edge` — it can be `pub(crate)`
1097. **nit** — `crates/editor-ui/src/ui/text/editor.rs:787:1` `ui::text::editor::utf16_offset_to_utf8` — it can be `pub(crate)`
1098. **nit** — `crates/editor-ui/src/ui/text/editor.rs:800:1` `ui::text::editor::utf16_range_to_utf8` — it can be `pub(crate)`
1099. **nit** — `crates/editor-ui/src/ui/text/editor.rs:806:1` `ui::text::editor::utf8_range_to_utf16` — it can be `pub(crate)`
1100. **nit** — `crates/editor-ui/src/ui/text/metrics.rs:6:1` `ui::text::metrics::EditorMetrics` — it can be `pub(crate)`
1101. **nit** — `crates/editor-ui/src/ui/text/metrics.rs:7:5` `ui::text::metrics::EditorMetrics::text_style` — it can be `pub(crate)`
1102. **nit** — `crates/editor-ui/src/ui/text/metrics.rs:8:5` `ui::text::metrics::EditorMetrics::line_height` — it can be `pub(crate)`
1103. **nit** — `crates/editor-ui/src/ui/text/metrics.rs:9:5` `ui::text::metrics::EditorMetrics::visible_lines` — it can be `pub(crate)`
1104. **nit** — `crates/editor-ui/src/ui/text/metrics.rs:10:5` `ui::text::metrics::EditorMetrics::padding_x` — it can be `pub(crate)`
1105. **nit** — `crates/editor-ui/src/ui/text/metrics.rs:11:5` `ui::text::metrics::EditorMetrics::padding_y` — it can be `pub(crate)`
1106. **nit** — `crates/editor-ui/src/ui/text/metrics.rs:12:5` `ui::text::metrics::EditorMetrics::radius` — it can be `pub(crate)`
1107. **nit** — `crates/editor-ui/src/ui/text/metrics.rs:13:5` `ui::text::metrics::EditorMetrics::border_width` — it can be `pub(crate)`
1108. **nit** — `crates/editor-ui/src/ui/text/metrics.rs:14:5` `ui::text::metrics::EditorMetrics::foreground` — it can be `pub(crate)`
1109. **nit** — `crates/editor-ui/src/ui/text/metrics.rs:15:5` `ui::text::metrics::EditorMetrics::caret` — it can be `pub(crate)`
1110. **nit** — `crates/editor-ui/src/ui/text/metrics.rs:16:5` `ui::text::metrics::EditorMetrics::placeholder` — it can be `pub(crate)`
1111. **nit** — `crates/editor-ui/src/ui/text/metrics.rs:17:5` `ui::text::metrics::EditorMetrics::background` — it can be `pub(crate)`
1112. **nit** — `crates/editor-ui/src/ui/text/metrics.rs:18:5` `ui::text::metrics::EditorMetrics::border` — it can be `pub(crate)`
1113. **nit** — `crates/editor-ui/src/ui/text/metrics.rs:19:5` `ui::text::metrics::EditorMetrics::focus_border` — it can be `pub(crate)`
1114. **nit** — `crates/editor-ui/src/ui/text/metrics.rs:20:5` `ui::text::metrics::EditorMetrics::selection` — it can be `pub(crate)`
1115. **nit** — `crates/editor-ui/src/ui/text/metrics.rs:21:5` `ui::text::metrics::EditorMetrics::disabled_opacity` — it can be `pub(crate)`
1116. **nit** — `crates/editor-ui/src/ui/text/metrics.rs:25:5` `ui::text::metrics::EditorMetrics::resolve` — it can be `pub(crate)`
1117. **nit** — `crates/editor-ui/src/ui/text/metrics.rs:86:5` `ui::text::metrics::EditorMetrics::text_appearance` — it can be `pub(crate)`
1118. **nit** — `crates/editor-ui/src/ui/text/metrics.rs:96:5` `ui::text::metrics::EditorMetrics::gpui_text_appearance` — it can be `pub(crate)`
1119. **nit** — `crates/editor-ui/src/ui/text/metrics.rs:107:5` `ui::text::metrics::EditorMetrics::min_height` — it can be `pub(crate)`
1120. **nit** — `crates/editor-ui/src/ui/text/tokens.rs:12:5` `ui::text::tokens::GlyphToken::asset` — it can be `pub(crate)`
1121. **nit** — `crates/editor-ui/src/ui/text/tokens.rs:13:5` `ui::text::tokens::GlyphToken::color` — it can be `pub(crate)`
1122. **nit** — `crates/editor-ui/src/ui/text/wheel_glide.rs:35:5` `ui::text::wheel_glide::WheelGlide::push` — it can be `pub(crate)`
1123. **nit** — `crates/editor-ui/src/ui/text/wheel_glide.rs:52:5` `ui::text::wheel_glide::WheelGlide::step` — it can be `pub(crate)`
1124. **nit** — `crates/editor-ui/src/ui/text/wheel_glide.rs:75:5` `ui::text::wheel_glide::WheelGlide::step_elapsed` — it can be `pub(crate)`
1125. **nit** — `crates/editor-ui/src/ui/theme.rs:15:5` `ui::theme::SrgbComponents::a` — it can be `pub(crate)`
1126. **nit** — `crates/editor-ui/src/ui/theme.rs:209:5` `ui::theme::Oklch::new` — it can be `pub(crate)`
1127. **nit** — `crates/editor-ui/src/ui/theme.rs:307:5` `ui::theme::Oklch::to_palette_paint` — it can be `pub(crate)`
1128. **nit** — `crates/editor-ui/src/ui/theme.rs:361:5` `ui::theme::StatusScale::bg` — it can be `pub(crate)`
1129. **nit** — `crates/editor-ui/src/ui/theme.rs:362:5` `ui::theme::StatusScale::border` — it can be `pub(crate)`
1130. **nit** — `crates/editor-ui/src/ui/theme.rs:365:5` `ui::theme::StatusScale::contrast` — it can be `pub(crate)`
1131. **nit** — `crates/editor-ui/src/ui/theme.rs:369:5` `ui::theme::BannerColors::bg` — it can be `pub(crate)`
1132. **nit** — `crates/editor-ui/src/ui/theme.rs:370:5` `ui::theme::BannerColors::border` — it can be `pub(crate)`
1133. **nit** — `crates/editor-ui/src/ui/theme.rs:375:5` `ui::theme::SyntaxColors::comment` — it can be `pub(crate)`
1134. **nit** — `crates/editor-ui/src/ui/theme.rs:376:5` `ui::theme::SyntaxColors::string` — it can be `pub(crate)`
1135. **nit** — `crates/editor-ui/src/ui/theme.rs:377:5` `ui::theme::SyntaxColors::number` — it can be `pub(crate)`
1136. **nit** — `crates/editor-ui/src/ui/theme.rs:378:5` `ui::theme::SyntaxColors::keyword` — it can be `pub(crate)`
1137. **nit** — `crates/editor-ui/src/ui/theme.rs:379:5` `ui::theme::SyntaxColors::r#type` — it can be `pub(crate)`
1138. **nit** — `crates/editor-ui/src/ui/theme.rs:380:5` `ui::theme::SyntaxColors::function` — it can be `pub(crate)`
1139. **nit** — `crates/editor-ui/src/ui/theme.rs:388:5` `ui::theme::ColorTokens::accent` — it can be `pub(crate)`
1140. **nit** — `crates/editor-ui/src/ui/theme.rs:389:5` `ui::theme::ColorTokens::accent_contrast` — it can be `pub(crate)`
1141. **nit** — `crates/editor-ui/src/ui/theme.rs:395:5` `ui::theme::ColorTokens::white` — it can be `pub(crate)`
1142. **nit** — `crates/editor-ui/src/ui/theme.rs:396:5` `ui::theme::ColorTokens::scrim` — it can be `pub(crate)`
1143. **nit** — `crates/editor-ui/src/ui/theme.rs:397:5` `ui::theme::ColorTokens::shadow` — it can be `pub(crate)`
1144. **nit** — `crates/editor-ui/src/ui/theme.rs:398:5` `ui::theme::ColorTokens::palette` — it can be `pub(crate)`
1145. **nit** — `crates/editor-ui/src/ui/theme.rs:404:5` `ui::theme::ColorTokens::raised` — it can be `pub(crate)`
1146. **nit** — `crates/editor-ui/src/ui/theme.rs:405:5` `ui::theme::ColorTokens::hover` — it can be `pub(crate)`
1147. **nit** — `crates/editor-ui/src/ui/theme.rs:406:5` `ui::theme::ColorTokens::selected` — it can be `pub(crate)`
1148. **nit** — `crates/editor-ui/src/ui/theme.rs:407:5` `ui::theme::ColorTokens::pressed` — it can be `pub(crate)`
1149. **nit** — `crates/editor-ui/src/ui/theme.rs:409:5` `ui::theme::ColorTokens::line_strong` — it can be `pub(crate)`
1150. **nit** — `crates/editor-ui/src/ui/theme.rs:413:5` `ui::theme::ColorTokens::text_faint` — it can be `pub(crate)`
1151. **nit** — `crates/editor-ui/src/ui/theme.rs:414:5` `ui::theme::ColorTokens::ring` — it can be `pub(crate)`
1152. **nit** — `crates/editor-ui/src/ui/theme.rs:415:5` `ui::theme::ColorTokens::text_selection` — it can be `pub(crate)`
1153. **nit** — `crates/editor-ui/src/ui/theme.rs:416:5` `ui::theme::ColorTokens::primary` — it can be `pub(crate)`
1154. **nit** — `crates/editor-ui/src/ui/theme.rs:417:5` `ui::theme::ColorTokens::primary_hover` — it can be `pub(crate)`
1155. **nit** — `crates/editor-ui/src/ui/theme.rs:418:5` `ui::theme::ColorTokens::primary_text` — it can be `pub(crate)`
1156. **nit** — `crates/editor-ui/src/ui/theme.rs:420:5` `ui::theme::ColorTokens::action_bottom` — it can be `pub(crate)`
1157. **nit** — `crates/editor-ui/src/ui/theme.rs:421:5` `ui::theme::ColorTokens::action_hover_top` — it can be `pub(crate)`
1158. **nit** — `crates/editor-ui/src/ui/theme.rs:422:5` `ui::theme::ColorTokens::action_hover_bottom` — it can be `pub(crate)`
1159. **nit** — `crates/editor-ui/src/ui/theme.rs:423:5` `ui::theme::ColorTokens::action_text` — it can be `pub(crate)`
1160. **nit** — `crates/editor-ui/src/ui/theme.rs:424:5` `ui::theme::ColorTokens::destructive` — it can be `pub(crate)`
1161. **nit** — `crates/editor-ui/src/ui/theme.rs:425:5` `ui::theme::ColorTokens::destructive_hover` — it can be `pub(crate)`
1162. **nit** — `crates/editor-ui/src/ui/theme.rs:427:5` `ui::theme::ColorTokens::destructive_contrast` — it can be `pub(crate)`
1163. **nit** — `crates/editor-ui/src/ui/theme.rs:430:5` `ui::theme::ColorTokens::banner_success` — it can be `pub(crate)`
1164. **nit** — `crates/editor-ui/src/ui/theme.rs:432:5` `ui::theme::ColorTokens::favourite` — it can be `pub(crate)`
1165. **nit** — `crates/editor-ui/src/ui/theme.rs:435:5` `ui::theme::ColorTokens::question_tint` — it can be `pub(crate)`
1166. **nit** — `crates/editor-ui/src/ui/theme.rs:440:5` `ui::theme::ColorTokens::syntax` — it can be `pub(crate)`
1167. **nit** — `crates/editor-ui/src/ui/theme.rs:747:5` `ui::theme::TypographyTokens::fonts` — it can be `pub(crate)`
1168. **nit** — `crates/editor-ui/src/ui/theme.rs:750:5` `ui::theme::TypographyTokens::heading` — it can be `pub(crate)`
1169. **nit** — `crates/editor-ui/src/ui/theme.rs:752:5` `ui::theme::TypographyTokens::control_text` — it can be `pub(crate)`
1170. **nit** — `crates/editor-ui/src/ui/theme.rs:753:5` `ui::theme::TypographyTokens::editor_text_desktop` — it can be `pub(crate)`
1171. **nit** — `crates/editor-ui/src/ui/theme.rs:754:5` `ui::theme::TypographyTokens::label_text` — it can be `pub(crate)`
1172. **nit** — `crates/editor-ui/src/ui/theme.rs:797:5` `ui::theme::FontSet::ALL` — it can be `pub(crate)`
1173. **nit** — `crates/editor-ui/src/ui/theme.rs:806:5` `ui::theme::FontSet::from_key` — it can be `pub(crate)`
1174. **nit** — `crates/editor-ui/src/ui/theme.rs:845:5` `ui::theme::ProseHeading::size_px` — it can be `pub(crate)`
1175. **nit** — `crates/editor-ui/src/ui/theme.rs:846:5` `ui::theme::ProseHeading::line_px` — it can be `pub(crate)`
1176. **nit** — `crates/editor-ui/src/ui/theme.rs:847:5` `ui::theme::ProseHeading::tracking_px` — it can be `pub(crate)`
1177. **nit** — `crates/editor-ui/src/ui/theme.rs:848:5` `ui::theme::ProseHeading::margin_top_px` — it can be `pub(crate)`
1178. **nit** — `crates/editor-ui/src/ui/theme.rs:849:5` `ui::theme::ProseHeading::margin_bottom_px` — it can be `pub(crate)`
1179. **nit** — `crates/editor-ui/src/ui/theme.rs:853:5` `ui::theme::ProseTypography::BODY_SIZE_PX` — it can be `pub(crate)`
1180. **nit** — `crates/editor-ui/src/ui/theme.rs:854:5` `ui::theme::ProseTypography::BODY_LINE_PX` — it can be `pub(crate)`
1181. **nit** — `crates/editor-ui/src/ui/theme.rs:855:5` `ui::theme::ProseTypography::BODY_WEIGHT` — it can be `pub(crate)`
1182. **nit** — `crates/editor-ui/src/ui/theme.rs:857:5` `ui::theme::ProseTypography::TRACKING_EM` — it can be `pub(crate)`
1183. **nit** — `crates/editor-ui/src/ui/theme.rs:862:5` `ui::theme::ProseTypography::HEADING_WEIGHT` — it can be `pub(crate)`
1184. **nit** — `crates/editor-ui/src/ui/theme.rs:864:5` `ui::theme::ProseTypography::LINK_WEIGHT` — it can be `pub(crate)`
1185. **nit** — `crates/editor-ui/src/ui/theme.rs:865:5` `ui::theme::ProseTypography::CODE_SIZE_PX` — it can be `pub(crate)`
1186. **nit** — `crates/editor-ui/src/ui/theme.rs:866:5` `ui::theme::ProseTypography::CODE_LINE_PX` — it can be `pub(crate)`
1187. **nit** — `crates/editor-ui/src/ui/theme.rs:868:5` `ui::theme::ProseTypography::PARAGRAPH_MARGIN_PX` — it can be `pub(crate)`
1188. **nit** — `crates/editor-ui/src/ui/theme.rs:869:5` `ui::theme::ProseTypography::FENCE_MARGIN_PX` — it can be `pub(crate)`
1189. **nit** — `crates/editor-ui/src/ui/theme.rs:870:5` `ui::theme::ProseTypography::LIST_MARGIN_PX` — it can be `pub(crate)`
1190. **nit** — `crates/editor-ui/src/ui/theme.rs:871:5` `ui::theme::ProseTypography::LIST_INDENT_PX` — it can be `pub(crate)`
1191. **nit** — `crates/editor-ui/src/ui/theme.rs:872:5` `ui::theme::ProseTypography::ITEM_GAP_PX` — it can be `pub(crate)`
1192. **nit** — `crates/editor-ui/src/ui/theme.rs:873:5` `ui::theme::ProseTypography::ITEM_PARAGRAPH_MARGIN_PX` — it can be `pub(crate)`
1193. **nit** — `crates/editor-ui/src/ui/theme.rs:874:5` `ui::theme::ProseTypography::NESTED_LIST_MARGIN_PX` — it can be `pub(crate)`
1194. **nit** — `crates/editor-ui/src/ui/theme.rs:875:5` `ui::theme::ProseTypography::TABLE_SIZE_PX` — it can be `pub(crate)`
1195. **nit** — `crates/editor-ui/src/ui/theme.rs:876:5` `ui::theme::ProseTypography::TABLE_LINE_PX` — it can be `pub(crate)`
1196. **nit** — `crates/editor-ui/src/ui/theme.rs:877:5` `ui::theme::ProseTypography::TABLE_MARGIN_PX` — it can be `pub(crate)`
1197. **nit** — `crates/editor-ui/src/ui/theme.rs:878:5` `ui::theme::ProseTypography::TABLE_CELL_PAD_PX` — it can be `pub(crate)`
1198. **nit** — `crates/editor-ui/src/ui/theme.rs:880:5` `ui::theme::ProseTypography::heading` — it can be `pub(crate)`
1199. **nit** — `crates/editor-ui/src/ui/theme.rs:917:5` `ui::theme::SpacingTokens::BASE_PX` — it can be `pub(crate)`
1200. **nit** — `crates/editor-ui/src/ui/theme.rs:919:5` `ui::theme::SpacingTokens::steps` — it can be `pub(crate)`
1201. **nit** — `crates/editor-ui/src/ui/theme.rs:946:5` `ui::theme::RadiusTokens::value` — it can be `pub(crate)`
1202. **nit** — `crates/editor-ui/src/ui/theme.rs:972:5` `ui::theme::ShadowLayer::color` — it can be `pub(crate)`
1203. **nit** — `crates/editor-ui/src/ui/theme.rs:973:5` `ui::theme::ShadowLayer::offset_x` — it can be `pub(crate)`
1204. **nit** — `crates/editor-ui/src/ui/theme.rs:974:5` `ui::theme::ShadowLayer::offset_y` — it can be `pub(crate)`
1205. **nit** — `crates/editor-ui/src/ui/theme.rs:975:5` `ui::theme::ShadowLayer::blur_radius` — it can be `pub(crate)`
1206. **nit** — `crates/editor-ui/src/ui/theme.rs:976:5` `ui::theme::ShadowLayer::spread_radius` — it can be `pub(crate)`
1207. **nit** — `crates/editor-ui/src/ui/theme.rs:1007:5` `ui::theme::ElevationTokens::hairline` — it can be `pub(crate)`
1208. **nit** — `crates/editor-ui/src/ui/theme.rs:1008:5` `ui::theme::ElevationTokens::raised` — it can be `pub(crate)`
1209. **nit** — `crates/editor-ui/src/ui/theme.rs:1009:5` `ui::theme::ElevationTokens::floating` — it can be `pub(crate)`
1210. **nit** — `crates/editor-ui/src/ui/theme.rs:1010:5` `ui::theme::ElevationTokens::overlay` — it can be `pub(crate)`
1211. **nit** — `crates/editor-ui/src/ui/theme.rs:1015:5` `ui::theme::Cast::dy` — it can be `pub(crate)`
1212. **nit** — `crates/editor-ui/src/ui/theme.rs:1016:5` `ui::theme::Cast::blur` — it can be `pub(crate)`
1213. **nit** — `crates/editor-ui/src/ui/theme.rs:1017:5` `ui::theme::Cast::spread` — it can be `pub(crate)`
1214. **nit** — `crates/editor-ui/src/ui/theme.rs:1018:5` `ui::theme::Cast::alpha` — it can be `pub(crate)`
1215. **nit** — `crates/editor-ui/src/ui/theme.rs:1022:5` `ui::theme::Cast::KEY` — it can be `pub(crate)`
1216. **nit** — `crates/editor-ui/src/ui/theme.rs:1026:5` `ui::theme::Cast::REST` — it can be `pub(crate)`
1217. **nit** — `crates/editor-ui/src/ui/theme.rs:1030:5` `ui::theme::Cast::FLOAT` — it can be `pub(crate)`
1218. **nit** — `crates/editor-ui/src/ui/theme.rs:1034:5` `ui::theme::Cast::OVERLAY` — it can be `pub(crate)`
1219. **nit** — `crates/editor-ui/src/ui/theme.rs:1038:5` `ui::theme::Cast::PRESS` — it can be `pub(crate)`
1220. **nit** — `crates/editor-ui/src/ui/theme.rs:1042:5` `ui::theme::Cast::LIFT` — it can be `pub(crate)`
1221. **nit** — `crates/editor-ui/src/ui/theme.rs:1051:5` `ui::theme::Cast::new` — it can be `pub(crate)`
1222. **nit** — `crates/editor-ui/src/ui/theme.rs:1061:5` `ui::theme::Cast::alpha_in` — it can be `pub(crate)`
1223. **nit** — `crates/editor-ui/src/ui/theme.rs:1071:5` `ui::theme::ElevationTokens::drops` — it can be `pub(crate)`
1224. **nit** — `crates/editor-ui/src/ui/theme.rs:1120:5` `ui::theme::DensityTokens::control_default` — it can be `pub(crate)`
1225. **nit** — `crates/editor-ui/src/ui/theme.rs:1121:5` `ui::theme::DensityTokens::command_list_max_height` — it can be `pub(crate)`
1226. **nit** — `crates/editor-ui/src/ui/theme.rs:1122:5` `ui::theme::DensityTokens::control_sm` — it can be `pub(crate)`
1227. **nit** — `crates/editor-ui/src/ui/theme.rs:1123:5` `ui::theme::DensityTokens::switch_default` — it can be `pub(crate)`
1228. **nit** — `crates/editor-ui/src/ui/theme.rs:1124:5` `ui::theme::DensityTokens::badge_height` — it can be `pub(crate)`
1229. **nit** — `crates/editor-ui/src/ui/theme.rs:1125:5` `ui::theme::DensityTokens::card_padding_compact` — it can be `pub(crate)`
1230. **nit** — `crates/editor-ui/src/ui/theme.rs:1129:5` `ui::theme::InteractionTokens::focus_ring_width` — it can be `pub(crate)`
1231. **nit** — `crates/editor-ui/src/ui/theme.rs:1135:5` `ui::theme::ArtisanTheme::spacing` — it can be `pub(crate)`
1232. **nit** — `crates/editor-ui/src/ui/theme.rs:1136:5` `ui::theme::ArtisanTheme::radius` — it can be `pub(crate)`
1233. **nit** — `crates/editor-ui/src/ui/theme.rs:1137:5` `ui::theme::ArtisanTheme::elevation` — it can be `pub(crate)`
1234. **nit** — `crates/editor-ui/src/ui/theme.rs:1138:5` `ui::theme::ArtisanTheme::density` — it can be `pub(crate)`
1235. **nit** — `crates/editor-ui/src/ui/theme.rs:1139:5` `ui::theme::ArtisanTheme::interaction` — it can be `pub(crate)`
1236. **nit** — `crates/editor-ui/src/ui/theme.rs:1166:5` `ui::theme::TypographyTokens::heading_tracking_px` — it can be `pub(crate)`

**editor-conversation** (231)

1237. **nit** — `crates/editor-conversation/src/conversation/activity.rs:58:1` `conversation::activity::category` — it can be `pub(crate)`
1238. **nit** — `crates/editor-conversation/src/conversation/activity.rs:138:5` `conversation::activity::Line::verb` — it can be `pub(crate)`
1239. **nit** — `crates/editor-conversation/src/conversation/activity.rs:139:5` `conversation::activity::Line::object` — it can be `pub(crate)`
1240. **nit** — `crates/editor-conversation/src/conversation/activity.rs:140:5` `conversation::activity::Line::code` — it can be `pub(crate)`
1241. **nit** — `crates/editor-conversation/src/conversation/activity.rs:147:1` `conversation::activity::line` — it can be `pub(crate)`
1242. **nit** — `crates/editor-conversation/src/conversation/activity.rs:420:1` `conversation::activity::doing` — it can be `pub(crate)`
1243. **nit** — `crates/editor-conversation/src/conversation/activity.rs:698:5` `conversation::activity::AgentMessage::to` — it can be `pub(crate)`
1244. **nit** — `crates/editor-conversation/src/conversation/activity.rs:699:5` `conversation::activity::AgentMessage::direction` — it can be `pub(crate)`
1245. **nit** — `crates/editor-conversation/src/conversation/activity.rs:700:5` `conversation::activity::AgentMessage::summary` — it can be `pub(crate)`
1246. **nit** — `crates/editor-conversation/src/conversation/activity.rs:701:5` `conversation::activity::AgentMessage::text` — it can be `pub(crate)`
1247. **nit** — `crates/editor-conversation/src/conversation/activity.rs:704:1` `conversation::activity::agent_message` — it can be `pub(crate)`
1248. **nit** — `crates/editor-conversation/src/conversation/activity.rs:740:1` `conversation::activity::chain_categories` — it can be `pub(crate)`
1249. **nit** — `crates/editor-conversation/src/conversation/activity.rs:744:1` `conversation::activity::chain_clause` — it can be `pub(crate)`
1250. **nit** — `crates/editor-conversation/src/conversation/activity.rs:769:1` `conversation::activity::chain_icon` — it can be `pub(crate)`
1251. **nit** — `crates/editor-conversation/src/conversation/adapter.rs:386:1` `conversation::adapter::read_boundary` — it can be `pub(crate)`
1252. **nit** — `crates/editor-conversation/src/conversation/admission.rs:5:1` `conversation::admission::immediate` — it can be `pub(crate)`
1253. **nit** — `crates/editor-conversation/src/conversation/admission.rs:61:1` `conversation::admission::behind_reveal` — it can be `pub(crate)`
1254. **nit** — `crates/editor-conversation/src/conversation/admission.rs:170:5` `conversation::admission::LiveBlockArrivals::seen` — it can be `pub(crate)`
1255. **nit** — `crates/editor-conversation/src/conversation/admission.rs:176:5` `conversation::admission::LiveBlockArrivals::baseline` — it can be `pub(crate)`
1256. **nit** — `crates/editor-conversation/src/conversation/admission.rs:184:5` `conversation::admission::LiveBlockArrivals::observed` — it can be `pub(crate)`
1257. **nit** — `crates/editor-conversation/src/conversation/admission.rs:188:5` `conversation::admission::LiveBlockArrivals::prepared` — it can be `pub(crate)`
1258. **nit** — `crates/editor-conversation/src/conversation/admission.rs:206:5` `conversation::admission::LiveBlockArrivals::claim` — it can be `pub(crate)`
1259. **nit** — `crates/editor-conversation/src/conversation/compaction.rs:20:5` `conversation::compaction::CompactionPhase::label` — it can be `pub(crate)`
1260. **nit** — `crates/editor-conversation/src/conversation/compaction.rs:132:1` `conversation::compaction::eased` — it can be `pub(crate)`
1261. **nit** — `crates/editor-conversation/src/conversation/content.rs:157:5` `conversation::content::SourceAction::matches_item` — it can be `pub(crate)`
1262. **nit** — `crates/editor-conversation/src/conversation/content.rs:174:5` `conversation::content::SourceExport::item` — it can be `pub(crate)`
1263. **nit** — `crates/editor-conversation/src/conversation/content.rs:177:5` `conversation::content::SourceExport::matches_item` — it can be `pub(crate)`
1264. **nit** — `crates/editor-conversation/src/conversation/content.rs:200:5` `conversation::content::SourceOwners::dispatch_changed` — it can be `pub(crate)`
1265. **nit** — `crates/editor-conversation/src/conversation/content.rs:215:5` `conversation::content::SourceOwners::drain_all` — it can be `pub(crate)`
1266. **nit** — `crates/editor-conversation/src/conversation/content.rs:273:5` `conversation::content::SourceOwners::clear` — it can be `pub(crate)`
1267. **nit** — `crates/editor-conversation/src/conversation/controller.rs:30:1` `conversation::controller::completion_is_current` — it can be `pub(crate)`
1268. **nit** — `crates/editor-conversation/src/conversation/controller.rs:113:5` `conversation::controller::Conversation::runs` — it can be `pub(crate)`
1269. **nit** — `crates/editor-conversation/src/conversation/controller.rs:116:5` `conversation::controller::Conversation::question_collection` — it can be `pub(crate)`
1270. **nit** — `crates/editor-conversation/src/conversation/controller.rs:121:5` `conversation::controller::Conversation::reveal_frontier` — it can be `pub(crate)`
1271. **nit** — `crates/editor-conversation/src/conversation/controller.rs:122:5` `conversation::controller::Conversation::arrivals` — it can be `pub(crate)`
1272. **nit** — `crates/editor-conversation/src/conversation/controller.rs:131:5` `conversation::controller::Conversation::scroll_frame` — it can be `pub(crate)`
1273. **nit** — `crates/editor-conversation/src/conversation/controller.rs:155:5` `conversation::controller::Conversation::row_position` — it can be `pub(crate)`
1274. **nit** — `crates/editor-conversation/src/conversation/controller.rs:426:5` `conversation::controller::Conversation::close` — it can be `pub(crate)`
1275. **nit** — `crates/editor-conversation/src/conversation/controller.rs:497:5` `conversation::controller::Conversation::work_idle` — it can be `pub(crate)`
1276. **nit** — `crates/editor-conversation/src/conversation/controller.rs:501:5` `conversation::controller::Conversation::landed` — it can be `pub(crate)`
1277. **nit** — `crates/editor-conversation/src/conversation/controller.rs:587:5` `conversation::controller::Conversation::jump_row` — it can be `pub(crate)`
1278. **nit** — `crates/editor-conversation/src/conversation/controller.rs:676:5` `conversation::controller::Conversation::observe_visible_rows` — it can be `pub(crate)`
1279. **nit** — `crates/editor-conversation/src/conversation/controller.rs:740:5` `conversation::controller::Conversation::observe_live_preparation` — it can be `pub(crate)`
1280. **nit** — `crates/editor-conversation/src/conversation/controller.rs:841:5` `conversation::controller::Conversation::publish_projections` — it can be `pub(crate)`
1281. **nit** — `crates/editor-conversation/src/conversation/controller.rs:872:5` `conversation::controller::Conversation::body_shape_count` — it can be `pub(crate)`
1282. **nit** — `crates/editor-conversation/src/conversation/controller.rs:879:5` `conversation::controller::Conversation::body_version` — it can be `pub(crate)`
1283. **nit** — `crates/editor-conversation/src/conversation/controller.rs:949:5` `conversation::controller::Conversation::load_work` — it can be `pub(crate)`
1284. **nit** — `crates/editor-conversation/src/conversation/controller.rs:1125:5` `conversation::controller::Conversation::toggle_work` — it can be `pub(crate)`
1285. **nit** — `crates/editor-conversation/src/conversation/controller.rs:1155:5` `conversation::controller::Conversation::toggle_chain` — it can be `pub(crate)`
1286. **nit** — `crates/editor-conversation/src/conversation/controller.rs:1178:5` `conversation::controller::Conversation::toggle_item` — it can be `pub(crate)`
1287. **nit** — `crates/editor-conversation/src/conversation/controller.rs:1185:5` `conversation::controller::Conversation::answer` — it can be `pub(crate)`
1288. **nit** — `crates/editor-conversation/src/conversation/controller.rs:1337:5` `conversation::controller::Conversation::generation` — it can be `pub(crate)`
1289. **nit** — `crates/editor-conversation/src/conversation/disclosure.rs:11:5` `conversation::disclosure::DisclosureState::disclosure_clips` — it can be `pub(crate)`
1290. **nit** — `crates/editor-conversation/src/conversation/disclosure.rs:12:5` `conversation::disclosure::DisclosureState::disclosure_hidden` — it can be `pub(crate)`
1291. **nit** — `crates/editor-conversation/src/conversation/disclosure.rs:13:5` `conversation::disclosure::DisclosureState::disclosure_fractions` — it can be `pub(crate)`
1292. **nit** — `crates/editor-conversation/src/conversation/disclosure.rs:14:5` `conversation::disclosure::DisclosureState::disclosure_gaps` — it can be `pub(crate)`
1293. **nit** — `crates/editor-conversation/src/conversation/disclosure.rs:22:5` `conversation::disclosure::DisclosureFrame::released` — it can be `pub(crate)`
1294. **nit** — `crates/editor-conversation/src/conversation/disclosure.rs:31:5` `conversation::disclosure::RowClip::height` — it can be `pub(crate)`
1295. **nit** — `crates/editor-conversation/src/conversation/disclosure.rs:32:5` `conversation::disclosure::RowClip::opacity` — it can be `pub(crate)`
1296. **nit** — `crates/editor-conversation/src/conversation/disclosure.rs:73:5` `conversation::disclosure::DisclosureState::start_disclosure` — it can be `pub(crate)`
1297. **nit** — `crates/editor-conversation/src/conversation/disclosure.rs:156:5` `conversation::disclosure::DisclosureState::retain_disclosure_rows` — it can be `pub(crate)`
1298. **nit** — `crates/editor-conversation/src/conversation/disclosure.rs:180:5` `conversation::disclosure::DisclosureState::index_disclosure_rows` — it can be `pub(crate)`
1299. **nit** — `crates/editor-conversation/src/conversation/disclosure.rs:193:5` `conversation::disclosure::DisclosureState::advance_disclosures` — it can be `pub(crate)`
1300. **nit** — `crates/editor-conversation/src/conversation/disclosure.rs:300:5` `conversation::disclosure::DisclosureState::row_disclosure` — it can be `pub(crate)`
1301. **nit** — `crates/editor-conversation/src/conversation/fold.rs:145:1` `conversation::fold::selection_order` — it can be `pub(crate)`
1302. **nit** — `crates/editor-conversation/src/conversation/fold.rs:165:1` `conversation::fold::plain_ranges` — it can be `pub(crate)`
1303. **nit** — `crates/editor-conversation/src/conversation/fold.rs:312:5` `conversation::fold::Row::semantic` — it can be `pub(crate)`
1304. **nit** — `crates/editor-conversation/src/conversation/fold.rs:504:1` `conversation::fold::Folded` — it can be `pub(crate)`
1305. **nit** — `crates/editor-conversation/src/conversation/fold.rs:505:5` `conversation::fold::Folded::rows` — it can be `pub(crate)`
1306. **nit** — `crates/editor-conversation/src/conversation/fold.rs:506:5` `conversation::fold::Folded::runs` — it can be `pub(crate)`
1307. **nit** — `crates/editor-conversation/src/conversation/fold.rs:509:1` `conversation::fold::in_composer_strip` — it can be `pub(crate)`
1308. **nit** — `crates/editor-conversation/src/conversation/fold.rs:519:1` `conversation::fold::turn_of` — it can be `pub(crate)`
1309. **nit** — `crates/editor-conversation/src/conversation/fold.rs:558:1` `conversation::fold::fold` — it can be `pub(crate)`
1310. **nit** — `crates/editor-conversation/src/conversation/fold.rs:812:1` `conversation::fold::span_member` — it can be `pub(crate)`
1311. **nit** — `crates/editor-conversation/src/conversation/fold.rs:843:1` `conversation::fold::expanded` — it can be `pub(crate)`
1312. **nit** — `crates/editor-conversation/src/conversation/fold.rs:1060:1` `conversation::fold::agent_report` — it can be `pub(crate)`
1313. **nit** — `crates/editor-conversation/src/conversation/fold.rs:1094:1` `conversation::fold::children` — it can be `pub(crate)`
1314. **nit** — `crates/editor-conversation/src/conversation/fold.rs:1233:1` `conversation::fold::gap` — it can be `pub(crate)`
1315. **nit** — `crates/editor-conversation/src/conversation/live.rs:9:5` `conversation::live::LiveBinding::scope` — it can be `pub(crate)`
1316. **nit** — `crates/editor-conversation/src/conversation/live.rs:10:5` `conversation::live::LiveBinding::item` — it can be `pub(crate)`
1317. **nit** — `crates/editor-conversation/src/conversation/live.rs:11:5` `conversation::live::LiveBinding::epoch` — it can be `pub(crate)`
1318. **nit** — `crates/editor-conversation/src/conversation/live.rs:13:5` `conversation::live::LiveBinding::coverage` — it can be `pub(crate)`
1319. **nit** — `crates/editor-conversation/src/conversation/phase.rs:17:1` `conversation::phase::turn_phase` — it can be `pub(crate)`
1320. **nit** — `crates/editor-conversation/src/conversation/phase.rs:93:1` `conversation::phase::status_copy` — it can be `pub(crate)`
1321. **nit** — `crates/editor-conversation/src/conversation/phase.rs:103:1` `conversation::phase::section_title` — it can be `pub(crate)`
1322. **nit** — `crates/editor-conversation/src/conversation/phase.rs:136:1` `conversation::phase::format_elapsed` — it can be `pub(crate)`
1323. **nit** — `crates/editor-conversation/src/conversation/presentation.rs:44:5` `conversation::presentation::PresentationScope::generation` — it can be `pub(crate)`
1324. **nit** — `crates/editor-conversation/src/conversation/presentation.rs:72:1` `conversation::presentation::PreparedProjectedRow` — it can be `pub(crate)`
1325. **nit** — `crates/editor-conversation/src/conversation/presentation.rs:73:5` `conversation::presentation::PreparedProjectedRow::row` — it can be `pub(crate)`
1326. **nit** — `crates/editor-conversation/src/conversation/presentation.rs:74:5` `conversation::presentation::PreparedProjectedRow::geometry` — it can be `pub(crate)`
1327. **nit** — `crates/editor-conversation/src/conversation/presentation.rs:75:5` `conversation::presentation::PreparedProjectedRow::text` — it can be `pub(crate)`
1328. **nit** — `crates/editor-conversation/src/conversation/presentation.rs:76:5` `conversation::presentation::PreparedProjectedRow::proof` — it can be `pub(crate)`
1329. **nit** — `crates/editor-conversation/src/conversation/presentation.rs:89:5` `conversation::presentation::ProjectedProof::matches_row` — it can be `pub(crate)`
1330. **nit** — `crates/editor-conversation/src/conversation/presentation.rs:110:5` `conversation::presentation::SelectionOwners::document` — it can be `pub(crate)`
1331. **nit** — `crates/editor-conversation/src/conversation/presentation.rs:624:5` `conversation::presentation::MarkdownOwners::prepare_projected_row_with_geometry` — it can be `pub(crate)`
1332. **nit** — `crates/editor-conversation/src/conversation/presentation.rs:719:5` `conversation::presentation::MarkdownOwners::code_services` — it can be `pub(crate)`
1333. **nit** — `crates/editor-conversation/src/conversation/presentation.rs:776:5` `conversation::presentation::MarkdownOwners::projected_leaf_info` — it can be `pub(crate)`
1334. **nit** — `crates/editor-conversation/src/conversation/presentation.rs:788:5` `conversation::presentation::MarkdownOwners::projected_leaf_is_current` — it can be `pub(crate)`
1335. **nit** — `crates/editor-conversation/src/conversation/presentation.rs:802:5` `conversation::presentation::MarkdownOwners::prepared_body_current` — it can be `pub(crate)`
1336. **nit** — `crates/editor-conversation/src/conversation/presentation.rs:833:5` `conversation::presentation::MarkdownOwners::capture_projected_row` — it can be `pub(crate)`
1337. **nit** — `crates/editor-conversation/src/conversation/presentation.rs:845:5` `conversation::presentation::MarkdownOwners::release_projected_reveal` — it can be `pub(crate)`
1338. **nit** — `crates/editor-conversation/src/conversation/presentation.rs:866:5` `conversation::presentation::MarkdownOwners::accepts_projected_row` — it can be `pub(crate)`
1339. **nit** — `crates/editor-conversation/src/conversation/projection.rs:69:1` `conversation::projection::block_version` — it can be `pub(crate)`
1340. **nit** — `crates/editor-conversation/src/conversation/projection.rs:98:5` `conversation::projection::Projections::clear` — it can be `pub(crate)`
1341. **nit** — `crates/editor-conversation/src/conversation/questions.rs:64:5` `conversation::questions::QuestionPages::sync` — it can be `pub(crate)`
1342. **nit** — `crates/editor-conversation/src/conversation/questions.rs:139:5` `conversation::questions::QuestionPages::clear` — it can be `pub(crate)`
1343. **nit** — `crates/editor-conversation/src/conversation/questions.rs:227:5` `conversation::questions::QuestionPages::option_action` — it can be `pub(crate)`
1344. **nit** — `crates/editor-conversation/src/conversation/questions.rs:265:1` `conversation::questions::option_preview` — it can be `pub(crate)`
1345. **nit** — `crates/editor-conversation/src/conversation/questions.rs:277:1` `conversation::questions::question_preview` — it can be `pub(crate)`
1346. **nit** — `crates/editor-conversation/src/conversation/questions.rs:304:5` `conversation::questions::QuestionCollectionAction::matches_item` — it can be `pub(crate)`
1347. **nit** — `crates/editor-conversation/src/conversation/questions.rs:311:5` `conversation::questions::QuestionCollection::identity` — it can be `pub(crate)`
1348. **nit** — `crates/editor-conversation/src/conversation/questions.rs:316:5` `conversation::questions::QuestionCollection::dispatch_changed` — it can be `pub(crate)`
1349. **nit** — `crates/editor-conversation/src/conversation/questions.rs:323:5` `conversation::questions::QuestionCollection::drain_changed` — it can be `pub(crate)`
1350. **nit** — `crates/editor-conversation/src/conversation/questions.rs:328:5` `conversation::questions::QuestionCollection::open_provider` — it can be `pub(crate)`
1351. **nit** — `crates/editor-conversation/src/conversation/questions.rs:366:5` `conversation::questions::QuestionCollection::retain` — it can be `pub(crate)`
1352. **nit** — `crates/editor-conversation/src/conversation/questions.rs:427:5` `conversation::questions::QuestionCollection::clear` — it can be `pub(crate)`
1353. **nit** — `crates/editor-conversation/src/conversation/questions.rs:431:5` `conversation::questions::QuestionCollection::browser` — it can be `pub(crate)`
1354. **nit** — `crates/editor-conversation/src/conversation/questions.rs:435:5` `conversation::questions::QuestionCollection::dispatch` — it can be `pub(crate)`
1355. **nit** — `crates/editor-conversation/src/conversation/questions.rs:461:5` `conversation::questions::QuestionCollection::view` — it can be `pub(crate)`
1356. **nit** — `crates/editor-conversation/src/conversation/scroll.rs:85:1` `conversation::scroll::scrolled` — it can be `pub(crate)`
1357. **nit** — `crates/editor-conversation/src/conversation/scroll.rs:128:1` `conversation::scroll::gap_visible` — it can be `pub(crate)`
1358. **nit** — `crates/editor-conversation/src/conversation/scroll.rs:141:1` `conversation::scroll::TRANSCRIPT` — it can be `pub(crate)`
1359. **nit** — `crates/editor-conversation/src/conversation/scroll.rs:153:1` `conversation::scroll::leave_tail` — it can be `pub(crate)`
1360. **nit** — `crates/editor-conversation/src/conversation/stored.rs:81:5` `conversation::stored::StoredProof::accepts` — it can be `pub(crate)`
1361. **nit** — `crates/editor-conversation/src/conversation/stored.rs:91:5` `conversation::stored::StoredProof::accepts_row` — it can be `pub(crate)`
1362. **nit** — `crates/editor-conversation/src/conversation/stored.rs:184:5` `conversation::stored::StoredConversation::ensure` — it can be `pub(crate)`
1363. **nit** — `crates/editor-conversation/src/conversation/stored.rs:316:5` `conversation::stored::StoredConversation::sync_after_layout` — it can be `pub(crate)`
1364. **nit** — `crates/editor-conversation/src/conversation/stored.rs:497:5` `conversation::stored::StoredConversation::leaf` — it can be `pub(crate)`
1365. **nit** — `crates/editor-conversation/src/conversation/stored.rs:507:5` `conversation::stored::StoredConversation::literal_leaf` — it can be `pub(crate)`
1366. **nit** — `crates/editor-conversation/src/conversation/stored.rs:579:5` `conversation::stored::StoredConversation::current_range` — it can be `pub(crate)`
1367. **nit** — `crates/editor-conversation/src/conversation/stored.rs:589:5` `conversation::stored::StoredConversation::proof` — it can be `pub(crate)`
1368. **nit** — `crates/editor-conversation/src/conversation/thread.rs:9:1` `conversation::thread::PAGE` — it can be `pub(crate)`
1369. **nit** — `crates/editor-conversation/src/conversation/thread.rs:10:1` `conversation::thread::ROWS_PAGE` — it can be `pub(crate)`
1370. **nit** — `crates/editor-conversation/src/conversation/thread.rs:37:5` `conversation::thread::ThreadView::thread` — it can be `pub(crate)`
1371. **nit** — `crates/editor-conversation/src/conversation/thread.rs:40:5` `conversation::thread::ThreadView::work` — it can be `pub(crate)`
1372. **nit** — `crates/editor-conversation/src/conversation/thread.rs:43:5` `conversation::thread::ThreadView::live` — it can be `pub(crate)`
1373. **nit** — `crates/editor-conversation/src/conversation/thread.rs:46:5` `conversation::thread::ThreadView::work_open` — it can be `pub(crate)`
1374. **nit** — `crates/editor-conversation/src/conversation/thread.rs:47:5` `conversation::thread::ThreadView::runs` — it can be `pub(crate)`
1375. **nit** — `crates/editor-conversation/src/conversation/thread.rs:91:5` `conversation::thread::ThreadView::resident_bytes` — it can be `pub(crate)`
1376. **nit** — `crates/editor-conversation/src/conversation/thread.rs:161:5` `conversation::thread::ThreadView::apply_item` — it can be `pub(crate)`
1377. **nit** — `crates/editor-conversation/src/conversation/thread.rs:449:1` `conversation::thread::refreshes` — it can be `pub(crate)`
1378. **nit** — `crates/editor-conversation/src/conversation/thread.rs:453:1` `conversation::thread::work_query` — it can be `pub(crate)`
1379. **nit** — `crates/editor-conversation/src/conversation/thread.rs:461:1` `conversation::thread::query` — it can be `pub(crate)`
1380. **nit** — `crates/editor-conversation/src/conversation/view/compaction.rs:193:1` `conversation::view::compaction::row` — it can be `pub(crate)`
1381. **nit** — `crates/editor-conversation/src/conversation/view/factory.rs:166:5` `conversation::view::factory::RowFactory::rows` — it can be `pub(crate)`
1382. **nit** — `crates/editor-conversation/src/conversation/view/factory.rs:169:5` `conversation::view::factory::RowFactory::markdown` — it can be `pub(crate)`
1383. **nit** — `crates/editor-conversation/src/conversation/view/factory.rs:170:5` `conversation::view::factory::RowFactory::statuses` — it can be `pub(crate)`
1384. **nit** — `crates/editor-conversation/src/conversation/view/factory.rs:171:5` `conversation::view::factory::RowFactory::theme` — it can be `pub(crate)`
1385. **nit** — `crates/editor-conversation/src/conversation/view/factory.rs:172:5` `conversation::view::factory::RowFactory::reduced_motion` — it can be `pub(crate)`
1386. **nit** — `crates/editor-conversation/src/conversation/view/factory.rs:173:5` `conversation::view::factory::RowFactory::allocate` — it can be `pub(crate)`
1387. **nit** — `crates/editor-conversation/src/conversation/view/factory.rs:174:5` `conversation::view::factory::RowFactory::invalidate` — it can be `pub(crate)`
1388. **nit** — `crates/editor-conversation/src/conversation/view/factory.rs:175:5` `conversation::view::factory::RowFactory::on_link` — it can be `pub(crate)`
1389. **nit** — `crates/editor-conversation/src/conversation/view/factory.rs:176:5` `conversation::view::factory::RowFactory::on_color` — it can be `pub(crate)`
1390. **nit** — `crates/editor-conversation/src/conversation/view/factory.rs:177:5` `conversation::view::factory::RowFactory::on_code` — it can be `pub(crate)`
1391. **nit** — `crates/editor-conversation/src/conversation/view/factory.rs:1672:5` `conversation::view::factory::RowFactory::realize_with_layout` — it can be `pub(crate)`
1392. **nit** — `crates/editor-conversation/src/conversation/view/mod.rs:29:1` `conversation::view::COLUMN_MAX` — it can be `pub(crate)`
1393. **nit** — `crates/editor-conversation/src/conversation/view/mod.rs:30:1` `conversation::view::GUTTER` — it can be `pub(crate)`
1394. **nit** — `crates/editor-conversation/src/conversation/view/mod.rs:31:1` `conversation::view::CHAIN_INDENT` — it can be `pub(crate)`
1395. **nit** — `crates/editor-conversation/src/conversation/view/mod.rs:39:1` `conversation::view::reading_width` — it can be `pub(crate)`
1396. **nit** — `crates/editor-conversation/src/conversation/view/mod.rs:84:1` `conversation::view::load_row` — it can be `pub(crate)`
1397. **nit** — `crates/editor-conversation/src/conversation/view/mod.rs:113:1` `conversation::view::row_frame` — it can be `pub(crate)`
1398. **nit** — `crates/editor-conversation/src/conversation/view/mod.rs:216:1` `conversation::view::measured_row_frame` — it can be `pub(crate)`
1399. **nit** — `crates/editor-conversation/src/conversation/view/motion.rs:72:5` `conversation::view::motion::RowMotion::rebind_scope` — it can be `pub(crate)`
1400. **nit** — `crates/editor-conversation/src/conversation/view/motion.rs:87:5` `conversation::view::motion::RowMotion::scope_is` — it can be `pub(crate)`
1401. **nit** — `crates/editor-conversation/src/conversation/view/motion.rs:220:5` `conversation::view::motion::RowMotion::sticking` — it can be `pub(crate)`
1402. **nit** — `crates/editor-conversation/src/conversation/view/motion.rs:228:5` `conversation::view::motion::RowMotion::begin_frame` — it can be `pub(crate)`
1403. **nit** — `crates/editor-conversation/src/conversation/view/motion.rs:266:5` `conversation::view::motion::RowMotion::chevron` — it can be `pub(crate)`
1404. **nit** — `crates/editor-conversation/src/conversation/view/motion.rs:300:5` `conversation::view::motion::RowMotion::compaction` — it can be `pub(crate)`
1405. **nit** — `crates/editor-conversation/src/conversation/view/motion.rs:382:1` `conversation::view::motion::swapped` — it can be `pub(crate)`
1406. **nit** — `crates/editor-conversation/src/conversation/view/motion.rs:400:1` `conversation::view::motion::arrived` — it can be `pub(crate)`
1407. **nit** — `crates/editor-conversation/src/conversation/view/notice.rs:10:1` `conversation::view::notice::render` — it can be `pub(crate)`
1408. **nit** — `crates/editor-conversation/src/conversation/view/request.rs:12:1` `conversation::view::request::Dispatch` — it can be `pub(crate)`
1409. **nit** — `crates/editor-conversation/src/conversation/view/request.rs:16:1` `conversation::view::request::field_role` — it can be `pub(crate)`
1410. **nit** — `crates/editor-conversation/src/conversation/view/request.rs:58:1` `conversation::view::request::approval` — it can be `pub(crate)`
1411. **nit** — `crates/editor-conversation/src/conversation/view/request.rs:175:1` `conversation::view::request::question` — it can be `pub(crate)`
1412. **nit** — `crates/editor-conversation/src/conversation/view/request.rs:222:1` `conversation::view::request::answered` — it can be `pub(crate)`
1413. **nit** — `crates/editor-conversation/src/conversation/view/request.rs:269:1` `conversation::view::request::question_theme` — it can be `pub(crate)`
1414. **nit** — `crates/editor-conversation/src/conversation/view/resources.rs:43:5` `conversation::view::resources::BatchRelease::apply` — it can be `pub(crate)`
1415. **nit** — `crates/editor-conversation/src/conversation/view/resources.rs:66:5` `conversation::view::resources::ResourceBatches::set_scope` — it can be `pub(crate)`
1416. **nit** — `crates/editor-conversation/src/conversation/view/resources.rs:74:5` `conversation::view::resources::ResourceBatches::take_releases` — it can be `pub(crate)`
1417. **nit** — `crates/editor-conversation/src/conversation/view/resources.rs:86:5` `conversation::view::resources::ResourceBatches::retain_deferred` — it can be `pub(crate)`
1418. **nit** — `crates/editor-conversation/src/conversation/view/resources.rs:193:5` `conversation::view::resources::ResourceBatches::retain` — it can be `pub(crate)`
1419. **nit** — `crates/editor-conversation/src/conversation/view/reveal.rs:157:5` `conversation::view::reveal::MountedReveals::sync` — it can be `pub(crate)`
1420. **nit** — `crates/editor-conversation/src/conversation/view/reveal.rs:335:1` `conversation::view::reveal::GeometryFrame` — it can be `pub(crate)`
1421. **nit** — `crates/editor-conversation/src/conversation/view/reveal.rs:336:5` `conversation::view::reveal::GeometryFrame::scope` — it can be `pub(crate)`
1422. **nit** — `crates/editor-conversation/src/conversation/view/reveal.rs:337:5` `conversation::view::reveal::GeometryFrame::row` — it can be `pub(crate)`
1423. **nit** — `crates/editor-conversation/src/conversation/view/reveal.rs:338:5` `conversation::view::reveal::GeometryFrame::row_version` — it can be `pub(crate)`
1424. **nit** — `crates/editor-conversation/src/conversation/view/reveal.rs:339:5` `conversation::view::reveal::GeometryFrame::block` — it can be `pub(crate)`
1425. **nit** — `crates/editor-conversation/src/conversation/view/reveal.rs:340:5` `conversation::view::reveal::GeometryFrame::proof` — it can be `pub(crate)`
1426. **nit** — `crates/editor-conversation/src/conversation/view/reveal.rs:341:5` `conversation::view::reveal::GeometryFrame::geometry` — it can be `pub(crate)`
1427. **nit** — `crates/editor-conversation/src/conversation/view/reveal.rs:342:5` `conversation::view::reveal::GeometryFrame::text` — it can be `pub(crate)`
1428. **nit** — `crates/editor-conversation/src/conversation/view/reveal.rs:343:5` `conversation::view::reveal::GeometryFrame::reveal` — it can be `pub(crate)`
1429. **nit** — `crates/editor-conversation/src/conversation/view/reveal.rs:346:1` `conversation::view::reveal::sync_geometry` — it can be `pub(crate)`
1430. **nit** — `crates/editor-conversation/src/conversation/view/row.rs:13:1` `conversation::view::row::RowResources` — it can be `pub(crate)`
1431. **nit** — `crates/editor-conversation/src/conversation/view/row_cache.rs:75:5` `conversation::view::row_cache::RowCache::CAPACITY` — it can be `pub(crate)`
1432. **nit** — `crates/editor-conversation/src/conversation/view/row_cache.rs:85:5` `conversation::view::row_cache::RowCache::scope_is` — it can be `pub(crate)`
1433. **nit** — `crates/editor-conversation/src/conversation/view/row_sources.rs:24:5` `conversation::view::row_sources::RowSourceAction::seq` — it can be `pub(crate)`
1434. **nit** — `crates/editor-conversation/src/conversation/view/row_sources.rs:26:5` `conversation::view::row_sources::RowSourceAction::provider` — it can be `pub(crate)`
1435. **nit** — `crates/editor-conversation/src/conversation/view/row_sources.rs:27:5` `conversation::view::row_sources::RowSourceAction::owner` — it can be `pub(crate)`
1436. **nit** — `crates/editor-conversation/src/conversation/view/row_sources.rs:28:5` `conversation::view::row_sources::RowSourceAction::action` — it can be `pub(crate)`
1437. **nit** — `crates/editor-conversation/src/conversation/view/row_sources.rs:39:5` `conversation::view::row_sources::RowSourceExport::provider` — it can be `pub(crate)`
1438. **nit** — `crates/editor-conversation/src/conversation/view/source_actions.rs:17:5` `conversation::view::source_actions::CopySource::from_parts` — it can be `pub(crate)`
1439. **nit** — `crates/editor-conversation/src/conversation/view/source_actions.rs:21:5` `conversation::view::source_actions::CopySource::bytes` — it can be `pub(crate)`
1440. **nit** — `crates/editor-conversation/src/conversation/view/source_actions.rs:35:1` `conversation::view::source_actions::copy_action` — it can be `pub(crate)`
1441. **nit** — `crates/editor-conversation/src/conversation/view/source_actions.rs:64:1` `conversation::view::source_actions::failed_body` — it can be `pub(crate)`
1442. **nit** — `crates/editor-conversation/src/conversation/view/status.rs:11:1` `conversation::view::status::StatusText` — it can be `pub(crate)`
1443. **nit** — `crates/editor-conversation/src/conversation/view/status.rs:22:5` `conversation::view::status::StatusShimmers::set_scope` — it can be `pub(crate)`
1444. **nit** — `crates/editor-conversation/src/conversation/view/status.rs:25:5` `conversation::view::status::StatusShimmers::render` — it can be `pub(crate)`
1445. **nit** — `crates/editor-conversation/src/conversation/view/status.rs:59:5` `conversation::view::status::StatusShimmers::agent` — it can be `pub(crate)`
1446. **nit** — `crates/editor-conversation/src/conversation/view/status.rs:76:1` `conversation::view::status::run` — it can be `pub(crate)`
1447. **nit** — `crates/editor-conversation/src/conversation/view/status.rs:124:1` `conversation::view::status::agent` — it can be `pub(crate)`
1448. **nit** — `crates/editor-conversation/src/conversation/view/transcript.rs:17:5` `conversation::view::transcript::RowSnapshot::row` — it can be `pub(crate)`
1449. **nit** — `crates/editor-conversation/src/conversation/view/transcript.rs:18:5` `conversation::view::transcript::RowSnapshot::version` — it can be `pub(crate)`
1450. **nit** — `crates/editor-conversation/src/conversation/view/transcript.rs:19:5` `conversation::view::transcript::RowSnapshot::item_seq` — it can be `pub(crate)`
1451. **nit** — `crates/editor-conversation/src/conversation/view/transcript.rs:20:5` `conversation::view::transcript::RowSnapshot::source` — it can be `pub(crate)`
1452. **nit** — `crates/editor-conversation/src/conversation/view/transcript.rs:22:5` `conversation::view::transcript::RowSnapshot::revealed` — it can be `pub(crate)`
1453. **nit** — `crates/editor-conversation/src/conversation/view/transcript.rs:23:5` `conversation::view::transcript::RowSnapshot::reveal_ready` — it can be `pub(crate)`
1454. **nit** — `crates/editor-conversation/src/conversation/view/transcript.rs:24:5` `conversation::view::transcript::RowSnapshot::live` — it can be `pub(crate)`
1455. **nit** — `crates/editor-conversation/src/conversation/view/transcript.rs:25:5` `conversation::view::transcript::RowSnapshot::stored` — it can be `pub(crate)`
1456. **nit** — `crates/editor-conversation/src/conversation/view/transcript.rs:47:5` `conversation::view::transcript::PaintedRow::proof` — it can be `pub(crate)`
1457. **nit** — `crates/editor-conversation/src/conversation/view/transcript.rs:189:5` `conversation::view::transcript::TranscriptObservations::viewport_callback` — it can be `pub(crate)`
1458. **nit** — `crates/editor-conversation/src/conversation/view/transcript.rs:209:5` `conversation::view::transcript::TranscriptObservations::callbacks` — it can be `pub(crate)`
1459. **nit** — `crates/editor-conversation/src/conversation/view/transcript.rs:873:1` `conversation::view::transcript::reader_scrolled` — it can be `pub(crate)`
1460. **nit** — `crates/editor-conversation/src/conversation/view/work.rs:18:1` `conversation::view::work::line_role` — it can be `pub(crate)`
1461. **nit** — `crates/editor-conversation/src/conversation/view/work.rs:53:1` `conversation::view::work::section_title` — it can be `pub(crate)`
1462. **nit** — `crates/editor-conversation/src/conversation/view/work.rs:62:1` `conversation::view::work::header` — it can be `pub(crate)`
1463. **nit** — `crates/editor-conversation/src/conversation/view/work.rs:113:1` `conversation::view::work::chain` — it can be `pub(crate)`
1464. **nit** — `crates/editor-conversation/src/conversation/view/work.rs:155:1` `conversation::view::work::agent` — it can be `pub(crate)`
1465. **nit** — `crates/editor-conversation/src/conversation/view/work.rs:197:1` `conversation::view::work::activity_row` — it can be `pub(crate)`
1466. **nit** — `crates/editor-conversation/src/conversation/view/work.rs:284:1` `conversation::view::work::plan_entry` — it can be `pub(crate)`
1467. **nit** — `crates/editor-conversation/src/conversation/viewport.rs:61:5` `conversation::viewport::Viewport::requested` — it can be `pub(crate)`

**editor-composer** (198)

1468. **nit** — `crates/editor-composer/src/composer/attachment.rs:2:1` `composer::attachment::Input` — it can be `pub(crate)`
1469. **nit** — `crates/editor-composer/src/composer/attachment.rs:15:5` `composer::attachment::State::hash` — it can be `pub(crate)`
1470. **nit** — `crates/editor-composer/src/composer/attachment.rs:21:5` `composer::attachment::State::failure` — it can be `pub(crate)`
1471. **nit** — `crates/editor-composer/src/composer/attachment.rs:27:5` `composer::attachment::State::accept` — it can be `pub(crate)`
1472. **nit** — `crates/editor-composer/src/composer/attachment.rs:56:1` `composer::attachment::Source` — it can be `pub(crate)`
1473. **nit** — `crates/editor-composer/src/composer/attachment.rs:102:5` `composer::attachment::Attachment::name` — it can be `pub(crate)`
1474. **nit** — `crates/editor-composer/src/composer/attachment.rs:103:5` `composer::attachment::Attachment::state` — it can be `pub(crate)`
1475. **nit** — `crates/editor-composer/src/composer/attachment.rs:106:5` `composer::attachment::Attachment::preview_error` — it can be `pub(crate)`
1476. **nit** — `crates/editor-composer/src/composer/attachment.rs:107:5` `composer::attachment::Attachment::revealed` — it can be `pub(crate)`
1477. **nit** — `crates/editor-composer/src/composer/attachment.rs:108:5` `composer::attachment::Attachment::shown` — it can be `pub(crate)`
1478. **nit** — `crates/editor-composer/src/composer/attachment.rs:131:5` `composer::attachment::Attachments::shake` — it can be `pub(crate)`
1479. **nit** — `crates/editor-composer/src/composer/attachment.rs:132:5` `composer::attachment::Attachments::notice` — it can be `pub(crate)`
1480. **nit** — `crates/editor-composer/src/composer/attachment.rs:139:5` `composer::attachment::Attachments::new` — it can be `pub(crate)`
1481. **nit** — `crates/editor-composer/src/composer/attachment.rs:152:5` `composer::attachment::Attachments::ready_hashes` — it can be `pub(crate)`
1482. **nit** — `crates/editor-composer/src/composer/attachment.rs:158:5` `composer::attachment::Attachments::add` — it can be `pub(crate)`
1483. **nit** — `crates/editor-composer/src/composer/attachment.rs:200:5` `composer::attachment::Attachments::restore` — it can be `pub(crate)`
1484. **nit** — `crates/editor-composer/src/composer/attachment.rs:233:5` `composer::attachment::Attachments::retry_preview` — it can be `pub(crate)`
1485. **nit** — `crates/editor-composer/src/composer/attachment.rs:243:5` `composer::attachment::Attachments::remove` — it can be `pub(crate)`
1486. **nit** — `crates/editor-composer/src/composer/attachment.rs:246:5` `composer::attachment::Attachments::clear` — it can be `pub(crate)`
1487. **nit** — `crates/editor-composer/src/composer/attachment.rs:249:5` `composer::attachment::Attachments::drain` — it can be `pub(crate)`
1488. **nit** — `crates/editor-composer/src/composer/controller.rs:69:5` `composer::controller::DraftBody::text` — it can be `pub(crate)`
1489. **nit** — `crates/editor-composer/src/composer/controller.rs:71:5` `composer::controller::DraftBody::images` — it can be `pub(crate)`
1490. **nit** — `crates/editor-composer/src/composer/controller.rs:73:5` `composer::controller::DraftBody::files` — it can be `pub(crate)`
1491. **nit** — `crates/editor-composer/src/composer/controller.rs:75:5` `composer::controller::DraftBody::config` — it can be `pub(crate)`
1492. **nit** — `crates/editor-composer/src/composer/controller.rs:77:5` `composer::controller::DraftBody::project` — it can be `pub(crate)`
1493. **nit** — `crates/editor-composer/src/composer/controller.rs:85:5` `composer::controller::DraftFile::folder` — it can be `pub(crate)`
1494. **nit** — `crates/editor-composer/src/composer/controller.rs:92:5` `composer::controller::Composer::strip` — it can be `pub(crate)`
1495. **nit** — `crates/editor-composer/src/composer/controller.rs:95:5` `composer::controller::Composer::config` — it can be `pub(crate)`
1496. **nit** — `crates/editor-composer/src/composer/controller.rs:96:5` `composer::controller::Composer::project` — it can be `pub(crate)`
1497. **nit** — `crates/editor-composer/src/composer/controller.rs:103:5` `composer::controller::Composer::cleared` — it can be `pub(crate)`
1498. **nit** — `crates/editor-composer/src/composer/controller.rs:105:5` `composer::controller::Composer::notice` — it can be `pub(crate)`
1499. **nit** — `crates/editor-composer/src/composer/controller.rs:109:5` `composer::controller::Composer::cancelling` — it can be `pub(crate)`
1500. **nit** — `crates/editor-composer/src/composer/controller.rs:111:5` `composer::controller::Composer::continuing` — it can be `pub(crate)`
1501. **nit** — `crates/editor-composer/src/composer/controller.rs:225:5` `composer::controller::Composer::matches_host` — it can be `pub(crate)`
1502. **nit** — `crates/editor-composer/src/composer/controller.rs:233:5` `composer::controller::Composer::mount_editor` — it can be `pub(crate)`
1503. **nit** — `crates/editor-composer/src/composer/controller.rs:244:5` `composer::controller::Composer::stop` — it can be `pub(crate)`
1504. **nit** — `crates/editor-composer/src/composer/controller.rs:248:5` `composer::controller::Composer::take_failed_stops` — it can be `pub(crate)`
1505. **nit** — `crates/editor-composer/src/composer/controller.rs:252:5` `composer::controller::Composer::clear_goal` — it can be `pub(crate)`
1506. **nit** — `crates/editor-composer/src/composer/controller.rs:255:5` `composer::controller::Composer::unpark` — it can be `pub(crate)`
1507. **nit** — `crates/editor-composer/src/composer/controller.rs:259:5` `composer::controller::Composer::continue_turn` — it can be `pub(crate)`
1508. **nit** — `crates/editor-composer/src/composer/controller.rs:305:5` `composer::controller::Composer::apply_permissions` — it can be `pub(crate)`
1509. **nit** — `crates/editor-composer/src/composer/controller.rs:346:5` `composer::controller::Composer::drain_turn_actions` — it can be `pub(crate)`
1510. **nit** — `crates/editor-composer/src/composer/controller.rs:391:5` `composer::controller::Composer::draft_editable` — it can be `pub(crate)`
1511. **nit** — `crates/editor-composer/src/composer/controller.rs:397:5` `composer::controller::Composer::metadata_ready` — it can be `pub(crate)`
1512. **nit** — `crates/editor-composer/src/composer/controller.rs:404:5` `composer::controller::Composer::open_picker` — it can be `pub(crate)`
1513. **nit** — `crates/editor-composer/src/composer/controller.rs:424:5` `composer::controller::Composer::refresh_picker` — it can be `pub(crate)`
1514. **nit** — `crates/editor-composer/src/composer/controller.rs:432:5` `composer::controller::Composer::choose_model` — it can be `pub(crate)`
1515. **nit** — `crates/editor-composer/src/composer/controller.rs:447:5` `composer::controller::Composer::choose_project` — it can be `pub(crate)`
1516. **nit** — `crates/editor-composer/src/composer/controller.rs:460:5` `composer::controller::Composer::refresh_mentions` — it can be `pub(crate)`
1517. **nit** — `crates/editor-composer/src/composer/controller.rs:473:5` `composer::controller::Composer::sync_mentions` — it can be `pub(crate)`
1518. **nit** — `crates/editor-composer/src/composer/controller.rs:488:5` `composer::controller::Composer::mention_key` — it can be `pub(crate)`
1519. **nit** — `crates/editor-composer/src/composer/controller.rs:563:5` `composer::controller::Composer::remove_image` — it can be `pub(crate)`
1520. **nit** — `crates/editor-composer/src/composer/controller.rs:588:5` `composer::controller::Composer::add_image` — it can be `pub(crate)`
1521. **nit** — `crates/editor-composer/src/composer/controller.rs:605:5` `composer::controller::Composer::drain_attachments` — it can be `pub(crate)`
1522. **nit** — `crates/editor-composer/src/composer/controller.rs:792:5` `composer::controller::Composer::effective_config` — it can be `pub(crate)`
1523. **nit** — `crates/editor-composer/src/composer/controller.rs:828:5` `composer::controller::Composer::blocked` — it can be `pub(crate)`
1524. **nit** — `crates/editor-composer/src/composer/controller.rs:871:5` `composer::controller::Composer::primary` — it can be `pub(crate)`
1525. **nit** — `crates/editor-composer/src/composer/controller.rs:895:5` `composer::controller::Composer::send` — it can be `pub(crate)`
1526. **nit** — `crates/editor-composer/src/composer/draft_session.rs:7:1` `composer::draft_session::SAVE_DELAY` — it can be `pub(crate)`
1527. **nit** — `crates/editor-composer/src/composer/draft_session.rs:45:5` `composer::draft_session::DraftSession::scope` — it can be `pub(crate)`
1528. **nit** — `crates/editor-composer/src/composer/draft_session.rs:177:5` `composer::draft_session::DraftSession::use_remote` — it can be `pub(crate)`
1529. **nit** — `crates/editor-composer/src/composer/encode.rs:10:1` `composer::encode::MAX_INPUT_BYTES` — it can be `pub(crate)`
1530. **nit** — `crates/editor-composer/src/composer/engine_light.rs:73:5` `composer::engine_light::EngineLightImage::element` — it can be `pub(crate)`
1531. **nit** — `crates/editor-composer/src/composer/goal_strip.rs:35:5` `composer::goal_strip::GoalStripFrame::reveal` — it can be `pub(crate)`
1532. **nit** — `crates/editor-composer/src/composer/goal_strip.rs:37:5` `composer::goal_strip::GoalStripFrame::label_opacity` — it can be `pub(crate)`
1533. **nit** — `crates/editor-composer/src/composer/goal_strip.rs:38:5` `composer::goal_strip::GoalStripFrame::label_offset` — it can be `pub(crate)`
1534. **nit** — `crates/editor-composer/src/composer/goal_strip.rs:39:5` `composer::goal_strip::GoalStripFrame::moving` — it can be `pub(crate)`
1535. **nit** — `crates/editor-composer/src/composer/goal_strip.rs:43:5` `composer::goal_strip::GoalStrip::set` — it can be `pub(crate)`
1536. **nit** — `crates/editor-composer/src/composer/mentions.rs:6:5` `composer::mentions::Trigger::sigil` — it can be `pub(crate)`
1537. **nit** — `crates/editor-composer/src/composer/mentions.rs:7:5` `composer::mentions::Trigger::range` — it can be `pub(crate)`
1538. **nit** — `crates/editor-composer/src/composer/mentions.rs:8:5` `composer::mentions::Trigger::query` — it can be `pub(crate)`
1539. **nit** — `crates/editor-composer/src/composer/mentions.rs:9:5` `composer::mentions::Trigger::at_start` — it can be `pub(crate)`
1540. **nit** — `crates/editor-composer/src/composer/mentions.rs:12:1` `composer::mentions::trigger` — it can be `pub(crate)`
1541. **nit** — `crates/editor-composer/src/composer/mentions.rs:49:5` `composer::mentions::Kind::sigil` — it can be `pub(crate)`
1542. **nit** — `crates/editor-composer/src/composer/mentions.rs:60:5` `composer::mentions::Row::kind` — it can be `pub(crate)`
1543. **nit** — `crates/editor-composer/src/composer/mentions.rs:61:5` `composer::mentions::Row::name` — it can be `pub(crate)`
1544. **nit** — `crates/editor-composer/src/composer/mentions.rs:62:5` `composer::mentions::Row::title` — it can be `pub(crate)`
1545. **nit** — `crates/editor-composer/src/composer/mentions.rs:63:5` `composer::mentions::Row::description` — it can be `pub(crate)`
1546. **nit** — `crates/editor-composer/src/composer/mentions.rs:98:5` `composer::mentions::Mentions::threads` — it can be `pub(crate)`
1547. **nit** — `crates/editor-composer/src/composer/mentions.rs:99:5` `composer::mentions::Mentions::current` — it can be `pub(crate)`
1548. **nit** — `crates/editor-composer/src/composer/mentions.rs:110:5` `composer::mentions::Mentions::refresh` — it can be `pub(crate)`
1549. **nit** — `crates/editor-composer/src/composer/mentions.rs:245:5` `composer::mentions::Mentions::clear_trigger` — it can be `pub(crate)`
1550. **nit** — `crates/editor-composer/src/composer/mentions.rs:254:5` `composer::mentions::Mentions::dismiss` — it can be `pub(crate)`
1551. **nit** — `crates/editor-composer/src/composer/mentions.rs:260:5` `composer::mentions::Mentions::up` — it can be `pub(crate)`
1552. **nit** — `crates/editor-composer/src/composer/mentions.rs:276:5` `composer::mentions::Mentions::hover` — it can be `pub(crate)`
1553. **nit** — `crates/editor-composer/src/composer/mentions.rs:292:5` `composer::mentions::Mentions::pointer_at` — it can be `pub(crate)`
1554. **nit** — `crates/editor-composer/src/composer/mentions.rs:309:5` `composer::mentions::Mentions::kinds` — it can be `pub(crate)`
1555. **nit** — `crates/editor-composer/src/composer/mentions.rs:319:5` `composer::mentions::Mentions::mark_revealed` — it can be `pub(crate)`
1556. **nit** — `crates/editor-composer/src/composer/mentions.rs:324:5` `composer::mentions::Mentions::scroll` — it can be `pub(crate)`
1557. **nit** — `crates/editor-composer/src/composer/mentions.rs:342:5` `composer::mentions::Mentions::replacement` — it can be `pub(crate)`
1558. **nit** — `crates/editor-composer/src/composer/mentions.rs:499:5` `composer::mentions::Metrics::divider` — it can be `pub(crate)`
1559. **nit** — `crates/editor-composer/src/composer/mentions.rs:500:5` `composer::mentions::Metrics::heading` — it can be `pub(crate)`
1560. **nit** — `crates/editor-composer/src/composer/mentions.rs:501:5` `composer::mentions::Metrics::row` — it can be `pub(crate)`
1561. **nit** — `crates/editor-composer/src/composer/mentions.rs:502:5` `composer::mentions::Metrics::viewport` — it can be `pub(crate)`
1562. **nit** — `crates/editor-composer/src/composer/model_line.rs:73:1` `composer::model_line::claude_lineup` — it can be `pub(crate)`
1563. **nit** — `crates/editor-composer/src/composer/model_line.rs:78:1` `composer::model_line::codex_lineup` — it can be `pub(crate)`
1564. **nit** — `crates/editor-composer/src/composer/model_list.rs:14:1` `composer::model_list::ENGINE_ROW_HEIGHT` — it can be `pub(crate)`
1565. **nit** — `crates/editor-composer/src/composer/model_list.rs:15:1` `composer::model_list::GATE_ROW_HEIGHT` — it can be `pub(crate)`
1566. **nit** — `crates/editor-composer/src/composer/model_list.rs:16:1` `composer::model_list::GROUP_ROW_HEIGHT` — it can be `pub(crate)`
1567. **nit** — `crates/editor-composer/src/composer/model_list.rs:17:1` `composer::model_list::MODEL_ROW_HEIGHT` — it can be `pub(crate)`
1568. **nit** — `crates/editor-composer/src/composer/model_list.rs:18:1` `composer::model_list::ROW_GAP` — it can be `pub(crate)`
1569. **nit** — `crates/editor-composer/src/composer/model_list.rs:19:1` `composer::model_list::SECTION_SPACER` — it can be `pub(crate)`
1570. **nit** — `crates/editor-composer/src/composer/model_list.rs:20:1` `composer::model_list::PADDING` — it can be `pub(crate)`
1571. **nit** — `crates/editor-composer/src/composer/model_list.rs:151:5` `composer::model_list::ModelScroll::target` — it can be `pub(crate)`
1572. **nit** — `crates/editor-composer/src/composer/persistence.rs:303:5` `composer::persistence::DraftHandle::flush` — it can be `pub(crate)`
1573. **nit** — `crates/editor-composer/src/composer/persistence.rs:322:5` `composer::persistence::DraftHandle::keep_local` — it can be `pub(crate)`
1574. **nit** — `crates/editor-composer/src/composer/persistence.rs:327:5` `composer::persistence::DraftHandle::use_remote` — it can be `pub(crate)`
1575. **nit** — `crates/editor-composer/src/composer/picker.rs:32:5` `composer::picker::PickerTooltip::clear` — it can be `pub(crate)`
1576. **nit** — `crates/editor-composer/src/composer/picker.rs:77:5` `composer::picker::PickerTooltip::revision` — it can be `pub(crate)`
1577. **nit** — `crates/editor-composer/src/composer/picker.rs:84:5` `composer::picker::AxisOption::special` — it can be `pub(crate)`
1578. **nit** — `crates/editor-composer/src/composer/picker.rs:86:5` `composer::picker::AxisOption::advisory` — it can be `pub(crate)`
1579. **nit** — `crates/editor-composer/src/composer/picker.rs:109:5` `composer::picker::ModelRow::engine` — it can be `pub(crate)`
1580. **nit** — `crates/editor-composer/src/composer/picker.rs:118:1` `composer::picker::gate` — it can be `pub(crate)`
1581. **nit** — `crates/editor-composer/src/composer/picker.rs:142:1` `composer::picker::offered_engines` — it can be `pub(crate)`
1582. **nit** — `crates/editor-composer/src/composer/picker.rs:189:5` `composer::picker::PickerState::show_all_models` — it can be `pub(crate)`
1583. **nit** — `crates/editor-composer/src/composer/picker.rs:191:5` `composer::picker::PickerState::customizations` — it can be `pub(crate)`
1584. **nit** — `crates/editor-composer/src/composer/picker.rs:193:5` `composer::picker::PickerState::open_serial` — it can be `pub(crate)`
1585. **nit** — `crates/editor-composer/src/composer/picker.rs:194:5` `composer::picker::PickerState::axis` — it can be `pub(crate)`
1586. **nit** — `crates/editor-composer/src/composer/picker.rs:196:5` `composer::picker::PickerState::models` — it can be `pub(crate)`
1587. **nit** — `crates/editor-composer/src/composer/picker.rs:197:5` `composer::picker::PickerState::projects` — it can be `pub(crate)`
1588. **nit** — `crates/editor-composer/src/composer/picker.rs:198:5` `composer::picker::PickerState::project_edges` — it can be `pub(crate)`
1589. **nit** — `crates/editor-composer/src/composer/picker.rs:199:5` `composer::picker::PickerState::model_edges` — it can be `pub(crate)`
1590. **nit** — `crates/editor-composer/src/composer/picker.rs:203:5` `composer::picker::PickerState::refresh_settings` — it can be `pub(crate)`
1591. **nit** — `crates/editor-composer/src/composer/picker.rs:212:5` `composer::picker::PickerState::for_target` — it can be `pub(crate)`
1592. **nit** — `crates/editor-composer/src/composer/picker.rs:294:5` `composer::picker::PickerState::refresh` — it can be `pub(crate)`
1593. **nit** — `crates/editor-composer/src/composer/picker.rs:336:5` `composer::picker::PickerState::toggle_group` — it can be `pub(crate)`
1594. **nit** — `crates/editor-composer/src/composer/picker.rs:339:5` `composer::picker::PickerState::model_config` — it can be `pub(crate)`
1595. **nit** — `crates/editor-composer/src/composer/picker.rs:378:5` `composer::picker::PickerState::pane_config` — it can be `pub(crate)`
1596. **nit** — `crates/editor-composer/src/composer/placeholder.rs:105:1` `composer::placeholder::phrase` — it can be `pub(crate)`
1597. **nit** — `crates/editor-composer/src/composer/questionnaire.rs:30:5` `composer::questionnaire::QuestionnairePanel::error` — it can be `pub(crate)`
1598. **nit** — `crates/editor-composer/src/composer/questionnaire.rs:36:5` `composer::questionnaire::QuestionnairePanel::visible` — it can be `pub(crate)`
1599. **nit** — `crates/editor-composer/src/composer/questionnaire.rs:49:5` `composer::questionnaire::QuestionnairePanel::writing_answer` — it can be `pub(crate)`
1600. **nit** — `crates/editor-composer/src/composer/questionnaire.rs:52:5` `composer::questionnaire::QuestionnairePanel::aside_text` — it can be `pub(crate)`
1601. **nit** — `crates/editor-composer/src/composer/questionnaire.rs:55:5` `composer::questionnaire::QuestionnairePanel::restore_aside_text` — it can be `pub(crate)`
1602. **nit** — `crates/editor-composer/src/composer/questionnaire.rs:145:5` `composer::questionnaire::QuestionnairePanel::toggle` — it can be `pub(crate)`
1603. **nit** — `crates/editor-composer/src/composer/questionnaire.rs:194:5` `composer::questionnaire::QuestionnairePanel::dismiss` — it can be `pub(crate)`
1604. **nit** — `crates/editor-composer/src/composer/questionnaire.rs:205:5` `composer::questionnaire::QuestionnairePanel::retry` — it can be `pub(crate)`
1605. **nit** — `crates/editor-composer/src/composer/questionnaire.rs:211:5` `composer::questionnaire::QuestionnairePanel::send` — it can be `pub(crate)`
1606. **nit** — `crates/editor-composer/src/composer/questionnaire.rs:218:5` `composer::questionnaire::QuestionnairePanel::settled` — it can be `pub(crate)`
1607. **nit** — `crates/editor-composer/src/composer/sliding_tab.rs:23:5` `composer::sliding_tab::Bounds::width` — it can be `pub(crate)`
1608. **nit** — `crates/editor-composer/src/composer/sliding_tab.rs:24:5` `composer::sliding_tab::Bounds::height` — it can be `pub(crate)`
1609. **nit** — `crates/editor-composer/src/composer/sliding_tab.rs:25:5` `composer::sliding_tab::Bounds::split` — it can be `pub(crate)`
1610. **nit** — `crates/editor-composer/src/composer/sliding_tab.rs:26:5` `composer::sliding_tab::Bounds::radius` — it can be `pub(crate)`
1611. **nit** — `crates/editor-composer/src/composer/sliding_tab.rs:50:5` `composer::sliding_tab::SlidingTab::resize` — it can be `pub(crate)`
1612. **nit** — `crates/editor-composer/src/composer/sliding_tab.rs:54:5` `composer::sliding_tab::SlidingTab::snap` — it can be `pub(crate)`
1613. **nit** — `crates/editor-composer/src/composer/sliding_tab.rs:83:5` `composer::sliding_tab::SlidingTab::cutout` — it can be `pub(crate)`
1614. **nit** — `crates/editor-composer/src/composer/sliding_tab.rs:112:5` `composer::sliding_tab::SlidingTab::outline` — it can be `pub(crate)`
1615. **nit** — `crates/editor-composer/src/composer/swap.rs:13:1` `composer::swap::DURATION` — it can be `pub(crate)`
1616. **nit** — `crates/editor-composer/src/composer/swap.rs:14:1` `composer::swap::INCOMING` — it can be `pub(crate)`
1617. **nit** — `crates/editor-composer/src/composer/swap.rs:15:1` `composer::swap::OUTGOING` — it can be `pub(crate)`
1618. **nit** — `crates/editor-composer/src/composer/swap.rs:16:1` `composer::swap::PROMPT` — it can be `pub(crate)`
1619. **nit** — `crates/editor-composer/src/composer/swap.rs:17:1` `composer::swap::SETTLE` — it can be `pub(crate)`
1620. **nit** — `crates/editor-composer/src/composer/swap.rs:25:5` `composer::swap::Layer::opacity` — it can be `pub(crate)`
1621. **nit** — `crates/editor-composer/src/composer/swap.rs:26:5` `composer::swap::Layer::scale` — it can be `pub(crate)`
1622. **nit** — `crates/editor-composer/src/composer/swap.rs:27:5` `composer::swap::Layer::blur` — it can be `pub(crate)`
1623. **nit** — `crates/editor-composer/src/composer/swap.rs:32:5` `composer::swap::Layer::incoming` — it can be `pub(crate)`
1624. **nit** — `crates/editor-composer/src/composer/swap.rs:41:5` `composer::swap::Layer::outgoing` — it can be `pub(crate)`
1625. **nit** — `crates/editor-composer/src/composer/swap.rs:46:5` `composer::swap::Layer::transform` — it can be `pub(crate)`
1626. **nit** — `crates/editor-composer/src/composer/swap.rs:68:5` `composer::swap::PrimarySwap::observe` — it can be `pub(crate)`
1627. **nit** — `crates/editor-composer/src/composer/swap.rs:79:5` `composer::swap::PrimarySwap::progress` — it can be `pub(crate)`
1628. **nit** — `crates/editor-composer/src/composer/swap.rs:91:5` `composer::swap::PrimarySwap::settle` — it can be `pub(crate)`
1629. **nit** — `crates/editor-composer/src/composer/swap.rs:116:1` `composer::swap::face` — it can be `pub(crate)`
1630. **nit** — `crates/editor-composer/src/composer/swap.rs:128:1` `composer::swap::advance` — it can be `pub(crate)`
1631. **nit** — `crates/editor-composer/src/composer/swap.rs:148:1` `composer::swap::settle_progress` — it can be `pub(crate)`
1632. **nit** — `crates/editor-composer/src/composer/swap.rs:155:1` `composer::swap::advance_prompt` — it can be `pub(crate)`
1633. **nit** — `crates/editor-composer/src/composer/view.rs:122:1` `composer::view::primary` — it can be `pub(crate)`
1634. **nit** — `crates/editor-composer/src/composer/view.rs:161:1` `composer::view::frame` — it can be `pub(crate)`
1635. **nit** — `crates/editor-composer/src/composer/view.rs:167:1` `composer::view::model_trigger` — it can be `pub(crate)`
1636. **nit** — `crates/editor-composer/src/composer/view.rs:253:1` `composer::view::controls` — it can be `pub(crate)`
1637. **nit** — `crates/editor-composer/src/composer/view.rs:716:1` `composer::view::CardParts` — it can be `pub(crate)`
1638. **nit** — `crates/editor-composer/src/composer/view.rs:717:5` `composer::view::CardParts::prompt` — it can be `pub(crate)`
1639. **nit** — `crates/editor-composer/src/composer/view.rs:718:5` `composer::view::CardParts::primary` — it can be `pub(crate)`
1640. **nit** — `crates/editor-composer/src/composer/view.rs:719:5` `composer::view::CardParts::questionnaires` — it can be `pub(crate)`
1641. **nit** — `crates/editor-composer/src/composer/view.rs:720:5` `composer::view::CardParts::attachments` — it can be `pub(crate)`
1642. **nit** — `crates/editor-composer/src/composer/view.rs:721:5` `composer::view::CardParts::mention_overlay` — it can be `pub(crate)`
1643. **nit** — `crates/editor-composer/src/composer/view.rs:722:5` `composer::view::CardParts::thread_error` — it can be `pub(crate)`
1644. **nit** — `crates/editor-composer/src/composer/view.rs:723:5` `composer::view::CardParts::draft_error` — it can be `pub(crate)`
1645. **nit** — `crates/editor-composer/src/composer/view.rs:734:1` `composer::view::composer_card` — it can be `pub(crate)`
1646. **nit** — `crates/editor-composer/src/composer/view.rs:807:1` `composer::view::draft_error` — it can be `pub(crate)`
1647. **nit** — `crates/editor-composer/src/composer/view.rs:848:1` `composer::view::goal_label` — it can be `pub(crate)`
1648. **nit** — `crates/editor-composer/src/composer/view.rs:1030:5` `composer::view::PickerTooltipPresentation::anchor` — it can be `pub(crate)`
1649. **nit** — `crates/editor-composer/src/composer/view.rs:1033:5` `composer::view::PickerTooltipPresentation::content` — it can be `pub(crate)`
1650. **nit** — `crates/editor-composer/src/composer/view.rs:1037:5` `composer::view::PickerTooltipSurface::anchor` — it can be `pub(crate)`
1651. **nit** — `crates/editor-composer/src/composer/view.rs:1854:5` `composer::view::ComposerOwner::drain_answers` — it can be `pub(crate)`
1652. **nit** — `crates/editor-composer/src/composer/view.rs:1948:5` `composer::view::ComposerOwner::dispatch` — it can be `pub(crate)`
1653. **nit** — `crates/editor-composer/src/composer/view.rs:2810:5` `composer::view::PopupKind::gap` — it can be `pub(crate)`
1654. **nit** — `crates/editor-composer/src/composer/view.rs:2975:5` `composer::view::ModelMenuItem::pane` — it can be `pub(crate)`
1655. **nit** — `crates/editor-composer/src/composer/view.rs:2976:5` `composer::view::ModelMenuItem::label` — it can be `pub(crate)`
1656. **nit** — `crates/editor-composer/src/composer/view.rs:2977:5` `composer::view::ModelMenuItem::value` — it can be `pub(crate)`
1657. **nit** — `crates/editor-composer/src/composer/view.rs:2980:1` `composer::view::ModelMenuFrame` — it can be `pub(crate)`
1658. **nit** — `crates/editor-composer/src/composer/view.rs:2981:5` `composer::view::ModelMenuFrame::items` — it can be `pub(crate)`
1659. **nit** — `crates/editor-composer/src/composer/view.rs:2982:5` `composer::view::ModelMenuFrame::pane` — it can be `pub(crate)`
1660. **nit** — `crates/editor-composer/src/composer/view.rs:2983:5` `composer::view::ModelMenuFrame::scroll` — it can be `pub(crate)`
1661. **nit** — `crates/editor-composer/src/composer/view.rs:2984:5` `composer::view::ModelMenuFrame::spied` — it can be `pub(crate)`
1662. **nit** — `crates/editor-composer/src/composer/view.rs:2985:5` `composer::view::ModelMenuFrame::tab` — it can be `pub(crate)`
1663. **nit** — `crates/editor-composer/src/composer/view.rs:2986:5` `composer::view::ModelMenuFrame::engine_light` — it can be `pub(crate)`
1664. **nit** — `crates/editor-composer/src/composer/view.rs:3479:1` `composer::view::model_menu` — it can be `pub(crate)`
1665. **nit** — `crates/editor-composer/src/composer/view.rs:3696:1` `composer::view::project_tab` — it can be `pub(crate)`

**editor-sidebar** (100)

1666. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:55:5` `sidebar::controller::Sidebar::search` — it can be `pub(crate)`
1667. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:56:5` `sidebar::controller::Sidebar::history_open` — it can be `pub(crate)`
1668. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:60:5` `sidebar::controller::Sidebar::rail_motion` — it can be `pub(crate)`
1669. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:66:5` `sidebar::controller::Sidebar::thread_viewport` — it can be `pub(crate)`
1670. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:68:5` `sidebar::controller::Sidebar::menu` — it can be `pub(crate)`
1671. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:69:5` `sidebar::controller::Sidebar::menu_generation` — it can be `pub(crate)`
1672. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:99:5` `sidebar::controller::ThreadMenu::thread` — it can be `pub(crate)`
1673. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:100:5` `sidebar::controller::ThreadMenu::position` — it can be `pub(crate)`
1674. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:105:5` `sidebar::controller::PickerIdentity::host` — it can be `pub(crate)`
1675. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:106:5` `sidebar::controller::PickerIdentity::instance` — it can be `pub(crate)`
1676. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:107:5` `sidebar::controller::PickerIdentity::generation` — it can be `pub(crate)`
1677. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:274:5` `sidebar::controller::Sidebar::screen` — it can be `pub(crate)`
1678. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:282:5` `sidebar::controller::Sidebar::visible_picker` — it can be `pub(crate)`
1679. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:291:5` `sidebar::controller::Sidebar::picker_identity` — it can be `pub(crate)`
1680. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:300:5` `sidebar::controller::Sidebar::dismiss_picker` — it can be `pub(crate)`
1681. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:583:5` `sidebar::controller::Sidebar::mark_unread` — it can be `pub(crate)`
1682. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:590:5` `sidebar::controller::Sidebar::archive` — it can be `pub(crate)`
1683. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:606:5` `sidebar::controller::Sidebar::submit_rename` — it can be `pub(crate)`
1684. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:628:5` `sidebar::controller::Sidebar::slide_threads` — it can be `pub(crate)`
1685. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:649:5` `sidebar::controller::Sidebar::report_thread_viewport` — it can be `pub(crate)`
1686. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:711:5` `sidebar::controller::Sidebar::advance_history` — it can be `pub(crate)`
1687. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:740:5` `sidebar::controller::Sidebar::load_visible_projects` — it can be `pub(crate)`
1688. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:755:5` `sidebar::controller::Sidebar::retry_project_icons` — it can be `pub(crate)`
1689. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:785:5` `sidebar::controller::Sidebar::browse` — it can be `pub(crate)`
1690. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:823:5` `sidebar::controller::Sidebar::search_text` — it can be `pub(crate)`
1691. **nit** — `crates/editor-sidebar/src/sidebar/controller.rs:861:5` `sidebar::controller::Sidebar::create_project` — it can be `pub(crate)`
1692. **nit** — `crates/editor-sidebar/src/sidebar/drag_preview.rs:12:1` `sidebar::drag_preview::preview_position` — it can be `pub(crate)`
1693. **nit** — `crates/editor-sidebar/src/sidebar/drag_preview.rs:19:1` `sidebar::drag_preview::preview` — it can be `pub(crate)`
1694. **nit** — `crates/editor-sidebar/src/sidebar/geometry.rs:4:1` `sidebar::geometry::RAIL_WIDTH` — it can be `pub(crate)`
1695. **nit** — `crates/editor-sidebar/src/sidebar/geometry.rs:5:1` `sidebar::geometry::READ_AHEAD` — it can be `pub(crate)`
1696. **nit** — `crates/editor-sidebar/src/sidebar/geometry.rs:6:1` `sidebar::geometry::LIST` — it can be `pub(crate)`
1697. **nit** — `crates/editor-sidebar/src/sidebar/geometry.rs:7:1` `sidebar::geometry::HEADING` — it can be `pub(crate)`
1698. **nit** — `crates/editor-sidebar/src/sidebar/geometry.rs:19:5` `sidebar::geometry::ThreadLayout::row` — it can be `pub(crate)`
1699. **nit** — `crates/editor-sidebar/src/sidebar/geometry.rs:26:5` `sidebar::geometry::ThreadLayout::height` — it can be `pub(crate)`
1700. **nit** — `crates/editor-sidebar/src/sidebar/geometry.rs:182:5` `sidebar::geometry::Splice::start` — it can be `pub(crate)`
1701. **nit** — `crates/editor-sidebar/src/sidebar/geometry.rs:183:5` `sidebar::geometry::Splice::removed` — it can be `pub(crate)`
1702. **nit** — `crates/editor-sidebar/src/sidebar/geometry.rs:184:5` `sidebar::geometry::Splice::inserted` — it can be `pub(crate)`
1703. **nit** — `crates/editor-sidebar/src/sidebar/geometry.rs:253:1` `sidebar::geometry::history_fraction` — it can be `pub(crate)`
1704. **nit** — `crates/editor-sidebar/src/sidebar/list.rs:65:5` `sidebar::list::AgeGroup::label` — it can be `pub(crate)`
1705. **nit** — `crates/editor-sidebar/src/sidebar/list.rs:94:5` `sidebar::list::HitRow::snippet` — it can be `pub(crate)`
1706. **nit** — `crates/editor-sidebar/src/sidebar/list.rs:95:5` `sidebar::list::HitRow::matched` — it can be `pub(crate)`
1707. **nit** — `crates/editor-sidebar/src/sidebar/picker.rs:6:5` `sidebar::picker::FolderPicker::hidden` — it can be `pub(crate)`
1708. **nit** — `crates/editor-sidebar/src/sidebar/picker.rs:7:5` `sidebar::picker::FolderPicker::loading` — it can be `pub(crate)`
1709. **nit** — `crates/editor-sidebar/src/sidebar/picker.rs:9:5` `sidebar::picker::FolderPicker::error` — it can be `pub(crate)`
1710. **nit** — `crates/editor-sidebar/src/sidebar/picker.rs:14:5` `sidebar::picker::FolderPicker::rows` — it can be `pub(crate)`
1711. **nit** — `crates/editor-sidebar/src/sidebar/rail.rs:15:1` `sidebar::rail::ICON_SIZE` — it can be `pub(crate)`
1712. **nit** — `crates/editor-sidebar/src/sidebar/rail.rs:26:5` `sidebar::rail::Glyph::duration` — it can be `pub(crate)`
1713. **nit** — `crates/editor-sidebar/src/sidebar/rail.rs:181:1` `sidebar::rail::icon` — it can be `pub(crate)`
1714. **nit** — `crates/editor-sidebar/src/sidebar/rail.rs:219:1` `sidebar::rail::build` — it can be `pub(crate)`
1715. **nit** — `crates/editor-sidebar/src/sidebar/row.rs:9:1` `sidebar::row::STATE_DOT` — it can be `pub(crate)`
1716. **nit** — `crates/editor-sidebar/src/sidebar/row.rs:11:1` `sidebar::row::WORKING_PULSE_MILLIS` — it can be `pub(crate)`
1717. **nit** — `crates/editor-sidebar/src/sidebar/row.rs:12:1` `sidebar::row::WORKING_PULSE_FLOOR` — it can be `pub(crate)`
1718. **nit** — `crates/editor-sidebar/src/sidebar/row.rs:22:1` `sidebar::row::menu_actions` — it can be `pub(crate)`
1719. **nit** — `crates/editor-sidebar/src/sidebar/row.rs:49:1` `sidebar::row::drag` — it can be `pub(crate)`
1720. **nit** — `crates/editor-sidebar/src/sidebar/row.rs:61:1` `sidebar::row::state_label` — it can be `pub(crate)`
1721. **nit** — `crates/editor-sidebar/src/sidebar/row.rs:72:1` `sidebar::row::rename_value` — it can be `pub(crate)`
1722. **nit** — `crates/editor-sidebar/src/sidebar/row.rs:79:5` `sidebar::row::RowVisuals::subtitle` — it can be `pub(crate)`
1723. **nit** — `crates/editor-sidebar/src/sidebar/row.rs:80:5` `sidebar::row::RowVisuals::project_mark` — it can be `pub(crate)`
1724. **nit** — `crates/editor-sidebar/src/sidebar/row.rs:84:1` `sidebar::row::TruncatedText` — it can be `pub(crate)`
1725. **nit** — `crates/editor-sidebar/src/sidebar/row.rs:87:1` `sidebar::row::WorkingPulse` — it can be `pub(crate)`
1726. **nit** — `crates/editor-sidebar/src/sidebar/row.rs:113:1` `sidebar::row::marked_text` — it can be `pub(crate)`
1727. **nit** — `crates/editor-sidebar/src/sidebar/row.rs:166:1` `sidebar::row::hit` — it can be `pub(crate)`
1728. **nit** — `crates/editor-sidebar/src/sidebar/row.rs:207:1` `sidebar::row::visual_factory` — it can be `pub(crate)`
1729. **nit** — `crates/editor-sidebar/src/sidebar/row.rs:289:1` `sidebar::row::build` — it can be `pub(crate)`
1730. **nit** — `crates/editor-sidebar/src/sidebar/row.rs:473:1` `sidebar::row::drag_preview` — it can be `pub(crate)`
1731. **nit** — `crates/editor-sidebar/src/sidebar/settings_nav.rs:9:5` `sidebar::settings_nav::Group::heading` — it can be `pub(crate)`
1732. **nit** — `crates/editor-sidebar/src/sidebar/settings_nav.rs:10:5` `sidebar::settings_nav::Group::entries` — it can be `pub(crate)`
1733. **nit** — `crates/editor-sidebar/src/sidebar/settings_nav.rs:15:5` `sidebar::settings_nav::Entry::page` — it can be `pub(crate)`
1734. **nit** — `crates/editor-sidebar/src/sidebar/settings_nav.rs:16:5` `sidebar::settings_nav::Entry::icon` — it can be `pub(crate)`
1735. **nit** — `crates/editor-sidebar/src/sidebar/settings_nav.rs:17:5` `sidebar::settings_nav::Entry::need` — it can be `pub(crate)`
1736. **nit** — `crates/editor-sidebar/src/sidebar/settings_nav.rs:18:5` `sidebar::settings_nav::Entry::expanded` — it can be `pub(crate)`
1737. **nit** — `crates/editor-sidebar/src/sidebar/settings_nav.rs:19:5` `sidebar::settings_nav::Entry::children` — it can be `pub(crate)`
1738. **nit** — `crates/editor-sidebar/src/sidebar/settings_nav.rs:22:1` `sidebar::settings_nav::engine_need` — it can be `pub(crate)`
1739. **nit** — `crates/editor-sidebar/src/sidebar/settings_nav.rs:34:1` `sidebar::settings_nav::children` — it can be `pub(crate)`
1740. **nit** — `crates/editor-sidebar/src/sidebar/settings_nav.rs:58:1` `sidebar::settings_nav::groups` — it can be `pub(crate)`
1741. **nit** — `crates/editor-sidebar/src/sidebar/settings_nav.rs:111:5` `sidebar::settings_nav::NavMotion::sync` — it can be `pub(crate)`
1742. **nit** — `crates/editor-sidebar/src/sidebar/settings_nav.rs:119:5` `sidebar::settings_nav::NavMotion::animating` — it can be `pub(crate)`
1743. **nit** — `crates/editor-sidebar/src/sidebar/settings_nav.rs:135:1` `sidebar::settings_nav::ROW` — it can be `pub(crate)`
1744. **nit** — `crates/editor-sidebar/src/sidebar/usage.rs:3:1` `sidebar::usage::METER_SEGMENTS` — it can be `pub(crate)`
1745. **nit** — `crates/editor-sidebar/src/sidebar/usage.rs:13:5` `sidebar::usage::Cadence::title` — it can be `pub(crate)`
1746. **nit** — `crates/editor-sidebar/src/sidebar/usage.rs:24:5` `sidebar::usage::Meter::index` — it can be `pub(crate)`
1747. **nit** — `crates/editor-sidebar/src/sidebar/usage.rs:25:5` `sidebar::usage::Meter::cadence` — it can be `pub(crate)`
1748. **nit** — `crates/editor-sidebar/src/sidebar/usage.rs:26:5` `sidebar::usage::Meter::scope` — it can be `pub(crate)`
1749. **nit** — `crates/editor-sidebar/src/sidebar/usage.rs:27:5` `sidebar::usage::Meter::used_percent` — it can be `pub(crate)`
1750. **nit** — `crates/editor-sidebar/src/sidebar/usage.rs:28:5` `sidebar::usage::Meter::resets_at` — it can be `pub(crate)`
1751. **nit** — `crates/editor-sidebar/src/sidebar/usage.rs:30:1` `sidebar::usage::meter` — it can be `pub(crate)`
1752. **nit** — `crates/editor-sidebar/src/sidebar/usage.rs:44:1` `sidebar::usage::groups` — it can be `pub(crate)`
1753. **nit** — `crates/editor-sidebar/src/sidebar/usage.rs:62:1` `sidebar::usage::reset_duration` — it can be `pub(crate)`
1754. **nit** — `crates/editor-sidebar/src/sidebar/usage.rs:70:1` `sidebar::usage::reset_remaining` — it can be `pub(crate)`
1755. **nit** — `crates/editor-sidebar/src/sidebar/usage.rs:89:1` `sidebar::usage::checked_label` — it can be `pub(crate)`
1756. **nit** — `crates/editor-sidebar/src/sidebar/usage.rs:103:1` `sidebar::usage::lit_of` — it can be `pub(crate)`
1757. **nit** — `crates/editor-sidebar/src/sidebar/usage.rs:113:1` `sidebar::usage::remaining_percent` — it can be `pub(crate)`
1758. **nit** — `crates/editor-sidebar/src/sidebar/usage_controller.rs:53:5` `sidebar::usage_controller::UsagePage::refresh_host` — it can be `pub(crate)`
1759. **nit** — `crates/editor-sidebar/src/sidebar/usage_controller.rs:121:5` `sidebar::usage_controller::UsagePage::refreshing` — it can be `pub(crate)`
1760. **nit** — `crates/editor-sidebar/src/sidebar/usage_controller.rs:126:5` `sidebar::usage_controller::UsagePage::error` — it can be `pub(crate)`
1761. **nit** — `crates/editor-sidebar/src/sidebar/view.rs:393:1` `sidebar::view::thread_menu` — it can be `pub(crate)`
1762. **nit** — `crates/editor-sidebar/src/sidebar/view.rs:498:1` `sidebar::view::rename_editor` — it can be `pub(crate)`
1763. **nit** — `crates/editor-sidebar/src/sidebar/view.rs:560:1` `sidebar::view::rename_dialog` — it can be `pub(crate)`
1764. **nit** — `crates/editor-sidebar/src/sidebar/view.rs:593:1` `sidebar::view::project_picker_dialog` — it can be `pub(crate)`
1765. **nit** — `crates/editor-sidebar/src/sidebar/view.rs:831:1` `sidebar::view::thread_list_view` — it can be `pub(crate)`

**forge-engines** (95)

1766. **nit** — `crates/forge-engines/src/account.rs:501:5` `account::SignIn::new` — it can be `pub(crate)`
1767. **nit** — `crates/forge-engines/src/account.rs:518:5` `account::SignIn::state` — it can be `pub(crate)`
1768. **nit** — `crates/forge-engines/src/account.rs:523:5` `account::SignIn::subscribe` — it can be `pub(crate)`
1769. **nit** — `crates/forge-engines/src/account.rs:559:5` `account::SignIn::check` — it can be `pub(crate)`
1770. **nit** — `crates/forge-engines/src/account.rs:595:5` `account::SignIn::login` — it can be `pub(crate)`
1771. **nit** — `crates/forge-engines/src/account.rs:610:5` `account::SignIn::submit_code` — it can be `pub(crate)`
1772. **nit** — `crates/forge-engines/src/account.rs:632:5` `account::SignIn::cancel_login` — it can be `pub(crate)`
1773. **nit** — `crates/forge-engines/src/account.rs:637:5` `account::SignIn::logout` — it can be `pub(crate)`
1774. **nit** — `crates/forge-engines/src/claudeai/capabilities.rs:15:5` `claudeai::capabilities::Capabilities::org_uuid` — it can be `pub(crate)`
1775. **nit** — `crates/forge-engines/src/claudeai/capabilities.rs:17:5` `claudeai::capabilities::Capabilities::account` — it can be `pub(crate)`
1776. **nit** — `crates/forge-engines/src/claudeai/capabilities.rs:19:5` `claudeai::capabilities::Capabilities::rate_limit_tier` — it can be `pub(crate)`
1777. **nit** — `crates/forge-engines/src/claudeai/capabilities.rs:21:5` `claudeai::capabilities::Capabilities::chat` — it can be `pub(crate)`
1778. **nit** — `crates/forge-engines/src/claudeai/capabilities.rs:23:5` `claudeai::capabilities::Capabilities::client_tools` — it can be `pub(crate)`
1779. **nit** — `crates/forge-engines/src/claudeai/decoder.rs:94:5` `claudeai::decoder::ClaudeAiDecoder::waiting_on_caller` — it can be `pub(crate)`
1780. **nit** — `crates/forge-engines/src/claudeai/decoder.rs:100:5` `claudeai::decoder::ClaudeAiDecoder::ended` — it can be `pub(crate)`
1781. **nit** — `crates/forge-engines/src/claudeai/decoder.rs:105:5` `claudeai::decoder::ClaudeAiDecoder::take_failure` — it can be `pub(crate)`
1782. **nit** — `crates/forge-engines/src/claudeai/decoder.rs:112:5` `claudeai::decoder::ClaudeAiDecoder::take_connector` — it can be `pub(crate)`
1783. **nit** — `crates/forge-engines/src/claudeai/decoder.rs:118:5` `claudeai::decoder::ClaudeAiDecoder::connector_started` — it can be `pub(crate)`
1784. **nit** — `crates/forge-engines/src/claudeai/decoder.rs:125:5` `claudeai::decoder::ClaudeAiDecoder::take_calls` — it can be `pub(crate)`
1785. **nit** — `crates/forge-engines/src/claudeai/decoder.rs:138:5` `claudeai::decoder::ClaudeAiDecoder::leaf` — it can be `pub(crate)`
1786. **nit** — `crates/forge-engines/src/claudeai/mcp.rs:2064:1` `claudeai::mcp::fake_server` — it can be `pub(crate)`
1787. **nit** — `crates/forge-engines/src/claudeai/mcp.rs:2094:1` `claudeai::mcp::fake_stdio` — it can be `pub(crate)`
1788. **nit** — `crates/forge-engines/src/claudeai/mod.rs:115:1` `claudeai::run` — it can be `pub(crate)`
1789. **nit** — `crates/forge-engines/src/claudeai/mod.rs:219:1` `claudeai::drive` — it can be `pub(crate)`
1790. **nit** — `crates/forge-engines/src/claudeai/mod.rs:230:1` `claudeai::drive_with` — it can be `pub(crate)`
1791. **nit** — `crates/forge-engines/src/claudeai/session.rs:30:5` `claudeai::session::Session::cookies` — it can be `pub(crate)`
1792. **nit** — `crates/forge-engines/src/claudeai/session.rs:32:5` `claudeai::session::Session::user_agent` — it can be `pub(crate)`
1793. **nit** — `crates/forge-engines/src/claudeai/session.rs:34:5` `claudeai::session::Session::org_uuid` — it can be `pub(crate)`
1794. **nit** — `crates/forge-engines/src/claudeai/session.rs:37:5` `claudeai::session::Session::account` — it can be `pub(crate)`
1795. **nit** — `crates/forge-engines/src/claudeai/session.rs:41:5` `claudeai::session::Session::needs_renewal` — it can be `pub(crate)`
1796. **nit** — `crates/forge-engines/src/claudeai/session.rs:44:5` `claudeai::session::Session::expired` — it can be `pub(crate)`
1797. **nit** — `crates/forge-engines/src/claudeai/session.rs:91:5` `claudeai::session::Session::cookie_header` — it can be `pub(crate)`
1798. **nit** — `crates/forge-engines/src/claudeai/session.rs:101:5` `claudeai::session::Session::cookie` — it can be `pub(crate)`
1799. **nit** — `crates/forge-engines/src/claudeai/session.rs:128:5` `claudeai::session::Session::status` — it can be `pub(crate)`
1800. **nit** — `crates/forge-engines/src/claudeai/session.rs:147:5` `claudeai::session::Session::load` — it can be `pub(crate)`
1801. **nit** — `crates/forge-engines/src/claudeai/session.rs:205:5` `claudeai::session::Session::update` — it can be `pub(crate)`
1802. **nit** — `crates/forge-engines/src/claudeai/session.rs:221:5` `claudeai::session::Session::remove` — it can be `pub(crate)`
1803. **nit** — `crates/forge-engines/src/claudeai/session.rs:232:5` `claudeai::session::Session::apply` — it can be `pub(crate)`
1804. **nit** — `crates/forge-engines/src/claudeai/tools.rs:50:5` `claudeai::tools::Call::id` — it can be `pub(crate)`
1805. **nit** — `crates/forge-engines/src/claudeai/tools.rs:52:5` `claudeai::tools::Call::name` — it can be `pub(crate)`
1806. **nit** — `crates/forge-engines/src/claudeai/tools.rs:54:5` `claudeai::tools::Call::input` — it can be `pub(crate)`
1807. **nit** — `crates/forge-engines/src/claudeai/tools.rs:72:5` `claudeai::tools::Answer::text` — it can be `pub(crate)`
1808. **nit** — `crates/forge-engines/src/claudeai/tools.rs:74:5` `claudeai::tools::Answer::is_error` — it can be `pub(crate)`
1809. **nit** — `crates/forge-engines/src/claudeai/tools.rs:96:5` `claudeai::tools::Runtime::skills` — it can be `pub(crate)`
1810. **nit** — `crates/forge-engines/src/claudeai/tools.rs:97:5` `claudeai::tools::Runtime::mcp` — it can be `pub(crate)`
1811. **nit** — `crates/forge-engines/src/claudeai/tools.rs:98:5` `claudeai::tools::Runtime::shells` — it can be `pub(crate)`
1812. **nit** — `crates/forge-engines/src/claudeai/tools.rs:99:5` `claudeai::tools::Runtime::rules` — it can be `pub(crate)`
1813. **nit** — `crates/forge-engines/src/claudeai/tools.rs:105:5` `claudeai::tools::Runtime::build` — it can be `pub(crate)`
1814. **nit** — `crates/forge-engines/src/claudeai/tools.rs:134:5` `claudeai::tools::Allowed::remember` — it can be `pub(crate)`
1815. **nit** — `crates/forge-engines/src/claudeai/transport.rs:38:5` `claudeai::transport::Request::prompt` — it can be `pub(crate)`
1816. **nit** — `crates/forge-engines/src/claudeai/transport.rs:40:5` `claudeai::transport::Request::model` — it can be `pub(crate)`
1817. **nit** — `crates/forge-engines/src/claudeai/transport.rs:43:5` `claudeai::transport::Request::thinking_mode` — it can be `pub(crate)`
1818. **nit** — `crates/forge-engines/src/claudeai/transport.rs:46:5` `claudeai::transport::Request::effort` — it can be `pub(crate)`
1819. **nit** — `crates/forge-engines/src/claudeai/transport.rs:50:5` `claudeai::transport::Request::tools` — it can be `pub(crate)`
1820. **nit** — `crates/forge-engines/src/claudeai/transport.rs:54:5` `claudeai::transport::Request::tool_states` — it can be `pub(crate)`
1821. **nit** — `crates/forge-engines/src/claudeai/transport.rs:57:5` `claudeai::transport::Request::files` — it can be `pub(crate)`
1822. **nit** — `crates/forge-engines/src/claudeai/transport.rs:64:5` `claudeai::transport::Request::parent` — it can be `pub(crate)`
1823. **nit** — `crates/forge-engines/src/claudeai/transport.rs:70:5` `claudeai::transport::Request::body` — it can be `pub(crate)`
1824. **nit** — `crates/forge-engines/src/claudeai/transport.rs:279:5` `claudeai::transport::Transport::open` — it can be `pub(crate)`
1825. **nit** — `crates/forge-engines/src/claudeai/transport.rs:290:5` `claudeai::transport::Transport::new` — it can be `pub(crate)`
1826. **nit** — `crates/forge-engines/src/claudeai/transport.rs:307:5` `claudeai::transport::Transport::session` — it can be `pub(crate)`
1827. **nit** — `crates/forge-engines/src/claudeai/transport.rs:395:5` `claudeai::transport::Transport::get` — it can be `pub(crate)`
1828. **nit** — `crates/forge-engines/src/claudeai/transport.rs:405:5` `claudeai::transport::Transport::rename_conversation` — it can be `pub(crate)`
1829. **nit** — `crates/forge-engines/src/harness.rs:51:5` `harness::Launch::gateway_mcp` — it can be `pub(crate)`
1830. **nit** — `crates/forge-engines/src/harness.rs:131:5` `harness::EngineEvent::background_wait` — it can be `pub(crate)`
1831. **nit** — `crates/forge-engines/src/manager.rs:640:5` `manager::Engines::refresh_models` — it can be `pub(crate)`
1832. **nit** — `crates/forge-engines/src/mcp_oauth.rs:39:5` `mcp_oauth::Discovery::resource` — it can be `pub(crate)`
1833. **nit** — `crates/forge-engines/src/mcp_oauth.rs:40:5` `mcp_oauth::Discovery::authorization_endpoint` — it can be `pub(crate)`
1834. **nit** — `crates/forge-engines/src/mcp_oauth.rs:41:5` `mcp_oauth::Discovery::token_endpoint` — it can be `pub(crate)`
1835. **nit** — `crates/forge-engines/src/mcp_oauth.rs:43:5` `mcp_oauth::Discovery::registration_endpoint` — it can be `pub(crate)`
1836. **nit** — `crates/forge-engines/src/mcp_oauth.rs:45:5` `mcp_oauth::Discovery::scope` — it can be `pub(crate)`
1837. **nit** — `crates/forge-engines/src/mcp_oauth.rs:51:5` `mcp_oauth::Client::client_id` — it can be `pub(crate)`
1838. **nit** — `crates/forge-engines/src/mcp_oauth.rs:53:5` `mcp_oauth::Client::client_secret` — it can be `pub(crate)`
1839. **nit** — `crates/forge-engines/src/mcp_oauth.rs:64:5` `mcp_oauth::Tokens::expires_at` — it can be `pub(crate)`
1840. **nit** — `crates/forge-engines/src/mcp_oauth.rs:79:5` `mcp_oauth::Pkce::challenge` — it can be `pub(crate)`
1841. **nit** — `crates/forge-engines/src/mcp_oauth.rs:153:1` `mcp_oauth::discover` — it can be `pub(crate)`
1842. **nit** — `crates/forge-engines/src/mcp_oauth.rs:437:21` `mcp_oauth::Rejected::0` — it can be `pub(crate)`
1843. **nit** — `crates/forge-engines/src/opencode/decoder.rs:190:5` `opencode::decoder::Decoder::has_root` — it can be `pub(crate)`
1844. **nit** — `crates/forge-engines/src/process.rs:13:1` `process::MAX_LINE_BYTES` — it can be `pub(crate)`
1845. **nit** — `crates/forge-engines/src/process.rs:16:1` `process::EngineProcess` — it can be `pub(crate)`
1846. **nit** — `crates/forge-engines/src/process.rs:30:1` `process::spawn` — it can be `pub(crate)`
1847. **nit** — `crates/forge-engines/src/process.rs:178:5` `process::EngineProcess::send` — it can be `pub(crate)`
1848. **nit** — `crates/forge-engines/src/process.rs:199:5` `process::EngineProcess::close_stdin` — it can be `pub(crate)`
1849. **nit** — `crates/forge-engines/src/process.rs:206:5` `process::EngineProcess::note` — it can be `pub(crate)`
1850. **nit** — `crates/forge-engines/src/process.rs:214:5` `process::EngineProcess::next_line` — it can be `pub(crate)`
1851. **nit** — `crates/forge-engines/src/process.rs:260:5` `process::EngineProcess::stderr_tail` — it can be `pub(crate)`
1852. **nit** — `crates/forge-engines/src/process.rs:269:5` `process::EngineProcess::kill` — it can be `pub(crate)`
1853. **nit** — `crates/forge-engines/src/process.rs:277:5` `process::EngineProcess::wait` — it can be `pub(crate)`
1854. **nit** — `crates/forge-engines/src/transcript_tests.rs:6:1` `transcript_tests::read` — it can be `pub(crate)`
1855. **nit** — `crates/forge-engines/src/transcript_tests.rs:11:1` `transcript_tests::read_path` — it can be `pub(crate)`
1856. **nit** — `crates/forge-engines/src/transcript_tests.rs:19:1` `transcript_tests::merge_live` — it can be `pub(crate)`
1857. **nit** — `crates/forge-engines/src/usage.rs:91:5` `usage::UsageCache::new` — it can be `pub(crate)`
1858. **nit** — `crates/forge-engines/src/usage.rs:101:5` `usage::UsageCache::cached` — it can be `pub(crate)`
1859. **nit** — `crates/forge-engines/src/usage.rs:109:5` `usage::UsageCache::read` — it can be `pub(crate)`
1860. **nit** — `crates/forge-engines/src/usage.rs:141:5` `usage::UsageCache::invalidate` — it can be `pub(crate)`

**editor-settings** (89)

1861. **nit** — `crates/editor-settings/src/settings/about.rs:11:1` `settings::about::rows` — it can be `pub(crate)`
1862. **nit** — `crates/editor-settings/src/settings/account_view.rs:55:1` `settings::account_view::sign_in` — it can be `pub(crate)`
1863. **nit** — `crates/editor-settings/src/settings/account_view.rs:331:1` `settings::account_view::page` — it can be `pub(crate)`
1864. **nit** — `crates/editor-settings/src/settings/archive.rs:9:5` `settings::archive::ArchiveForm::host` — it can be `pub(crate)`
1865. **nit** — `crates/editor-settings/src/settings/archive.rs:10:5` `settings::archive::ArchiveForm::restoring` — it can be `pub(crate)`
1866. **nit** — `crates/editor-settings/src/settings/archive.rs:11:5` `settings::archive::ArchiveForm::error` — it can be `pub(crate)`
1867. **nit** — `crates/editor-settings/src/settings/archive.rs:12:5` `settings::archive::ArchiveForm::ids` — it can be `pub(crate)`
1868. **nit** — `crates/editor-settings/src/settings/archive.rs:18:5` `settings::archive::ArchiveRequest::host` — it can be `pub(crate)`
1869. **nit** — `crates/editor-settings/src/settings/archive.rs:19:5` `settings::archive::ArchiveRequest::thread` — it can be `pub(crate)`
1870. **nit** — `crates/editor-settings/src/settings/archive.rs:24:5` `settings::archive::ArchiveForm::invalidate` — it can be `pub(crate)`
1871. **nit** — `crates/editor-settings/src/settings/archive.rs:28:5` `settings::archive::ArchiveForm::sync` — it can be `pub(crate)`
1872. **nit** — `crates/editor-settings/src/settings/archive.rs:53:5` `settings::archive::ArchiveForm::begin` — it can be `pub(crate)`
1873. **nit** — `crates/editor-settings/src/settings/archive.rs:66:5` `settings::archive::ArchiveForm::complete` — it can be `pub(crate)`
1874. **nit** — `crates/editor-settings/src/settings/archive_view.rs:92:1` `settings::archive_view::cleanup` — it can be `pub(crate)`
1875. **nit** — `crates/editor-settings/src/settings/archive_view.rs:147:1` `settings::archive_view::archived` — it can be `pub(crate)`
1876. **nit** — `crates/editor-settings/src/settings/controller.rs:32:5` `settings::controller::SettingsController::reported_prompt_dirty` — it can be `pub(crate)`
1877. **nit** — `crates/editor-settings/src/settings/controller.rs:33:5` `settings::controller::SettingsController::reported_font_menu` — it can be `pub(crate)`
1878. **nit** — `crates/editor-settings/src/settings/controller.rs:34:5` `settings::controller::SettingsController::reported_streaming_menu` — it can be `pub(crate)`
1879. **nit** — `crates/editor-settings/src/settings/controller.rs:35:5` `settings::controller::SettingsController::reported_retention_menu` — it can be `pub(crate)`
1880. **nit** — `crates/editor-settings/src/settings/controller.rs:36:5` `settings::controller::SettingsController::mcp_editors` — it can be `pub(crate)`
1881. **nit** — `crates/editor-settings/src/settings/controller.rs:41:5` `settings::controller::SettingsController::forge_code` — it can be `pub(crate)`
1882. **nit** — `crates/editor-settings/src/settings/controller.rs:42:5` `settings::controller::SettingsController::forge_error` — it can be `pub(crate)`
1883. **nit** — `crates/editor-settings/src/settings/controller.rs:43:5` `settings::controller::SettingsController::archive` — it can be `pub(crate)`
1884. **nit** — `crates/editor-settings/src/settings/controller.rs:44:5` `settings::controller::SettingsController::engines` — it can be `pub(crate)`
1885. **nit** — `crates/editor-settings/src/settings/controller.rs:45:5` `settings::controller::SettingsController::extensions` — it can be `pub(crate)`
1886. **nit** — `crates/editor-settings/src/settings/controller.rs:195:5` `settings::controller::SettingsController::claim_forge` — it can be `pub(crate)`
1887. **nit** — `crates/editor-settings/src/settings/controller.rs:201:5` `settings::controller::SettingsController::unarchive` — it can be `pub(crate)`
1888. **nit** — `crates/editor-settings/src/settings/controller.rs:272:5` `settings::controller::SettingsController::open_mcp` — it can be `pub(crate)`
1889. **nit** — `crates/editor-settings/src/settings/controller.rs:287:5` `settings::controller::SettingsController::save_mcp` — it can be `pub(crate)`
1890. **nit** — `crates/editor-settings/src/settings/controller.rs:303:5` `settings::controller::SettingsController::remove_mcp` — it can be `pub(crate)`
1891. **nit** — `crates/editor-settings/src/settings/controller.rs:312:5` `settings::controller::SettingsController::sign_in_mcp` — it can be `pub(crate)`
1892. **nit** — `crates/editor-settings/src/settings/controller.rs:323:5` `settings::controller::SettingsController::cancel_mcp_sign_in` — it can be `pub(crate)`
1893. **nit** — `crates/editor-settings/src/settings/controller.rs:332:5` `settings::controller::SettingsController::sign_out_mcp` — it can be `pub(crate)`
1894. **nit** — `crates/editor-settings/src/settings/controller.rs:363:5` `settings::controller::SettingsController::sign_in_claude_ai` — it can be `pub(crate)`
1895. **nit** — `crates/editor-settings/src/settings/controller.rs:428:5` `settings::controller::SettingsController::engine_action` — it can be `pub(crate)`
1896. **nit** — `crates/editor-settings/src/settings/controller.rs:466:5` `settings::controller::SettingsController::save_prompt` — it can be `pub(crate)`
1897. **nit** — `crates/editor-settings/src/settings/controller.rs:470:5` `settings::controller::SettingsController::resolve_prompt` — it can be `pub(crate)`
1898. **nit** — `crates/editor-settings/src/settings/controller.rs:517:5` `settings::controller::SettingsController::load_latest_prompt` — it can be `pub(crate)`
1899. **nit** — `crates/editor-settings/src/settings/controller.rs:624:1` `settings::controller::same_host_instance` — it can be `pub(crate)`
1900. **nit** — `crates/editor-settings/src/settings/engine_view.rs:481:1` `settings::engine_view::page` — it can be `pub(crate)`
1901. **nit** — `crates/editor-settings/src/settings/engines.rs:11:5` `settings::engines::EnginesForm::host` — it can be `pub(crate)`
1902. **nit** — `crates/editor-settings/src/settings/engines.rs:13:5` `settings::engines::EnginesForm::busy` — it can be `pub(crate)`
1903. **nit** — `crates/editor-settings/src/settings/engines.rs:15:5` `settings::engines::EnginesForm::choosing_version` — it can be `pub(crate)`
1904. **nit** — `crates/editor-settings/src/settings/engines.rs:21:5` `settings::engines::EngineRequest::host` — it can be `pub(crate)`
1905. **nit** — `crates/editor-settings/src/settings/engines.rs:22:5` `settings::engines::EngineRequest::engine` — it can be `pub(crate)`
1906. **nit** — `crates/editor-settings/src/settings/engines.rs:39:5` `settings::engines::EnginesForm::begin` — it can be `pub(crate)`
1907. **nit** — `crates/editor-settings/src/settings/engines.rs:53:5` `settings::engines::EnginesForm::complete` — it can be `pub(crate)`
1908. **nit** — `crates/editor-settings/src/settings/extensions.rs:4:1` `settings::extensions::MCP_SERVERS_KEY` — it can be `pub(crate)`
1909. **nit** — `crates/editor-settings/src/settings/extensions.rs:18:5` `settings::extensions::ServerFields::from_server` — it can be `pub(crate)`
1910. **nit** — `crates/editor-settings/src/settings/extensions.rs:82:5` `settings::extensions::ExtensionsForm::host` — it can be `pub(crate)`
1911. **nit** — `crates/editor-settings/src/settings/extensions.rs:85:5` `settings::extensions::ExtensionsForm::saving` — it can be `pub(crate)`
1912. **nit** — `crates/editor-settings/src/settings/extensions.rs:86:5` `settings::extensions::ExtensionsForm::error` — it can be `pub(crate)`
1913. **nit** — `crates/editor-settings/src/settings/extensions.rs:92:5` `settings::extensions::MutationRequest::host` — it can be `pub(crate)`
1914. **nit** — `crates/editor-settings/src/settings/extensions.rs:93:5` `settings::extensions::MutationRequest::generation` — it can be `pub(crate)`
1915. **nit** — `crates/editor-settings/src/settings/extensions.rs:94:5` `settings::extensions::MutationRequest::mutation` — it can be `pub(crate)`
1916. **nit** — `crates/editor-settings/src/settings/extensions.rs:95:5` `settings::extensions::MutationRequest::close_form` — it can be `pub(crate)`
1917. **nit** — `crates/editor-settings/src/settings/extensions.rs:118:5` `settings::extensions::ExtensionsForm::cancel` — it can be `pub(crate)`
1918. **nit** — `crates/editor-settings/src/settings/extensions.rs:140:5` `settings::extensions::ExtensionsForm::remove` — it can be `pub(crate)`
1919. **nit** — `crates/editor-settings/src/settings/mcp_view.rs:205:1` `settings::mcp_view::section` — it can be `pub(crate)`
1920. **nit** — `crates/editor-settings/src/settings/page.rs:8:1` `settings::page::description` — it can be `pub(crate)`
1921. **nit** — `crates/editor-settings/src/settings/preferences.rs:5:5` `settings::preferences::SettingRequest::key` — it can be `pub(crate)`
1922. **nit** — `crates/editor-settings/src/settings/preferences.rs:6:5` `settings::preferences::SettingRequest::value` — it can be `pub(crate)`
1923. **nit** — `crates/editor-settings/src/settings/preferences.rs:19:1` `settings::preferences::rollback` — it can be `pub(crate)`
1924. **nit** — `crates/editor-settings/src/settings/prompt.rs:5:5` `settings::prompt::PromptForm::host` — it can be `pub(crate)`
1925. **nit** — `crates/editor-settings/src/settings/prompt.rs:8:5` `settings::prompt::PromptForm::base_version` — it can be `pub(crate)`
1926. **nit** — `crates/editor-settings/src/settings/prompt.rs:9:5` `settings::prompt::PromptForm::newer` — it can be `pub(crate)`
1927. **nit** — `crates/editor-settings/src/settings/prompt.rs:10:5` `settings::prompt::PromptForm::pending` — it can be `pub(crate)`
1928. **nit** — `crates/editor-settings/src/settings/prompt.rs:18:5` `settings::prompt::PromptRequest::host` — it can be `pub(crate)`
1929. **nit** — `crates/editor-settings/src/settings/prompt.rs:19:5` `settings::prompt::PromptRequest::save` — it can be `pub(crate)`
1930. **nit** — `crates/editor-settings/src/settings/prompt.rs:86:5` `settings::prompt::PromptForm::accepts` — it can be `pub(crate)`
1931. **nit** — `crates/editor-settings/src/settings/sign_in.rs:19:5` `settings::sign_in::SignInForm::request_link` — it can be `pub(crate)`
1932. **nit** — `crates/editor-settings/src/settings/sign_in.rs:25:5` `settings::sign_in::SignInForm::finish_sign_in` — it can be `pub(crate)`
1933. **nit** — `crates/editor-settings/src/settings/ui.rs:16:1` `settings::ui::CONTROL_WIDTH` — it can be `pub(crate)`
1934. **nit** — `crates/editor-settings/src/settings/ui.rs:39:1` `settings::ui::text` — it can be `pub(crate)`
1935. **nit** — `crates/editor-settings/src/settings/ui.rs:43:1` `settings::ui::line` — it can be `pub(crate)`
1936. **nit** — `crates/editor-settings/src/settings/ui.rs:47:1` `settings::ui::column` — it can be `pub(crate)`
1937. **nit** — `crates/editor-settings/src/settings/ui.rs:51:1` `settings::ui::stack` — it can be `pub(crate)`
1938. **nit** — `crates/editor-settings/src/settings/ui.rs:55:1` `settings::ui::actions` — it can be `pub(crate)`
1939. **nit** — `crates/editor-settings/src/settings/ui.rs:61:1` `settings::ui::fixed` — it can be `pub(crate)`
1940. **nit** — `crates/editor-settings/src/settings/ui.rs:69:1` `settings::ui::grow` — it can be `pub(crate)`
1941. **nit** — `crates/editor-settings/src/settings/ui.rs:76:1` `settings::ui::button` — it can be `pub(crate)`
1942. **nit** — `crates/editor-settings/src/settings/ui.rs:90:1` `settings::ui::header` — it can be `pub(crate)`
1943. **nit** — `crates/editor-settings/src/settings/ui.rs:97:1` `settings::ui::section` — it can be `pub(crate)`
1944. **nit** — `crates/editor-settings/src/settings/ui.rs:122:1` `settings::ui::card` — it can be `pub(crate)`
1945. **nit** — `crates/editor-settings/src/settings/ui.rs:135:1` `settings::ui::row` — it can be `pub(crate)`
1946. **nit** — `crates/editor-settings/src/settings/ui.rs:161:1` `settings::ui::block` — it can be `pub(crate)`
1947. **nit** — `crates/editor-settings/src/settings/ui.rs:170:1` `settings::ui::note` — it can be `pub(crate)`
1948. **nit** — `crates/editor-settings/src/settings/ui.rs:174:1` `settings::ui::error_text` — it can be `pub(crate)`
1949. **nit** — `crates/editor-settings/src/settings/ui.rs:178:1` `settings::ui::callout` — it can be `pub(crate)`

**editor-core** (75)

1950. **nit** — `crates/editor-core/src/core/access.rs:19:5` `core::access::OnboardingRecord::account_id` — it can be `pub(crate)`
1951. **nit** — `crates/editor-core/src/core/access.rs:20:5` `core::access::OnboardingRecord::base_url` — it can be `pub(crate)`
1952. **nit** — `crates/editor-core/src/core/access.rs:21:5` `core::access::OnboardingRecord::machine_id` — it can be `pub(crate)`
1953. **nit** — `crates/editor-core/src/core/access.rs:24:1` `core::access::live_machines` — it can be `pub(crate)`
1954. **nit** — `crates/editor-core/src/core/access.rs:31:1` `core::access::live_machine` — it can be `pub(crate)`
1955. **nit** — `crates/editor-core/src/core/access.rs:38:1` `core::access::entry_gate` — it can be `pub(crate)`
1956. **nit** — `crates/editor-core/src/core/access.rs:62:1` `core::access::load_onboarding` — it can be `pub(crate)`
1957. **nit** — `crates/editor-core/src/core/access.rs:70:1` `core::access::save_onboarding` — it can be `pub(crate)`
1958. **nit** — `crates/editor-core/src/core/account/mod.rs:299:5` `core::account::Account::entry_gate` — it can be `pub(crate)`
1959. **nit** — `crates/editor-core/src/core/account/mod.rs:908:1` `core::account::is_session_invalid` — it can be `pub(crate)`
1960. **nit** — `crates/editor-core/src/core/ae.rs:15:5` `core::ae::Ae::run` — it can be `pub(crate)`
1961. **nit** — `crates/editor-core/src/core/client.rs:48:5` `core::client::ForgeClient::is_relay` — it can be `pub(crate)`
1962. **nit** — `crates/editor-core/src/core/client.rs:53:5` `core::client::ForgeClient::relay` — it can be `pub(crate)`
1963. **nit** — `crates/editor-core/src/core/client.rs:60:5` `core::client::ForgeClient::close_relay` — it can be `pub(crate)`
1964. **nit** — `crates/editor-core/src/core/client.rs:238:5` `core::client::ForgeClient::state` — it can be `pub(crate)`
1965. **nit** — `crates/editor-core/src/core/client.rs:315:5` `core::client::ForgeClient::mcp_sign_in_start` — it can be `pub(crate)`
1966. **nit** — `crates/editor-core/src/core/client.rs:323:5` `core::client::ForgeClient::mcp_sign_in_finish` — it can be `pub(crate)`
1967. **nit** — `crates/editor-core/src/core/client.rs:331:5` `core::client::ForgeClient::mcp_sign_in_cancel` — it can be `pub(crate)`
1968. **nit** — `crates/editor-core/src/core/client.rs:358:5` `core::client::ForgeClient::set_model_config` — it can be `pub(crate)`
1969. **nit** — `crates/editor-core/src/core/client.rs:370:5` `core::client::ForgeClient::set_model_group` — it can be `pub(crate)`
1970. **nit** — `crates/editor-core/src/core/client.rs:419:5` `core::client::ForgeClient::thread` — it can be `pub(crate)`
1971. **nit** — `crates/editor-core/src/core/client.rs:438:5` `core::client::ForgeClient::inspector` — it can be `pub(crate)`
1972. **nit** — `crates/editor-core/src/core/client.rs:447:5` `core::client::ForgeClient::content_range` — it can be `pub(crate)`
1973. **nit** — `crates/editor-core/src/core/client.rs:493:5` `core::client::ForgeClient::collection_range` — it can be `pub(crate)`
1974. **nit** — `crates/editor-core/src/core/client.rs:538:5` `core::client::ForgeClient::source_index_range` — it can be `pub(crate)`
1975. **nit** — `crates/editor-core/src/core/client.rs:704:5` `core::client::ForgeClient::reply_editor_call` — it can be `pub(crate)`
1976. **nit** — `crates/editor-core/src/core/client.rs:766:5` `core::client::ForgeClient::project_icon` — it can be `pub(crate)`
1977. **nit** — `crates/editor-core/src/core/client.rs:774:5` `core::client::ForgeClient::project_repository` — it can be `pub(crate)`
1978. **nit** — `crates/editor-core/src/core/client.rs:789:5` `core::client::ForgeClient::skill_catalog` — it can be `pub(crate)`
1979. **nit** — `crates/editor-core/src/core/client.rs:820:5` `core::client::ForgeClient::global_prompt` — it can be `pub(crate)`
1980. **nit** — `crates/editor-core/src/core/client.rs:848:5` `core::client::ForgeClient::pair` — it can be `pub(crate)`
1981. **nit** — `crates/editor-core/src/core/client.rs:904:5` `core::client::ForgeClient::via_relay` — it can be `pub(crate)`
1982. **nit** — `crates/editor-core/src/core/client.rs:944:1` `core::client::check` — it can be `pub(crate)`
1983. **nit** — `crates/editor-core/src/core/client.rs:989:1` `core::client::read_body` — it can be `pub(crate)`
1984. **nit** — `crates/editor-core/src/core/delivery.rs:51:5` `core::delivery::MappedSender::<T, U>::is_closed` — it can be `pub(crate)`
1985. **nit** — `crates/editor-core/src/core/delivery.rs:54:5` `core::delivery::MappedSender::<T, U>::send` — it can be `pub(crate)`
1986. **nit** — `crates/editor-core/src/core/delivery.rs:150:5` `core::delivery::DeliverySender::<T>::map` — it can be `pub(crate)`
1987. **nit** — `crates/editor-core/src/core/delivery.rs:156:5` `core::delivery::DeliverySender::<T>::is_closed` — it can be `pub(crate)`
1988. **nit** — `crates/editor-core/src/core/diagnostics/open_timing.rs:164:1` `core::diagnostics::open_timing::rpc_done` — it can be `pub(crate)`
1989. **nit** — `crates/editor-core/src/core/diagnostics/rebuild_stats.rs:17:5` `core::diagnostics::rebuild_stats::RebuildStats::event` — it can be `pub(crate)`
1990. **nit** — `crates/editor-core/src/core/diagnostics/rebuild_stats.rs:22:5` `core::diagnostics::rebuild_stats::RebuildStats::rebuild` — it can be `pub(crate)`
1991. **nit** — `crates/editor-core/src/core/diagnostics/rebuild_stats.rs:30:5` `core::diagnostics::rebuild_stats::RebuildStats::span` — it can be `pub(crate)`
1992. **nit** — `crates/editor-core/src/core/host.rs:352:5` `core::host::HostData::account` — it can be `pub(crate)`
1993. **nit** — `crates/editor-core/src/core/host.rs:358:5` `core::host::HostData::prompt_revision` — it can be `pub(crate)`
1994. **nit** — `crates/editor-core/src/core/host.rs:359:5` `core::host::HostData::catalog_revision` — it can be `pub(crate)`
1995. **nit** — `crates/editor-core/src/core/host.rs:568:5` `core::host::HostData::retry_now` — it can be `pub(crate)`
1996. **nit** — `crates/editor-core/src/core/host/images.rs:91:5` `core::host::images::<impl core::host::HostData>::set_image_assets` — it can be `pub(crate)`
1997. **nit** — `crates/editor-core/src/core/host/pages.rs:241:5` `core::host::pages::<impl core::host::HostData>::ensure_thread` — it can be `pub(crate)`
1998. **nit** — `crates/editor-core/src/core/host/wsl.rs:22:5` `core::host::wsl::<impl core::host::HostData>::set_pairing` — it can be `pub(crate)`
1999. **nit** — `crates/editor-core/src/core/hosts.rs:47:5` `core::hosts::HostsFile::hosts` — it can be `pub(crate)`
2000. **nit** — `crates/editor-core/src/core/hosts.rs:48:5` `core::hosts::HostsFile::last` — it can be `pub(crate)`
2001. **nit** — `crates/editor-core/src/core/hosts.rs:72:5` `core::hosts::EditorFiles::identity` — it can be `pub(crate)`
2002. **nit** — `crates/editor-core/src/core/hosts.rs:87:5` `core::hosts::EditorFiles::hosts` — it can be `pub(crate)`
2003. **nit** — `crates/editor-core/src/core/hosts.rs:91:5` `core::hosts::EditorFiles::save_hosts` — it can be `pub(crate)`
2004. **nit** — `crates/editor-core/src/core/hosts.rs:95:5` `core::hosts::EditorFiles::settings` — it can be `pub(crate)`
2005. **nit** — `crates/editor-core/src/core/hosts.rs:99:5` `core::hosts::EditorFiles::save_settings` — it can be `pub(crate)`
2006. **nit** — `crates/editor-core/src/core/hosts.rs:206:5` `core::hosts::WslDistro::default` — it can be `pub(crate)`
2007. **nit** — `crates/editor-core/src/core/hosts.rs:207:5` `core::hosts::WslDistro::running` — it can be `pub(crate)`
2008. **nit** — `crates/editor-core/src/core/hosts.rs:290:1` `core::hosts::pair` — it can be `pub(crate)`
2009. **nit** — `crates/editor-core/src/core/runtime.rs:20:1` `core::runtime::run_http` — it can be `pub(crate)`
2010. **nit** — `crates/editor-core/src/core/screen.rs:58:5` `core::screen::SettingsSection::parent` — it can be `pub(crate)`
2011. **nit** — `crates/editor-core/src/core/screen.rs:89:5` `core::screen::SettingsSection::label` — it can be `pub(crate)`
2012. **nit** — `crates/editor-core/src/core/screen.rs:162:5` `core::screen::SettingsPage::ALL` — it can be `pub(crate)`
2013. **nit** — `crates/editor-core/src/core/screen.rs:189:5` `core::screen::SettingsPage::from_name` — it can be `pub(crate)`
2014. **nit** — `crates/editor-core/src/core/services.rs:32:5` `core::services::Services::installed` — it can be `pub(crate)`
2015. **nit** — `crates/editor-core/src/core/services.rs:61:5` `core::services::RelayDialRequest::account_id` — it can be `pub(crate)`
2016. **nit** — `crates/editor-core/src/core/services.rs:62:5` `core::services::RelayDialRequest::machine_id` — it can be `pub(crate)`
2017. **nit** — `crates/editor-core/src/core/services.rs:63:5` `core::services::RelayDialRequest::forge_ed25519_b64u` — it can be `pub(crate)`
2018. **nit** — `crates/editor-core/src/core/services.rs:64:5` `core::services::RelayDialRequest::grant_jwt` — it can be `pub(crate)`
2019. **nit** — `crates/editor-core/src/core/services.rs:65:5` `core::services::RelayDialRequest::cnf_secret` — it can be `pub(crate)`
2020. **nit** — `crates/editor-core/src/core/services.rs:74:1` `core::services::open_relay` — it can be `pub(crate)`
2021. **nit** — `crates/editor-core/src/core/store.rs:92:5` `core::store::Core::files` — it can be `pub(crate)`
2022. **nit** — `crates/editor-core/src/core/store.rs:123:5` `core::store::Core::new` — it can be `pub(crate)`
2023. **nit** — `crates/editor-core/src/core/store.rs:359:5` `core::store::Core::is_paired` — it can be `pub(crate)`
2024. **nit** — `crates/editor-core/src/core/store.rs:421:5` `core::store::Core::screen_behind_settings` — it can be `pub(crate)`

**editor-marketplace** (50)

2025. **nit** — `crates/editor-marketplace/src/marketplace/controller.rs:52:5` `marketplace::controller::MarketplaceController::search` — it can be `pub(crate)`
2026. **nit** — `crates/editor-marketplace/src/marketplace/controller.rs:58:5` `marketplace::controller::MarketplaceController::prompt_hovered` — it can be `pub(crate)`
2027. **nit** — `crates/editor-marketplace/src/marketplace/controller.rs:167:5` `marketplace::controller::MarketplaceController::loading_started` — it can be `pub(crate)`
2028. **nit** — `crates/editor-marketplace/src/marketplace/controller.rs:201:5` `marketplace::controller::MarketplaceController::edited` — it can be `pub(crate)`
2029. **nit** — `crates/editor-marketplace/src/marketplace/controller.rs:236:5` `marketplace::controller::MarketplaceController::clear_search` — it can be `pub(crate)`
2030. **nit** — `crates/editor-marketplace/src/marketplace/controller.rs:241:5` `marketplace::controller::MarketplaceController::list_key` — it can be `pub(crate)`
2031. **nit** — `crates/editor-marketplace/src/marketplace/controller.rs:256:5` `marketplace::controller::MarketplaceController::act` — it can be `pub(crate)`
2032. **nit** — `crates/editor-marketplace/src/marketplace/controller.rs:263:5` `marketplace::controller::MarketplaceController::operate` — it can be `pub(crate)`
2033. **nit** — `crates/editor-marketplace/src/marketplace/controller.rs:286:5` `marketplace::controller::MarketplaceController::retry` — it can be `pub(crate)`
2034. **nit** — `crates/editor-marketplace/src/marketplace/mod.rs:12:1` `marketplace::REMOTE_SEARCH_DELAY` — it can be `pub(crate)`
2035. **nit** — `crates/editor-marketplace/src/marketplace/mod.rs:40:5` `marketplace::SearchRequest::query` — it can be `pub(crate)`
2036. **nit** — `crates/editor-marketplace/src/marketplace/mod.rs:52:5` `marketplace::OperationRequest::operation` — it can be `pub(crate)`
2037. **nit** — `crates/editor-marketplace/src/marketplace/mod.rs:72:5` `marketplace::Marketplace::host` — it can be `pub(crate)`
2038. **nit** — `crates/editor-marketplace/src/marketplace/mod.rs:73:5` `marketplace::Marketplace::generation` — it can be `pub(crate)`
2039. **nit** — `crates/editor-marketplace/src/marketplace/mod.rs:75:5` `marketplace::Marketplace::kind` — it can be `pub(crate)`
2040. **nit** — `crates/editor-marketplace/src/marketplace/mod.rs:77:5` `marketplace::Marketplace::found_open` — it can be `pub(crate)`
2041. **nit** — `crates/editor-marketplace/src/marketplace/mod.rs:78:5` `marketplace::Marketplace::focus_search` — it can be `pub(crate)`
2042. **nit** — `crates/editor-marketplace/src/marketplace/mod.rs:83:5` `marketplace::Marketplace::shown` — it can be `pub(crate)`
2043. **nit** — `crates/editor-marketplace/src/marketplace/mod.rs:153:5` `marketplace::Marketplace::search` — it can be `pub(crate)`
2044. **nit** — `crates/editor-marketplace/src/marketplace/mod.rs:170:5` `marketplace::Marketplace::set_kind` — it can be `pub(crate)`
2045. **nit** — `crates/editor-marketplace/src/marketplace/mod.rs:175:5` `marketplace::Marketplace::results` — it can be `pub(crate)`
2046. **nit** — `crates/editor-marketplace/src/marketplace/mod.rs:197:5` `marketplace::Marketplace::list_key` — it can be `pub(crate)`
2047. **nit** — `crates/editor-marketplace/src/marketplace/mod.rs:210:5` `marketplace::Marketplace::act` — it can be `pub(crate)`
2048. **nit** — `crates/editor-marketplace/src/marketplace/mod.rs:237:5` `marketplace::Marketplace::execute` — it can be `pub(crate)`
2049. **nit** — `crates/editor-marketplace/src/marketplace/mod.rs:274:1` `marketplace::entries` — it can be `pub(crate)`
2050. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:8:5` `marketplace::presentation::SkillRow::id` — it can be `pub(crate)`
2051. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:9:5` `marketplace::presentation::SkillRow::name` — it can be `pub(crate)`
2052. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:10:5` `marketplace::presentation::SkillRow::description` — it can be `pub(crate)`
2053. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:11:5` `marketplace::presentation::SkillRow::size` — it can be `pub(crate)`
2054. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:12:5` `marketplace::presentation::SkillRow::enabled` — it can be `pub(crate)`
2055. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:13:5` `marketplace::presentation::SkillRow::origin` — it can be `pub(crate)`
2056. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:14:5` `marketplace::presentation::SkillRow::marks` — it can be `pub(crate)`
2057. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:15:5` `marketplace::presentation::SkillRow::attention` — it can be `pub(crate)`
2058. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:16:5` `marketplace::presentation::SkillRow::found_in` — it can be `pub(crate)`
2059. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:21:5` `marketplace::presentation::EngineMark::engine` — it can be `pub(crate)`
2060. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:22:5` `marketplace::presentation::EngineMark::lit` — it can be `pub(crate)`
2061. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:23:5` `marketplace::presentation::EngineMark::label` — it can be `pub(crate)`
2062. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:34:5` `marketplace::presentation::ListingRow::id` — it can be `pub(crate)`
2063. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:35:5` `marketplace::presentation::ListingRow::name` — it can be `pub(crate)`
2064. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:36:5` `marketplace::presentation::ListingRow::meta` — it can be `pub(crate)`
2065. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:63:5` `marketplace::presentation::Item::kind` — it can be `pub(crate)`
2066. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:70:5` `marketplace::presentation::Item::managed` — it can be `pub(crate)`
2067. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:74:5` `marketplace::presentation::Item::selector` — it can be `pub(crate)`
2068. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:92:5` `marketplace::presentation::Kind::admits` — it can be `pub(crate)`
2069. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:97:1` `marketplace::presentation::items` — it can be `pub(crate)`
2070. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:142:1` `marketplace::presentation::listing_item` — it can be `pub(crate)`
2071. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:254:1` `marketplace::presentation::prompt_attention` — it can be `pub(crate)`
2072. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:273:1` `marketplace::presentation::found_summary` — it can be `pub(crate)`
2073. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:300:1` `marketplace::presentation::engine_name` — it can be `pub(crate)`
2074. **nit** — `crates/editor-marketplace/src/marketplace/presentation.rs:329:1` `marketplace::presentation::skill_meta` — it can be `pub(crate)`

**editor-inspector** (47)

2075. **nit** — `crates/editor-inspector/src/inspector/controller.rs:12:5` `inspector::controller::Inspector::entries` — it can be `pub(crate)`
2076. **nit** — `crates/editor-inspector/src/inspector/controller.rs:13:5` `inspector::controller::Inspector::error` — it can be `pub(crate)`
2077. **nit** — `crates/editor-inspector/src/inspector/controller.rs:14:5` `inspector::controller::Inspector::source` — it can be `pub(crate)`
2078. **nit** — `crates/editor-inspector/src/inspector/controller.rs:15:5` `inspector::controller::Inspector::plan` — it can be `pub(crate)`
2079. **nit** — `crates/editor-inspector/src/inspector/controller.rs:60:5` `inspector::controller::Inspector::on_core_event` — it can be `pub(crate)`
2080. **nit** — `crates/editor-inspector/src/inspector/controller.rs:90:5` `inspector::controller::Inspector::sync_summary` — it can be `pub(crate)`
2081. **nit** — `crates/editor-inspector/src/inspector/controller.rs:111:5` `inspector::controller::Inspector::retry_summary` — it can be `pub(crate)`
2082. **nit** — `crates/editor-inspector/src/inspector/controller.rs:143:5` `inspector::controller::Inspector::open_agent_target` — it can be `pub(crate)`
2083. **nit** — `crates/editor-inspector/src/inspector/controller.rs:169:5` `inspector::controller::Inspector::open_source` — it can be `pub(crate)`
2084. **nit** — `crates/editor-inspector/src/inspector/controller.rs:182:5` `inspector::controller::Inspector::open_plan` — it can be `pub(crate)`
2085. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:16:5` `inspector::dialog::TerminalDialog::loading` — it can be `pub(crate)`
2086. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:17:5` `inspector::dialog::TerminalDialog::following` — it can be `pub(crate)`
2087. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:24:5` `inspector::dialog::Job::host` — it can be `pub(crate)`
2088. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:25:5` `inspector::dialog::Job::instance` — it can be `pub(crate)`
2089. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:26:5` `inspector::dialog::Job::thread` — it can be `pub(crate)`
2090. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:27:5` `inspector::dialog::Job::item` — it can be `pub(crate)`
2091. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:28:5` `inspector::dialog::Job::data` — it can be `pub(crate)`
2092. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:29:5` `inspector::dialog::Job::command` — it can be `pub(crate)`
2093. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:30:5` `inspector::dialog::Job::commands` — it can be `pub(crate)`
2094. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:31:5` `inspector::dialog::Job::cwd` — it can be `pub(crate)`
2095. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:32:5` `inspector::dialog::Job::status` — it can be `pub(crate)`
2096. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:33:5` `inspector::dialog::Job::screen` — it can be `pub(crate)`
2097. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:34:5` `inspector::dialog::Job::unverified` — it can be `pub(crate)`
2098. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:35:5` `inspector::dialog::Job::command_source` — it can be `pub(crate)`
2099. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:36:5` `inspector::dialog::Job::output_source` — it can be `pub(crate)`
2100. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:42:5` `inspector::dialog::Job::missing` — it can be `pub(crate)`
2101. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:46:5` `inspector::dialog::Job::copy_output` — it can be `pub(crate)`
2102. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:99:5` `inspector::dialog::TerminalDialog::generation` — it can be `pub(crate)`
2103. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:103:5` `inspector::dialog::TerminalDialog::close` — it can be `pub(crate)`
2104. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:111:5` `inspector::dialog::TerminalDialog::follow` — it can be `pub(crate)`
2105. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:126:5` `inspector::dialog::TerminalDialog::fetch` — it can be `pub(crate)`
2106. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:174:5` `inspector::dialog::TerminalDialog::on_core_event` — it can be `pub(crate)`
2107. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:219:5` `inspector::dialog::TerminalDialog::sync` — it can be `pub(crate)`
2108. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:334:5` `inspector::dialog::Layout::of` — it can be `pub(crate)`
2109. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:344:5` `inspector::dialog::Layout::total` — it can be `pub(crate)`
2110. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:348:5` `inspector::dialog::Layout::output` — it can be `pub(crate)`
2111. **nit** — `crates/editor-inspector/src/inspector/dialog.rs:352:5` `inspector::dialog::Layout::row` — it can be `pub(crate)`
2112. **nit** — `crates/editor-inspector/src/inspector/owner.rs:23:5` `inspector::owner::InspectorOwner::terminal` — it can be `pub(crate)`
2113. **nit** — `crates/editor-inspector/src/inspector/owner.rs:24:5` `inspector::owner::InspectorOwner::source` — it can be `pub(crate)`
2114. **nit** — `crates/editor-inspector/src/inspector/owner.rs:25:5` `inspector::owner::InspectorOwner::command` — it can be `pub(crate)`
2115. **nit** — `crates/editor-inspector/src/inspector/owner.rs:26:5` `inspector::owner::InspectorOwner::output` — it can be `pub(crate)`
2116. **nit** — `crates/editor-inspector/src/inspector/owner.rs:27:5` `inspector::owner::InspectorOwner::detail` — it can be `pub(crate)`
2117. **nit** — `crates/editor-inspector/src/inspector/owner.rs:78:5` `inspector::owner::InspectorOwner::dispatch` — it can be `pub(crate)`
2118. **nit** — `crates/editor-inspector/src/inspector/palette.rs:67:5` `inspector::palette::TerminalPalette::bold` — it can be `pub(crate)`
2119. **nit** — `crates/editor-inspector/src/inspector/palette.rs:73:5` `inspector::palette::TerminalPalette::for_theme` — it can be `pub(crate)`
2120. **nit** — `crates/editor-inspector/src/inspector/palette.rs:80:5` `inspector::palette::TerminalPalette::resolve` — it can be `pub(crate)`
2121. **nit** — `crates/editor-inspector/src/inspector/terminal.rs:28:5` `inspector::terminal::StreamState::fed` — it can be `pub(crate)`

**forge-computer** (38)

2122. **nit** — `crates/forge-computer/src/desktop.rs:18:5` `desktop::Rect::x` — it can be `pub(crate)`
2123. **nit** — `crates/forge-computer/src/desktop.rs:19:5` `desktop::Rect::y` — it can be `pub(crate)`
2124. **nit** — `crates/forge-computer/src/desktop.rs:20:5` `desktop::Rect::width` — it can be `pub(crate)`
2125. **nit** — `crates/forge-computer/src/desktop.rs:21:5` `desktop::Rect::height` — it can be `pub(crate)`
2126. **nit** — `crates/forge-computer/src/desktop.rs:26:5` `desktop::Rect::contains` — it can be `pub(crate)`
2127. **nit** — `crates/forge-computer/src/desktop.rs:34:5` `desktop::Rect::center` — it can be `pub(crate)`
2128. **nit** — `crates/forge-computer/src/desktop.rs:49:5` `desktop::Point::x` — it can be `pub(crate)`
2129. **nit** — `crates/forge-computer/src/desktop.rs:50:5` `desktop::Point::y` — it can be `pub(crate)`
2130. **nit** — `crates/forge-computer/src/desktop.rs:84:5` `desktop::WindowInfo::id` — it can be `pub(crate)`
2131. **nit** — `crates/forge-computer/src/desktop.rs:85:5` `desktop::WindowInfo::title` — it can be `pub(crate)`
2132. **nit** — `crates/forge-computer/src/desktop.rs:87:5` `desktop::WindowInfo::process` — it can be `pub(crate)`
2133. **nit** — `crates/forge-computer/src/desktop.rs:88:5` `desktop::WindowInfo::pid` — it can be `pub(crate)`
2134. **nit** — `crates/forge-computer/src/desktop.rs:89:5` `desktop::WindowInfo::rect` — it can be `pub(crate)`
2135. **nit** — `crates/forge-computer/src/desktop.rs:90:5` `desktop::WindowInfo::minimized` — it can be `pub(crate)`
2136. **nit** — `crates/forge-computer/src/frame.rs:16:5` `frame::Frame::width` — it can be `pub(crate)`
2137. **nit** — `crates/forge-computer/src/frame.rs:17:5` `frame::Frame::height` — it can be `pub(crate)`
2138. **nit** — `crates/forge-computer/src/frame.rs:19:5` `frame::Frame::rgb` — it can be `pub(crate)`
2139. **nit** — `crates/forge-computer/src/frame.rs:26:5` `frame::Frame::crop` — it can be `pub(crate)`
2140. **nit** — `crates/forge-computer/src/frame.rs:43:5` `frame::Frame::shrink` — it can be `pub(crate)`
2141. **nit** — `crates/forge-computer/src/frame.rs:81:5` `frame::Frame::png_base64` — it can be `pub(crate)`
2142. **nit** — `crates/forge-computer/src/frame.rs:100:1` `frame::scale_for` — it can be `pub(crate)`
2143. **nit** — `crates/forge-computer/src/frame.rs:109:1` `frame::scaled` — it can be `pub(crate)`
2144. **nit** — `crates/forge-computer/src/lib.rs:59:5` `ComputerMcp::with_desktop` — it can be `pub(crate)`
2145. **nit** — `crates/forge-computer/src/runner.rs:13:5` `runner::View::rect` — it can be `pub(crate)`
2146. **nit** — `crates/forge-computer/src/runner.rs:14:5` `runner::View::width` — it can be `pub(crate)`
2147. **nit** — `crates/forge-computer/src/runner.rs:15:5` `runner::View::height` — it can be `pub(crate)`
2148. **nit** — `crates/forge-computer/src/runner.rs:18:5` `runner::View::new` — it can be `pub(crate)`
2149. **nit** — `crates/forge-computer/src/runner.rs:27:5` `runner::View::point` — it can be `pub(crate)`
2150. **nit** — `crates/forge-computer/src/runner.rs:40:1` `runner::Outcome` — it can be `pub(crate)`
2151. **nit** — `crates/forge-computer/src/runner.rs:41:5` `runner::Outcome::completed` — it can be `pub(crate)`
2152. **nit** — `crates/forge-computer/src/runner.rs:42:5` `runner::Outcome::interrupted` — it can be `pub(crate)`
2153. **nit** — `crates/forge-computer/src/runner.rs:43:5` `runner::Outcome::screen_changed` — it can be `pub(crate)`
2154. **nit** — `crates/forge-computer/src/runner.rs:44:5` `runner::Outcome::settled` — it can be `pub(crate)`
2155. **nit** — `crates/forge-computer/src/runner.rs:244:1` `runner::run` — it can be `pub(crate)`
2156. **nit** — `crates/forge-computer/src/steps.rs:16:5` `steps::At::x` — it can be `pub(crate)`
2157. **nit** — `crates/forge-computer/src/steps.rs:17:5` `steps::At::y` — it can be `pub(crate)`
2158. **nit** — `crates/forge-computer/src/steps.rs:108:1` `steps::parse` — it can be `pub(crate)`
2159. **nit** — `crates/forge-computer/src/steps.rs:285:1` `steps::key` — it can be `pub(crate)`

**artisan-account** (33)

2160. **nit** — `crates/artisan-account/src/browser.rs:27:5` `browser::Provider::slug` — it can be `pub(crate)`
2161. **nit** — `crates/artisan-account/src/browser.rs:46:5` `browser::Problem::of` — it can be `pub(crate)`
2162. **nit** — `crates/artisan-account/src/browser.rs:57:5` `browser::Problem::slug` — it can be `pub(crate)`
2163. **nit** — `crates/artisan-account/src/frames.rs:33:5` `frames::OpenFrame::v` — it can be `pub(crate)`
2164. **nit** — `crates/artisan-account/src/frames.rs:34:5` `frames::OpenFrame::channel` — it can be `pub(crate)`
2165. **nit** — `crates/artisan-account/src/frames.rs:109:5` `frames::ChannelClosedFrame::v` — it can be `pub(crate)`
2166. **nit** — `crates/artisan-account/src/frames.rs:111:5` `frames::ChannelClosedFrame::reason` — it can be `pub(crate)`
2167. **nit** — `crates/artisan-account/src/frames.rs:116:5` `frames::PresenceFrame::v` — it can be `pub(crate)`
2168. **nit** — `crates/artisan-account/src/frames.rs:117:5` `frames::PresenceFrame::forge_online` — it can be `pub(crate)`
2169. **nit** — `crates/artisan-account/src/frames.rs:118:5` `frames::PresenceFrame::clients` — it can be `pub(crate)`
2170. **nit** — `crates/artisan-account/src/frames.rs:123:5` `frames::ErrorFrame::v` — it can be `pub(crate)`
2171. **nit** — `crates/artisan-account/src/grant.rs:96:5` `grant::Aud::values` — it can be `pub(crate)`
2172. **nit** — `crates/artisan-account/src/grant.rs:104:5` `grant::Aud::contains` — it can be `pub(crate)`
2173. **nit** — `crates/artisan-account/src/machine_proof.rs:22:1` `machine_proof::EMPTY_BODY_HASH` — it can be `pub(crate)`
2174. **nit** — `crates/artisan-account/src/network.rs:139:5` `network::SessionEntry::id` — it can be `pub(crate)`
2175. **nit** — `crates/artisan-account/src/network.rs:141:5` `network::SessionEntry::device_name` — it can be `pub(crate)`
2176. **nit** — `crates/artisan-account/src/network.rs:143:5` `network::SessionEntry::created_at` — it can be `pub(crate)`
2177. **nit** — `crates/artisan-account/src/network.rs:145:5` `network::SessionEntry::expires_at` — it can be `pub(crate)`
2178. **nit** — `crates/artisan-account/src/network.rs:147:5` `network::SessionEntry::revoked_at` — it can be `pub(crate)`
2179. **nit** — `crates/artisan-account/src/network.rs:160:5` `network::ClaimRequest::code` — it can be `pub(crate)`
2180. **nit** — `crates/artisan-account/src/network.rs:172:5` `network::GrantIssueRequest::audience` — it can be `pub(crate)`
2181. **nit** — `crates/artisan-account/src/network.rs:173:5` `network::GrantIssueRequest::scope` — it can be `pub(crate)`
2182. **nit** — `crates/artisan-account/src/network.rs:179:5` `network::GrantIssueRequest::ttl_seconds` — it can be `pub(crate)`
2183. **nit** — `crates/artisan-account/src/network.rs:185:5` `network::GrantIssueRequest::target_machine_id` — it can be `pub(crate)`
2184. **nit** — `crates/artisan-account/src/network.rs:187:5` `network::GrantIssueRequest::cnf` — it can be `pub(crate)`
2185. **nit** — `crates/artisan-account/src/network.rs:193:5` `network::GrantToken::expires_at` — it can be `pub(crate)`
2186. **nit** — `crates/artisan-account/src/network.rs:200:5` `network::BundleHead::version` — it can be `pub(crate)`
2187. **nit** — `crates/artisan-account/src/network.rs:201:5` `network::BundleHead::content_hash` — it can be `pub(crate)`
2188. **nit** — `crates/artisan-account/src/network.rs:610:5` `network::TotpEnrollResponse::otpauth_url` — it can be `pub(crate)`
2189. **nit** — `crates/artisan-account/src/network.rs:611:5` `network::TotpEnrollResponse::recovery_codes` — it can be `pub(crate)`
2190. **nit** — `crates/artisan-account/src/network.rs:618:5` `network::TotpVerifyRequest::code` — it can be `pub(crate)`
2191. **nit** — `crates/artisan-account/src/network.rs:625:5` `network::TotpDisableRequest::code` — it can be `pub(crate)`
2192. **nit** — `crates/artisan-account/src/network.rs:631:5` `network::ClaimedEnrollment::enrollment_id` — it can be `pub(crate)`

**artisan-api** (27)

2193. **nit** — `crates/artisan-api/src/item.rs:281:5` `item::MessageDirection::is_sent` — it can be `pub(crate)`
2194. **nit** — `crates/artisan-api/src/model.rs:210:5` `model::ModelIdentity::harness` — it can be `pub(crate)`
2195. **nit** — `crates/artisan-api/src/model.rs:211:5` `model::ModelIdentity::native_id` — it can be `pub(crate)`
2196. **nit** — `crates/artisan-api/src/model.rs:212:5` `model::ModelIdentity::gateway` — it can be `pub(crate)`
2197. **nit** — `crates/artisan-api/src/model.rs:295:5` `model::ModelChoice::origin` — it can be `pub(crate)`
2198. **nit** — `crates/artisan-api/src/model.rs:297:5` `model::ModelChoice::visibility_override` — it can be `pub(crate)`
2199. **nit** — `crates/artisan-api/src/model.rs:343:5` `model::ModelSpecification::identity` — it can be `pub(crate)`
2200. **nit** — `crates/artisan-api/src/model.rs:347:5` `model::ModelSpecification::evidence` — it can be `pub(crate)`
2201. **nit** — `crates/artisan-api/src/model.rs:384:5` `model::ModelCustomizationSet::version` — it can be `pub(crate)`
2202. **nit** — `crates/artisan-api/src/model.rs:385:5` `model::ModelCustomizationSet::rules` — it can be `pub(crate)`
2203. **nit** — `crates/artisan-api/src/request.rs:349:5` `request::InspectorTerminal::status` — it can be `pub(crate)`
2204. **nit** — `crates/artisan-api/src/request.rs:389:5` `request::InspectorAgent::background` — it can be `pub(crate)`
2205. **nit** — `crates/artisan-api/src/settings.rs:37:5` `settings::SettingDecl::default` — it can be `pub(crate)`
2206. **nit** — `crates/artisan-api/src/transport.rs:79:1` `transport::CLAUDE_AI_SESSION_COOKIES` — it can be `pub(crate)`
2207. **nit** — `crates/artisan-api/src/tunnel.rs:16:5` `tunnel::TunnelBytes::content_type` — it can be `pub(crate)`
2208. **nit** — `crates/artisan-api/src/tunnel.rs:90:5` `tunnel::TunnelChunk::transfer` — it can be `pub(crate)`
2209. **nit** — `crates/artisan-api/src/tunnel.rs:91:5` `tunnel::TunnelChunk::index` — it can be `pub(crate)`
2210. **nit** — `crates/artisan-api/src/tunnel.rs:92:5` `tunnel::TunnelChunk::total` — it can be `pub(crate)`
2211. **nit** — `crates/artisan-api/src/tunnel.rs:93:5` `tunnel::TunnelChunk::total_bytes` — it can be `pub(crate)`
2212. **nit** — `crates/artisan-api/src/tunnel.rs:94:5` `tunnel::TunnelChunk::bytes_b64u` — it can be `pub(crate)`
2213. **nit** — `crates/artisan-api/src/tunnel.rs:97:1` `tunnel::CHUNK_RAW_BYTES` — it can be `pub(crate)`
2214. **nit** — `crates/artisan-api/src/tunnel.rs:99:1` `tunnel::TRANSFER_MAX_BYTES` — it can be `pub(crate)`
2215. **nit** — `crates/artisan-api/src/tunnel.rs:100:1` `tunnel::REASSEMBLY_MAX_BYTES_PER_CONN` — it can be `pub(crate)`
2216. **nit** — `crates/artisan-api/src/tunnel.rs:101:1` `tunnel::TRANSFER_MAX_PARTS` — it can be `pub(crate)`
2217. **nit** — `crates/artisan-api/src/tunnel.rs:106:1` `tunnel::REASSEMBLY_MAX_IN_FLIGHT` — it can be `pub(crate)`
2218. **nit** — `crates/artisan-api/src/tunnel.rs:133:1` `tunnel::decode_chunk_bytes` — it can be `pub(crate)`
2219. **nit** — `crates/artisan-api/src/tunnel.rs:214:5` `tunnel::StreamEvent::stream` — it can be `pub(crate)`

**forge-http** (22)

2220. **nit** — `crates/forge-http/src/account_auth.rs:158:5` `account_auth::tests::verifier_with_stub` — it can be `pub(crate)`
2221. **nit** — `crates/forge-http/src/account_auth.rs:184:5` `account_auth::tests::grant_token` — it can be `pub(crate)`
2222. **nit** — `crates/forge-http/src/core.rs:19:1` `core::DEFAULT_CORE_BASE_URL` — it can be `pub(crate)`
2223. **nit** — `crates/forge-http/src/core.rs:209:5` `core::CoreClient::issue_machine_grant` — it can be `pub(crate)`
2224. **nit** — `crates/forge-http/src/core.rs:268:5` `core::GrantToken::token` — it can be `pub(crate)`
2225. **nit** — `crates/forge-http/src/core.rs:269:5` `core::GrantToken::expires_at` — it can be `pub(crate)`
2226. **nit** — `crates/forge-http/src/core.rs:276:5` `core::Jwk::kty` — it can be `pub(crate)`
2227. **nit** — `crates/forge-http/src/core.rs:277:5` `core::Jwk::crv` — it can be `pub(crate)`
2228. **nit** — `crates/forge-http/src/core.rs:278:5` `core::Jwk::x` — it can be `pub(crate)`
2229. **nit** — `crates/forge-http/src/core.rs:279:5` `core::Jwk::kid` — it can be `pub(crate)`
2230. **nit** — `crates/forge-http/src/core.rs:281:5` `core::Jwk::r#use` — it can be `pub(crate)`
2231. **nit** — `crates/forge-http/src/core.rs:283:5` `core::Jwk::alg` — it can be `pub(crate)`
2232. **nit** — `crates/forge-http/src/core.rs:290:5` `core::Jwks::keys` — it can be `pub(crate)`
2233. **nit** — `crates/forge-http/src/lib.rs:109:1` `start_with_harness` — it can be `pub(crate)`
2234. **nit** — `crates/forge-http/src/lib.rs:123:1` `start_generation_with_harness` — it can be `pub(crate)`
2235. **nit** — `crates/forge-http/src/sync.rs:11:1` `sync::CONFIG_BUNDLES_LATEST_PATH` — it can be `pub(crate)`
2236. **nit** — `crates/forge-http/src/sync.rs:12:1` `sync::CONFIG_STATE_PATH` — it can be `pub(crate)`
2237. **nit** — `crates/forge-http/src/sync.rs:22:5` `sync::ConfigState::bundle_id` — it can be `pub(crate)`
2238. **nit** — `crates/forge-http/src/sync.rs:24:5` `sync::ConfigState::content_hash` — it can be `pub(crate)`
2239. **nit** — `crates/forge-http/src/sync.rs:25:5` `sync::ConfigState::etag` — it can be `pub(crate)`
2240. **nit** — `crates/forge-http/src/sync.rs:29:5` `sync::ConfigState::mcp_servers` — it can be `pub(crate)`
2241. **nit** — `crates/forge-http/src/sync.rs:38:1` `sync::sync_state_path` — it can be `pub(crate)`

**editor-assets** (19)

2242. **nit** — `crates/editor-assets/src/file_icons.rs:570:1` `file_icons::folder` — it can be `pub(crate)`
2243. **nit** — `crates/editor-assets/src/fonts.rs:19:5` `fonts::BundledFont::file_name` — it can be `pub(crate)`
2244. **nit** — `crates/editor-assets/src/fonts.rs:21:5` `fonts::BundledFont::family` — it can be `pub(crate)`
2245. **nit** — `crates/editor-assets/src/fonts.rs:23:5` `fonts::BundledFont::weights` — it can be `pub(crate)`
2246. **nit** — `crates/editor-assets/src/fonts.rs:25:5` `fonts::BundledFont::license_path` — it can be `pub(crate)`
2247. **nit** — `crates/editor-assets/src/fonts.rs:27:5` `fonts::BundledFont::license_text` — it can be `pub(crate)`
2248. **nit** — `crates/editor-assets/src/fonts.rs:29:5` `fonts::BundledFont::bytes` — it can be `pub(crate)`
2249. **nit** — `crates/editor-assets/src/fonts.rs:41:5` `fonts::UnknownFont::id` — it can be `pub(crate)`
2250. **nit** — `crates/editor-assets/src/lib.rs:23:5` `AssetId::as_str` — it can be `pub(crate)`
2251. **nit** — `crates/editor-assets/src/lib.rs:118:5` `Family::prefix` — it can be `pub(crate)`
2252. **nit** — `crates/editor-assets/src/lib.rs:155:5` `Asset::id` — it can be `pub(crate)`
2253. **nit** — `crates/editor-assets/src/lib.rs:156:5` `Asset::family` — it can be `pub(crate)`
2254. **nit** — `crates/editor-assets/src/lib.rs:157:5` `Asset::view_box` — it can be `pub(crate)`
2255. **nit** — `crates/editor-assets/src/lib.rs:158:5` `Asset::monochrome` — it can be `pub(crate)`
2256. **nit** — `crates/editor-assets/src/lib.rs:160:5` `Asset::source_path` — it can be `pub(crate)`
2257. **nit** — `crates/editor-assets/src/lib.rs:586:5` `LicenseFile::path` — it can be `pub(crate)`
2258. **nit** — `crates/editor-assets/src/lib.rs:587:5` `LicenseFile::contents` — it can be `pub(crate)`
2259. **nit** — `crates/editor-assets/src/lib.rs:592:5` `UnknownAsset::id` — it can be `pub(crate)`
2260. **nit** — `crates/editor-assets/src/lib.rs:651:1` `lookup` — it can be `pub(crate)`

**forge-runs** (18)

2261. **nit** — `crates/forge-runs/src/agents.rs:62:5` `agents::AgentState::is_settled` — it can be `pub(crate)`
2262. **nit** — `crates/forge-runs/src/gateway.rs:123:5` `gateway::<impl Hub>::answer_editor_call` — it can be `pub(crate)`
2263. **nit** — `crates/forge-runs/src/lib.rs:240:5` `Hub::data_dir` — it can be `pub(crate)`
2264. **nit** — `crates/forge-runs/src/lib.rs:624:5` `Hub::cancel_local` — it can be `pub(crate)`
2265. **nit** — `crates/forge-runs/src/lib.rs:727:5` `Hub::live_snapshot` — it can be `pub(crate)`
2266. **nit** — `crates/forge-runs/src/mcp_reach.rs:34:5` `mcp_reach::<impl Hub>::probe_mcp_servers` — it can be `pub(crate)`
2267. **nit** — `crates/forge-runs/src/resources.rs:9:1` `resources::MAX_RESOURCE_BYTES` — it can be `pub(crate)`
2268. **nit** — `crates/forge-runs/src/resources.rs:43:5` `resources::Written::parse` — it can be `pub(crate)`
2269. **nit** — `crates/forge-runs/src/resources.rs:106:1` `resources::written_sources` — it can be `pub(crate)`
2270. **nit** — `crates/forge-runs/src/resources.rs:144:5` `resources::Host::home` — it can be `pub(crate)`
2271. **nit** — `crates/forge-runs/src/resources.rs:145:5` `resources::Host::windows_drives` — it can be `pub(crate)`
2272. **nit** — `crates/forge-runs/src/resources.rs:149:1` `resources::automount_root` — it can be `pub(crate)`
2273. **nit** — `crates/forge-runs/src/resources.rs:251:1` `resources::host_path` — it can be `pub(crate)`
2274. **nit** — `crates/forge-runs/src/runtime/registry.rs:146:5` `runtime::registry::Registry::owner` — it can be `pub(crate)`
2275. **nit** — `crates/forge-runs/src/runtime/registry.rs:158:5` `runtime::registry::Registry::owners` — it can be `pub(crate)`
2276. **nit** — `crates/forge-runs/src/runtime/registry.rs:234:5` `runtime::registry::Generation::claim` — it can be `pub(crate)`
2277. **nit** — `crates/forge-runs/src/runtime/registry.rs:271:5` `runtime::registry::Generation::slot` — it can be `pub(crate)`
2278. **nit** — `crates/forge-runs/src/scratch.rs:47:5` `scratch::<impl Hub>::thread_folder` — it can be `pub(crate)`

**forge-store** (13)

2279. **nit** — `crates/forge-store/src/coordination.rs:245:5` `coordination::Content::kind` — it can be `pub(crate)`
2280. **nit** — `crates/forge-store/src/coordination.rs:259:5` `coordination::Record::project` — it can be `pub(crate)`
2281. **nit** — `crates/forge-store/src/coordination.rs:298:5` `coordination::Change::owner` — it can be `pub(crate)`
2282. **nit** — `crates/forge-store/src/coordination.rs:299:5` `coordination::Change::project` — it can be `pub(crate)`
2283. **nit** — `crates/forge-store/src/coordination.rs:300:5` `coordination::Change::kind` — it can be `pub(crate)`
2284. **nit** — `crates/forge-store/src/coordination.rs:301:5` `coordination::Change::revision` — it can be `pub(crate)`
2285. **nit** — `crates/forge-store/src/coordination.rs:302:5` `coordination::Change::created_at` — it can be `pub(crate)`
2286. **nit** — `crates/forge-store/src/discovery.rs:9:5` `discovery::FileCursor::row_id` — it can be `pub(crate)`
2287. **nit** — `crates/forge-store/src/discovery.rs:10:5` `discovery::FileCursor::line` — it can be `pub(crate)`
2288. **nit** — `crates/forge-store/src/lib.rs:87:1` `park_setting` — it can be `pub(crate)`
2289. **nit** — `crates/forge-store/src/pairing.rs:16:5` `pairing::Device::public_key` — it can be `pub(crate)`
2290. **nit** — `crates/forge-store/src/recall.rs:430:5` `recall::GrepMatch::field` — it can be `pub(crate)`
2291. **nit** — `crates/forge-store/src/writer.rs:95:1` `writer::Stamp` — it can be `pub(crate)`

**editor-sources** (9)

2292. **nit** — `crates/editor-sources/src/sources/controller.rs:76:5` `sources::controller::ContentText::rows` — it can be `pub(crate)`
2293. **nit** — `crates/editor-sources/src/sources/controller.rs:77:5` `sources::controller::ContentText::page` — it can be `pub(crate)`
2294. **nit** — `crates/editor-sources/src/sources/controller.rs:145:5` `sources::controller::ContentText::retry` — it can be `pub(crate)`
2295. **nit** — `crates/editor-sources/src/sources/controller.rs:157:5` `sources::controller::ContentText::reconcile` — it can be `pub(crate)`
2296. **nit** — `crates/editor-sources/src/sources/controller.rs:191:5` `sources::controller::ContentText::sync` — it can be `pub(crate)`
2297. **nit** — `crates/editor-sources/src/sources/controller.rs:204:5` `sources::controller::ContentText::previous` — it can be `pub(crate)`
2298. **nit** — `crates/editor-sources/src/sources/controller.rs:216:5` `sources::controller::ContentText::next` — it can be `pub(crate)`
2299. **nit** — `crates/editor-sources/src/sources/controller.rs:271:5` `sources::controller::ContentText::export` — it can be `pub(crate)`
2300. **nit** — `crates/editor-sources/src/sources/controller.rs:377:5` `sources::controller::CollectionBrowser::<T>::reconcile` — it can be `pub(crate)`

**artisan-mentions** (4)

2301. **nit** — `crates/artisan-mentions/src/lib.rs:40:1` `named_mentions` — it can be `pub(crate)`
2302. **nit** — `crates/artisan-mentions/src/lib.rs:110:1` `MAX_THREAD_TITLE_BYTES` — it can be `pub(crate)`
2303. **nit** — `crates/artisan-mentions/src/lib.rs:164:1` `leading_command` — it can be `pub(crate)`
2304. **nit** — `crates/artisan-mentions/src/lib.rs:176:1` `SPECIAL_COMMANDS` — it can be `pub(crate)`

**claude-ai-browser** (4)

2305. **nit** — `crates/claude-ai-browser/src/lib.rs:27:1` `SIGN_IN_URL` — it can be `pub(crate)`
2306. **nit** — `crates/claude-ai-browser/src/lib.rs:57:1` `profile_dir` — it can be `pub(crate)`
2307. **nit** — `crates/claude-ai-browser/src/lib.rs:96:1` `browser_candidates` — it can be `pub(crate)`
2308. **nit** — `crates/claude-ai-browser/src/lib.rs:208:1` `claude_ai_cookies` — it can be `pub(crate)`

**forge-mcp** (4)

2309. **nit** — `crates/forge-mcp/src/server.rs:28:5` `server::ToolResult::content` — it can be `pub(crate)`
2310. **nit** — `crates/forge-mcp/src/server.rs:29:5` `server::ToolResult::structured_content` — it can be `pub(crate)`
2311. **nit** — `crates/forge-mcp/src/server.rs:33:5` `server::ToolResult::structured` — it can be `pub(crate)`
2312. **nit** — `crates/forge-mcp/src/server.rs:197:1` `server::list` — it can be `pub(crate)`

**artisan-paths** (1)

2313. **nit** — `crates/artisan-paths/src/lib.rs:102:1` `default_root` — it can be `pub(crate)`

**editor-audio** (1)

2314. **nit** — `crates/editor-audio/src/mix.rs:18:1` `mix::render` — it can be `pub(crate)`

**forge-content** (1)

2315. **nit** — `crates/forge-content/src/lib.rs:64:1` `compact_collection` — it can be `pub(crate)`

</details>

### 14.3 Restricted items that can be private

<details><summary>201 findings</summary>


**editor-ui** (175)

2316. **nit** — `crates/editor-ui/src/ui/chrome/card.rs:144:1` `ui::chrome::card::vertical` — it can be private
2317. **nit** — `crates/editor-ui/src/ui/markdown/inline.rs:28:5` `ui::markdown::inline::FlatInline::swatches` — it can be private
2318. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:206:5` `ui::markdown::MarkdownDocument::blocks` — it can be private
2319. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:209:1` `ui::markdown::mark_command_spans` — it can be private
2320. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:224:5` `ui::markdown::MarkdownDocument::resident_bytes` — it can be private
2321. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:341:5` `ui::markdown::MarkdownEngine::parse_document_with_fence_memo` — it can be private
2322. **nit** — `crates/editor-ui/src/ui/markdown/mod.rs:501:1` `ui::markdown::FenceTokenMemo` — it can be private
2323. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:31:5` `ui::rich::charts::chart::SeriesSpec::name` — it can be private
2324. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:33:5` `ui::rich::charts::chart::SeriesSpec::values` — it can be private
2325. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:35:5` `ui::rich::charts::chart::SeriesSpec::points` — it can be private
2326. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:41:5` `ui::rich::charts::chart::Axis::label` — it can be private
2327. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:43:5` `ui::rich::charts::chart::Axis::unit` — it can be private
2328. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:45:5` `ui::rich::charts::chart::Axis::min` — it can be private
2329. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:47:5` `ui::rich::charts::chart::Axis::max` — it can be private
2330. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:53:5` `ui::rich::charts::chart::Annotation::x` — it can be private
2331. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:55:5` `ui::rich::charts::chart::Annotation::y` — it can be private
2332. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:57:5` `ui::rich::charts::chart::Annotation::x_end` — it can be private
2333. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:59:5` `ui::rich::charts::chart::Annotation::label` — it can be private
2334. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:74:5` `ui::rich::charts::chart::Param::id` — it can be private
2335. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:76:5` `ui::rich::charts::chart::Param::kind` — it can be private
2336. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:78:5` `ui::rich::charts::chart::Param::label` — it can be private
2337. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:79:5` `ui::rich::charts::chart::Param::min` — it can be private
2338. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:80:5` `ui::rich::charts::chart::Param::max` — it can be private
2339. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:82:5` `ui::rich::charts::chart::Param::step` — it can be private
2340. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:83:5` `ui::rich::charts::chart::Param::value` — it can be private
2341. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:89:5` `ui::rich::charts::chart::ChartSpec::mark` — it can be private
2342. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:91:5` `ui::rich::charts::chart::ChartSpec::stacked` — it can be private
2343. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:93:5` `ui::rich::charts::chart::ChartSpec::normalize` — it can be private
2344. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:95:5` `ui::rich::charts::chart::ChartSpec::categories` — it can be private
2345. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:96:5` `ui::rich::charts::chart::ChartSpec::series` — it can be private
2346. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:98:5` `ui::rich::charts::chart::ChartSpec::x` — it can be private
2347. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:100:5` `ui::rich::charts::chart::ChartSpec::y` — it can be private
2348. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:102:5` `ui::rich::charts::chart::ChartSpec::annotations` — it can be private
2349. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:104:5` `ui::rich::charts::chart::ChartSpec::params` — it can be private
2350. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:109:5` `ui::rich::charts::chart::ChartStyle::mark` — it can be private
2351. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:110:5` `ui::rich::charts::chart::ChartStyle::stacked` — it can be private
2352. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:111:5` `ui::rich::charts::chart::ChartStyle::normalize` — it can be private
2353. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:112:5` `ui::rich::charts::chart::ChartStyle::x` — it can be private
2354. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:113:5` `ui::rich::charts::chart::ChartStyle::y` — it can be private
2355. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:114:5` `ui::rich::charts::chart::ChartStyle::annotations` — it can be private
2356. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:115:5` `ui::rich::charts::chart::ChartStyle::params` — it can be private
2357. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:163:5` `ui::rich::charts::chart::ChartStyle::stacks` — it can be private
2358. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/mod.rs:167:5` `ui::rich::charts::chart::ChartStyle::thresholds` — it can be private
2359. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/model.rs:237:5` `ui::rich::charts::chart::model::Model::aligned` — it can be private
2360. **nit** — `crates/editor-ui/src/ui/rich/charts/chart/plot.rs:611:1` `ui::rich::charts::chart::plot::runs` — it can be private
2361. **nit** — `crates/editor-ui/src/ui/rich/charts/concentration.rs:93:5` `ui::rich::charts::concentration::Concentration::curve` — it can be private
2362. **nit** — `crates/editor-ui/src/ui/rich/charts/concentration.rs:97:5` `ui::rich::charts::concentration::Concentration::share_of_total` — it can be private
2363. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:45:5` `ui::rich::charts::distribution::Sample::name` — it can be private
2364. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:46:5` `ui::rich::charts::distribution::Sample::sorted` — it can be private
2365. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:47:5` `ui::rich::charts::distribution::Sample::summary` — it can be private
2366. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:48:5` `ui::rich::charts::distribution::Sample::counts` — it can be private
2367. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:53:5` `ui::rich::charts::distribution::Distribution::mode` — it can be private
2368. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:54:5` `ui::rich::charts::distribution::Distribution::unit` — it can be private
2369. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:55:5` `ui::rich::charts::distribution::Distribution::threshold` — it can be private
2370. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:56:5` `ui::rich::charts::distribution::Distribution::samples` — it can be private
2371. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:57:5` `ui::rich::charts::distribution::Distribution::bins` — it can be private
2372. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:92:5` `ui::rich::charts::distribution::Distribution::from_spec` — it can be private
2373. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:132:5` `ui::rich::charts::distribution::Distribution::is_empty` — it can be private
2374. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:136:5` `ui::rich::charts::distribution::Distribution::domain` — it can be private
2375. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:140:5` `ui::rich::charts::distribution::Distribution::slider` — it can be private
2376. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:147:5` `ui::rich::charts::distribution::Bins::domain` — it can be private
2377. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:148:5` `ui::rich::charts::distribution::Bins::count` — it can be private
2378. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:152:5` `ui::rich::charts::distribution::Bins::new` — it can be private
2379. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:161:5` `ui::rich::charts::distribution::Bins::width` — it can be private
2380. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:165:5` `ui::rich::charts::distribution::Bins::index` — it can be private
2381. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:169:5` `ui::rich::charts::distribution::Bins::edges` — it can be private
2382. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:174:5` `ui::rich::charts::distribution::Bins::counts` — it can be private
2383. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:202:5` `ui::rich::charts::distribution::Summary::count` — it can be private
2384. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:203:5` `ui::rich::charts::distribution::Summary::min` — it can be private
2385. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:204:5` `ui::rich::charts::distribution::Summary::q1` — it can be private
2386. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:205:5` `ui::rich::charts::distribution::Summary::median` — it can be private
2387. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:206:5` `ui::rich::charts::distribution::Summary::q3` — it can be private
2388. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:207:5` `ui::rich::charts::distribution::Summary::max` — it can be private
2389. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:208:5` `ui::rich::charts::distribution::Summary::mean` — it can be private
2390. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:209:5` `ui::rich::charts::distribution::Summary::whiskers` — it can be private
2391. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:210:5` `ui::rich::charts::distribution::Summary::outliers` — it can be private
2392. **nit** — `crates/editor-ui/src/ui/rich/charts/distribution.rs:213:1` `ui::rich::charts::distribution::summary` — it can be private
2393. **nit** — `crates/editor-ui/src/ui/rich/charts/funnel.rs:50:5` `ui::rich::charts::funnel::StepReading::value` — it can be private
2394. **nit** — `crates/editor-ui/src/ui/rich/charts/funnel.rs:51:5` `ui::rich::charts::funnel::StepReading::of_first` — it can be private
2395. **nit** — `crates/editor-ui/src/ui/rich/charts/funnel.rs:52:5` `ui::rich::charts::funnel::StepReading::drop` — it can be private
2396. **nit** — `crates/editor-ui/src/ui/rich/charts/funnel.rs:82:5` `ui::rich::charts::funnel::Funnel::reading` — it can be private
2397. **nit** — `crates/editor-ui/src/ui/rich/charts/heatmap.rs:105:5` `ui::rich::charts::heatmap::Heatmap::rows` — it can be private
2398. **nit** — `crates/editor-ui/src/ui/rich/charts/heatmap.rs:108:5` `ui::rich::charts::heatmap::Heatmap::columns` — it can be private
2399. **nit** — `crates/editor-ui/src/ui/rich/charts/heatmap.rs:112:5` `ui::rich::charts::heatmap::Heatmap::cell` — it can be private
2400. **nit** — `crates/editor-ui/src/ui/rich/charts/heatmap.rs:134:5` `ui::rich::charts::heatmap::Heatmap::position` — it can be private
2401. **nit** — `crates/editor-ui/src/ui/rich/charts/hierarchy.rs:11:1` `ui::rich::charts::hierarchy::GROUP_PAD` — it can be private
2402. **nit** — `crates/editor-ui/src/ui/rich/charts/hierarchy.rs:89:5` `ui::rich::charts::hierarchy::TreeNode::name` — it can be private
2403. **nit** — `crates/editor-ui/src/ui/rich/charts/hierarchy.rs:90:5` `ui::rich::charts::hierarchy::TreeNode::size` — it can be private
2404. **nit** — `crates/editor-ui/src/ui/rich/charts/hierarchy.rs:91:5` `ui::rich::charts::hierarchy::TreeNode::parent` — it can be private
2405. **nit** — `crates/editor-ui/src/ui/rich/charts/hierarchy.rs:93:5` `ui::rich::charts::hierarchy::TreeNode::children` — it can be private
2406. **nit** — `crates/editor-ui/src/ui/rich/charts/hierarchy.rs:98:5` `ui::rich::charts::hierarchy::Tree::nodes` — it can be private
2407. **nit** — `crates/editor-ui/src/ui/rich/charts/hierarchy.rs:146:5` `ui::rich::charts::hierarchy::Tree::node` — it can be private
2408. **nit** — `crates/editor-ui/src/ui/rich/charts/hierarchy.rs:150:5` `ui::rich::charts::hierarchy::Tree::path` — it can be private
2409. **nit** — `crates/editor-ui/src/ui/rich/charts/hierarchy.rs:161:5` `ui::rich::charts::hierarchy::Tree::branch` — it can be private
2410. **nit** — `crates/editor-ui/src/ui/rich/charts/hierarchy.rs:175:5` `ui::rich::charts::hierarchy::Rect::x` — it can be private
2411. **nit** — `crates/editor-ui/src/ui/rich/charts/hierarchy.rs:176:5` `ui::rich::charts::hierarchy::Rect::y` — it can be private
2412. **nit** — `crates/editor-ui/src/ui/rich/charts/hierarchy.rs:177:5` `ui::rich::charts::hierarchy::Rect::w` — it can be private
2413. **nit** — `crates/editor-ui/src/ui/rich/charts/hierarchy.rs:178:5` `ui::rich::charts::hierarchy::Rect::h` — it can be private
2414. **nit** — `crates/editor-ui/src/ui/rich/charts/hierarchy.rs:194:5` `ui::rich::charts::hierarchy::Cell::node` — it can be private
2415. **nit** — `crates/editor-ui/src/ui/rich/charts/hierarchy.rs:195:5` `ui::rich::charts::hierarchy::Cell::rect` — it can be private
2416. **nit** — `crates/editor-ui/src/ui/rich/charts/hierarchy.rs:196:5` `ui::rich::charts::hierarchy::Cell::group` — it can be private
2417. **nit** — `crates/editor-ui/src/ui/rich/charts/hierarchy.rs:264:1` `ui::rich::charts::hierarchy::squarify` — it can be private
2418. **nit** — `crates/editor-ui/src/ui/rich/charts/hierarchy.rs:338:5` `ui::rich::charts::hierarchy::Bar::node` — it can be private
2419. **nit** — `crates/editor-ui/src/ui/rich/charts/hierarchy.rs:339:5` `ui::rich::charts::hierarchy::Bar::x0` — it can be private
2420. **nit** — `crates/editor-ui/src/ui/rich/charts/hierarchy.rs:340:5` `ui::rich::charts::hierarchy::Bar::x1` — it can be private
2421. **nit** — `crates/editor-ui/src/ui/rich/charts/hierarchy.rs:341:5` `ui::rich::charts::hierarchy::Bar::level` — it can be private
2422. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:80:5` `ui::rich::charts::sankey::Link::from` — it can be private
2423. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:81:5` `ui::rich::charts::sankey::Link::to` — it can be private
2424. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:82:5` `ui::rich::charts::sankey::Link::value` — it can be private
2425. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:87:5` `ui::rich::charts::sankey::Graph::labels` — it can be private
2426. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:88:5` `ui::rich::charts::sankey::Graph::links` — it can be private
2427. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:89:5` `ui::rich::charts::sankey::Graph::ranks` — it can be private
2428. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:90:5` `ui::rich::charts::sankey::Graph::incoming` — it can be private
2429. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:91:5` `ui::rich::charts::sankey::Graph::outgoing` — it can be private
2430. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:95:5` `ui::rich::charts::sankey::Graph::tallest` — it can be private
2431. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:112:5` `ui::rich::charts::sankey::Graph::from_spec` — it can be private
2432. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:173:5` `ui::rich::charts::sankey::Graph::inflow` — it can be private
2433. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:177:5` `ui::rich::charts::sankey::Graph::outflow` — it can be private
2434. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:181:5` `ui::rich::charts::sankey::Graph::throughput` — it can be private
2435. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:185:5` `ui::rich::charts::sankey::Graph::rank_count` — it can be private
2436. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:257:5` `ui::rich::charts::sankey::NodeBox::rank` — it can be private
2437. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:258:5` `ui::rich::charts::sankey::NodeBox::y0` — it can be private
2438. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:259:5` `ui::rich::charts::sankey::NodeBox::y1` — it can be private
2439. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:264:5` `ui::rich::charts::sankey::Band::source` — it can be private
2440. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:265:5` `ui::rich::charts::sankey::Band::target` — it can be private
2441. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:270:5` `ui::rich::charts::sankey::Layout::ranks` — it can be private
2442. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:271:5` `ui::rich::charts::sankey::Layout::nodes` — it can be private
2443. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:272:5` `ui::rich::charts::sankey::Layout::bands` — it can be private
2444. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:273:5` `ui::rich::charts::sankey::Layout::columns` — it can be private
2445. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:274:5` `ui::rich::charts::sankey::Layout::hit_groups` — it can be private
2446. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:279:5` `ui::rich::charts::sankey::LinkGroup::from` — it can be private
2447. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:280:5` `ui::rich::charts::sankey::LinkGroup::to` — it can be private
2448. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:281:5` `ui::rich::charts::sankey::LinkGroup::links` — it can be private
2449. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:282:5` `ui::rich::charts::sankey::LinkGroup::y0` — it can be private
2450. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:283:5` `ui::rich::charts::sankey::LinkGroup::y1` — it can be private
2451. **nit** — `crates/editor-ui/src/ui/rich/charts/sankey.rs:287:5` `ui::rich::charts::sankey::Layout::column` — it can be private
2452. **nit** — `crates/editor-ui/src/ui/rich/charts/spans/mod.rs:49:5` `ui::rich::charts::spans::Span::label` — it can be private
2453. **nit** — `crates/editor-ui/src/ui/rich/charts/spans/mod.rs:50:5` `ui::rich::charts::spans::Span::start` — it can be private
2454. **nit** — `crates/editor-ui/src/ui/rich/charts/spans/mod.rs:51:5` `ui::rich::charts::spans::Span::end` — it can be private
2455. **nit** — `crates/editor-ui/src/ui/rich/charts/spans/mod.rs:52:5` `ui::rich::charts::spans::Span::row` — it can be private
2456. **nit** — `crates/editor-ui/src/ui/rich/charts/spans/mod.rs:53:5` `ui::rich::charts::spans::Span::lane` — it can be private
2457. **nit** — `crates/editor-ui/src/ui/rich/charts/spans/mod.rs:54:5` `ui::rich::charts::spans::Span::color` — it can be private
2458. **nit** — `crates/editor-ui/src/ui/rich/charts/spans/mod.rs:55:5` `ui::rich::charts::spans::Span::group` — it can be private
2459. **nit** — `crates/editor-ui/src/ui/rich/charts/spans/mod.rs:60:5` `ui::rich::charts::spans::Lane::name` — it can be private
2460. **nit** — `crates/editor-ui/src/ui/rich/charts/spans/mod.rs:61:5` `ui::rich::charts::spans::Lane::first_row` — it can be private
2461. **nit** — `crates/editor-ui/src/ui/rich/charts/spans/mod.rs:62:5` `ui::rich::charts::spans::Lane::rows` — it can be private
2462. **nit** — `crates/editor-ui/src/ui/rich/charts/spans/mod.rs:67:5` `ui::rich::charts::spans::Marker::at` — it can be private
2463. **nit** — `crates/editor-ui/src/ui/rich/charts/spans/mod.rs:68:5` `ui::rich::charts::spans::Marker::label` — it can be private
2464. **nit** — `crates/editor-ui/src/ui/rich/charts/spans/mod.rs:73:5` `ui::rich::charts::spans::Layout::spans` — it can be private
2465. **nit** — `crates/editor-ui/src/ui/rich/charts/spans/mod.rs:74:5` `ui::rich::charts::spans::Layout::row_spans` — it can be private
2466. **nit** — `crates/editor-ui/src/ui/rich/charts/spans/mod.rs:75:5` `ui::rich::charts::spans::Layout::lanes` — it can be private
2467. **nit** — `crates/editor-ui/src/ui/rich/charts/spans/mod.rs:76:5` `ui::rich::charts::spans::Layout::markers` — it can be private
2468. **nit** — `crates/editor-ui/src/ui/rich/charts/spans/mod.rs:77:5` `ui::rich::charts::spans::Layout::rows` — it can be private
2469. **nit** — `crates/editor-ui/src/ui/rich/charts/spans/mod.rs:78:5` `ui::rich::charts::spans::Layout::domain` — it can be private
2470. **nit** — `crates/editor-ui/src/ui/rich/charts/spans/mod.rs:79:5` `ui::rich::charts::spans::Layout::dates` — it can be private
2471. **nit** — `crates/editor-ui/src/ui/rich/charts/spans/mod.rs:80:5` `ui::rich::charts::spans::Layout::unit` — it can be private
2472. **nit** — `crates/editor-ui/src/ui/rich/charts/spans/mod.rs:313:5` `ui::rich::charts::spans::Layout::time` — it can be private
2473. **nit** — `crates/editor-ui/src/ui/rich/charts/spans/mod.rs:321:5` `ui::rich::charts::spans::Layout::duration` — it can be private
2474. **nit** — `crates/editor-ui/src/ui/rich/code/annotated/view.rs:43:5` `ui::rich::code::annotated::view::AnnotatedState::update_model` — it can be private
2475. **nit** — `crates/editor-ui/src/ui/rich/code/file_tree.rs:81:5` `ui::rich::code::file_tree::Row::depth` — it can be private
2476. **nit** — `crates/editor-ui/src/ui/rich/code/file_tree.rs:82:5` `ui::rich::code::file_tree::Row::name` — it can be private
2477. **nit** — `crates/editor-ui/src/ui/rich/code/file_tree.rs:83:5` `ui::rich::code::file_tree::Row::kind` — it can be private
2478. **nit** — `crates/editor-ui/src/ui/rich/data/stats.rs:27:5` `ui::rich::data::stats::Figure::text` — it can be private
2479. **nit** — `crates/editor-ui/src/ui/rich/data/stats.rs:301:5` `ui::rich::data::stats::Tile::tone` — it can be private
2480. **nit** — `crates/editor-ui/src/ui/rich/kit.rs:104:5` `ui::rich::kit::CodeRef::end` — it can be private
2481. **nit** — `crates/editor-ui/src/ui/rich/kit.rs:108:5` `ui::rich::kit::CodeRef::parse` — it can be private
2482. **nit** — `crates/editor-ui/src/ui/rich/media/mod.rs:6:5` `ui::rich::media::SizeHint::width` — it can be private
2483. **nit** — `crates/editor-ui/src/ui/rich/media/mod.rs:8:5` `ui::rich::media::SizeHint::height` — it can be private
2484. **nit** — `crates/editor-ui/src/ui/rich/media/model.rs:23:5` `ui::rich::media::model::Shading::ALL` — it can be private
2485. **nit** — `crates/editor-ui/src/ui/rich/media/model.rs:25:5` `ui::rich::media::model::Shading::label` — it can be private
2486. **nit** — `crates/editor-ui/src/ui/rich/mod.rs:26:5` `ui::rich::Prepared::resident_bytes` — it can be private
2487. **nit** — `crates/editor-ui/src/ui/theme.rs:19:5` `ui::theme::Oklch::l` — it can be private
2488. **nit** — `crates/editor-ui/src/ui/theme.rs:20:5` `ui::theme::Oklch::c` — it can be private
2489. **nit** — `crates/editor-ui/src/ui/theme.rs:21:5` `ui::theme::Oklch::h` — it can be private
2490. **nit** — `crates/editor-ui/src/ui/theme.rs:22:5` `ui::theme::Oklch::a` — it can be private

**forge-store** (6)

2491. **nit** — `crates/forge-store/src/import_writes.rs:9:5` `import_writes::<impl Store>::create_project_at` — it can be private
2492. **nit** — `crates/forge-store/src/recall.rs:26:5` `recall::Searchable::text` — it can be private
2493. **nit** — `crates/forge-store/src/recall.rs:27:5` `recall::Searchable::output` — it can be private
2494. **nit** — `crates/forge-store/src/recall.rs:29:5` `recall::Searchable::paths` — it can be private
2495. **nit** — `crates/forge-store/src/retention.rs:45:5` `retention::<impl writer::Tx<'_>>::remove_thread` — it can be private
2496. **nit** — `crates/forge-store/src/writer.rs:721:5` `writer::Tx::<'_>::unpark_all` — it can be private

**editor-conversation** (5)

2497. **nit** — `crates/editor-conversation/src/conversation/activity.rs:28:5` `conversation::activity::ActivityCategory::count_label` — it can be private
2498. **nit** — `crates/editor-conversation/src/conversation/disclosure.rs:68:5` `conversation::disclosure::Flight::includes` — it can be private
2499. **nit** — `crates/editor-conversation/src/conversation/fold.rs:203:5` `conversation::fold::TextSection::inline_text` — it can be private
2500. **nit** — `crates/editor-conversation/src/conversation/fold.rs:304:5` `conversation::fold::Row::text` — it can be private
2501. **nit** — `crates/editor-conversation/src/conversation/view/factory.rs:2636:1` `conversation::view::factory::image_caption` — it can be private

**editor-core** (5)

2502. **nit** — `crates/editor-core/src/core/host.rs:37:5` `core::host::ScopedSourceProvider::host` — it can be private
2503. **nit** — `crates/editor-core/src/core/host.rs:38:5` `core::host::ScopedSourceProvider::instance` — it can be private
2504. **nit** — `crates/editor-core/src/core/host.rs:39:5` `core::host::ScopedSourceProvider::client` — it can be private
2505. **nit** — `crates/editor-core/src/core/host.rs:40:5` `core::host::ScopedSourceProvider::authority` — it can be private
2506. **nit** — `crates/editor-core/src/core/hosts.rs:210:1` `core::hosts::parse_wsl_verbose` — it can be private

**editor-audio** (3)

2507. **nit** — `crates/editor-audio/src/decode.rs:49:5` `decode::StreamDecoder::channels` — it can be private
2508. **nit** — `crates/editor-audio/src/decode.rs:50:5` `decode::StreamDecoder::rate` — it can be private
2509. **nit** — `crates/editor-audio/src/state.rs:78:5` `state::Inner::loaded` — it can be private

**editor-aqua** (2)

2510. **nit** — `crates/editor-aqua/src/shell/application.rs:1303:5` `shell::application::Tm::threshold_ms` — it can be private
2511. **nit** — `crates/editor-aqua/src/shell/root/wheel_sync.rs:7:5` `shell::root::wheel_sync::<impl shell::root::RootViews>::follow_tail_early` — it can be private

**forge-runs** (2)

2512. **nit** — `crates/forge-runs/src/lib.rs:226:5` `Hub::skills_sh_url` — it can be private
2513. **nit** — `crates/forge-runs/src/side.rs:296:5` `side::<impl Hub>::sync_title` — it can be private

**forge-engines** (1)

2514. **nit** — `crates/forge-engines/src/codex/decoder/requests.rs:599:5` `codex::decoder::requests::<impl codex::decoder::Decoder>::reply` — it can be private

**forge-http** (1)

2515. **nit** — `crates/forge-http/src/lib.rs:530:1` `hash_code` — it can be private

**forge-isolation** (1)

2516. **nit** — `crates/forge-isolation/src/lib.rs:21:5` `Outcome::of` — it can be private

</details>

### 14.4 Flagged on one target only (platform-specific, not dead)

- Linux only: `hawk::dead_public` `crates/editor-aqua/src/shell/native_notifications.rs:40:1` `shell::native_notifications::record_cold_launch`
- Linux only: `hawk::dead_public` `crates/forge-http/src/lib.rs:82:1` `serve`
- Linux only: `hawk::unnecessary_public` `crates/editor-aqua/src/shell/browser.rs:92:1` `shell::browser::resolve`
- Linux only: `hawk::unnecessary_public` `crates/forge-http/src/upgrade/mod.rs:13:5` `upgrade::Gateway::control`
- Linux only: `hawk::unnecessary_public` `crates/forge-http/src/upgrade/mod.rs:14:5` `upgrade::Gateway::token`
- Linux only: `hawk::unnecessary_public` `crates/forge-http/src/upgrade/mod.rs:17:5` `upgrade::Gateway::protocol`
- Linux only: `hawk::unnecessary_public` `crates/forge-http/src/upgrade/mod.rs:23:5` `upgrade::Ready::gateway`
- Linux only: `hawk::unnecessary_public` `crates/forge-http/src/upgrade/mod.rs:28:5` `upgrade::Upgrade::binary`
- Linux only: `hawk::unnecessary_public` `crates/forge-http/src/upgrade/mod.rs:94:1` `upgrade::read`
- Linux only: `hawk::unnecessary_public` `crates/forge-isolation/src/linux.rs:160:1` `linux::protect_forge`
- Linux only: `hawk::unnecessary_restricted_visibility` `crates/editor-aqua/src/shell/mod.rs:38:1` `shell::open_url`
- Linux only: `hawk::unnecessary_restricted_visibility` `crates/editor-audio/src/state.rs:259:5` `state::Inner::fill_mapped`
- Windows only: `hawk::dead_public` `crates/forge-http/src/lib.rs:116:1` `start_generation`
- Windows only: `hawk::dead_public` `crates/forge-runs/src/runtime/registry.rs:109:5` `runtime::registry::Registry::root`
- Windows only: `hawk::dead_public` `crates/forge-runs/src/runtime/registry.rs:206:5` `runtime::registry::Registry::upgrade_lock`
- Windows only: `hawk::unnecessary_public` `crates/editor-aqua/src/shell/browser.rs:98:1` `shell::browser::resolve`
- Windows only: `hawk::unnecessary_public` `crates/editor-aqua/src/shell/native_notifications.rs:40:1` `shell::native_notifications::record_cold_launch`
- Windows only: `hawk::unnecessary_public` `crates/forge-http/src/lib.rs:326:5` `Server::hub`
- Windows only: `hawk::unnecessary_public` `crates/forge-http/src/lib.rs:336:5` `Server::run_until`
- Windows only: `hawk::unnecessary_public` `crates/forge-http/src/lib.rs:487:1` `invite`
- Windows only: `hawk::unnecessary_public` `crates/forge-runs/src/runtime/registry.rs:113:5` `runtime::registry::Registry::active`
- Windows only: `hawk::unnecessary_restricted_visibility` `crates/editor-aqua/src/shell/mod.rs:32:1` `shell::open_url`
- Windows only: `hawk::unnecessary_restricted_visibility` `crates/editor-audio/src/state.rs:248:5` `state::Inner::fill`

## 15. Illegal states, data structures, algorithms and correctness

A second pass by five GPT 6.1 Sol agents, split by crate, hunting for states the types allow but should not, runtime bail-outs for those states, data structures, algorithms, correctness and architecture. Each reviewer was told to skip findings already recorded above. Reviewer text is verbatim.

### 15.1 editor-aqua

Reviewed `10f84a7ada6519109564cd433aed23c4e36676fa`, starting with `shell/root.rs` **3,873 lines**, `shell/workspace.rs` **1,750**, and `shell/application.rs` **1,379**. No edits, builds, or tests were run. Findings below were checked against `findings.md`; existing findings are not repeated. Locations refer to the pinned commit.

1. **Medium — correctness — Successful background submission replaces the user’s current screen.**  
   [shell/workspace.rs:1458](crates/editor-aqua/src/shell/workspace.rs:1458)

   An accepted new-thread submission unconditionally calls `core.navigate(Screen::Thread { … })`. It does not check whether the user still wants to navigate there. `sync_composer()` retains the composer on other surfaces, and Settings deliberately retains the underlying workspace route.

   **Failure:** send a new thread over a slow connection, open Settings, and begin editing. When submission finishes, Settings closes and the new thread takes over.

   **Fix:** associate navigation intent with the submission and originating route revision. Accept the submission independently; navigate only while that intent remains current. Preserve a Settings overlay when changing its underlying workspace route.

2. **Medium — correctness — Signing out abandons notification cleanup.**  
   [shell/entry.rs:255](crates/editor-aqua/src/shell/entry.rs:255), [shell/native_notifications.rs:139](crates/editor-aqua/src/shell/native_notifications.rs:139)

   Losing account readiness drops `Workspace`, including its `Notifications::posted` set, without producing withdrawal effects or clearing the taskbar badge. The native notification center survives in `ShellApplication`. Activation subsequently parses a host/thread pair and navigates without an account identity.

   **Failure:** receive a notification, sign out, then click the surviving notification. Its old title remains in the OS notification center, and its route can become pending navigation for a later sign-in.

   **Fix:** make account exit an explicit transition that withdraws every posted notification and clears the badge before destroying the workspace. Bind activation records to the account session and resolve them against that session at the boundary.

3. **Medium — data-structure — Deleted threads retain notifications indefinitely, and cleanup scans all posted notifications.**  
   [shell/notifications.rs:63](crates/editor-aqua/src/shell/notifications.rs:63)

   `withdraw()` uses `.is_none_or(...)`, so missing thread metadata means **retain**. A real `ThreadRemoved` event removes the thread and run, then emits `ThreadChanged`; that reaches this exact retention path.

   **Failure:** Forge retention deletes a previously notified thread. Its notification survives, and its entry remains in `posted` until the host disappears. Clicking it opens a deleted thread.

   Each thread change also scans the complete cross-host `posted` set: **O(p)** per event, or **O(np)** over `n` updates. Hundreds or thousands of retained notifications make ordinary activity increasingly expensive.

   **Fix:** propagate explicit thread removal to notification ownership. Distinguish deletion from cache eviction, index posted notifications by host/thread, and perform targeted withdrawal.

4. **Medium — data-structure — Resource previews copy the complete file again on every content rebuild.**  
   [shell/resource_preview/view.rs:154](crates/editor-aqua/src/shell/resource_preview/view.rs:154)

   `Arc::from(content.text.as_str())` allocates and copies the entire file each time `content()` runs. The original `String` remains resident; preparation separately copies the selected preview lines. The 5,000-line limit does not bound bytes—a single generated line can contain most of the file.

   **Failure:** preview a 64 MiB generated text file while unrelated host events rebuild the workspace. The UI thread repeatedly allocates and copies 64 MiB. Original text, prepared text, and the Copy-file closure can together retain roughly three full copies before rendering overhead.

   **Complexity:** **O(b)** allocation and copying per rebuild, where `b` can approach the client’s **128 MiB** resource limit.

   **Fix:** convert the fetched text to shared immutable storage once. Reuse that allocation for copying, and prepare preview data once per resource generation with an explicit byte budget.

5. **Medium — algorithm — Ordinary composer processing repeatedly copies the entire prompt.**  
   [shell/workspace.rs:827](crates/editor-aqua/src/shell/workspace.rs:827), [shell/workspace.rs:923](crates/editor-aqua/src/shell/workspace.rs:923), [shell/workspace.rs:1434](crates/editor-aqua/src/shell/workspace.rs:1434)

   `sync_question_pages()` first copies the editor text solely to detect changes. Its adapter can copy it again and reconstruct the questionnaire list. Composer dispatch calls synchronization before each action and after the loop; domain draining calls it twice more.

   **Failure:** type or move the caret in a large prompt while a thread summary is present. A normal action/drain pass performs several full prompt copies even when no question changed.

   **Complexity:** **O(d + q)** per synchronization, repeated several times per input, where `d` is prompt bytes and `q` is questionnaire data. Building a prompt character by character produces **O(d²)** cumulative copying in these paths.

   **Fix:** synchronize on questionnaire revision changes. Return an explicit editor-change result instead of cloning text to compare it, and perform one synchronization after applying the relevant batch.

6. **Medium — correctness — A hung installer makes Setup impossible to close normally.**  
   [setup/window.rs:69](crates/editor-aqua/src/setup/window.rs:69), [setup/window.rs:186](crates/editor-aqua/src/setup/window.rs:186), [setup/install.rs:45](crates/editor-aqua/src/setup/install.rs:45)

   Installation runs on a detached thread. The child process is owned only inside `install::run()`, which blocks reading stderr and then waiting for termination. Meanwhile `can_close()` rejects closing throughout `Installing`. There is no cancellation path or retained job owner.

   **Failure:** `ae` or a WSL operation hangs without exiting. Setup remains `Installing`, and the user must terminate processes externally.

   **Fix:** retain an installation job that owns cancellation and child termination/reaping. Model cancellation explicitly and allow closing to initiate it.

7. **Medium — correctness — The missing-host thread indicator resets its own loading clock.**  
   [shell/root.rs:2965](crates/editor-aqua/src/shell/root.rs:2965), [shell/root.rs:3043](crates/editor-aqua/src/shell/root.rs:3043)

   A Thread route without a current conversation host calls `self.connection.waiting(...)`. Later in the same build, every route other than NewThread calls `self.connection.reset()`.

   The built canvas starts invisible for 500 ms, but the reset removes its deadline and animation state.

   **Failure:** open a thread whose host is not yet available, or activate a notification for a removed pairing. The waiting surface can stay blank because nothing schedules its delayed reveal. Rebuilding restarts the delay again.

   **Fix:** retain the connection presenter for both Thread and NewThread waiting states. Reset it only when leaving that state or changing its owner.

8. **Medium — correctness — Qualification restore destroys the destination before replacement is secured.**  
   [qualification/fixture.rs:321](crates/editor-aqua/src/qualification/fixture.rs:321)

   Restore deletes the current files and directories before copying the snapshot back. No staged replacement or rollback protects the destination. Every subsequent restore first checkpoints the destination database.

   **Failure:** copying fails after deletion because of disk exhaustion or a file-access failure. The destination now contains a missing or partial database. Retrying can fail during `checkpoint_database()` before reaching the copy, requiring manual recovery.

   **Fix:** construct and verify a complete replacement in a sibling directory, then replace the destination with recoverable rename steps. A failed restore must leave either the previous tree or a complete replacement usable.

9. **Medium — bail-out — An unresolvable scroll request blocks all later requests. Runtime trigger suspected.**  
   [shell/root.rs:1607](crates/editor-aqua/src/shell/root.rs:1607), [shell/root/wheel_sync.rs:16](crates/editor-aqua/src/shell/root/wheel_sync.rs:16)

   The queue retains its front entry when directive resolution or application returns `None`. It does not distinguish “wait for layout” from “this target no longer exists.” Raw `Row(usize)` requests also survive row mutations without an epoch.

   **Failure scenario:** a pending reveal’s row disappears or becomes invalid after refolding; a later Jump-to-latest request sits behind it indefinitely. The blocking mechanism is verified; I did not establish a complete runtime reproduction at this commit.

   **Fix:** use one scoped, supersedable scroll intent. Resolve stable row identity against the current layout and represent waiting versus cancellation explicitly.

   ```rust
   enum ScrollIntent {
       Latest,
       Row { scope: PresentationScope, key: RowKey },
   }

   enum ScrollResolution {
       AwaitingLayout(ScrollIntent),
       Ready(ScrollDirective),
       Cancelled,
   }
   ```

10. **Medium — illegal-state — Setup exposes an unchecked vector/index invariant.**  
    [setup/model.rs:121](crates/editor-aqua/src/setup/model.rs:121), [setup/model.rs:167](crates/editor-aqua/src/setup/model.rs:167)

    `machines`, `selected`, `kind`, `relocatable`, and `phase` are independently public. `arguments()` and distro reconciliation directly index `machines[selected]`. Event validation protects only one mutation route.

    **Failure:** a caller clears `machines` or assigns an out-of-range `selected`, then starts installation or applies discovery results. Both are legal API operations followed by a panic. No current production caller performing those mutations was found.

    **Fix:** store the selected machine directly and freeze a complete plan when installation starts. Keep construction and transitions private.

    ```rust
    enum InstallPlan {
        Fresh { machine: Machine, location: PathBuf },
        Update { from: Version, location: PathBuf },
    }

    enum SetupState {
        Choosing(Choices),
        Installing { plan: InstallPlan, progress: String },
        Failed { plan: InstallPlan, error: InstallError },
        Done,
    }
    ```

11. **Medium — algorithm — Sign-in’s repeating spinner rebuilds the complete entry surface.**  
    [frame/sign_in/mod.rs:269](crates/editor-aqua/src/frame/sign_in/mod.rs:269), [shell/root.rs:2718](crates/editor-aqua/src/shell/root.rs:2718), [shell/refresh.rs:37](crates/editor-aqua/src/shell/refresh.rs:37)

    A spinner-step change makes `SignIn::tick()` report change. Root records `anim_signing`, which falls through to `Refresh::Workspace`; the application rebuilds the entry tree.

    **Failure:** wait for an email sign-in link for several minutes. The repeating spinner repeatedly reconstructs its ancestors throughout the wait.

    **Fix:** animate the spinner through local paint or scene state, rebuilding the card only for account, feedback, or layout changes. This independently violates **“Repeating animations must not dirty ancestors every frame”**; the existing Setup and Marketplace findings concern different paths.

12. **Low — illegal-state — `PreviewText` permits invalid UTF-8 slicing through ordinary field mutation.**  
    [shell/resource_preview.rs:4](crates/editor-aqua/src/shell/resource_preview.rs:4), [shell/resource_preview/view.rs:210](crates/editor-aqua/src/shell/resource_preview/view.rs:210)

    Text, byte ranges, line origin, and line count are independently public. `prepare_text()` trusts those ranges and slices directly.

    **Failure:** construct a preview, replace its text with a shorter string, then prepare it; or provide a range splitting a multibyte character. Both states are representable and panic. No current production producer of malformed ranges was found.

    **Fix:** expose an immutable normalized preview. Store valid line text, or use an encapsulated slice type owning its validated backing text.

    ```rust
    struct PreviewText {
        full: Arc<str>,
        first_line: NonZeroUsize,
        lines: Box<[String]>,
    }
    ```

13. **Low — illegal-state — Raster shapes are publicly constructible without their buffer invariants.**  
    [frame/sign_in/dither.rs:3](crates/editor-aqua/src/frame/sign_in/dither.rs:3), [frame/sign_in/dither.rs:84](crates/editor-aqua/src/frame/sign_in/dither.rs:84), [frame/sign_in/dither.rs:114](crates/editor-aqua/src/frame/sign_in/dither.rs:114)

    `Source` separately exposes dimensions and RGBA bytes. `Mask` separately exposes dimensions and pixels.

    **Failure:** a nonempty mask with width zero reaches remainder-by-zero in `paint()`. A source whose dimensions exceed its buffer reaches an out-of-bounds read in `area()`. Current bundled-art construction supplies valid data; these are verified API invariant holes.

    **Fix:** use normalized image buffers, keep their representation private, and iterate image coordinates instead of reconstructing coordinates from unchecked dimensions.

    ```rust
    struct Source(image::RgbaImage);
    struct Mask(image::GrayImage);
    ```

14. **Low — illegal-state — `VerifiedBootstrap` does not preserve what its name promises.**  
    [qualification/fixture.rs:354](crates/editor-aqua/src/qualification/fixture.rs:354)

    The request, fixture, snapshot, server, and editor directory are all public and independently replaceable after verification. `verify_inputs()` rechecks only some relationships.

    **Failure:** a caller changes `editor_dir`, the requested thread, or snapshot after `load()`. Downstream code can consume the object as verified even though its validated relationships no longer hold. No such in-crate mutation was found.

    **Fix:** separate raw input from a private validated representation; expose read-only accessors and narrowly scoped operations.

    ```rust
    struct VerifiedBootstrap {
        request: ValidatedRequest,
        fixture: ImmutableFixture,
        snapshot: StoreSnapshot,
        server: ForgeServer,
        editor: QualificationDirectory,
    }
    ```

15. **Low — architecture — Diagnostic strings control rendering behavior.**  
    [shell/refresh.rs:28](crates/editor-aqua/src/shell/refresh.rs:28)

    Rendering scope is inferred from strings collected through diagnostics. Unknown strings silently request a workspace rebuild. For example, root records `"rows"` at `shell/root.rs:3632`, while the classifier recognizes `"viewport_rows"`.

    **Failure:** that path rebuilds the workspace despite changing transcript rows. Renaming or adding a diagnostic label can silently change runtime work.

    **Fix:** return typed invalidation directly from state transitions. Derive diagnostic labels from that value. Keep profiling notes outside rendering control flow.

The ten types I would redesign first are below. Existing defects are referenced where applicable rather than relisted as new findings.

| Type | Location at `10f84a7` | What it permits | Proposed shape |
|---|---|---|---|
| `RootViews` | [shell/root.rs:101](crates/editor-aqua/src/shell/root.rs:101) | Independent dialog, content, row, media, scope, and timing options require coordinated resets. Oversized ownership is already recorded in §5.1 finding 8. | Cohesive owners; `ConversationPresentation::{Absent, Waiting, Mounted(MountedConversation)}`; each mounted dialog owns its content and identity. |
| `Workspace` | [shell/workspace.rs:35](crates/editor-aqua/src/shell/workspace.rs:35) | Composer, duplicated target, optional instance, focus flags, and completion navigation can disagree. | One `ComposerSlot` owning target, instance, editor, and submission intent; background draft work has separate ownership. |
| `Entry` | [shell/entry.rs:7](crates/editor-aqua/src/shell/entry.rs:7) | Account readiness and optional workspace are reconciled after the fact; destruction omits external cleanup. | `EntryState::{SignedOut, Restoring, Ready(AccountWorkspace)}` with explicit exit effects. |
| `Setup` | [setup/model.rs:121](crates/editor-aqua/src/setup/model.rs:121) | Invalid machine index; contradictory installation flags; mutable configuration during installed phases. | `Choosing(Choices)`, `Installing(InstallJob)`, `Failed(FailedInstall)`, `Done`; selected `Machine`, frozen `InstallPlan`. |
| `Notifications` | [shell/notifications.rs:23](crates/editor-aqua/src/shell/notifications.rs:23) | Missing baseline, offline baseline, posted OS state, deletion, and account ownership are loosely coupled. | Account-scoped owner; explicit host baseline enum; per-thread posted records and removal transitions. |
| `ResourcePreview` | [shell/resource_preview.rs:50](crates/editor-aqua/src/shell/resource_preview.rs:50) | Loading content without a request; public path/content can disagree. The revoked-request failure is already §5.2 finding 21. | `PreviewState::{Loading(OwnedRequest), Ready(SharedPreview), Failed(PreviewError), Unavailable}`. |
| `PreviewText` | [shell/resource_preview.rs:4](crates/editor-aqua/src/shell/resource_preview.rs:4) | Byte ranges can disagree with text and UTF-8 boundaries. | Immutable full text plus normalized lines or validated owned slices. |
| `Interaction` | [shell/thread_navigation.rs:236](crates/editor-aqua/src/shell/thread_navigation.rs:236) | Cursor outside `entries`, open-but-unavailable state, and index-based reveal after replacement. `steer()` papers over invalid cursors with clamping. | `Unavailable`, `Closed(Outline)`, `Open(SelectedOutline)`; selection owns a current entry and uses stable identity. |
| `Palette` | [shell/palette.rs:52](crates/editor-aqua/src/shell/palette.rs:52) | Public query/results/selection can change independently of request generation; completed and pending searches share optional fields. | Private `SearchState::{Idle, Searching, Complete}` and selection by entry identity. |
| `VerifiedBootstrap` | [qualification/fixture.rs:354](crates/editor-aqua/src/qualification/fixture.rs:354) | Post-validation mutation invalidates its proof. | Private validated request and owned resources, exposed through read-only operations. |

Scores judge the complete assigned slice, including previously recorded defects. Performance conclusions above are static complexity and allocation analysis, not benchmark measurements.

| Category | Score |
|---|---:|
| Correctness | **5/10** |
| State handling — illegal-state discipline | **3/10** |
| Data structures and algorithms | **5/10** |
| Code quality and architecture | **4/10** |
| **Overall** | **4/10** |

### 15.2 editor-ui, editor-conversation, editor-composer

Reviewed **`10f84a7ada6519109564cd433aed23c4e36676fa`**, read-only, using the [review skill](/home/sander/.local/share/artisan-street/forge/data/engines/codex/home/.codex/skills/review/SKILL.md) for independent checks. No edits, builds, tests, or profiling. I checked cited files against `findings.md` and excluded recorded findings. All line references below are at the target commit; performance costs are derived from the code, not measurements.

1. **High · correctness — Page responses overwrite newer SSE updates.**  
   [crates/editor-conversation/src/conversation/thread.rs:230](crates/editor-conversation/src/conversation/thread.rs:230), also `:238` and `:252`.

   Refresh, Tail, and At application remove existing items before inserting the response. That removes the newer revision that `upsert()` would otherwise compare against at `:148`. Refresh also removes items received after the response’s snapshot.

   **Scenario:** Forge reads assistant item A at sequence 100; SSE delivers its completed revision at sequence 110; the delayed page deletes revision 110 and reinstalls revision 100. Completed text or requests can revert until another update arrives.

   **Fix:** merge overlapping identities by sequence before pruning. Carry a page snapshot watermark so pruning cannot delete changes newer than the response.

2. **High · correctness — Failed-send recovery truncates and saves the next prompt.**  
   [crates/editor-composer/src/composer/controller.rs:763](crates/editor-composer/src/composer/controller.rs:763), especially `:770` and `:777`.

   Recovery concatenates the submitted text and text typed while sending, then restores that concatenation through `Editor::set_text()`. That setter truncates to 64 KiB and clears undo history. `edited()` then persists the truncated recovery.

   **Scenario:** send a 64 KiB prompt, type a follow-up while the request is pending, then receive a send failure. The whole follow-up disappears and the shortened draft is saved. No target switch is required.

   **Fix:** preserve the two drafts independently:

   ```rust
   enum Outbox {
       Idle,
       Sending { submitted: DraftBody },
       Recovering { submitted: DraftBody, next: DraftBody },
   }
   ```

   Recovery must not pass combined user data through a truncating display setter.

3. **Medium · illegal-state — A reveal without geometry requests animation frames indefinitely.**  
   [crates/editor-conversation/src/conversation/view/reveal.rs:247](crates/editor-conversation/src/conversation/view/reveal.rs:247).

   `MountedReveal` permits both `frame` and `stored_frame` to be absent. `sync()` interprets that combination as `animating = true`. The factory acquires a reveal before preparation, but stored loading/failure placeholders and Markdown preparation fallbacks can reach activation without recording either frame.

   **Scenario:** a stored Markdown body fails to load. Its error placeholder remains mounted, and every synchronization requests another animation frame. The scheduler explicitly bypasses its pending-stall throttle for this case. Actual repaint cost was not measured, but the endless frame-request loop is verified.

   **Fix:** activate only an explicit static result or an animation with geometry:

   ```rust
   enum RevealGeometry {
       Inline(OwnedGeometry),
       Stored(StoredGeometry),
   }

   enum MountedReveal {
       Static,
       Animated {
           reveal: Rc<PreparedReveal>,
           geometry: RevealGeometry,
       },
   }
   ```

   Pending reads should wake through completion notifications. This violates **“idle CPU must stay near zero.”**

4. **Medium · correctness — Chunked inline errors and captions repeat their complete text.**  
   [crates/editor-conversation/src/conversation/projection.rs:161](crates/editor-conversation/src/conversation/projection.rs:161) and [crates/editor-conversation/src/conversation/view/factory.rs:2424](crates/editor-conversation/src/conversation/view/factory.rs:2424).

   Projection creates one row per approximately 1 KiB `plain_ranges()` chunk. Rendering ignores `block` and supplies the entire source to every row.

   **Scenario:** a 3 KiB inline error appears three times in full. Image captions have the same defect.

   **Complexity:** with source size **S** and **P ≈ S/1024** rows, realizing all chunks copies and renders **O(P·S)** rather than **O(S)**. A 16 KiB value can produce roughly 256 KiB of repeated text.

   **Fix:** carry each validated source range into the literal row and render that slice.

5. **Medium · bail-out — Valid rich fences inside lists cannot render.**  
   [crates/editor-ui/src/ui/markdown/presenter.rs:418](crates/editor-ui/src/ui/markdown/presenter.rs:418) and [crates/editor-ui/src/ui/markdown/projected.rs:572](crates/editor-ui/src/ui/markdown/projected.rs:572).

   The presenter retains rich payloads only for top-level `Block::Code` entries. Projected normalization recognizes rich code leaves inside lists and suppresses their ordinary source rendering, but `bind_rich()` has no corresponding retained entry. Rendering then returns `Unavailable("nested rich block")`.

   **Scenario:** a valid chart or callout fence works outside a numbered list and fails inside its list item.

   **Fix:** the normalized leaf should own its rendering payload rather than an optional index into another document:

   ```rust
   enum LeafBody {
       Text(PreparedText),
       Code(Fence),
       Rich(RichFence),
       Diagram(PreparedDiagram),
       Table(PreparedTableRow),
   }
   ```

   This removes the representable “rich content without a rich renderer” state.

6. **Medium · correctness — One failed stored-page prefetch silently prevents further progress.**  
   [crates/editor-ui/src/ui/markdown/stored.rs:478](crates/editor-ui/src/ui/markdown/stored.rs:478), `:498`, and [crates/editor-conversation/src/conversation/stored.rs:572](crates/editor-conversation/src/conversation/stored.rs:572).

   After a window fetch fails, `current()` returns the old ready source before checking the error. A subsequent request for the failed offset returns the cached failure. The conversation reduces that result to a Boolean and discards the failure; there is no index retry transition.

   **Scenario:** loading the next descriptor window suffers one transient error. Previously loaded content remains visible, but scrolling cannot advance through that boundary and supplies no explanation.

   **Fix:** expose retained content and window-fetch state together, including the failed offset and cause. Render an explicit recovery action that restarts that failed operation.

7. **Medium · algorithm — Code-fence preparation scans every token for every line.**  
   [crates/editor-ui/src/ui/markdown/fence.rs:118](crates/editor-ui/src/ui/markdown/fence.rs:118).

   For each line, `Fence::new()` filters the complete token vector and complete color-swatch vector.

   **Complexity:** **O(L·(T+C))**, where **L** is lines, **T** highlight tokens, and **C** swatches. A 2,000-line fence with 20,000 tokens performs approximately 40 million token checks. Closed fences are prepared again when their surrounding streamed document changes.

   **Scenario:** a long code answer continues streaming after a substantial closed fence, repeatedly paying this preparation cost.

   **Fix:** distribute ordered tokens and swatches into lines with advancing cursors, giving **O(L+T+C)** work, apart from tokens spanning multiple lines. This is separate from the already recorded fence-matching and whole-source parsing findings.

8. **Medium · data-structure — Every projected leaf copies its complete message into observation state.**  
   [crates/editor-conversation/src/conversation/view/factory.rs:2547](crates/editor-conversation/src/conversation/view/factory.rs:2547), also `:2555`.

   Each realized leaf independently materializes the live rope or allocates an `Rc<str>` containing the entire inline message. Live rendering also materializes the rope earlier at `:2082`.

   **Complexity:** **O(V·S)** copying and allocation for **V** realized leaves belonging to a source of **S** bytes. Twenty leaves from a 16 KiB inline message create 320 KiB of observation-source copies alone.

   **Scenario:** many short paragraphs from one streaming response are mounted together, multiplying copying on each realization.

   **Fix:** share one immutable source snapshot per body revision; leaf observations should retain that shared snapshot and their own leaf identity. This adds a multiplicative cost beyond the shallow transcript cloning already recorded in §3.1 #7.

9. **Medium · data-structure — The transcript window does not bound work spans or their runs.**  
   [crates/editor-conversation/src/conversation/thread.rs:347](crates/editor-conversation/src/conversation/thread.rs:347).

   The early return considers only item count and item-body bytes. Work-span and run pruning occurs later, after item eviction. Neither container contributes to the admission decision.

   **Scenario:** many completed continuation runs sit between relatively few standalone messages. Transcript pages can contain thousands of spans while remaining below both item thresholds.

   **Complexity:** retained memory and subsequent folding grow **O(W+R)** with spans and runs independently of the advertised item window.

   **Fix:** budget and evict the combined ordered transcript representation, including span, run, and allocation costs. This is additional to §3.1 #8’s server-side hidden-work scan.

10. **Medium · data-structure — Failed draft writes accumulate duplicate complete JSON bodies.**  
    [crates/editor-composer/src/composer/draft_session.rs:288](crates/editor-composer/src/composer/draft_session.rs:288).

    Every failed write appends its body to `unconfirmed`, including retries of an identical request. Successful reconciliation later searches these complete values with `contains()`.

    **Scenario:** repeatedly retry saving an unchanged large draft while Forge remains unavailable. Each failure retains another copy. A thousand failures of a 64 KiB body retain approximately 62.5 MiB before JSON overhead.

    **Complexity:** **O(K·B)** memory and worst-case comparison work for **K** failures and body size **B**.

    **Fix:** explicitly own an ambiguous write and reconcile it before replacing it with another distinct write. Identical failures must not add bodies. Use a typed draft payload internally rather than repeatedly retaining raw `Value`s.

11. **Medium · illegal-state — Answer ownership survives eviction while its “sending” state does not.**  
    [crates/editor-conversation/src/conversation/controller.rs:1194](crates/editor-conversation/src/conversation/controller.rs:1194), `:1484`, and [crates/editor-conversation/src/conversation/thread.rs:382](crates/editor-conversation/src/conversation/thread.rs:382).

    `ItemUi.sending/error` and `answer_tasks` independently represent one operation. Window eviction deletes `ItemUi` without cancelling or preserving the corresponding operation state.

    **Scenario:** submit an answer, evict that item while its request is pending, then reload it. Controls return enabled because the UI flag vanished. A second answer replaces and aborts the first task. If the first completion is already queued, its item-only completion handling can also remove the replacement task. Failures arriving while the UI entry is absent disappear.

    **Fix:** keep operation ownership outside evictable presentation state:

    ```rust
    enum AnswerState {
        Idle,
        Sending {
            request: AnswerRequestId,
            task: AbortOnDropHandle<()>,
        },
        Failed(String),
    }
    ```

    Match completions to the request identity. Unlike §5.2 #14, this failure needs no conversation switch or parking.

12. **Medium · algorithm — Folding calculates versions that expansion immediately discards.**  
    [crates/editor-conversation/src/conversation/fold.rs:739](crates/editor-conversation/src/conversation/fold.rs:739), `:848`, and `:937`.

    `fold()` calculates versions for its rows. `expanded()` rebuilds the child index, calculates versions again, and explicitly discards incoming revisions.

    **Scenario:** every streamed update performs both passes across a loaded window of hundreds of items, even when expansion leaves most rows unchanged.

    **Complexity:** an unnecessary additional child-index construction and row/dependency hashing pass per rebuild. The extra work scales with loaded items and visited dependencies.

    **Fix:** fold into unversioned semantic rows, expand them, and calculate final versions once using one child index.

13. **Medium · algorithm — A row revision query reconstructs the row model.**  
    [crates/editor-conversation/src/conversation/view/factory.rs:1251](crates/editor-conversation/src/conversation/view/factory.rs:1251) and [crates/editor-conversation/src/conversation/adapter.rs:225](crates/editor-conversation/src/conversation/adapter.rs:225).

    `revision()` calls `fingerprint()`, which rebuilds `RowModel`: allocating item/UI maps, cloning UI state, and scanning live targets. Fingerprinting then sorts UI entries using allocating `to_string()` keys and formats disclosure state.

    **Scenario:** checking whether a collapsed chain of hundreds of tools can be reused reconstructs and sorts the chain’s state before deciding to reuse it.

    **Complexity:** roughly **O(L+K log K)** plus cloning per query, for **L** live targets and **K** chain members. This work repeats during transcript refreshes and after realization.

    **Fix:** compute stable fingerprints alongside fold/snapshot updates. A revision lookup should retrieve the stored revision without constructing render inputs.

14. **Low · correctness — Undo restores the wrong active end of a selection.**  
    [crates/editor-ui/src/ui/text/editor.rs:68](crates/editor-ui/src/ui/text/editor.rs:68) and `:756`.

    History stores the sorted selection range but omits direction. Restoration always sets `reversed = false`.

    **Scenario:** select characters 1–3 backwards, replace them, then undo. The highlight returns, but its active end moves from 1 to 3; Shift+Left shrinks the selection instead of extending it left.

    **Fix:** preserve one directional selection value in editor state and history:

    ```rust
    struct Selection {
        anchor: TextPosition,
        head: TextPosition,
    }

    struct Snapshot {
        text: String,
        selection: Selection,
    }
    ```

15. **Low · correctness — Answer rendering removes legitimate answer content.**  
    [crates/editor-conversation/src/conversation/view/factory.rs:698](crates/editor-conversation/src/conversation/view/factory.rs:698).

    `replace("You answered: ", "")` removes every occurrence, including occurrences inside the answer. The modified text also becomes its selection/copy source.

    **Scenario:** an answer saying `Print "You answered: yes" literally` displays and copies as `Print "yes" literally`.

    **Fix:** preserve normalized answer text. If a legacy format requires removing a display prefix, recognize that format at ingestion and remove only its leading prefix.

16. **Low · illegal-state — Questionnaire progress survives replacement of its question set. Production trigger suspected.**  
    [crates/editor-composer/src/composer/questionnaire.rs:129](crates/editor-composer/src/composer/questionnaire.rs:129).

    State reload/clamping happens only when the active item ID changes. Replacing the same item’s questions retains positional `step`, recorded answers, and selected option IDs.

    **Scenario:** answer A in `[A, B]`, then replace that same item’s collection with `[B, A]`. Submission associates the recorded answer with B. Shrinking the collection can leave `step` outside its bounds.

    The public API behavior and production caller are verified; I did not establish a Forge path that mutates an already displayed question set this way.

    **Fix:** bind progress to collection revision and question identity:

    ```rust
    struct AnswerSession {
        revision: QuestionSetRevision,
        current: QuestionId,
        answers: BTreeMap<QuestionId, QuestionAnswer>,
    }
    ```

    Reconcile or invalidate the session when the revision changes.

17. **Low · bail-out — `RowFactory::new()` exposes rendering before required services exist.**  
    [crates/editor-conversation/src/conversation/view/factory.rs:1210](crates/editor-conversation/src/conversation/view/factory.rs:1210), with failures at `:1845` and `:1849`.

    Construction leaves dispatch, work, source, and other services absent, while making realization callbacks available. Eager and lazy row representations also coexist independently.

    **Scenario:** construct the public factory with a valid work row and invoke its callback before remembering `.with_work()` or `.with_dispatch()`. Valid content becomes a collection validation error. Current production construction supplies these services; the API permits the failure.

    **Fix:** separate configuration from a ready factory:

    ```rust
    enum RowSource {
        Eager(Vec<RealizedRow>),
        Lazy(Vec<LazyRow>),
    }

    struct ReadyRowFactory {
        rows: RowSource,
        dispatch: Dispatch,
        work: WorkResources,
        sources: SourceResources,
    }
    ```

    Each lazy row should own its snapshot, disclosure, and admission values together.

18. **Low · illegal-state — Empty rows manufacture a different identity on every lookup.**  
    [crates/editor-conversation/src/conversation/fold.rs:425](crates/editor-conversation/src/conversation/fold.rs:425), also `:439`.

    Empty chains and empty loose-work rows fall back to `ItemId::default()`. That default generates a fresh UUID.

    **Scenario:** construct either publicly representable empty row and call `key()` twice. The keys differ, defeating virtualization identity, cached heights, and observation matching. Current chain expansion avoids this through a runtime count check.

    **Fix:** require a first member and distinguish legitimate empty run headers:

    ```rust
    struct Chain {
        first: ItemId,
        rest: Vec<ItemId>,
    }

    enum WorkMember {
        Loaded(ItemId),
        Unloaded(Seq),
    }

    enum WorkRow {
        Header(RunId),
        Members {
            run: Option<RunId>,
            first: WorkMember,
            rest: Vec<WorkMember>,
        },
    }
    ```

19. **Low · illegal-state — A phase can be both settled and running.**  
    [crates/editor-conversation/src/conversation/phase.rs:13](crates/editor-conversation/src/conversation/phase.rs:13).

    `Settled` accepts every `RunStatus`. Explicit fallback arms handle `Settled(Running)` at `:121` and `:131`, while `RunSummary::live()` classifies it as nonlive.

    **Scenario:** constructing that public value produces “Working” or “Thinking” text while lifecycle logic treats the run as settled. `turn_phase()` currently avoids constructing it.

    **Fix:** restrict its payload to terminal outcomes:

    ```rust
    enum TerminalStatus {
        Done,
        Failed,
        Refused,
        Cancelled,
        Interrupted,
    }

    enum Phase {
        Unknown,
        Thinking,
        Working,
        Replying,
        Compacting,
        WaitingOnYou,
        Settled(TerminalStatus),
    }
    ```

The ten types I would redesign first:

| Type | File:line at `10f84a7` | What it permits | Proposed shape |
|---|---|---|---|
| `Outbox` | `crates/editor-composer/src/composer/controller.rs:60` | Recovery collapses two independent drafts into one bounded buffer | Separate submitted and subsequent drafts in `Recovering` |
| `ThreadView` | `crates/editor-conversation/src/conversation/thread.rs:36` | Revision regression; independently mutable item/index/accounting containers; unbudgeted spans | Private, revision-aware transcript window with complete accounting |
| `Row` | `crates/editor-conversation/src/conversation/fold.rs:12` | Empty identity-bearing collections; literal ordinal without its range; recursively nested `Projected` rows | Nonempty work/chain payloads, validated literal slices, separate semantic and projected row types |
| Projected `Leaf` | `crates/editor-ui/src/ui/markdown/projected.rs:24` | Rich content without an owned rendering payload | `LeafBody` enum owning preparation |
| `MountedReveal` | `crates/editor-conversation/src/conversation/view/reveal.rs:21` | Neither geometry frame, or both; absence interpreted as animation | `Static` or `Animated` with exactly one geometry variant |
| `IndexEntry` | `crates/editor-ui/src/ui/markdown/stored.rs:232` | Retained content hides failed window-fetch state | Explicit initial-load state, then retained source plus `Idle/Loading/Failed` window state |
| `DraftSession` | `crates/editor-composer/src/composer/draft_session.rs:44` | Unlimited duplicate bodies representing ambiguous writes | Typed draft payload and explicitly owned ambiguous-write state |
| `ItemUi` | `crates/editor-conversation/src/conversation/thread.rs:19` | Evictable presentation flags disagree with answer-task ownership | Conversation-owned `AnswerState` keyed by item and request identity |
| `Editor` / `Snapshot` | `crates/editor-ui/src/ui/text/editor.rs:68` | Selection range and direction are separate; history loses direction | One directional `Selection` stored consistently |
| `RowFactory` | `crates/editor-conversation/src/conversation/view/factory.rs:165` | Public realization with missing capabilities; parallel row representations | Builder producing `ReadyRowFactory` with required services and one `RowSource` |

Both reviewers corroborated the page race, recovery loss, repeated literal chunks, nested rich failure, observation copies, draft retention, empty-row keys, and impossible phase. Opus rated recovery loss medium; I rate it high because it destroys and persists user-authored text. I excluded duplicated raw-settings/god-module findings and rejected the proposed connection between nested-rich rendering failure and the fallback animation loop: that rendering error exits through a different path.

Scores cover the assigned crates as a whole, including existing defects omitted from this numbered list.

| Scope | Correctness | State handling | Data structures and algorithms | Code quality and architecture | Overall |
|---|---:|---:|---:|---:|---:|
| `editor-ui` | 5/10 | 4/10 | 4/10 | 3/10 | **4/10** |
| `editor-conversation` | 4/10 | 3/10 | 3/10 | 3/10 | **4/10** |
| `editor-composer` | 4/10 | 3/10 | 5/10 | 4/10 | **4/10** |
| **Combined assessment** | **4/10** | **3/10** | **4/10** | **3/10** | **4/10** |
| Opus 5.5 High, aggregate | 4/10 | 3/10 | 4/10 | 3/10 | **4/10** |
| GPT 6.1 Sol High, aggregate | 5/10 | 3/10 | 4/10 | 4/10 | **4/10** |

### 15.3 editor-core and the Editor feature crates

Reviewed commit **`10f84a7ada6519109564cd433aed23c4e36676fa`**, read-only. No edits, builds, Cargo/Nix commands, or tests. All locations below refer to that snapshot. I checked cited files against `findings.md` and omitted recorded findings.

The most serious new issues are a release-process abort from malformed Markdown metadata and Settings targeting the wrong Forge after Back navigation.

1. **High · illegal-state — Deserialized Markdown metadata can abort the Editor.**  
   [crates/artisan-markdown-index/src/lib.rs:107](crates/artisan-markdown-index/src/lib.rs:107), [lib.rs:355](crates/artisan-markdown-index/src/lib.rs:355).

   `Token::Structural` accepts any deserialized parser event, including `Html`. Each event carries an unchecked `Range<usize>`. Replay checks subtraction from the leaf’s origin, but does not establish ordered bounds, containment within the fetched text, or UTF-8 boundaries.

   **Failure:** a record whose outer leaf covers `0..1` contains a structural HTML event covering `0..2`. Replay accepts it. The stored-Markdown caller deserializes the record and passes its events into unchecked `source[range]` slicing at `editor-ui/src/ui/markdown/mod.rs:772`. Release builds use `panic = "abort"` at `Cargo.toml:165`, so this terminates the Editor. This path is statically verified; it was not executed.

   **Fix:** deserialize a raw DTO, then validate it against the exact source before constructing replayable data. Restrict structural tokens to actual structural events.

   ```rust
   struct CheckedLeaf<'a> {
       source: &'a str,
       events: Vec<CheckedEvent<'a>>,
   }

   struct CheckedRange {
       start: usize,
       end: usize,
   }

   enum CheckedEvent<'a> {
       Structural(StructuralEvent, CheckedRange),
       Text(&'a str, CheckedRange),
       Html(&'a str, CheckedRange),
   }
   ```

   Keep constructors private. Boundary validation must prove every range and the supported event shape. This violates **“Normalize messy input data into clean structures at the boundary.”**

2. **High · illegal-state — Back navigation makes Settings operate on the wrong host.**  
   [crates/editor-core/src/core/store.rs:363](crates/editor-core/src/core/store.rs:363), [store.rs:451](crates/editor-core/src/core/store.rs:451).

   Forward navigation updates `hosts_file.last`; Back only restores `screen`. `active_host()` uses the current screen’s host, otherwise the persisted last host. Settings and Marketplace screens carry no host.

   **Failure:** visit A → visit B → Back to A → open Settings. The workspace belongs to A, but Settings binds its prompt, engine, and MCP forms to B. Failed persistence during ordinary navigation creates the same disagreement. Settings binding is at `editor-settings/src/settings/controller.rs:126`.

   **Fix:** make the destination host part of every host-specific route. Persisted launch preferences must not determine the current operation’s target.

   ```rust
   enum SettingsRoute {
       Editor(EditorSettingsPage),
       Forge {
           scope: HostScope,
           page: ForgeSettingsPage,
       },
   }

   enum Route {
       Workspace(WorkspaceRoute),
       Settings {
           target: SettingsRoute,
           return_to: WorkspaceRoute,
       },
   }
   ```

3. **Medium · correctness — An oversized relay event permanently blocks connection processing.**  
   [crates/editor-core/src/core/connection.rs:275](crates/editor-core/src/core/connection.rs:275).

   The connection semaphore contains 16 MiB of permits. Relay events are serialized and passed directly to `acquire_many_owned(raw.len())`, without checking whether that amount can ever be available. The direct transport has a 4 MiB decoder limit; the relay branch bypasses it. Tunnel transfers permit up to 32 MiB.

   **Failure:** a broken or hostile Forge sends a relay envelope between 16 and 32 MiB. The permit acquisition can never finish, even after every queued event drains. Retry and observation changes cannot interrupt this await, leaving the connection marked Live.

   **Fix:** enforce one event-size limit before admission on both transports. Reject oversized envelopes explicitly, and make admission cancellable. The deadlock is statically established; production occurrence was not observed.

4. **Medium · illegal-state — Page ingestion bypasses pending Stop handling and deletion invariants.**  
   [crates/editor-core/src/core/host/pages.rs:214](crates/editor-core/src/core/host/pages.rs:214), [crates/editor-core/src/core/host.rs:633](crates/editor-core/src/core/host.rs:633).

   Optimistic Stop rewrites the server run as `Cancelled`, sets `ended_at`, and stores intent in a separate `stopping` map. SSE and snapshot ingestion use `remember_run()`, which applies that intent. HTTP page ingestion writes directly into `runs`.

   **Failure:** Stop marks a run cancelled; an already-running page request returns its Running version with an equal or newer sequence and restores Running. The page’s run loop also lacks the deletion check applied to its thread loop: an old page can restore a deleted thread’s running run, which pruning preserves.

   **Fix:** preserve server state and represent pending intent explicitly. All observations must pass through one private reducer.

   ```rust
   enum RunView {
       Following(Run),
       Stopping {
           observed: Run,
           requested_at: Timestamp,
       },
   }

   struct ThreadEntry {
       thread: Thread,
       run: Option<RunView>,
   }
   ```

   Deletion must retire the associated run state. A Stop request should not masquerade as confirmed server completion.

5. **Medium · illegal-state — An obsolete terminal fetch can abort its replacement.**  
   [crates/editor-inspector/src/inspector/dialog.rs:126](crates/editor-inspector/src/inspector/dialog.rs:126), [dialog.rs:156](crates/editor-inspector/src/inspector/dialog.rs:156).

   `fetch()` reuses the dialog generation. Reset and Retry therefore create new fetches with the same identity as previous fetches.

   **Failure:** A’s completion is queued; Reset starts B; A drains afterward. Its generation still matches, so `self.load = None` drops B’s `AbortOnDropHandle`. A’s stale item or error becomes the result, and the refresh is cancelled. Item sequence validation occurs after the replacement handle has already been discarded.

   **Fix:** separate dialog identity from request identity and put task ownership inside the loading state.

   ```rust
   enum FetchState {
       Idle,
       Loading {
           request: RequestId,
           task: AbortOnDropHandle<()>,
       },
       Failed(String),
   }

   enum TerminalState {
       Closed,
       Open {
           job: ShellJob,
           fetch: FetchState,
       },
   }
   ```

6. **Medium · illegal-state — Marketplace paints one selection but activates another index.**  
   [crates/editor-marketplace/src/marketplace/mod.rs:204](crates/editor-marketplace/src/marketplace/mod.rs:204), [mod.rs:292](crates/editor-marketplace/src/marketplace/mod.rs:292).

   Selection is a stored `usize` into recomputed results. Rendering clamps it to the new result count; Enter uses the original index. Catalog refreshes do not reconcile selection.

   **Failure:** a catalog change removes a matching item while selection is near the end. The last remaining item appears selected, but Enter finds no item at the stale index and silently does nothing.

   **Fix:** retain selection by item identity and reconcile it whenever results change. Painting and activation must use the same resolved selection.

   ```rust
   enum ResultSelection {
       Empty,
       Selected {
           before: Vec<Item>,
           item: Item,
           after: Vec<Item>,
       },
   }
   ```

7. **Medium · correctness — An MCP server can hide an unrelated marketplace skill.**  
   [crates/editor-marketplace/src/marketplace/mod.rs:185](crates/editor-marketplace/src/marketplace/mod.rs:185).

   Remote listings are suppressed whenever **any** local item has the same display name. That includes MCP servers.

   **Failure:** an MCP server named `github` causes a remotely returned skill named `github` to disappear, even when the Skills filter is selected. Suppression happens before kind filtering.

   **Fix:** deduplicate against installed skills only, using the intended skill identity. Names from different item kinds must not share one identity namespace.

8. **Medium · algorithm — Thread mention parsing repeatedly validates and scans the entire title catalog.**  
   [crates/artisan-mentions/src/lib.rs:138](crates/artisan-mentions/src/lib.rs:138).

   Every eligible `@` scans every title and repeats `valid_thread_title()`, including its newline scan. Composer tokenization invokes this parser at `editor-composer/src/composer/mentions.rs:467`.

   **Complexity:** `O(B + M × T × L)` in the worst case, where B is message bytes, M eligible sigils, T titles, and L title length. A catalog of 5,000 titles averaging 100 bytes and a draft with 50 candidates entails roughly 25 million title bytes examined per tokenization.

   **Failure:** typing into a mention-heavy draft becomes increasingly expensive as thread history grows.

   **Fix:** validate titles when the catalog changes and compile a reusable prefix index or trie that preserves longest-match and terminator rules.

9. **Medium · algorithm — Every thread update sorts and prunes the complete retained history.**  
   [crates/editor-core/src/core/host/pages.rs:355](crates/editor-core/src/core/host/pages.rs:355).

   `remember_thread()` sorts the whole page and then rebuilds retention membership and scans related maps. This is separate from the already-recorded quadratic page-merging issue.

   **Complexity:** sorting is `O(N log N)` worst case, with additional `O(N)` pruning per thread update. Nearly sorted input can reduce sorting work, but the full retention scans remain.

   **Failure:** with 5,000 retained threads and 20 thread updates per second, bookkeeping repeatedly traverses the full history on the UI path.

   **Fix:** maintain activity ordering incrementally and prune when retention membership changes, rather than after every metadata update.

10. **Low · algorithm — Sidebar pulse cleanup performs nested full-list searches.**  
    [crates/editor-sidebar/src/sidebar/controller.rs:380](crates/editor-sidebar/src/sidebar/controller.rs:380).

    Each retained pulse searches Inbox and History using `.any()`.

    **Complexity:** `O(P × N)` per sync, for P pulse entries and N displayed threads. Fifty retained pulses across 5,000 rows can require 250,000 comparisons per sync.

    **Failure:** concurrent work amplifies sidebar refresh costs despite row virtualization.

    **Fix:** collect current working IDs once into a set, then retain pulses by membership: `O(N + P)`.

11. **Low · data-structure — Deletion and stopping metadata have no complete retirement policy.**  
    [crates/editor-core/src/core/host/pages.rs:376](crates/editor-core/src/core/host/pages.rs:376), [crates/editor-core/src/core/host.rs:669](crates/editor-core/src/core/host.rs:669).

    Every deletion enters `removed`; only a snapshot reset clears that set. Ordinary resumable reconnects do not. Thread deletion also leaves `stopping` entries behind, and observing a different run through `Held::Other` does not retire them.

    **Failure:** a long-lived host connection accumulates deleted IDs and obsolete cancellation records despite thread-cache pruning.

    **Fix:** retire cancellation state with its run. Keep deletion protection only while older outstanding requests could return the deleted entity, then discard it safely.

12. **Low · correctness — An EOF request can return an arbitrary earlier cached page.**  
    [crates/editor-core/src/core/range_window.rs:180](crates/editor-core/src/core/range_window.rs:180).

    Cache matching accepts `offset == page.total()` for every cached page. It does not require that the page itself reaches EOF.

    **Failure:** a 1,000-byte source with cached page `0..100` receives `request(1000)`. That earlier page matches, becomes Ready, and cancels any outstanding read.

    **Fix:** require `offset == page.end() && page.end() == page.total()` for the EOF exception, or represent End separately.

    This is a verified public-API defect. A normal UI trigger remains **suspected**: the source-view Next button disables at EOF.

13. **Low · illegal-state — `ThreadPage` does not encode its loading lifecycle.**  
    [crates/editor-core/src/core/host/pages.rs:17](crates/editor-core/src/core/host/pages.rs:17).

    `loading`, `loaded`, `error`, task ownership, and generation are independent. The type permits Loading without a task and simultaneous loading/error states.

    **Failure scenario:** a transition leaves `loading` set after dropping its task; `load_threads()` then refuses further work indefinitely. No current production transition producing that combination was established; the representable invalid state is verified.

    **Fix:** retain prior rows independently, but make request lifecycle atomic.

    ```rust
    enum PageLoad {
        Unloaded,
        Loading {
            request: PageRequest,
            task: AbortOnDropHandle<()>,
        },
        Ready,
        Failed(String),
    }

    struct ThreadPage {
        rows: ThreadRows,
        load: PageLoad,
    }
    ```

14. **Low · bail-out — Internal APIs accept domains broader than their implementations support.**  
    [crates/editor-settings/src/settings/view.rs:516](crates/editor-settings/src/settings/view.rs:516), [crates/editor-core/src/core/agent_calls.rs:60](crates/editor-core/src/core/agent_calls.rs:60), [crates/editor-settings/src/settings/controller.rs:89](crates/editor-settings/src/settings/controller.rs:89).

    `thread_section()` accepts all settings pages and an optionally available host, then uses two `unreachable!()` paths. `run_call()` accepts wire variants that another function must have removed. Controllers unwrap an optional mailbox even though their particular delivery constructor always owns one.

    **Failure scenario:** a routing or constructor refactor compiles successfully but reaches a process-aborting invariant check. Current callers avoid these cases; these are verified type-contract defects, not demonstrated current crashes.

    **Fix:** narrow the inputs and distinguish owned-mailbox delivery.

    ```rust
    enum ThreadSection {
        Automation,
        Assistance,
    }

    struct SectionInput<'a> {
        host: &'a HostData,
        section: ThreadSection,
    }

    enum ClassifiedCall {
        Immediate(ImmediateCall),
        McpSignIn(McpSignInCall),
        RejectedHostAdd,
    }

    struct OwnedDelivery<T> {
        mailbox: Mailbox<()>,
        queue: DeliveryQueue<T>,
    }
    ```

    These shapes remove the unsupported match arms and mailbox unwraps.

The ten highest-priority type redesigns follow. This table identifies structural priorities; it is not ten additional findings.

| Type | Location at `10f84a7` | What it permits | Proposed shape |
|---|---|---|---|
| `LeafRecord` / `IndexedEvent` | `crates/artisan-markdown-index/src/lib.rs:115` | Untrusted events presented as replayable, with unchecked ranges | Raw DTO → source-bound `CheckedLeaf`; restricted structural-event enum |
| `Core` navigation | `crates/editor-core/src/core/store.rs:91` | Workspace host and hostless-page target derive from different authorities | Explicit host-scoped routes; persistence separate from current selection |
| `HostData` | `crates/editor-core/src/core/host.rs:337` | Direct writes bypass cross-map invariants | Private cohesive owners and reducers; read-only projections. File: **1,519 lines**; `drain`: **269 lines** |
| Runs plus `stopping` | `crates/editor-core/src/core/host.rs:348` | Pending intent encoded as fake terminal server state | `RunView::Following / Stopping`, owned with thread lifecycle |
| `ThreadPage` | `crates/editor-core/src/core/host/pages.rs:17` | Independent lifecycle flags, error and task | Retained rows plus `PageLoad` |
| `ThreadSummary` / `ProjectionLoad` | `crates/editor-core/src/core/host.rs:159` | Loading independent of task; payload freshness mixed with request status | Revisioned value plus request-owned `Idle / Loading / Failed` |
| `TerminalDialog` / `Job` | `crates/editor-inspector/src/inspector/dialog.rs:13` | Closed/loading contradictions; generic item mixed with shell state | `Closed / Open { ShellJob, FetchState }`; fresh request identity |
| `PromptForm` | `crates/editor-settings/src/settings/prompt.rs:4` | Host, baseline, version, loaded, newer and pending tracked independently | Host-scoped `Loading / Editing / Saving / Conflict / Failed`, preserving draft and baseline |
| `ExtensionsForm` | `crates/editor-settings/src/settings/extensions.rs:81` | Nested editing options and saving state independent of submitted form | `Closed / Creating / Editing / Saving`, with owned submission and edit revision |
| Audio `Inner` | `crates/editor-audio/src/state.rs:33` | Playback status independent of track, decoded audio, stream and output availability | `Idle / Loading / OpeningOutput / Ready / Failed`; playback mode inside Ready |

Both independent reviewers confirmed findings **1, 2, 4, 5 and 12**. Opus also identified the lifecycle weaknesses in **11 and 13**. I dropped its proposed cross-host Editor-call authorization finding because the reviewed interface deliberately exposes global host-management operations; the evidence did not establish a caller-isolation contract.

Independent checks used the [review skill](/home/sander/.local/share/artisan-street/forge/data/engines/codex/home/.codex/skills/review/SKILL.md). Scores cover the assigned crates together, including their existing recorded defects; they are not runtime-test results.

| Score | Lead review | Opus 5.5 High | GPT 6.1 Sol High |
|---|---:|---:|---:|
| Correctness | 4/10 | 5/10 | 4/10 |
| State handling / illegal-state discipline | 3/10 | 3/10 | 3/10 |
| Data structures and algorithms | 5/10 | 5/10 | 5/10 |
| Code quality and architecture | 4/10 | 4/10 | 4/10 |
| **Overall** | **4/10** | **4/10** | **4/10** |

### 15.4 forge-runs, forge-store, forge-http, forge

Audited **`10f84a7ada6519109564cd433aed23c4e36676fa`**, using committed source. No files changed; no builds or tests ran. Findings were checked against `findings.md`; previously recorded defects are excluded.

I used the [review skill](/home/sander/.local/share/artisan-street/forge/data/engines/codex/home/.codex/skills/review/SKILL.md). Attribution below: **R** = my inspection, **O** = Opus 5.5 High, **S** = GPT 6.1 Sol High. Rust sketches show proposed domain shapes, not complete implementations.

1. **High — Stop can erase delivery acknowledgements and cause duplicate input delivery.**  
   **[crates/forge-store/src/items.rs:234](crates/forge-store/src/items.rs:234) — illegal-state, correctness. R/O/S.**

   Cancellation changes pending messages to `Cancelled`. Teardown continues applying queued `InputAccepted` events, but `accept_inputs` updates only messages still marked `Running`.

   **Failure:** the engine receives an input; Stop commits before its acknowledgement reaches the writer; the acknowledgement is ignored. Continue restores the supposedly undelivered input and sends it again. Work already performed from that prompt can be repeated.

   **Fix:** distinguish queued inputs from attempts awaiting acceptance. Stop must preserve the latter until teardown resolves their delivery.

   ```rust
   enum InputDelivery {
       Queued,
       Attempting { run: RunId, phase: AttemptPhase },
       Accepted { run: RunId },
       CancelledBeforeSend,
   }

   enum AttemptPhase {
       Running,
       Stopping,
   }
   ```

2. **High — Failed, unaccepted agent inputs are classified as working indefinitely.**  
   **[crates/forge-runs/src/agents.rs:542](crates/forge-runs/src/agents.rs:542) — illegal-state, correctness. R/O/S.**

   `child_state` returns `Working` whenever `pending_inputs` is nonempty. The scheduler separately excludes inputs covered by the latest failed/refused run’s cutoff. These two readers disagree about whether work can run.

   **Failure:** preparation fails before input acceptance—for example, the working directory becomes unavailable. The prompt remains `Running`, but dispatch deliberately blocks it. Agent status remains `Working`, waits time out, and background failure delivery never occurs.

   **Fix:** persist eligibility explicitly and use the same representation for scheduling and status.

   ```rust
   enum InputState {
       Ready,
       Attempting { run: RunId },
       Accepted { run: RunId },
       Blocked { failed_run: RunId, cause: String },
       Cancelled,
   }
   ```

   This adds a different failure path from the partial-agent-creation defect already recorded in §4.2.

3. **High — Generation ownership remains live while agent completion is evaluated.**  
   **[crates/forge-runs/src/lib.rs:923](crates/forge-runs/src/lib.rs:923) — correctness. R/O.**

   The run task retains `_leases` until it exits. Before releasing them, it removes the local live entry and calls `agent_run_ended`. That callback reaches `child_state`, whose `is_live` check still finds the registry owner.

   **Failure:** a successfully completed agent in generation mode is classified as `Working` during its completion callback. Its background result is not delivered. A later explicit status read can repair the displayed state, but completion does not arrange another delivery attempt.

   **Fix:** release execution ownership before evaluating completion. Give completion processing an already-settled run value; registry ownership must not stand in for unfinished execution.

4. **High — Live-snapshot deduplication can swallow a Reset that represents missing durable changes.**  
   **[crates/forge-http/src/events.rs:328](crates/forge-http/src/events.rs:328) — illegal-state, correctness. R/O.**

   Every unsequenced envelope is filtered using the live-snapshot revision, including `Reset`. However, a live snapshot reconstructs streaming state, not durable changes omitted by synchronization.

   **Failure:** SSE reads its backlog; a peer commits enough changes to overflow synchronization’s row/byte allowance; synchronization advances its cursor and publishes `Reset`; SSE then captures a newer live revision. The queued Reset fails the revision comparison and disappears. The omitted durable changes are never sent, while subsequent events continue normally.

   **Fix:** separate durable changes, snapshot-covered updates, and control messages. Reset must bypass live-snapshot deduplication.

   ```rust
   enum Envelope {
       Stored { seq: Seq, change: StoredChange },
       Live { revision: NonZeroU64, event: LiveEvent },
       Control(ControlEvent),
   }

   enum ControlEvent {
       Hello(Hello),
       Reset { through: Seq },
   }
   ```

5. **Medium — Answers are routed to the thread’s current run rather than the question’s owning run.**  
   **[crates/forge-runs/src/lib.rs:390](crates/forge-runs/src/lib.rs:390) — illegal-state, correctness. R/O/S.**

   `LiveRun` has no run ID. `answer` selects an inbox using only `thread_id`; the durable fallback for nonblocking questions runs only when no inbox exists.

   **Failure:** run A leaves a nonblocking question unanswered, then run B starts. Answering A’s question sends A’s native key to B’s decoder and reports success. An unknown key is ignored, leaving the question unanswered.

   **Fix:** resolve an explicit destination and compare native answers against the owning run.

   ```rust
   struct LiveRun {
       id: RunId,
       phase: LivePhase,
   }

   enum AnswerRoute {
       Native { run: RunId, key: NativeRequestId },
       Deferred { question: ItemId },
   }
   ```

   Old deferred questions must use durable answer insertion even while another run is active.

6. **Medium — Transcript expansion multiplies the page budget across live work spans.**  
   **[crates/forge-store/src/read.rs:533](crates/forge-store/src/read.rs:533) — algorithm, correctness. R/O/S.**

   Ordinary rows receive one page budget. Each live work span then independently receives another allowance of up to 100 items and 8 MiB. All results are appended without an aggregate response check.

   **Failure:** interleaved questions/messages split a long live run into several spans. Individually admissible expansions can produce a response exceeding the Editor’s 32 MiB response limit, making conversation refresh fail repeatedly.

   **Complexity:** expansion admits **O(100s)** items and approximately **O(8 MiB × s)** item bodies for `s` live spans. Ten spans can admit roughly 80 MiB before other response fields.

   **Fix:** share one item/byte budget across rows, expanded work, run metadata and summaries. Preserve omitted work as pageable spans.

7. **Medium — Global cache reset does not invalidate reads for previously unknown threads.**  
   **[crates/forge-store/src/hot.rs:154](crates/forge-store/src/hot.rs:154) — correctness. R/O/S.**

   An absent thread returns `Miss(Version(0))` without registering an entry. Reset increments versions only for existing entries. `hold` can subsequently create that absent thread at version zero and accept the old read.

   **Failure:** an uncached read captures old data, synchronization resets the cache after a backlog overflow, then the old read completes. Its stale page is cached and remains available until another invalidation affects that thread.

   **Fix:** every read token must include a global reset epoch.

   ```rust
   struct Version {
       reset_epoch: u64,
       thread_revision: u64,
   }
   ```

   Compare both fields when admitting a completed read. This is separate from the cache metadata leak already recorded in §3.

8. **Medium — Session selection and launch transport are resolved independently.**  
   **[crates/forge-runs/src/run.rs:656](crates/forge-runs/src/run.rs:656) — illegal-state, correctness. R/O.**

   `drive` reads Claude transport when selecting a resumable session. `prepare` reads it again after intervening awaits. Moreover, `claude_transport` converts a database error into the Code transport.

   **Failure:** changing transport during preparation can pair a session selected for one transport with another launch transport. A transient settings-read error can discard a usable chat session and trigger a fresh history handoff.

   **Fix:** resolve one typed launch/session choice, propagate read failures, and pass that choice through preparation.

   ```rust
   enum LaunchSession {
       ClaudeCode { resume: Option<CodeSessionId> },
       ClaudeAi { resume: Option<ChatSessionId> },
       Codex { resume: Option<CodexThreadId> },
       OpenCode { resume: Option<OpenCodeSessionId> },
   }
   ```

   This also removes empty-string session sentinels from the domain.

9. **Medium — A message’s steer role is guessed before delivery.**  
   **[crates/forge-runs/src/lib.rs:314](crates/forge-runs/src/lib.rs:314) — illegal-state, correctness. R/O.**

   Acceptance stores `steer = is_live()`. If that run is winding down, its inbox send can fail silently and the message becomes the next run’s initial prompt.

   **Failure:** the next run accepts that initial prompt, giving it a `run_id`, but its stale `steer: true` remains. `folds_into_work` hides it inside collapsed work, and the outline query excludes it.

   **Fix:** record the role when delivery occurs.

   ```rust
   enum DeliveredAs {
       Initial { run: RunId },
       Steer { run: RunId },
   }
   ```

   Queued input should have no delivered role.

10. **Medium — Stop intent is lost when cleanup fails; automatic retry remains possible.**  
    **[crates/forge-runs/src/run.rs:460](crates/forge-runs/src/run.rs:460) — illegal-state, correctness. R/O.**

    Cleanup can replace a cancelled outcome with the adapter’s complete failed outcome, including retry metadata. `finish` then considers parking any failed outcome. Stop already removed continuation requests earlier.

    **Failure:** an adapter returning a retryable failure while stopping can cause the explicitly stopped turn to resume automatically. This production adapter interleaving is **suspected**; the permissive controller path is verified. Independently, tested cleanup failures change status to `Failed`, causing Continue to reject the user-stopped turn.

    **Fix:** preserve stop intent separately from cleanup diagnostics.

    ```rust
    enum Ending {
        Finished,
        EngineFailed { error: String, retry: Option<Retry> },
        Stopped { by: StopCause, cleanup: Cleanup },
    }

    enum Cleanup {
        Confirmed,
        Failed(String),
        Unconfirmed,
    }
    ```

    Only `EngineFailed` may park. A user-stopped turn remains continuable even when cleanup reports failure.

11. **Medium — Retention repeatedly rebuilds the entire thread family graph inside writer transactions.**  
    **[crates/forge-store/src/retention.rs:25](crates/forge-store/src/retention.rs:25) — algorithm. R/O.**

    Every deletion batch reloads all candidate threads and reconstructs families. `family_root` independently walks each thread’s ancestry. The Hub repeats this process for batches targeting eight threads.

    **Failure:** enabling retention on a large old installation repeatedly occupies the sole writer, delaying messages and run updates.

    **Complexity:** unrelated removable threads require approximately **O(n² / 8)** candidate visits across cleanup. A handoff chain of length `n` also causes **O(n²)** ancestry traversal in one reconstruction. At 10,000 removable singleton threads, cleanup performs about 1,250 full candidate scans.

    **Fix:** construct indexed family membership once per collection pass, memoize roots, and delete bounded batches with transactional eligibility rechecks.

12. **Medium — Run bookkeeping grows with every historical adapter key.**  
    **[crates/forge-runs/src/run.rs:779](crates/forge-runs/src/run.rs:779) — data-structure, algorithm. R/S.**

    `Work.by_key` retains entries until the entire run ends. Terminal processing does not retire them. `hidden` also grows monotonically. Streaming targets allocate tracked identities before the Hub’s preview admission limit is applied.

    **Failure:** a long-lived run containing tens of thousands of completed calls retains their keys and bookkeeping. Preview rejection does not prevent that growth.

    **Complexity:** memory and final settlement scanning are **O(total historical keys)** rather than active work.

    **Fix:** keep active entries and a bounded late-update index. Resolve retired identities through durable native-ID lookup. Admit preview targets before allocating new identities.

13. **Medium — Background terminal previews can starve foreground streaming text completely.**  
    **[crates/forge-runs/src/lib.rs:1078](crates/forge-runs/src/lib.rs:1078) — data-structure, correctness. R/O.**

    The per-target allowance is computed from the run’s remaining shared byte budget. Existing targets keep their allocations.

    **Failure:** eight background shells each retain 256 KiB, exhausting the 2 MiB run allowance. A new assistant-text target receives a zero-byte allowance; its streaming chunks publish empty text until durable output arrives.

    **Fix:** reserve capacity for foreground text/reasoning and evict or shrink terminal previews when admitting new targets. Budget exhaustion needs an explicit admission/eviction policy.

14. **Medium — An unresolved parent can become a persisted dangling item reference.**  
    **[crates/forge-runs/src/run.rs:934](crates/forge-runs/src/run.rs:934) — illegal-state, correctness. R/O.**

    When native-parent lookup finds nothing, `parent_of` allocates an ID in `Work` without persisting a parent. The child can immediately be stored with that ID as `parent_id`.

    **Failure:** if the corresponding parent event never arrives, the child is absent from root transcript queries and has no persisted agent through which users can reach it. Occurrence with current provider traces is **suspected**; the accepted dangling-reference path is verified.

    **Fix:** distinguish reserved identities from persisted parent capabilities. Buffer unresolved children within a bound or explicitly classify them as root items.

    ```rust
    struct StoredAgent {
        id: ItemId,
    }

    enum ParentLink {
        Root,
        Agent(StoredAgent),
    }
    ```

    Only the store’s validated persistence path should construct `StoredAgent`.

15. **Low — Pairing’s hash and certificate invariants are absent from its argument types.**  
    **[crates/forge-store/src/pairing.rs:75](crates/forge-store/src/pairing.rs:75) — illegal-state. R/O.**

    `pair_device` accepts a `String` called `code`, although it expects a SHA-256 hash. Certificate identity is another unrestricted string. Store tests successfully pair arbitrary strings such as `"fresh"` and `"key-a"`.

    **Failure:** a caller can save a raw invitation code through `add_pair_code`; HTTP pairing hashes the submitted code and can never consume that invitation. The store reports successful insertion despite the unusable boundary state.

    **Fix:** make raw invitation codes, hashes and certificate identities distinct.

    ```rust
    struct PairCode(String);
    struct PairCodeHash([u8; 32]);
    struct ClientCertificateId(Vec<u8>);

    struct PairingAttempt {
        code: PairCodeHash,
        certificate: ClientCertificateId,
        name: DeviceName,
    }
    ```

    The current HTTP route hashes correctly; this is an internal API invariant defect, not a demonstrated authentication bypass.

16. **Low — Thread paging carries malformed cursor combinations into storage.**  
    **[crates/forge-store/src/read.rs:964](crates/forge-store/src/read.rs:964) — illegal-state, bail-out. R/S.**

    `ThreadsQuery` contains four independent optional cursor fields. Storage must reject half-cursors and simultaneous before/after cursors, then reconstruct the valid combinations using default sequence and empty-string sentinels.

    **Failure:** malformed external cursor combinations reach domain/storage code, where every reader must remember those invariants. Current requests are rejected rather than corrupting data; the defect is boundary normalization.

    **Fix:** retain a raw query DTO only at HTTP decoding, then construct:

    ```rust
    enum ThreadWindow {
        Newest,
        Before(ThreadCursor),
        After(ThreadCursor),
    }

    struct ThreadPageRequest {
        window: ThreadWindow,
        limit: PageLimit,
        filter: ThreadFilter,
    }
    ```

17. **Low — Event normalization retains the variant it claims to eliminate.**  
    **[crates/forge-runs/src/run.rs:1241](crates/forge-runs/src/run.rs:1241) — bail-out, architecture. R/O.**

    `apply` converts `EngineEvent::Image` into an item event but keeps the result typed as `EngineEvent`. Its dispatch consequently needs `unreachable!("normalized before dispatch")`.

    The branch is unreachable under today’s control flow; no current panic path was found. The normalization invariant nevertheless exists only in control flow and can be broken when dispatch is reused or reorganized.

    **Fix:** normalize into a type that cannot contain file-backed images.

    ```rust
    enum AdmittedEvent {
        Item(ItemUpdate),
        Status(StatusUpdate),
        Live(LiveUpdate),
        Run(RunUpdate),
        Commands(CommandCatalog),
        InputAccepted(ItemId),
    }
    ```

    This boundary is especially useful because `apply` spans **289 lines**, `execute` **238 lines**, and their module **1,339 lines**. Both large functions suppress `too_many_lines`.

The **ten types to redesign first** are below. This prioritization includes previously recorded design debt without repeating it as new findings.

| Type | File:line at `10f84a7` | What it permits | Proposed shape |
|---|---|---|---|
| User-input lifecycle in `Item` | `crates/artisan-api/src/item.rs:29` | Accepted, blocked and undelivered inputs share unrelated status/run fields | Explicit queued/attempting/accepted/blocked/cancelled variants |
| `Run` | `crates/artisan-api/src/thread.rs:63` | Status, end time, error, session and launch configuration disagree independently | Immutable identity and resolved launch plus payload-bearing lifecycle variants |
| `LiveRun` | `crates/forge-runs/src/lib.rs:100` | Inbox without owning run identity or explicit stopping phase | Run identity plus preparing/running/stopping states |
| `Tracked` | `crates/forge-runs/src/run.rs:850` | Allocated identity looks like a usable persisted parent; persistence and status vary independently | Reserved identity versus persisted typed item handle |
| `Envelope` | `crates/artisan-api/src/event.rs:10` | Durable events without sequence numbers, live events with durable sequences, control events subject to live deduplication | Stored/live/control variants |
| `ItemKind::Agent` | `crates/artisan-api/src/item.rs:107` | Independent engine/thread options permit incomplete Forge-agent identity; consumers use different discriminators | `NativeAgent` versus `ForgeAgent { engine, thread, … }` |
| `Park` | `crates/forge-store/src/lib.rs:80` | Requested/restart continuation reasons can be represented as delayed provider retries | Overload and usage-limit retry variants with validated deadlines |
| `Tx` | `crates/forge-store/src/writer.rs:83` | Sequence allocation and event insertion order are independent | Private ordered event batch assigning sequence numbers at emission |
| `ThreadsQuery` | `crates/artisan-api/src/request.rs:169` | Half-cursors and conflicting paging directions | Raw boundary DTO decoded into `ThreadWindow` |
| `TunnelAuth` | `crates/forge-http/src/routes.rs:46` | Any internal caller can wrap ordinary claims as “verified” authorization | Private verified-grant capability constructed only by successful verification |

Two reviewer hypotheses were dropped: ordinary writer/poller publication races do not apply because joined stores disable direct writer broadcasts; changing a stopped run to `Failed` is deliberately tested, so finding 10 concerns lost stop intent and retry eligibility.

My scores for the requested crates:

| Crate | Correctness | State handling | Data structures and algorithms | Code quality and architecture | Overall |
|---|---:|---:|---:|---:|---:|
| `forge-runs` | 3/10 | 2/10 | 5/10 | 4/10 | **3/10** |
| `forge-store` | 4/10 | 3/10 | 5/10 | 5/10 | **4/10** |
| `forge-http` | 4/10 | 4/10 | 5/10 | 5/10 | **4/10** |
| `forge` | 8/10 | 6/10 | 8/10 | 7/10 | **7/10** |

The thin `forge` executable has substantially less lifecycle responsibility; its score does not imply the services it starts are sound.

| Scope score | My assessment | Opus 5.5 High | GPT 6.1 Sol High |
|---|---:|---:|---:|
| Correctness | 4/10 | 5/10 | 4/10 |
| State handling | 3/10 | 3/10 | 3/10 |
| Data structures and algorithms | 5/10 | 5/10 | 5/10 |
| Code quality and architecture | 4/10 | 4/10 | 4/10 |
| **Overall** | **4/10** | **4/10** | **4/10** |

### 15.5 forge-engines, ae and the shared crates

Read-only static audit of `10f84a7ada6519109564cd433aed23c4e36676fa`. No edits, builds, or tests. Failure scenarios below are traced through source, not exercised against live engines.

Codex has substantially better notification normalization than the other adapters, but still accepts invalid terminal states. Claude, OpenCode, and claude.ai retain raw JSON interpretation inside their state machines.

Existing findings—including invalid `Outcome` combinations (§6.1), decoder retention (§3.1), and browser ownership (§6.2)—are excluded from the numbered findings. They remain relevant to the redesign table and scores.

1. **High — claude.ai discards the saved continuation before attempting a resumed turn.**  
   **Category:** correctness.  
   **Location:** [crates/forge-engines/src/claudeai/mod.rs:292](crates/forge-engines/src/claudeai/mod.rs:292).

   A resume parses both conversation and saved leaf, but immediately emits a `Session` containing only the conversation. `forge-runs/src/run.rs:995` persists that replacement. The leaf is restored only after a completion advances it.

   **Failure:** resume `claude.ai:conversation/last-answer`, then encounter an upload failure, HTTP refusal, cancellation, or crash before the leaf update. The next resume has no parent and can start from the conversation root, losing the preceding context.

   **Fix:** retain the complete existing continuation until a new one is confirmed. Distinguish conversation creation from continuation advancement:

   ```rust
   enum ClaudeAiContinuation {
       New { conversation: ConversationId },
       Saved { conversation: ConversationId, leaf: MessageId },
   }
   ```

2. **High — claude.ai acknowledges image-bearing steers after dropping their images.**  
   **Category:** correctness.  
   **Location:** [crates/forge-engines/src/claudeai/mod.rs:166](crates/forge-engines/src/claudeai/mod.rs:166).

   `Inbox::take` reduces `UserInput` to `Steer { id, text }`. Its image paths disappear. Subsequent requests explicitly clear `request.files`, while retaining those message IDs for `InputAccepted`.

   **Failure:** send a screenshot with “fix this” while Claude is working. Forge marks the message delivered, but the model receives only its text.

   **Fix:** retain complete `UserInput` values in the inbox; upload and attach their images before acknowledging delivery.

3. **High — malformed Claude permission requests erase their error and leave the CLI waiting.**  
   **Category:** bail-out.  
   **Location:** [crates/forge-engines/src/claude/decoder.rs:1153](crates/forge-engines/src/claude/decoder.rs:1153).

   `control_request` calls `tool_use` and blindly removes its last event with `out.pop()`. When input validation fails, that event is the error, and no tool was inserted. The subsequent missing-tool branch returns without a control response.

   **Failure:** a `can_use_tool` request for `Bash` omits `command`. The error disappears, no approval appears, and Claude waits indefinitely for its unanswered request.

   **Fix:** return construction success or failure explicitly; every identified request must receive a response.

   ```rust
   enum PreparedPermission {
       Ready(PendingPermission),
       Rejected { request: RequestId, reason: ProtocolError },
   }
   ```

4. **High — Claude can create an approval that the shared API cannot answer.**  
   **Category:** illegal-state.  
   **Location:** [crates/forge-engines/src/claude/decoder.rs:1176](crates/forge-engines/src/claude/decoder.rs:1176).

   The decoder attaches approval data only to `Tool` and `Plan`, but unconditionally changes every constructed kind to `AwaitingUser`. `Task` and `Agent` calls produce `ItemKind::Agent`, which has no approval field. `Hub::answer` explicitly rejects approval answers for agents at `forge-runs/src/lib.rs:367`.

   **Failure:** Claude asks permission to launch a valid agent. Its item becomes awaiting-user, but neither ordinary approval submission nor Full-access approval observation can resolve it.

   **Fix:** make awaiting approval carry both the operation and its approval. Agent spawning must be an approvable operation.

   ```rust
   enum OperationState {
       AwaitingApproval {
           operation: ApprovableOperation,
           approval: PendingApproval,
       },
       Running(RunningOperation),
       Finished(FinishedOperation),
   }
   ```

5. **High — stopping a run does not stop its computer-input worker.**  
   **Category:** correctness.  
   **Location:** [crates/forge-computer/src/lib.rs:188](crates/forge-computer/src/lib.rs:188).

   Computer execution runs inside `spawn_blocking`. Dropping the awaiting future does not stop that worker. `Batch::check` observes pause state, physical activity, display changes, and elapsed time; it receives no requesting-run cancellation.

   **Failure:** stop a thread during a batch containing waits followed by clicks or typing. The worker can continue issuing input until its own checks terminate it.

   **Fix:** bind each execution to its requesting run’s cancellation and check it before every input and during waits. This adds a distinct cancellation defect beyond the already-recorded setting-disable problem in §6.2, finding 38.

6. **High — OpenCode resolves choices through labels and can send the wrong value.**  
   **Category:** data-structure.  
   **Location:** [crates/forge-engines/src/opencode/decoder.rs:1269](crates/forge-engines/src/opencode/decoder.rs:1269).

   Questions receive distinct `option-N` IDs, but `native_answers` converts them to labels. `FormField.values` then maps labels to native values. Duplicate labels overwrite one another.

   **Failure:** two options both display “Production” but represent different deployments. Selecting the first opaque ID sends the second option’s native value.

   **Fix:** map opaque choice IDs directly to native values. Keep labels solely for presentation. The claude.ai question builder has a related identity defect at `claudeai/tools.rs:388`: it uses labels themselves as option IDs.

7. **Medium — rejected Codex steers reverse user-message ordering.**  
   **Category:** correctness.  
   **Location:** [crates/forge-engines/src/codex.rs:415](crates/forge-engines/src/codex.rs:415).

   Every rejected steer is inserted at position zero of `queued`.

   **Failure:** steers A and B are submitted in that order and rejected in that order near a turn boundary. The next turn receives B followed by A. Out-of-order responses make the resulting order depend on response timing.

   **Fix:** assign an input ordinal at admission and reconstruct the next turn by that ordinal, independently of RPC completion order.

8. **Medium — Codex treats `inProgress` and unknown completion statuses as success.**  
   **Category:** illegal-state.  
   **Location:** [crates/forge-engines/src/codex/protocol.rs:172](crates/forge-engines/src/codex/protocol.rs:172).

   `#[serde(other)] Ended` absorbs both `inProgress` and unknown statuses. `decoder.rs:569` maps it to `Outcome::done()`.

   **Failure:** a malformed `turn/completed` carrying `inProgress`, or a newly introduced unsuccessful terminal status, becomes a successful run.

   **Fix:** decode completed notifications into an exclusively terminal type. Reject unsupported statuses explicitly.

   ```rust
   enum CompletedTurn {
       Completed,
       Interrupted,
       Failed(CodexFailure),
   }
   ```

9. **Medium — claude.ai calls truncated and refused completions finished answers.**  
   **Category:** correctness.  
   **Location:** [crates/forge-engines/src/claudeai/decoder.rs:377](crates/forge-engines/src/claudeai/decoder.rs:377).

   Every non-null stop reason except `"tool_use"` sets `ended = true`. The driver subsequently returns `Outcome::done()`.

   **Failure:** `max_tokens` settles truncated prose as `Phase::Final` and reports success. A refusal stop reason likewise lacks its own outcome.

   **Fix:** normalize stop reasons into explicit completed, tool-waiting, truncated, refused, and unsupported cases; derive both item phase and run outcome from that result.

10. **Medium — claude.ai block state permits missing identities, colliding indices, and invalid tool input.**  
    **Category:** illegal-state.  
    **Location:** [crates/forge-engines/src/claudeai/decoder.rs:57](crates/forge-engines/src/claudeai/decoder.rs:57).

    Every block stores a string kind plus unrelated tool fields. Missing indices become zero; missing IDs and names become empty strings. Invalid accumulated tool JSON falls back to the stop event’s usually absent input.

    **Failure:** a malformed block start overwrites block zero, or an omitted tool ID creates an empty-key operation. Malformed arguments proceed inward as `Null` rather than a protocol failure.

    **Fix:** decode event identity first and construct variant-specific block state.

    ```rust
    enum Block {
        Text { key: TextKey, text: String },
        Thinking,
        Tool {
            id: ToolCallId,
            execution: ToolExecution,
            input: JsonAccumulator,
        },
    }

    enum ToolExecution {
        Caller(ToolName),
        Provider(ProviderToolName),
    }
    ```

11. **Medium — malformed claude.ai SSE messages disappear silently.**  
    **Category:** correctness.  
    **Location:** [crates/forge-engines/src/claudeai/transport.rs:474](crates/forge-engines/src/claudeai/transport.rs:474).

    JSON decoding uses `.ok().map(Ok)`, turning malformed payloads into absent stream events.

    **Failure:** a damaged text delta vanishes while a later `message_stop` still completes the run. A damaged tool-block stop loses the call and changes subsequent execution.

    **Fix:** propagate JSON errors with stream context. Unknown, well-formed event variants can be handled separately from unreadable protocol data.

12. **Medium — Claude refusal state leaks into later turns.**  
    **Category:** illegal-state.  
    **Location:** [crates/forge-engines/src/claude/decoder.rs:44](crates/forge-engines/src/claude/decoder.rs:44).

    `refused` becomes true on a refusal and is never reset. The same decoder spans steers, background follow-up turns, and replacement processes.

    **Failure:** a refusal is followed by a queued benign request in the same process. Its successful result still becomes `RunStatus::Refused`.

    **Fix:** make refusal belong to one turn and consume that state when the turn ends.

    ```rust
    enum TurnState {
        Idle,
        Active(ActiveTurn),
        Refused(Refusal),
        Finished(TerminalOutcome),
    }
    ```

13. **Medium — Claude’s fallback peer-message key overwrites unrelated messages.**  
    **Category:** correctness.  
    **Location:** [crates/forge-engines/src/claude/decoder.rs:1007](crates/forge-engines/src/claude/decoder.rs:1007).

    Without `msg_id` or frame UUID, the key is `peer:{self.tools.len()}`. Receiving a peer message does not change the tool count.

    **Failure:** two valid peer bodies without those optional identities arrive between tool calls. Both use the same key; the second replaces the first.

    **Fix:** require a native message identity or allocate a dedicated monotonically increasing peer-message identity.

14. **Medium — malformed Claude background snapshots mean “all background work ended.”**  
    **Category:** correctness.  
    **Location:** [crates/forge-engines/src/claude/decoder.rs:402](crates/forge-engines/src/claude/decoder.rs:402).

    Missing or incorrectly typed `tasks` becomes an empty slice. The decoder overwrites both background count and waiting-agent relationships.

    **Failure:** a malformed snapshot arrives while agents are working. The next result can close stdin, or the settling timer can close it, terminating background work.

    **Fix:** deserialize a complete background snapshot before replacing current state. An invalid snapshot must produce a protocol failure, not an empty authoritative snapshot.

15. **Medium — OpenCode loses completed text when its step subsequently fails.**  
    **Category:** correctness.  
    **Location:** [crates/forge-engines/src/opencode/decoder.rs:506](crates/forge-engines/src/opencode/decoder.rs:506).

    `session.text.ended` removes the key from `open_texts` and retains its final body in `step_texts`. Only `step_ended` emits that body durably. `step.failed` simply removes it.

    **Failure:** a text ends, then its step fails. The durable assistant item remains empty; its final body existed only in the live preview.

    **Fix:** persist completed text when its text event ends, then update phase separately. On step failure, settle retained text explicitly before removing it.

16. **Medium — OpenCode orphan agents have no successful completion transition.**  
    **Category:** correctness.  
    **Location:** [crates/forge-engines/src/opencode/decoder.rs:679](crates/forge-engines/src/opencode/decoder.rs:679).

    `bind_orphan` creates a running `agent:{session}` item without a corresponding tracked tool. Execution-success and interruption handlers only update the root. Child execution failure emits an error but does not settle this agent.

    **Failure:** a followed child without a claimed subagent call finishes successfully. Its agent remains running and can eventually be settled as cancelled during run cleanup.

    **Fix:** track every followed child with its owning agent and explicit execution state; apply child execution endings to that agent.

17. **Medium — OpenCode publishes a terminal shell status before its authoritative notification.**  
    **Category:** correctness.  
    **Location:** [crates/forge-engines/src/opencode/decoder.rs:899](crates/forge-engines/src/opencode/decoder.rs:899).

    `shell.exited` treats `"exited"` with no exit code as `Done`. The later synthetic notification can report a nonzero exit or timeout and change the decoder to `Failed`. Shared `Item::next_status` preserves the first terminal status for tools.

    **Failure:** an exit event omits the code; its later notification reports failure. Forge retains a successful status alongside the failure output.

    **Fix:** retain an exited-but-awaiting-result shell state and publish the terminal status only once its result is authoritative.

18. **Medium — OpenCode numeric forms accept invalid values and consume the pending request.**  
    **Category:** illegal-state.  
    **Location:** [crates/forge-engines/src/opencode/decoder.rs:1283](crates/forge-engines/src/opencode/decoder.rs:1283).

    `"integer"` and `"number"` collapse into `FieldKind::Number`. Parsing failures become JSON strings; fractional answers remain valid for integer fields. `answer_raw` removes the pending form and emits `Done` before the driver submits the request.

    **Failure:** answer an integer field with `1.5` or `tomorrow`. Forge marks it answered, then OpenCode can reject the submitted value.

    **Fix:** preserve native field types and validate answers before consuming pending state. Commit completion after successful submission.

    ```rust
    enum AnsweredField {
        Text(String),
        Boolean(bool),
        Integer(i64),
        Number(FiniteNumber),
        Choice(NativeChoice),
        Choices(Vec<NativeChoice>),
    }
    ```

19. **Medium — Claude stops observing cancellation after stdout EOF.**  
    **Category:** correctness.  
    **Location:** [crates/forge-engines/src/claude.rs:225](crates/forge-engines/src/claude.rs:225).

    EOF ends `drive`, after which `engine.wait().await` has no cancellation branch or exit deadline. Closing stdout does not imply process exit.

    **Failure:** the child closes stdout and remains alive. The run waits indefinitely without user intervention. After Stop, Forge’s outer 45-second deadline eventually drops the adapter and reports failure rather than obtaining normal cancellation.

    **Fix:** model output closure separately from process termination; keep cancellation and bounded termination active through reaping.

20. **Low — shared pagination types expose invalid cursor combinations to internal callers.**  
    **Category:** illegal-state.  
    **Location:** [crates/artisan-api/src/request.rs:169](crates/artisan-api/src/request.rs:169).

    `ThreadsQuery` has four independent optional cursor fields. Storage must check that each activity/ID pair is complete and that before/after are exclusive at `forge-store/src/read.rs:965`.

    **Failure:** an internal client constructs a before cursor with only its activity, or sets both directions. A value accepted by the shared type reaches a runtime rejection.

    **Fix:** normalize the wire query into one directional cursor before passing it inward.

    ```rust
    enum ThreadWindow {
        Latest,
        Before(ThreadCursor),
        After(ThreadCursor),
    }

    struct ThreadsQuery {
        window: ThreadWindow,
        limit: PageLimit,
        filter: ThreadFilter,
    }
    ```

21. **Low — computer steps allow empty drags and invalid coordinates past parsing.**  
    **Category:** illegal-state.  
    **Location:** [crates/forge-computer/src/steps.rs:39](crates/forge-computer/src/steps.rs:39).

    `Step::Drag` accepts an empty vector, but the public runner indexes `path[0]`. Public `At` also accepts negative and nonfinite values; `View::point` checks only upper bounds.

    **Failure:** a Rust caller constructs an empty drag and panics the runner, or passes a negative coordinate that maps outside the display. The JSON parser currently prevents these cases; the public domain types do not.

    **Fix:** make construction enforce the invariants.

    ```rust
    struct At {
        x: NonNegativeFinite,
        y: NonNegativeFinite,
    }

    enum Step {
        Drag { start: At, rest: Vec<At>, button: Button },
    }
    ```

22. **Low — frame dimensions and pixel storage can disagree, including through `crop`.**  
    **Category:** illegal-state.  
    **Location:** [crates/forge-computer/src/frame.rs:15](crates/forge-computer/src/frame.rs:15).

    All fields are independently writable. `crop` can produce zero-width frames, while `shrink` forces destination dimensions to at least one and indexes source pixels.

    **Failure:** cropping a valid 2×2 frame at `x = 2` with height 2 produces a 0×2 frame. Shrinking it to 1×1 attempts to read nonexistent pixels.

    **Fix:** use private, constructor-validated dimensions and storage; return absence for an empty crop.

    ```rust
    struct Frame {
        width: NonZeroU32,
        height: NonZeroU32,
        pixels: Box<[[u8; 3]]>,
    }
    ```

    Construction must verify the checked width × height product equals the pixel count.

23. **Low — tunnel reassembly repeatedly scans all received parts.**  
    **Category:** algorithm.  
    **Location:** [crates/artisan-api/src/reassembly.rs:85](crates/artisan-api/src/reassembly.rs:85).

    Every chunk checks `entry.parts.iter().any(Option::is_none)`. With sequential delivery, the received prefix grows on every call.

    **Failure/cost:** completion detection costs **O(p²)** for `p` parts. A supported 32 MiB transfer has 820 parts and performs approximately 337,000 slot inspections instead of 820.

    **Fix:** maintain a received-part count, incremented only after accepting a previously empty slot. Completion becomes O(1) per chunk, followed by one O(bytes) assembly.

The ten types most worth redesigning are:

| Type | File:line at `10f84a7` | What it permits | Proposed shape |
|---|---|---|---|
| `Outcome` | [harness.rs:171](crates/forge-engines/src/harness.rs:171) | Running outcomes, successful errors, unrelated retry metadata; already recorded in §6.1 | `Done \| Cancelled \| Refused(Reason) \| Failed { error, retry }` |
| Shared `Run` | [thread.rs:63](crates/artisan-api/src/thread.rs:63) | Independent status, error, and ending time; already recorded in §6.1 | Common identity plus `Running` or `Finished { outcome, ended_at }` |
| `Turn` | [harness.rs:18](crates/forge-engines/src/harness.rs:18) | Disagreement between `engine`, `config.engine`, adapter, and string session identity | Engine-specific validated turn variants with typed continuations |
| `Item` / `ItemKind` | [item.rs:29](crates/artisan-api/src/item.rs:29) | Awaiting-user items without answerable interactions; contradictory operation/result fields | Kind-specific lifecycle variants; pending interactions carry their required payload |
| `ClaudeDecoder` | [claude/decoder.rs:27](crates/forge-engines/src/claude/decoder.rs:27) | Turn-local refusal surviving later turns; disconnected pending/tool state | Separate session state, consumable turn state, and typed pending operations |
| `ClaudeAiDecoder` / `Block` | [claudeai/decoder.rs:29](crates/forge-engines/src/claudeai/decoder.rs:29) | Saved messages without UUIDs; tool blocks without identities; ended and failed states independently | Message lifecycle ADT plus variant-specific blocks and completion reasons |
| Codex driver `Run` | [codex.rs:147](crates/forge-engines/src/codex.rs:147) | Independently tracked starting, active-turn, cancellation, nudge, and outcome state | `Starting \| Active \| Stopping \| Finished`, carrying state-specific data; ordered input ledger |
| OpenCode `Decoder` | [opencode/decoder.rs:72](crates/forge-engines/src/opencode/decoder.rs:72) | Root/sessions duplication; orphan ownership gaps; independently latched execution flags/errors | Session map with explicit root/child roles and per-session execution states |
| `FormField` | [opencode/decoder.rs:164](crates/forge-engines/src/opencode/decoder.rs:164) | Label-keyed choice identity; integer/number collapse; arbitrary answers | Typed field variants with opaque-ID-to-native-value mappings and validated answers |
| `EngineConfig` | [engine.rs:57](crates/artisan-api/src/engine.rs:57) | Arbitrary JSON options and settings belonging to another engine | Separate wire DTO and validated `ClaudeConfig \| CodexConfig \| OpenCodeConfig` |

The architecture also concentrates too much responsibility in decoder modules: Claude’s decoder is **1,466 lines**, Codex’s **1,510**, and OpenCode’s **1,491**. Codex’s `item` dispatcher alone spans approximately **344 lines**. These modules combine protocol reading, interaction bookkeeping, lifecycle transitions, presentation decisions, and retained-content policy. Splitting validated wire events from pure domain transitions would address the recurring defects more effectively than adding further field checks.

Scores for the assigned crates collectively, including relevant findings already recorded in `findings.md`:

| Category | Score |
|---|---:|
| Correctness | **5/10** |
| State handling — illegal-state discipline | **3/10** |
| Data structures and algorithms | **6/10** |
| Code quality and architecture | **4/10** |
| **Overall** | **4/10** |
