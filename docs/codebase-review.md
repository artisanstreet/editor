# Full codebase review

Review started on 2026-10-04. Scope: the complete current working tree, including uncommitted changes. Baseline commit: `c0409e73a6e3bf97742be428b61307cf384ee927` on `master`. Existing user changes are preserved. This audit changes documentation only.

## Review status and continuity

This report is updated during the review so evidence and progress survive context compaction. Findings are provisional until their complete call paths and applicable safeguards have been checked. Static inventory coverage is distinct from manual semantic review and executed tests.

- Initial repository and instruction discovery complete. No applicable `AGENTS.md` found in the repository or ancestor directories.
- Rust workspace discovered with 19 members: native GPUI desktop application, Forge backend, SQLite database, QUIC transport, Cap'n Proto protocol, engine adapters, installer, and development/build tooling.
- Work streams: backend/domain/database/native-engine security and correctness; frontend/UI/catalog maintainability and UX; transport/protocol/installer/migrations/tracing/build/dependencies and final integration.
- The report now consolidates root, backend and frontend evidence, the ordered findings table, architecture, quick wins and regression/remediation plans. Historical checkpoints remain below as continuity records.

## Scope and verification limits

Source references are relative to the repository root and describe the working tree at review time. Generated code, vendored graphics/fonts, binary assets, and historical planning documents are inventoried separately from handwritten production code. Current dependency advisories and release information require external verification; version numbers alone do not prove a package is obsolete or vulnerable.

## Codebase map

The workspace has 19 Rust members. The complete named Rust/Python declaration inventory is [codebase-function-map.md](codebase-function-map.md): 1,360 files, 33,931 callables and 40,700 declarations. The inventory includes generated code and tests; it excludes anonymous closures and macro expansion. Four syntax-parser warnings are limitations of the inventory, not compiler errors. This is not a claim that every function branch was manually proved correct.

| Module | Responsibility / major boundaries |
| --- | --- |
| frontend | GPUI window, NativeApplication/NativeWorkspace, composer, model selection, subscriptions, presentation and local settings |
| ui | Shared controls, menus, text/markdown rendering, themes and motion |
| assets | Embedded fonts, icons and provenance records |
| backend | Forge runtime/listener, request handlers, durable run dispatcher, provider adapters and background services |
| database | SQLite repositories, commands/receipts, draft and attachment storage, outbox leases, observation ledger and projections |
| migrations | 25 ordered, transactionally applied native schema migrations |
| domain | Typed identifiers, validation, permission/configuration policies and conversation/run models |
| protocol | Cap'n Proto schemas/generated bindings, validated owned wire codecs and feature negotiation |
| transport | QUIC/TLS identity and framing, deadlines, handshake, session/correlation and dispatch |
| native_engine | Managed executable installation, trust, generations, launch certification, probes and usage readers |
| cli | ae setup/start/stop/open/doctor, credentials, hosts and systemd/Windows autostart |
| installer | Release signatures, staging, extraction, installation authority, activation and owned integrations |
| catalog | Checked-in harness/model policy and validated discovery/wire representations |
| build_info | Runtime release identity from signed staged resources |
| tracing | Optional bounded flight-recording and incident export |
| packaging | Payload ZIP/manifest generation and release signing tools |
| native_dev | Nix build/stage/install, host provisioning, WSL/Windows path/launch and startup receipts |
| capnp_codegen | Explicit Rust Cap'n Proto binding generator |
| screen_demo | Native visual fixture/demo tooling |

Primary entries: editor -> frontend::run -> native_application::run; forge -> directory helper or backend::run -> forge_runtime::run; ae -> CLI command dispatch; installer -> shared installation library; dev/forge-host -> native_dev orchestration; release-tool/payload-manifest-generator -> packaging; artisan-capnp-codegen -> schema generation.

Internal dependencies: frontend -> ui/assets/catalog/domain/cli/protocol/transport/tracing/build_info; backend -> database/domain/migrations/catalog/native_engine/protocol/transport/tracing; database -> domain/migrations; protocol -> domain/catalog; transport -> protocol/domain; native_engine -> domain; ui -> assets. The migration-to-database reverse dependency is test-only, not a demonstrated production dependency cycle.

Major external dependencies: pinned GPUI fork, Tokio, SeaORM/SQLx SQLite, Quinn/rustls, Cap'n Proto, reqwest, syntect and statig. Cargo.lock has 942 package records; optional/platform dependencies are not all active on the current host.

Major data flows:

1. Compose -> autosave/upload coordinator -> authenticated QUIC request -> transactional SQLite draft/attachment storage -> acknowledged revision -> durable message/outbox -> claim/bind/authorize -> provider process -> observations -> SQLite ledger/projection -> subscribed UI transcript.
2. Provider approval/question -> typed observation -> durable interaction -> UI decision -> validated repository transition -> correlated provider response. Claude/Grok gaps occur on this complete round trip.
3. Host invitation -> pinned identity/private credential store -> session negotiation/reconnect -> bounded requests/events -> NativeWorkspace generation fencing -> history reconciliation.
4. Signed release -> manifest/signature verification -> bounded artifact acquisition/extraction -> immutable staged version -> process retirement -> active pointer/launcher/integrations -> setup/restore. Current ordering and ZIP modes violate the final activation contract.
5. Nix stage -> build identity + product binaries -> payload generator -> signed release metadata -> CI artifacts. The binary-only archive loop currently loses identity resources.


## Prioritized findings

51 findings: **13 High, 30 Medium, 7 Low, 1 Nit; no confirmed Critical**. All recorded findings have High confidence in the stated code defect; runtime/provider/platform behavior remains limited as described per finding. Ordered first by severity, then likely impact.

| Priority | ID | Severity | Finding |
| ---: | --- | --- | --- |
| 1 | [B09](#b09-claude-receives-the-actual-prompt-before-the-durable-authorization-gate) | High | Claude receives the actual prompt before the durable authorization gate |
| 2 | [FQ-01](#fq-01-failed-autosave-lets-send-submit-an-older-prompt) | High | Failed autosave lets Send submit an older prompt |
| 3 | [B10](#b10-process-pumps-can-block-stderr-outside-cancellationdeadline-selection) | High | Process pumps can block stderr outside cancellation/deadline selection |
| 4 | [R05](#r05-linux-zip-installs-lose-all-executable-permissions) | High | Linux ZIP installs lose all executable permissions |
| 5 | [R07](#r07-installer-reports-activation-complete-before-fallible-finalization-and-cannot-restore-the-previous-release) | High | Installer reports activation complete before fallible finalization and cannot restore the previous release |
| 6 | [B06](#b06-grok-completes-successfully-while-discarding-all-assistant-content) | High | Grok completes successfully while discarding all assistant content |
| 7 | [B07](#b07-grok-interactive-requests-are-never-answered-on-the-provider-connection) | High | Grok interactive requests are never answered on the provider connection |
| 8 | [B08](#b08-claude-permission-requests-are-tracked-but-never-exposed-as-approvals) | High | Claude permission requests are tracked but never exposed as approvals |
| 9 | [R10](#r10-service-managed-stop-violates-the-explicit---if-idle-contract) | High | Service-managed stop violates the explicit --if-idle contract |
| 10 | [B05](#b05-cursor-is-selectable-although-every-configured-launch-is-unavailable) | High | Cursor is selectable although every configured launch is unavailable |
| 11 | [FQ-02](#fq-02-failed-image-upload-permanently-suppresses-retry) | High | Failed image upload permanently suppresses retry |
| 12 | [FQ-05](#fq-05-shared-controls-and-the-composer-omit-existing-accessibility-semantics) | High | Shared controls and the composer omit existing accessibility semantics |
| 13 | [R03](#r03-the-exported-nixos-forge-module-references-a-missing-package) | High | The exported NixOS Forge module references a missing package |
| 14 | [R08](#r08-active-rustls-pin-is-affected-by-a-published-tls-13-advisory) | Medium | Active rustls pin is affected by a published TLS 1.3 advisory |
| 15 | [B01](#b01-codex-web-search-policy-is-omitted-at-the-provider-boundary) | Medium | Codex web-search policy is omitted at the provider boundary |
| 16 | [B02](#b02-project-icon-metadata-can-fetch-arbitrary-https-addresses) | Medium | Project icon metadata can fetch arbitrary HTTPS addresses |
| 17 | [FQ-04](#fq-04-evaluation-only-fonts-are-part-of-every-production-binary) | Medium | Evaluation-only fonts are part of every production binary |
| 18 | [R20](#r20-the-whole-workspace-check-cannot-compile-frontend-approval-fixtures) | Medium | The whole-workspace check cannot compile frontend approval fixtures |
| 19 | [R09](#r09-three-existing-tests-fail-because-their-fixtures-assume-the-wrong-state-or-platform) | Medium | Three existing tests fail because their fixtures assume the wrong state or platform |
| 20 | [B14](#b14-tiny-attachment-chunks-cause-quadratic-database-work-and-huge-row-counts) | Medium | Tiny attachment chunks cause quadratic database work and huge row counts |
| 21 | [B11](#b11-codex-probe-line-reads-allocate-past-their-declared-byte-ceiling) | Medium | Codex probe line reads allocate past their declared byte ceiling |
| 22 | [B12](#b12-local-project-icon-files-can-block-uncancellable-worker-threads) | Medium | Local project-icon files can block uncancellable worker threads |
| 23 | [R01](#r01-release-manifest-size-limits-apply-after-unbounded-body-buffering) | Medium | Release manifest size limits apply after unbounded body buffering |
| 24 | [R02](#r02-installer-network-operations-can-hold-the-installation-lock-indefinitely) | Medium | Installer network operations can hold the installation lock indefinitely |
| 25 | [R11](#r11-service-manager-and-installer-subprocess-waits-escape-their-advertised-deadlines) | Medium | Service-manager and installer subprocess waits escape their advertised deadlines |
| 26 | [B03](#b03-draft-reads-mix-revisions-and-attachments-across-concurrent-saves) | Medium | Draft reads mix revisions and attachments across concurrent saves |
| 27 | [FQ-10](#fq-10-settings-persistence-is-unsafe-between-editor-processes) | Medium | Settings persistence is unsafe between Editor processes |
| 28 | [FQ-08](#fq-08-observation-storage-grows-while-every-replay-processes-the-full-history) | Medium | Observation storage grows while every replay processes the full history |
| 29 | [FQ-07](#fq-07-remembered-drafts-retain-image-buffers-without-a-window-wide-budget) | Medium | Remembered drafts retain image buffers without a window-wide budget |
| 30 | [FQ-06](#fq-06-typing-copies-every-pending-image-before-upload-deduplication) | Medium | Typing copies every pending image before upload deduplication |
| 31 | [FQ-09](#fq-09-one-ui-service-poll-can-drain-an-unbounded-batch) | Medium | One UI service poll can drain an unbounded batch |
| 32 | [B04](#b04-engine-management-requests-wait-behind-serial-long-network-calls) | Medium | Engine-management requests wait behind serial long network calls |
| 33 | [FQ-03](#fq-03-rich-link-retention-can-create-a-recurring-fetchrepaint-loop) | Medium | Rich-link retention can create a recurring fetch/repaint loop |
| 34 | [FQ-16](#fq-16-rich-link-work-is-marked-pending-before-its-action-is-admitted) | Medium | Rich-link work is marked pending before its action is admitted |
| 35 | [R12](#r12-cancelling-the-async-installer-leaks-its-owned-stage-directory) | Medium | Cancelling the async installer leaks its owned stage directory |
| 36 | [R13](#r13-ci-never-builds-the-shipped-windows-production-stage) | Medium | CI never builds the shipped Windows production stage |
| 37 | [R19](#r19-release-zip-packaging-omits-the-build-identity-generated-by-the-production-stage) | Medium | Release ZIP packaging omits the build identity generated by the production stage |
| 38 | [R04](#r04-existing-file-size-policy-blocks-the-current-tree) | Medium | Existing file-size policy blocks the current tree |
| 39 | [FQ-11](#fq-11-a-panicked-transport-thread-never-reports-completion) | Medium | A panicked transport thread never reports completion |
| 40 | [FQ-15](#fq-15-transient-error-notices-lack-announcement-and-keyboard-lifetime-control) | Medium | Transient error notices lack announcement and keyboard lifetime control |
| 41 | [FQ-12](#fq-12-the-window-root-still-owns-unrelated-state-machines-and-presentation-internals) | Medium | The window root still owns unrelated state machines and presentation internals |
| 42 | [R06](#r06-public-cli-browser-and-handoff-flows-target-an-http-api-that-forge-no-longer-serves) | Medium | Public CLI browser and handoff flows target an HTTP API that Forge no longer serves |
| 43 | [B15](#b15-questionnaire-reads-decode-the-entire-thread-history-for-one-group) | Medium | Questionnaire reads decode the entire thread history for one group |
| 44 | [R14](#r14-normal-editor-launch-discards-diagnostics-and-sets-obsolete-diagnostic-variables) | Low | Normal editor launch discards diagnostics and sets obsolete diagnostic variables |
| 45 | [R15](#r15-transport-settlement-deep-clones-every-successful-owned-response) | Low | Transport settlement deep-clones every successful owned response |
| 46 | [R16](#r16-active-transitive-packages-have-unmaintained-advisories-with-no-recorded-migration-policy) | Low | Active transitive packages have unmaintained advisories with no recorded migration policy |
| 47 | [R17](#r17-the-formatting-gate-fails-on-six-current-files) | Low | The formatting gate fails on six current files |
| 48 | [B13](#b13-small-metadata-lookups-perform-unbounded-or-synchronous-whole-file-reads) | Low | Small metadata lookups perform unbounded or synchronous whole-file reads |
| 49 | [FQ-13](#fq-13-context-window-formatting-overflows-for-accepted-u64-values) | Low | Context-window formatting overflows for accepted u64 values |
| 50 | [FQ-14](#fq-14-resolved-rich-link-titles-never-enter-the-expiry-refresh-path) | Low | Resolved rich-link titles never enter the expiry-refresh path |
| 51 | [R18](#r18-unix-permission-failures-are-mislabeled-as-windows-acl-failures) | Nit | Unix permission failures are mislabeled as Windows ACL failures |


## Detailed findings

Each finding includes severity, confidence, exact files/functions/lines, the defect, impact, a concrete failure scenario, recommended fix, and useful patch examples or regression tests.

## Cross-cutting architectural findings

1. **One provider capability contract is missing.** Catalog visibility, model/account discovery and executable readiness are separately maintained. Advertise only adapters certified for launch, content, interactions, cancellation and durable completion (B05-B10).
2. **Admission must be atomic with ownership.** Autosave/upload/link state can change before the downstream queue accepts work. A refusal must retain retryable work rather than mark it settled or in-flight (FQ-01/FQ-02/FQ-16).
3. **Durability must precede effects.** Prompt delivery and activation can occur before the durable fence or after a false complete state. Record uncertainty and preserve recovery paths instead of classifying possible execution as unstarted (B09/R07/R12).
4. **Bound complete lifetimes, not only batches.** Channels, per-file/per-image bounds and scene windows do not cap line allocation, chunk counts, retained histories, inactive image drafts, manager backlogs or work per UI tick (B04/B11/B14/B15/FQ-06-FQ-09).
5. **Security policy must reach every actual boundary.** Web-search configuration and decorative icon fetches bypass stronger nearby contracts. Share typed enforcement without assuming a generic sandbox covers provider-hosted tools or every HTTP client (B01/B02).
6. **Extract state ownership, not only files.** NativeApplication's numerous impl files still mutate one large owner; draft/upload, profile and subscription lifetimes need narrow entities and commands (FQ-12/R04).
7. **Tests certify helpers more often than complete adapters/artifacts.** Hundreds of passing fixtures coexist with unsupported runnable providers, non-executable ZIPs, stale tests and missing production-target CI coverage. Test the shipping entry and real configured executor contracts (B05-B10/R05/R09/R13/R20).
8. **Native migration documentation is inconsistent.** Electron/HTTP handoff comments, old diagnostic variables and stale accessibility assumptions coexist with GPUI/QUIC/AccessKit. Update architecture, supported-platform/provider tables and operational recovery instructions (R06/R14/FQ-05).
9. **Settings and accessibility are platform contracts.** Sequential JSON writes and stored label metadata do not prove interprocess atomicity or emitted assistive-technology semantics (FQ-05/FQ-10/FQ-15).


## Quick wins and larger refactors

Quick wins: gate Cursor/Grok; keep rejected saves dirty; retry refused uploads; filter image IDs before copying; normalize ZIP binary/data modes; fix the NixOS default; patch rustls; bound manifest/signature reads and waits; add question-tag filtering; cap UI polling; use safe context rounding; correct test fixtures/formatting and Unix diagnostics. Connect existing accessible labels/roles and gate evaluation assets before publishing.

Larger refactors: certified provider lifecycle and interaction delivery; acknowledged-body/revision draft ownership; recoverable installation activation/finalization; byte-budgeted draft/activity caches with incremental projections; bounded cancellable per-engine scheduling; interprocess settings transactions; narrower NativeApplication state owners; actual production-stage/platform/accessibility integration coverage.


## Remediation plan and tests to add

1. Prevent unintended/repeated execution and lost user work: B09, FQ-01, B10, R10. Tests must withhold authorization, force binding failure, cancel before first output, and reject an autosave before Send.
2. Restore release installability/recovery: R05/R07/R03. Run an actual Linux ZIP executable; inject failure at every activation boundary; evaluate the enabled NixOS module.
3. Gate incomplete providers and finish their contracts: B05-B08. Exercise each advertised adapter's real configured child, content persistence and correlated approval/question delivery.
4. Restore attachment admission/retry correctness: FQ-02/B03/B14. Test queue refusal, upload errors, shutdown references, concurrent draft reads and tiny-chunk rejection/idempotence.
5. Resolve exposed security/provenance risks: R08/B01/B02/FQ-04. Patch TLS, enforce provider search policy, block cross-origin private icon destinations, and certify shipped assets.
6. Make all existing quality gates green: R20/R09/R04/R17. Correct stale fixtures and missing fields, decompose cohesive oversized owners, then run complete all-target/clippy/test/codegen checks.
7. Bound network/filesystem/process acquisition and cancellation: R01/R02/R11/R12/B04/B11/B12/B13. Test stalls, drip feeds, no-newline output, FIFOs, dropped futures and abandoned work.
8. Bound frontend/session resources and query work: FQ-03/FQ-06-FQ-09/FQ-16/B15/R15. Stress many links/scopes/tools; measure bytes, allocations, query rows and input/frame progress.
9. Complete core accessibility and persistent error UX: FQ-05/FQ-15. Assert platform trees, textbox selection/actions, keyboard controls and announced inspectable errors; perform platform screen-reader checks.
10. Fix cross-process settings and unwind lifecycle: FQ-10/FQ-11. Use coordinated two-process write tests and panicked-worker shutdown tests, keeping packaged abort behavior explicit.
11. Certify actual production artifacts and diagnostics: R13/R14/R19/R16. Build/install/run GNU Windows and Linux stages, preserve build identity, capture safe startup failures and maintain advisory exceptions.
12. Extract tested independent owners and clear remaining low-impact debt: FQ-12/FQ-13/FQ-14/R18. Document budgets, authority, cancellation, supported providers and platforms alongside the implementation.


## Verification log

- Repository status captured before edits. The working tree has substantial existing staged and unstaged application changes plus an untracked historical session handoff; these belong to the user.

## Review checkpoint 1

The complete declaration inventory is available in [codebase-function-map.md](codebase-function-map.md): 1,360 Rust/Python files, 33,931 named callable declarations, and 40,700 total declarations. Generated protocol methods account for much of this volume. Four files use syntax the parser flags; these warnings are inventory limitations, not compiler diagnoses. Cargo metadata resolves all 19 workspace members. Supplemental reviewer evidence is stored in [review-backend-notes.md](review-backend-notes.md) and [review-frontend-notes.md](review-frontend-notes.md).

Verification so far: external Rust test registration passes; five Python ratchet regression tests pass; 32 targeted database tests pass. The file-size gate fails on 72 files in the current tree. QUIC/TLS pinning and application framing have been inspected without a confirmed pinning bypass. Credential storage has explicit file modes, bounded secret reads, and inode/handle identity checks; these safeguards invalidate generic claims that secrets are simply world-readable or unbounded.

### R01: Release manifest size limits apply after unbounded body buffering

Severity: Medium
Confidence: High
Exact files, functions, and lines: `modules/installer/rust/manifest.rs`, `fetch`, lines 255-289; `verify_signature`, lines 324-345.

What is wrong: `fetch` checks the manifest's advertised Content-Length and then calls `Response::bytes`. Chunked responses without Content-Length can grow without a cap. The signature response also calls `bytes` before checking its length. The final 1 MiB verification limit does not bound allocations while receiving either body.

Why it matters: Release content is untrusted until signature verification. A server or redirected endpoint can exhaust installer memory without providing a valid signature.

Concrete failure scenario: The release URL responds with a chunked stream containing hundreds of megabytes or an indefinitely growing signature body. The installer buffers it while holding its installation lock, then either exhausts memory or blocks installation maintenance.

Recommended fix: Stream both responses into a shared bounded reader, stop when the next chunk would exceed MAX_MANIFEST_BYTES, and enforce a deadline independently of byte limits.

Example code, adapted to the existing error type:

```rust
async fn bounded_body(mut response: reqwest::Response) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await
        .map_err(InstallerError::ManifestRequest)? {
        if bytes.len().saturating_add(chunk.len()) > MAX_MANIFEST_BYTES as usize {
            return Err(InstallerError::ManifestTooLarge(MAX_MANIFEST_BYTES));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
```

Tests to add: chunked manifest and signature responses with no Content-Length, exactly-at-limit and limit-plus-one bodies, and incorrect Content-Length. Assert bounded consumption and rejection before signature parsing.

### R02: Installer network operations can hold the installation lock indefinitely

Severity: Medium
Confidence: High
Exact files, functions, and lines: `modules/installer/rust/install/source.rs`, `acquire`, lines 157-166, and `install_artifact`, lines 301-340; `modules/installer/rust/install/release.rs`, `install`, lines 37-41.

What is wrong: The remote reqwest client sets HTTPS and redirect policy but no connect, read, or whole-operation timeout. Manifest fetching and artifact chunk reads can wait indefinitely. The root lock is acquired before these operations.

Why it matters: A stalled release server blocks updates, repair, and uninstall for the installation until the process is forcibly stopped. A maximum byte count does not limit a response that stops transmitting.

Concrete failure scenario: The server accepts TLS and returns headers, then never sends the remaining artifact bytes. A second installer command fails with InstallationRootBusy for as long as the first remains alive.

Recommended fix: Add explicit connect and read limits plus a total acquisition/materialization deadline and a cancellation path. Consider acquiring the activation lock only after immutable content has been downloaded to a separately owned staging location.

Tests to add: stalled TLS connection, headers with stalled body, drip-fed chunks that outlast a total deadline, and a second maintenance invocation after timeout. Verify the first releases its lock and staging resources.

### R03: The exported NixOS Forge module references a missing package

Severity: High
Confidence: High
Exact files, functions, and lines: `nix/forge-service.nix`, `options.services.artisan-forge.package.default`, line 62; `nix/workspace.nix`, exported `packages`, lines 458-498; `flake.nix`, exported `nixosModules`, lines 47-48.

What is wrong: The service module defaults its executable package to self.packages.<system>.forge, but the flake exports linux-production, linux-debug, and other tools, with no forge package.

Why it matters: Enabling the supplied NixOS module with its default package fails evaluation before the service can be deployed.

Concrete failure scenario: A NixOS user imports nixosModules.default, enables services.artisan-forge, and supplies credentials and policy. The absent forge attribute prevents configuration evaluation.

Recommended fix: Point the default at linux-production, which includes bin/forge, or export a dedicated Forge package. Add a module evaluation check to the flake checks.

Example patch:

```nix
default = self.packages.${pkgs.stdenv.hostPlatform.system}.linux-production;
```

Evidence: `nix eval --json .#packages.x86_64-linux --apply builtins.attrNames` succeeded and listed 19 packages without forge. `nix eval --raw .#packages.x86_64-linux.forge.outPath` failed with a missing-attribute error.

Tests to add: evaluate an enabled NixOS module for each supported Linux architecture with fake runtime credential paths and complete policy. Assert ExecStart points to an exported package's bin/forge.

### R04: Existing file-size policy blocks the current tree

Severity: Medium
Confidence: High
Exact files, functions, and lines: `scripts/file_size_ratchet.py`, `inspect`, lines 11-30 and main gate lines 33-44; `scripts/file-size-allowlist.txt`; current affected source files listed in the verification evidence.

What is wrong: The checked-in gate rejects 72 files in the working tree. Growth is concentrated in engine orchestration, conversation state/rendering, native application orchestration, and protocol codecs. This includes files previously below the 800-line limit and substantial growth beyond frozen allowances.

Why it matters: The CI file-sizes job and normal local check.py stop on this gate. It also measures real accumulation of coordination logic, rather than a hypothetical preference about file size.

Concrete failure scenario: The current implementation is submitted as-is. `python3 scripts/file_size_ratchet.py` exits nonzero, preventing the Linux checks from completing and the dependent artifacts job from running.

Recommended fix: Split cohesive responsibilities and their tests, reduce duplicate state/control logic, and preserve the existing frozen policy. Do not raise allowances merely to make the check green. Coordinate with the architectural recommendations in the frontend/backend passes.

Tests to add: No new mirror test is needed for current growth; the five existing ratchet tests pass. Rerun the gate after decomposition and include it in local and CI checks.

## Review checkpoint 2

The root Cargo run completed: **716 passed, three failed** across transport, protocol, installer, and CLI library/integration targets. The three failures are recorded under R09; they do not establish a TLS, lock-identity, or wire-decoder vulnerability. Two reproductions import actual source modules with small type/error shims: frontend stale-draft submission and installer ZIP executable-mode loss. Source code remains unchanged. Dependency investigation checked the pinned lockfile against RustSec database commit `ef6173cbc5c50ec8166f9a5b28f07834144373ee`, then distinguished current-host dependency reachability from optional lockfile entries.

### R05: Linux ZIP installs lose all executable permissions

Severity: High
Confidence: High
Exact files, functions, and lines: `modules/installer/rust/archive.rs`, `extract_zip`, lines 96-104; `packaging/portable/rust/payload_manifest.rs`, `payload_archive`, lines 124-149; `nix/workspace.nix`, `payloadArchive`, lines 214-225; `modules/installer/rust/install/workflow.rs`, `invoke_ae`, lines 655-665.

What is wrong: ZIP extraction creates every file with File::create and copies bytes without applying executable permissions. The production Linux payload is a ZIP, and the packager records executable modes in it. Verification checks content/layout but does not repair the required binaries' modes.

Why it matters: A cryptographically valid Linux installation can activate binaries that cannot execute. Setup, CLI, editor, and Forge then fail with Permission denied. The archive tests check bytes using non-executable dummy fixtures and omit an actual launch.

Concrete failure scenario: Install the Nix-produced Linux payload under an ordinary umask. bin/ae, bin/editor, bin/forge and bin/installer lose their execute bits. First-run invoke_ae fails after activation, or an install that skips setup succeeds but cannot subsequently start the application.

Validation: `.dist/review/zip-mode-repro.rs` imports the actual archive module. A ZIP member with Unix mode 0755 extracts with mode 0600 under the review environment's umask and no execute bits. The reproduction compiled and passed its assertion. This is an extraction reproduction; a full published-release installation was not run.

Recommended fix: Give required executable payload members a deliberate owner-safe executable mode on Unix, and ordinary resources a non-executable mode. Do not blindly propagate the generator's world-writable 0777. Extend signed layout/verification to check executable intent and normalize the generator to 0755 for binaries and 0644 for data.

Tests to add: Linux ZIP install with a real script or fixture executable must launch after extraction and after stable-CLI copying. Exercise restrictive and ordinary umasks; verify executable bits, absence of group/world write, and non-executable resources.

### R06: Public CLI browser and handoff flows target an HTTP API that Forge no longer serves

Severity: Medium
Confidence: High
Exact files, functions, and lines: `modules/cli/rust/commands/cli.rs`, Commands::Open fields, lines 185-196 and dispatch lines 444-456; `modules/cli/rust/commands/open.rs`, `forge_http_endpoint`, lines 66-68; `mint_pair_code`, lines 70-86; `open_ready`, lines 120-151. Native Forge entry: `modules/backend/src/main.rs`, `main`, lines 15-21; `modules/backend/src/listener.rs`, QUIC listener; `modules/cli/rust/http.rs`, `request_until`, lines 59-73.

What is wrong: --browser, --origin and the hidden --handoff still convert the native Forge UDP readiness address into an HTTP TCP origin and POST /api/pair/request. The current backend has an authenticated QUIC/Cap'n Proto listener and no implementation of that HTTP route. Electron/Node handoff comments and the suggested browser recovery path describe the previous architecture.

Why it matters: Advertised command paths and a recommended recovery instruction fail after the expensive readiness/start path succeeds. Maintaining both architectural stories obscures the actual supported entry and authentication contracts.

Concrete failure scenario: Run `ae open --browser` against a healthy installed native Forge. The CLI attempts TCP on the QUIC endpoint's numeric port and fails rather than opening a paired editor. An unrelated TCP service on that port also does not implement the expected pairing API.

Recommended fix: Remove unsupported browser/handoff options and obsolete dependencies/comments, or implement a deliberate supported bridge with its own authenticated endpoint and ownership contract. Keep native editor launch documentation aligned with its actual host invitation flow. Return a direct unsupported-mode error if migration compatibility must temporarily remain.

Tests to add: Every advertised Open mode against a real native fixture. Assert unsupported modes fail before starting Forge, or supported modes use a real declared service. Exercise the missing-editor recovery message so it never points at a broken mode.

### R07: Installer reports activation complete before fallible finalization and cannot restore the previous release

Severity: High
Confidence: High
Exact files, functions, and lines: `modules/installer/rust/install/workflow.rs`, `activate_release`, lines 100-133; `activate`, lines 257-302; `install_stable_cli`, lines 306-374. `modules/installer/rust/install/release.rs`, `install`, lines 35-36 and 82-88; `finish_activation`, lines 138-147.

What is wrong: Activation retires the old process and replaces the stable CLI, swaps the active pointer with finalization_state = complete, removes the previous pointer, and only then applies protocol/shortcut integrations and runs setup or restores Forge. Any later error returns failed install without rolling back those changes. This contradicts the documented promise that a failed install leaves the previous release active.

Why it matters: A failure can leave the new release active but partly configured, with the old Forge stopped and the rollback pointer gone. The state record claims finalization completed, making recovery and diagnosis misleading.

Concrete failure scenario: A verified update swaps installation.json; a shortcut write fails, or the new CLI's setup/start exits unsuccessfully. install returns Err while installation.json names the new version and complete finalization. The former Forge is no longer running. R05 is one concrete way first-run setup can fail after this commit.

Recommended fix: Model prepare, activation commit and finalization as recoverable stages. Preserve the previous pointer and launcher until successful finalization. Mark pending/failed finalization durably; roll back or expose a resumable repair state with clear semantics. Include retired-process restoration and integration ownership in the recovery plan.

Tests to add: Inject failure after stable launcher replacement, pointer swap, protocol registration, shortcut application, setup and Forge restoration. Restart the installer at each boundary and verify a working previous release or a clearly recoverable pending installation, never a false complete state.

### R08: Active rustls pin is affected by a published TLS 1.3 advisory

Severity: Medium
Confidence: High
Exact files, functions, and lines: `Cargo.toml`, workspace rustls pin, lines 62-66; `Cargo.lock`, rustls package, lines 6526 onward; `modules/transport/Cargo.toml`, line 12; `modules/cli/Cargo.toml`, line 29. Also transitive through reqwest/hyper-rustls in installer, backend and engine management.

What is wrong: rustls is pinned to 0.23.43. RUSTSEC-2026-0285 affects 0.23.13 through 0.23.44 and is patched in 0.23.45. The flaw accepts certain TLS 1.3 handshake messages at the wrong encryption level. [RustSec advisory](https://rustsec.org/advisories/RUSTSEC-2026-0285.html), [upstream security advisory](https://github.com/rustls/rustls/security/advisories/GHSA-2mjx-qc3c-rqvc).

Why it matters: This is an active network dependency, including HTTPS release acquisition. The advisory describes a protocol-enforcement defect. It does not show that this project's certificate pinning can be bypassed: transcript authentication still prevents an on-path attacker from freely altering a completed handshake.

Concrete failure scenario: A peer sends otherwise authenticated handshake messages at an encryption level the TLS state machine should reject; the affected implementation accepts them. This is the upstream-defined scenario, not a project-specific exploit reproduction.

Recommended fix: Update the exact workspace pin and lockfile to at least the patched 0.23.45, then run credential/certificate, HTTPS and QUIC compatibility tests. Add a maintained advisory check that distinguishes active runtime packages, optional entries and informational maintenance warnings.

Validation: Primary RustSec/upstream advisories were consulted. `cargo tree --locked --offline -i rustls@0.23.43 --depth 4` proves current-host reachability. The review's TOML/version-range scan is not an official cargo-audit run or exploit test. rkyv 0.7.46 also matches a RustSec advisory in the lockfile, but current-host cargo tree shows no enabled reverse dependencies; all-target reachability remains unverified and it is not reported as a confirmed active vulnerability.

Tests to add: Repeat existing handshake/pinning and certificate tests after the update; include upstream TLS state-machine regressions through the patched dependency. Check advisories in CI with documented narrow exceptions and expiry where required.

### R09: Three existing tests fail because their fixtures assume the wrong state or platform

Severity: Medium
Confidence: High
Exact files, functions, and lines: `modules/installer/rust/install/tests.rs`, `lock_rejects_path_identity_substitution`, lines 482-499; `modules/cli/rust/commands.rs`, `scheduled_task_runs_stable_cmd_launchers_through_trusted_cmd`, lines 755-791; `modules/cli/rust/commands/autostart/logon_task.rs`, `stable_launcher_kind`, lines 64-72; `tests/protocol/observation_events.rs`, `thread_fixture`, lines 988-999 and `v1_event_bytes_decode_unchanged`, lines 1030-1055.

What is wrong: The installer test expects sentinel bytes in the moved lock but never writes them. The CLI test feeds a Windows path to the host-native Path::file_name on Linux and expects Windows separator behavior. The legacy protocol fixture omits the added has_started_response Bool (whose schema default is false) but compares it with a helper that now sets true.

Why it matters: The normal Linux test gate is red and security/compatibility tests produce false signals. These defects can encourage weakening correct production validation to satisfy incorrect assertions.

Concrete failure scenario: Run the current transport/protocol/installer/CLI suite on Linux. It returns exit 101 with these three failures while 716 tests pass. Lock identity rejection works; the assertion on unused fixture bytes fails. Wire decoding returns the valid absent-field default; the changed helper expectation fails.

Recommended fix: Seed the sentinel fixture or assert the intended original content; isolate Windows-native path behavior to Windows and use platform-neutral action-generation tests separately; construct an explicit legacy expected thread with default fields. Prefer a checked-in historic golden message for wire compatibility instead of rebuilding it with the current schema and helper defaults.

Tests to add: Correct the existing tests rather than writing duplicates. Run them on Linux and Windows, and add an old-schema golden-byte decoding regression that intentionally distinguishes new fields' defaults from new-message fixtures.

### R10: Service-managed stop violates the explicit --if-idle contract

Severity: High
Confidence: High
Exact files, functions, and lines: `modules/cli/rust/commands/lifecycle.rs`, `stop`, lines 102-132; `modules/cli/rust/commands/autostart/systemd.rs`, `stop`, lines 61-86; `modules/cli/rust/service.rs`, `ForgeService::stop`, lines 488-494 and unit KillSignal/TimeoutStopSec, lines 356-357; `modules/backend/src/lib.rs`, `run_with_signal`, lines 171-200.

What is wrong: The CLI requires --if-idle but takes the systemd branch before sending the authenticated conditional-stop request. That branch intentionally does not inspect active work and stops the service outright. SIGINT unconditionally cancels Forge; it does not wait for an idle transition.

Why it matters: A caller explicitly requesting an idle-only shutdown can terminate a running agent. Update retirement can consequently interrupt work instead of refusing a busy installation. The current comment describes an orderly signal but does not preserve the conditional operation's semantics.

Concrete failure scenario: A systemd-managed Forge is executing a tool. Run `ae stop --pid <its PID> --if-idle`. service.stop succeeds, SIGINT cancels the run, and the CLI reports stopped rather than busy. The same authenticated native operation without this branch would reject active work.

Recommended fix: Preserve the idle precondition for every ownership mode. Add a supported lifecycle/control channel for a second authorized local caller, or have the supervisor forward a conditional-stop request to the existing owner. If idle cannot be established, return busy/unavailable without issuing systemctl stop. Reserve unconditional service-stop for a separately named explicit action. Verify the service owns the requested process before mutation as well.

Tests to add: A real or scripted service-managed busy Forge refuses --if-idle and remains alive; an idle Forge stops; the wrong PID/service instance is never stopped; losing the idle-check connection fails closed. Assert update retirement uses the same semantics.

### R11: Service-manager and installer subprocess waits escape their advertised deadlines

Severity: Medium
Confidence: High
Exact files, functions, and lines: `modules/cli/rust/service.rs`, `UserSystemctl::run`, lines 62-79; `modules/cli/rust/commands/autostart/systemd.rs`, `start_and_wait`, lines 92-123 and `stop`, lines 75-85; `modules/installer/rust/install/workflow.rs`, `run_setup_sequence`, lines 204-208 and `invoke_ae`, lines 655-672.

What is wrong: Synchronous Command::output/status waits have no timeout or cancellation. Service start and is-active calls occur outside the caller's readiness deadline. Service stop completes its blocking systemctl call before creating the 40-second polling deadline. Installer setup invokes child commands while holding the installer lock, also without a deadline.

Why it matters: A stalled user manager, start job, or child can wedge a command beyond its configured lifecycle budget. Subsequent installation maintenance is denied while the installer remains locked. A later Instant check does not bound a blocking call already in progress.

Concrete failure scenario: systemctl start blocks on a user-manager job longer than the CLI readiness budget, or a new ae setup child never exits. The controlling process cannot apply the deadline it documents and holds its resources until forcibly terminated.

Recommended fix: Use the project's existing process-lifecycle abstraction with explicit overall deadline, bounded output, cancellation and child/group cleanup. Pass remaining time into every service operation instead of starting another timeout afterward. Consider --no-block followed by deadline-bounded job/readiness observation when the systemd operation contract permits it.

Tests to add: Fake systemctl and ae children that hang, fill output pipes, ignore graceful stop and exceed the deadline. Assert command duration stays bounded, escalation targets only the owned child, and installer locks become available afterward.

### R12: Cancelling the async installer leaks its owned stage directory

Severity: Medium
Confidence: High
Exact files, functions, and lines: `modules/installer/rust/install/authority.rs`, `StageLease`, lines 641-686 and `complete_install_locked`, lines 698-706; `modules/installer/rust/install/release.rs`, `install`, lines 60-88 and `verify_and_activate_existing`, lines 116-133.

What is wrong: StageLease cleanup exists only in code reached after awaited materialization completes. The lease has no Drop fallback or durable startup recovery for these stages. Dropping/cancelling the public install future skips complete_install_locked and leaves its partial files on disk. The stage name includes version and PID, so retrying that version in the same process collides with the leaked directory.

Why it matters: Cancellation is a normal async failure mode. Partial payloads can consume substantial disk space, and a caller that applies a timeout cannot immediately retry even after the root lock is released. Abrupt process termination also leaves accumulated stages to address at recovery.

Concrete failure scenario: An embedding caller cancels install during an artifact await. Its next invocation in the same PID tries .stage-<version>-<pid>, finds the old directory and returns ExistingRelease. Repeated interrupted versions retain their downloads/extracted files.

Recommended fix: Keep explicit cleanup so errors can be reported, and add an identity-fenced best-effort cancellation guard. Give the lease ownership/lock lifetime needed to remove only its own stage. Add startup reconciliation of proven abandoned stages, with a policy for uncertain ownership. Avoid destructors that blindly remove substituted paths.

Tests to add: Poll install until staging begins, then abort/drop it. Verify cleanup and lock release, retry the same version in the same process, and recover after a simulated crash. Substitute a stage identity and prove cleanup preserves the replacement.

### R13: CI never builds the shipped Windows production stage

Severity: Medium
Confidence: High
Exact files, functions, and lines: `.github/workflows/ci.yml`, Linux matrix line 17, Windows commands lines 47-52 and artifact commands lines 71-81; `nix/workspace.nix`, `stageChecks`, lines 349-353; `nix/stages.nix`, Windows host configuration, lines 153-209 and stage packages near lines 323-340; `.cargo/config.toml`, Windows GNU rustflags, lines 8-11.

What is wrong: Nix exports stage-* checks for the product payloads, but the CI matrix lists only named quality checks. Windows CI builds the host's default MSVC development targets; the shipped Nix Windows target is GNU with MinGW/static runtime and a distinct production profile. Linux artifacts build the Linux production payload only.

Why it matters: Green CI does not certify the linker, runtime closure, profile, feature flags or actual Windows binaries that the dev runner installs. This is especially consequential for GPUI/platform linking and abort-versus-unwind behavior.

Concrete failure scenario: A change compiles and tests on Windows MSVC in dev but fails the MinGW production link, or leaves a missing runtime dependency in the cross-built editor. Every selected CI check passes; a user's Windows deployment later fails to build or launch.

Recommended fix: Build each supported shipping stage through its exported Nix check or an equivalent artifact job. Run smoke tests on the resulting GNU Windows payload, not only a host-debug build. Keep quality tests and product-artifact checks explicit and document which platforms/profiles are certified.

Tests to add: CI builds Windows production and debug payloads and the Windows dev runner; a Windows runner installs and starts the actual artifact, exercises host connection, and verifies its declared runtime files. Include the enabled NixOS module evaluation from R03.

### R14: Normal editor launch discards diagnostics and sets obsolete diagnostic variables

Severity: Low
Confidence: High
Exact files, functions, and lines: `modules/cli/rust/commands/open.rs`, `launch_editor`, lines 168-194; `modules/tracing/src/recorder.rs`, `start`, lines 73-83; contrasting native dev launcher: `scripts/native_dev/src/editor.rs`, `deploy`, lines 108-134.

What is wrong: The production CLI redirects editor stdout and stderr to null and reports success once spawn succeeds. It sets ARTISAN_DIAGNOSTICS_DIR, ARTISAN_EDITOR_RENDERER_DIAGNOSTICS and ARTISAN_EDITOR_TRACE_FREEZES, but the current native frontend/tracing code has no readers for those old variables. The native flight recorder uses ARTISAN_TRACE_DIR and is not compiled into the production stage.

Why it matters: Startup failures after process creation lose their ordinary error chain and leave the user with no window and no useful CLI status. A profiling-enabled marker appears to enable diagnostics but does not activate the current recorder. The dev runner already demonstrates a receipt and log ownership path.

Concrete failure scenario: The editor process starts then exits on graphics initialization or host/configuration failure. `ae open` returns success; stderr that explains the failure has been discarded and the legacy diagnostics environment does not save it.

Recommended fix: Redirect detached output to a bounded rotating installation log, surface its path, and observe a bounded startup receipt. Wire the diagnostics preference to actual native configuration and clearly separate supported production diagnostics from debug-only tracing. Keep logs payload-safe and avoid capabilities/prompts in ordinary diagnostics.

Tests to add: A fixture editor exits after spawn with a known bounded error; launch returns or exposes a startup failure and preserves the safe diagnostic. Verify profiling/trace settings reach real consumers and rotation bounds disk usage.

### R15: Transport settlement deep-clones every successful owned response

Severity: Low
Confidence: High
Exact files, functions, and lines: `modules/transport/src/client_session/exchange.rs`, `classify_reply`, lines 359-388, especially line 373.

What is wrong: classify_reply borrows an already-owned WireEnvelope and clones its response into a new box after validation. Response payloads include owned history/text/image buffers, so this introduces a second copy before the original envelope is dropped.

Why it matters: Large image/history replies pay avoidable allocation/copy work and temporary peak memory. The response is transferred exactly once, so shared copying is unnecessary for the normal settlement path.

Concrete failure scenario: A client receives a near-frame-limit owned image/history response. Decoding allocates it; settlement clones its large payload before handing it to the caller, briefly retaining both versions.

Recommended fix: Validate version/family/correlation, then consume WireEnvelope and move its response/failure into SettledReply. Keep the rejection diagnostics borrowed or extract their small fields before the move. Preserve correlation and feature-version checks.

Tests to add: Existing wrong-version/family/correlation tests should remain. An allocation or ownership-focused check on a large Vec payload should prove the success path transfers its backing buffer without cloning; a duplicate implementation-mirroring test is unnecessary.

### R16: Active transitive packages have unmaintained advisories with no recorded migration policy

Severity: Low
Confidence: High
Exact files, functions, and lines: `Cargo.lock`, bincode near line 1174, proc-macro-error2 near line 5705 and ttf-parser near line 8180; `Cargo.toml`, statig and syntect pins, lines 83-88; GPUI fork pins, lines 40-41.

What is wrong: The active current-host graph includes bincode 1.3.3 through syntect, proc-macro-error2 2.0.1 through statig macros, and ttf-parser 0.25.1 through the GPUI/font stack. RustSec marks these as unmaintained. These are maintenance advisories, not confirmed vulnerabilities. [Bincode advisory](https://rustsec.org/advisories/RUSTSEC-2025-0141.html), [proc-macro-error2 advisory source](https://github.com/RustSec/advisory-db/blob/main/crates/proc-macro-error2/RUSTSEC-2026-0173.md), [ttf-parser advisory](https://rustsec.org/advisories/RUSTSEC-2026-0192.html).

Why it matters: The application depends on upstream fixes for syntax parsing, macros and font handling, while exact pins and a private GPUI fork make upgrades deliberate work. Without ownership or a dated exception, informational warnings can become silently permanent.

Concrete failure scenario: A platform or parser bug requires an upstream fix, but the archived dependency receives none and the project has no decided replacement or maintained fork. Routine advisory checks then either stay red or are broadly suppressed.

Recommended fix: Record active reachability, threat relevance, upgrade owner and reviewed exceptions. Track replacements through syntect/statig/GPUI upstream rather than forcing incompatible major versions throughout the tree. Keep optional paste/rkyv lockfile entries separate from active-runtime risk and validate all shipping target graphs.

Tests to add: Advisory/maintenance inventory in CI, preserving feature/target context; font/syntax/serialization regressions when the owning upstream library changes. No evidence here justifies a wholesale dependency rewrite.

### R17: The formatting gate fails on six current files

Severity: Low
Confidence: High
Exact files, functions, and lines: `modules/assets/src/fonts.rs`, line 214; `modules/backend/src/engine_owner/claude/adapter.rs`, `apply_event`, line 898; `modules/backend/src/native_run_dispatch/turn.rs`, `consume_turn`, line 244; `modules/database/src/repository/message_dispatch/lease.rs`, `renew_message_dispatch_lease`, line 110; `tests/backend/engine_usage.rs`, `unsupported_readers_report_honest_surfaces`, line 1182; `tests/database/message_dispatch_lifecycle.rs`, `lease_renewal_survives_a_wall_clock_that_stepped_back`, line 988. Exact formatter hunks are retained in `/tmp/artisan-review-fmt.log` during the review.

What is wrong: `cargo fmt --all -- --check` returns nonzero and identifies these six files. The current code does not meet its configured rustfmt baseline.

Why it matters: The Rust formatting jobs on both supported CI hosts stop before later checks. This is separate from functional defects and the frozen file-size policy.

Concrete failure scenario: Submit the current tree; the rustfmt gate fails on these hunks even if all targeted functional tests pass.

Recommended fix: Apply the workspace formatter in the implementation/remediation change and review the resulting mechanical diff. This audit does not format or alter user source files.

Tests to add: None; rerun the existing formatter check.

### R18: Unix permission failures are mislabeled as Windows ACL failures

Severity: Nit
Confidence: High
Exact files, functions, and lines: `modules/cli/rust/credentials/storage.rs`, `check_dir_mode`, lines 135-150 and `check_file_mode`, lines 153-169, especially lines 148 and 166.

What is wrong: The Unix branches return ForgeCredentialError::WindowsAcl when POSIX mode checks fail.

Why it matters: The typed error and displayed diagnostic misdirect users and support toward Windows ACLs for a Unix credential-mode issue. The checks themselves correctly fail closed.

Concrete failure scenario: A Linux user changes a secret file to mode 0644 and receives a Windows-ACL error instead of the required owner-only mode guidance.

Recommended fix: Use a platform-neutral insecure-permissions error with safe expected-mode guidance, or distinct UnixMode and WindowsAcl variants. Do not expose secret bytes or sensitive credential paths in public errors.

Tests to add: Existing Unix mode-rejection tests should also assert a platform-appropriate, payload-safe diagnostic.

### R19: Release ZIP packaging omits the build identity generated by the production stage

Severity: Medium
Confidence: High
Exact files, functions, and lines: `nix/stages.nix`, `identityScript`, lines 98-128 and payload install phase lines 261-269; `nix/workspace.nix`, `payloadArchive`, lines 214-225; `modules/build_info/src/lib.rs`, `BuildIdentity::for_executable`, lines 210-217 and `version`, lines 229-235; `modules/frontend/src/native_application/selectors.rs`, `window_title`, lines 15-22.

What is wrong: The production stage creates resources/build-info.json, but payloadArchive declares only the four binaries to the ZIP generator. The generator archives only declared inputs plus payload-manifest.json. The identity document disappears from the release artifact.

Why it matters: Installed binaries then identify themselves as Unstaged, falling back to the workspace package version 0.0.0 and losing release channel, commit, profile and target provenance. Window titles and diagnostic attribution disagree with the installation's actual release.

Concrete failure scenario: Build and install the CI-produced artisan-nix-payload.zip. Even after executable permissions are fixed, BuildIdentity::for_executable finds no resource identity and returns Unstaged for the released editor/CLI.

Recommended fix: Include the stage's resources/build-info.json as a declared, signed payload member and verify its version/channel/target agree with release metadata. Inventory all required resources and notices rather than reducing a staged payload to a binary-only loop.

Tests to add: Produce an archive from a complete stage, extract it, and assert actual BuildIdentity resolution remains Installed with the expected channel/commit/target. Validate agreement between release and identity metadata and reject missing required resources.

### R20: The whole-workspace check cannot compile frontend approval fixtures

Severity: Medium
Confidence: High
Exact files, functions, and lines: `tests/frontend/conversation_scene.rs`, `approval_item`, lines 177-189, initializer line 182; `tests/frontend/conversation_surface.rs`, `ordered_scene_items`, lines 222-320, initializer line 266, and `error_approval_question_and_transition_are_distinct_kinds`, lines 460-518, initializer line 467.

What is wrong: Three test constructors were not updated after the approval scene gained its required decision field. The whole-workspace all-target check fails with E0063 before these test targets can run.

Why it matters: This blocks the clippy/test build baseline, including checks unrelated to approval rendering. Runtime tests cannot establish correctness when their fixtures do not compile.

Concrete failure scenario: Run `cargo check --locked --workspace --all-targets --features artisan-frontend/visual-proof -j 2`. Both conversation_scene and conversation_surface test targets fail to compile with missing field decision.

Recommended fix: Supply deliberate pending/resolved decision values in these fixtures and use a focused approval fixture builder where it improves semantic consistency. Update tests that assert requested versus decided rendering; avoid defaulting every new field without checking the intended state.

Tests to add: Run these existing targets after correction and cover undecided, allow, deny and resolved approval presentation. Rerun the complete all-target check.

### B01: Codex web-search policy is omitted at the provider boundary
- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/backend/src/engine_selection/native.rs::inherited_network_web` 218-237; `modules/backend/src/engine_owner/codex/protocol.rs::CodexSettings` 48-58, `from_selection` 67-110, `thread_params` 140-191; `modules/backend/src/engine_owner/codex/continuation.rs::thread_resume_params` 133-151; contrasting enforced field at `modules/backend/src/engine_owner/operation/opencode.rs` 368-373.
- What is wrong: Native configurations carry `WebSearchAccess::Disabled` by default, but `CodexSettings` drops that axis and never emits a `web_search` override. The actual provider choice comes from its own default or managed config instead of the durable permission policy.
- Why it matters: The persisted permission can say search is denied while the native agent exposes a search tool. Network sandbox policy does not by itself disable a provider-hosted tool. This also prevents enabling the axis deliberately in a predictable way.
- Concrete failure scenario: A managed Codex configuration enables web search. A thread selects a policy with `web_search = disabled`; `thread/start` and `thread/resume` override sandbox/approval but leave that config value intact, so the engine may search anyway.
- Recommended fix: Add the web-search axis to `CodexSettings`, translate it to an explicit provider config value at fresh start and resume, and reject provider configurations that cannot enforce the selected policy. Confirm the intended mapping for enabled search against the supported CLI.
- Example patch/code where useful: `config.insert("web_search".into(), Value::String(if self.web_search_enabled { "live" } else { "disabled" }.into()));` after deriving the field from `selection.permission().web_search()`.
- Tests that should be added: Assert fresh and resume request JSON explicitly includes `web_search = disabled`; assert an enabled setting maps intentionally; test with a config file setting search on to prove the request override wins.


Official provider configuration sources: [Codex configuration](https://developers.openai.com/codex/config-basic), [app-server overrides](https://developers.openai.com/codex/app-server). No real-provider integration was executed.

### B02: Project icon metadata can fetch arbitrary HTTPS addresses
- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/backend/src/project_icon_service.rs::client` 97-108, `download` 110-136, `remote_icon` 138-155, `metadata_url` 158-190; `modules/backend/src/git_remote_url_policy.rs::classify_hostname` 432-447; invocation `modules/backend/src/project_subtitles.rs::refresh` 243-253 and `refresh_icon` 292-335.
- What is wrong: The icon fetcher accepts arbitrary absolute `avatar_url` values from remotely supplied JSON, permits redirects with no destination gate, uses the ambient proxy configuration, and checks only HTTPS plus absence of URL credentials. It has no address policy. Self-hosted GitLab/Gitea lookup is intentional, but that does not justify following their metadata to unrelated internal services.
- Why it matters: Opening an attached repository triggers backend HTTP requests from the host's network. A malicious repository hosting service can turn a decorative icon lookup into blind SSRF. This path bypasses the stronger validated-DNS/no-proxy policy already implemented for assistant rich links.
- Concrete failure scenario: The selected repository is on an attacker-controlled `gitlab.example` host. Its metadata returns `avatar_url: "https://internal.example/admin/do-action"`, or redirects an ordinary image URL there. Forge sends the GET to the internal destination before deciding whether the response decodes as an image. Trusted TLS at the internal host is still required; this is not a blanket HTTP/metadata-IP claim.
- Recommended fix: Reuse the public-address DNS and redirect policy for cross-origin icons. If private self-hosted Git servers are a supported requirement, confine that grant to the explicitly selected repository origin and independently require public destinations for cross-origin avatar URLs. Disable ambient proxies unless their use is a deliberate trust decision.
- Example patch/code where useful: Extract the rich-link public resolver and per-hop destination validation into a shared outbound-fetch boundary; inject it into both clients. Keep a separate explicit repository-origin allowance when required.
- Tests that should be added: Public metadata pointing to loopback/private DNS; public-to-private redirect; mixed public/private DNS answer; legitimate self-hosted same-origin icon; HTTPS avatar on an unrelated public CDN. Prove blocked endpoints receive no connection.

### B03: Draft reads mix revisions and attachments across concurrent saves
- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/database/src/repository/composer_draft.rs::Repository::read_composer_draft` 140-145, `read_draft` 553-616; concurrent writer `apply_save` 352-385; request use `modules/backend/src/request_handler/composer_drafts.rs::read_composer_draft` 254-269.
- What is wrong: The public read runs `DRAFT_SELECT` and `DRAFT_ATTACHMENTS_SELECT` as independent queries on the pool. A writer can commit a save between them. The result can combine text/revision from save A with attachment references from save B.
- Why it matters: A client can display, autosave or restore a draft that never existed. The revision is also the submission fence, so this can cause confusing stale submissions or preserve the wrong image with user text. The submission transaction itself correctly fences the revision, which limits direct send impact.
- Concrete failure scenario: Connection 1 reads revision 7/body A. Connection 2 saves revision 8/body B/images B. Connection 1's second read returns images B and reports them beside revision 7/body A. A subsequent client autosave persists the mixed draft.
- Recommended fix: Put the public read in a read transaction with a stable SQLite snapshot, or select draft and ordered references in one statement. Internal calls already supplied with the submit/save transaction can retain the helper.
- Example patch/code where useful: `let tx = self.database.begin().await?; let result = read_draft(&tx, scope).await; tx.commit().await?; result`, with typed error mapping and proportionate rollback handling.
- Tests that should be added: Use two file-backed connections and a controlled interleaving between body and attachment reads. Assert returned body/revision/refs belong entirely to either committed save. Existing serial round trips do not cover this race.

### B04: Engine-management requests wait behind serial long network calls
- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/backend/src/engine_manager.rs::EngineManager::start_with` 176-203, `list_versions` 247-259, `run` 268-307, `ensure` 346-371; `modules/native_engine/src/install/transport.rs` 15-16, `HttpsTransport::new` 83-100.
- What is wrong: One detached manager thread services an unbounded `std::sync::mpsc` queue. Before receiving any command it synchronously refreshes/installs every supported engine. Each network operation uses a 600-second request timeout, and the refresh path may fetch latest twice. Version-list requests do not carry a deadline/cancellation check to the worker; a response receiver dropped by the request timeout still causes the later network call.
- Why it matters: An unreachable provider can block version selection/listing for unrelated healthy providers. Timed-out retries accumulate in an unbounded queue and later execute useless network calls. The current UI can acknowledge a selection immediately while its actual application is delayed by the earlier startup sweep.
- Concrete failure scenario: An early provider accepts TCP but stalls its feed body for minutes. The user repeatedly opens version menus or selects a held version for another provider. Each request times out externally, but its manager command remains queued and is eventually executed, further delaying useful work.
- Recommended fix: Use bounded admission, per-engine workers or interruptible scheduling, short feed deadlines separate from large artifact deadlines, and cancellation-aware replies. Coalesce version-list/select work per engine. Retain/join worker ownership at shutdown if background completion is required.
- Tests that should be added: A scripted transport blocks one provider while another selection/list completes within the service deadline; dropped list replies cause no transport fetch; queue saturation returns a typed retryable error; shutdown drops pending work and proves worker exit.

### B05: Cursor is selectable although every configured launch is unavailable
- Severity: High
- Confidence: High
- Exact files/functions/lines: `modules/backend/src/native_model_catalog.rs::RUNNABLE_ENGINE_IDS` 37-43 and `drop_unauthenticated` 321-331; `modules/backend/src/native_run_dispatch/claim.rs::load_claim` 333-344 and `resolve_cursor_launch` 1402-1406; `modules/backend/src/engine_owner/operation/cursor.rs::execute_cursor_turn` 28-76. Frontend confirmation: `modules/frontend/src/native_model_selector/state.rs::engine_usable` 417-423 and `select_model_policy` 872-884; `modules/frontend/src/native_model_selector/render/menu.rs` 208 and 292-305.
- What is wrong: Cursor is included in the runnable catalog and remains selectable whenever discovery produces its model rows. Its configured launch resolver unconditionally returns `None`; the executor is also a stub that always fails configuration. There is no frontend readiness gate that repairs the catalog claim.
- Why it matters: A user can choose an advertised engine, submit a valid durable message, and receive no execution. The dispatcher classifies structural lack of an implementation as transient profile unavailability, repeatedly requeuing it instead of giving a terminal unsupported-capability result.
- Concrete failure scenario: Install/sign in to Cursor, discover its model rows, select one, and send a message. Each claim calls the same `resolve_cursor_launch -> None`, requeues as `engine profile unavailable`, and cannot launch the provider regardless of credentials or retries.
- Recommended fix: Remove Cursor from runnable capabilities until the managed resolver and executor satisfy the complete supported-turn contract. Treat an unimplemented engine as an admission failure, not a retriable outage. For the full integration, implement trusted managed launch, observation projection, cancellation, interactions and durable completion before exposing it.
- Example patch/code where useful: Build `runnable_harness_ids` from certified provider capabilities rather than a static five-engine list; report a typed unsupported-engine reason before storing a dispatch. A short-term list can include only actually integrated providers.
- Tests that should be added: Catalog/discovery test proving Cursor cannot be selected before integration; real configured-executor fixture covering selected model -> claimed dispatch -> assistant response -> terminal state; permanent unsupported launch produces one terminal failure, without requeue churn.

### B06: Grok completes successfully while discarding all assistant content
- Severity: High
- Confidence: High
- Exact files/functions/lines: `modules/backend/src/native_model_catalog.rs::RUNNABLE_ENGINE_IDS` 37-43; `modules/backend/src/engine_owner/operation/grok.rs::execute_grok_turn` 48-456 and `grok_pump_loop` 474-588, especially session-update handling 520-525 and completed prompt handling 548-572. Frontend selection path is the same as B05.
- What is wrong: The configured Grok executor sends real prompts but its pump only extracts summary titles from ACP `session/update` notifications. It drops message, thinking, tool-call and tool-result content and returns `TerminalState::Completed` when the prompt succeeds. There is no assistant-content observation projection on this path.
- Why it matters: A paid provider request, and potentially workspace operations, can finish without any visible or durable assistant answer. The successful terminal state hides the data loss. Existing Grok tests manually exercise generic turn fixtures and explicitly describe the provider as not yet runnable, so they do not cover this now-advertised path.
- Concrete failure scenario: A user selects a discovered Grok model and asks a question. Grok emits a complete assistant message in session updates and a successful prompt result. Forge stores usage/completion but renders an empty answer because every text update was ignored.
- Recommended fix: Gate Grok's runnable advertisement until its actual executor projects all supported session-update variants into the shared observation stream. Implement assistant text, reasoning, tool lifecycle and terminal consistency as a provider adapter, with bounded accumulation and deliberate unsupported-variant handling.
- Example patch/code where useful: Replace the title-only match with a shared ACP projection routine returning the appropriate `EngineObservation` sequence for each supported `SessionUpdate` variant; send those observations before accepting prompt completion.
- Tests that should be added: Run the real configured Grok executor against a scripted ACP child that streams multiple content fragments and a success result; assert exact persisted assistant content and tool history. Cover empty result, malformed updates, disconnect after partial text, usage and successful completion.

### B07: Grok interactive requests are never answered on the provider connection
- Severity: High
- Confidence: High
- Exact files/functions/lines: `modules/backend/src/engine_owner/operation/grok.rs::execute_grok_turn` 309-320, `grok_pump_loop` 526-546, `ask_grok_questionnaire` 596-642, `note_grok_agent_request` 651-660; `modules/backend/src/native_run_dispatch/turn.rs::deliver_applied_response` 756-860, especially engine gate 814-855.
- What is wrong: The Grok executor discards `steer_rx`, records permission requests only in a private bridge table without emitting approval observations, and emits questionnaire observations without retaining/delivering an answer. The dispatcher sends provider answers only for Codex and Claude. A Grok answer may therefore be acknowledged and durably marked resolved while no corresponding ACP response reaches the waiting provider.
- Why it matters: Any Grok turn needing permission or user input can stall until timeout. Permission prompts are invisible; questionnaire prompts falsely appear resolved. This is a separate implementation gap from B06's dropped content because it requires provider request correlation and delivery acknowledgement.
- Concrete failure scenario: Grok sends `session/request_permission` before a command. Forge inserts the bridge entry and never responds or shows approval, so both sides wait. Alternatively Grok asks a questionnaire: the UI displays it and stores the user's answer, but the provider continues waiting because its request ID receives no response.
- Recommended fix: Extend the shared provider-response path to support ACP request IDs and Grok's permission/elicitation bridge, retain the response channel, emit typed approval observations, deliver answers exactly once and require successful provider acknowledgement. Keep Grok unavailable until this contract passes end-to-end tests.
- Example patch/code where useful: Route `OwnedInteractionCommand` into a Grok response pump that calls the existing `answer_permission`/`answer_elicitation` bridge encoders and `transport.respond` for the original ACP ID; remove bridge entries only after successful delivery.
- Tests that should be added: Real executor scripted child asks permission, receives allow and deny on the exact original RPC ID; multi-field elicitation receives one validated answer after all fields; duplicate durable requests do not deliver twice; cancellation sends a denial/cancel or explicitly terminates the pending request; provider disconnect marks delivery uncertainty instead of reporting successful handoff.

### B08: Claude permission requests are tracked but never exposed as approvals
- Severity: High
- Confidence: High
- Exact files/functions/lines: `modules/backend/src/engine_owner/claude/adapter.rs::apply_event` 832-834 and `ClaudePendingTracker::waiting_on_user` 457-461; `modules/backend/src/engine_owner/claude/protocol.rs::decode_control` 644-692; `modules/backend/src/engine_owner/operation/claude.rs::claude_pump_loop_inner` 744-762 and 875-890.
- What is wrong: A valid streamed `can_use_tool` request becomes `ClaudeEvent::ApprovalRequested`, but the projection arm only inserts it into the private pending tracker. It never emits an approval observation. The complete live pump has no alternate approval projection. The existing response writer can answer known approvals, but no durable/UI approval exists to trigger it.
- Why it matters: Ordinary file/command requests under an asking permission mode wait with no action available to the user. Tracking the unseen approval also disables the inactivity timeout, so the stalled turn lasts until the full attempt deadline.
- Concrete failure scenario: Claude asks permission to run Bash or edit a file. The tracker reports `waiting_on_user = true`; the UI continues showing a running turn with no approval card, and cannot submit a valid answer because the durable approval was never created.
- Recommended fix: Convert and emit the request as a typed approval observation on the same successful insertion that records the provider request ID. Preserve the request-to-observation mapping for the existing answer path. Reject or deny unrepresentable requests explicitly rather than silently retaining an impossible wait.
- Example patch/code where useful: Add an `approval_rows` helper beside `question_rows`, using `request.to_domain_request()` and the run-scoped approval ID; have `apply_event` send `EngineObservation::Activity(Observation::Approval(...))` when `note_approval` accepts it.
- Tests that should be added: Feed the real `apply_event`/configured pump a permission frame and assert a durable requested approval is delivered, presented and answered on the original request ID. Cover command/file/action requests, malformed IDs, duplicate request suppression, deny/allow and cancellation. Current tests seed `note_approval` manually and cannot catch missing inbound projection.

### B09: Claude receives the actual prompt before the durable authorization gate
- Severity: High
- Confidence: High
- Exact files/functions/lines: `modules/backend/src/engine_owner/operation/claude.rs::execute_claude_turn` 175-178 (real prompt write), 199-237 (init read), 242-280 (prepared notification and authorization wait); `modules/backend/src/native_run_dispatch/claim.rs::admit_claim` 704-715 and `bind_claim` 860-924; `modules/backend/src/engine_owner/operation/turn_common.rs::wait_for_authorization` 151-174.
- What is wrong: The executor writes `user_message_payload(&session, &request.input.prompt)` before announcing a prepared session or waiting for dispatcher authorization. The later gate only pauses Forge's stdout consumption; it does not prevent the already-fed child from processing the prompt, billing the account or performing tools. This violates the lifecycle's promise that a prompt executes only after the provider binding is stored.
- Why it matters: Bind failure, cancellation or a process crash can classify the run as unstarted while the provider has already done work. Retrying an allegedly unstarted request can repeat irreversible commands, and durable observations/continuation cannot reliably identify the first execution.
- Concrete failure scenario: A prompt under Claude's bypass/auto-edit mode tells it to update a file. The child receives that prompt, emits init and starts tool work while SQLite is busy or the binding commit fails. The dispatcher then abandons/settles the unstarted run. The user sees a failed startup and retries, although the first child may already have changed the file.
- Recommended fix: Establish native readiness without feeding the actual prompt where the supported protocol permits it, then store binding and authorize before the real write. If native init is necessarily prompt-driven, redesign the durable lifecycle to bind the known session and record an execution/uncertainty fence before that write; do not label subsequent failures as never executed. Verify this against the certified Claude CLI protocol.
- Example patch/code where useful: Make the provider adapter expose separate `prepare_session` and `execute_authorized_prompt` operations and certify that preparation cannot carry user work. For a locally generated known session ID, commit the appropriate durable launch/binding state before `write_line`, with a typed uncertain-execution outcome if confirmation fails.
- Tests that should be added: Scripted real child records receipt of every prompt and attempts a side effect; withhold/deny authorization and assert no user prompt/side effect. Force bind failure and cancellation between preparation and authorization. Kill Forge after prompt write and prove restart never retries that work as a definitely unstarted turn.

### B10: Process pumps can block stderr outside cancellation/deadline selection
- Severity: High
- Confidence: High
- Exact files/functions/lines: `modules/backend/src/engine_owner/operation/claude.rs::claude_pump_loop_inner` 744-798, especially unconditional await 797; parallel stderr handling in `modules/backend/src/engine_owner/operation/codex.rs::codex_pump_loop_inner` 785-827; `modules/backend/src/engine_owner/process/stderr.rs::StderrCounter::pump` 114-153. Claude startup reads at 199-237 also lack a concurrent stderr drain.
- What is wrong: The live pump has no independently selected stderr reader. Claude's inactivity timer can enter `stderr_counter.pump().await` outside `tokio::select!` when `active_turn` is still absent. An open but silent stderr pipe leaves that await pending indefinitely, so the pump stops polling stdout, cancellation and the attempt deadline. Separately, enough stderr output can fill the OS pipe in both provider pumps because it is not continuously drained; reaching the cap does not close the pipe or terminate the provider either.
- Why it matters: A slow first response can leave a run uncancellable and keep the provider owner occupied indefinitely. Ordinary diagnostic bursts can prevent a child from reaching its next stdout frame, producing false stream failures or launch timeouts. The deadline configuration no longer bounds these paths.
- Concrete failure scenario: After Claude init and authorization, the provider takes longer than the configured inactivity interval to produce its first text and emits no stderr. `active_turn` remains `None`; `has_stalled(false, ...)` returns false, and line 797 starts waiting for stderr. Even a later stdout result, a user cancellation or expiration of the whole attempt cannot wake that branch.
- Recommended fix: Poll stderr as its own branch inside each cancellation/deadline select, or give it a supervised drain task with bounded lifetime and an explicit cap policy. Never await an unrelated pending read inside a selected timer arm. Define a first-response deadline that remains in the select before text establishes an active turn. On cap exceed, either terminate explicitly or continue draining without retaining additional bytes.
- Example patch/code where useful: Add `event = parts.stderr_counter.pump(), if state == StderrState::Open => { handle_stderr_event(event); }` alongside stdout and cancellation; remove the awaited pump from timer branches. If cap accounting stops, still close/terminate or drain the pipe through an intentional bounded task.
- Tests that should be added: Real child keeps stderr open/silent, delays first stdout content beyond inactivity, then emits a result: cancellation and attempt deadline must settle within budget. Child emits more than the OS pipe capacity before init and while streaming: the parent must drain or fail promptly. Exercise stderr cap exceed and shutdown with no orphaned child/drain task.

### B11: Codex probe line reads allocate past their declared byte ceiling
- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/native_engine/src/codex/process.rs::PipeDrain::spawn_pipe` 108-139, especially `read_until` 118 and the later ceiling check 124-127; callers `modules/native_engine/src/codex/probe.rs::run_codex_version` 368-388 and `modules/native_engine/src/codex/session.rs` 384-385.
- What is wrong: The worker calls `BufRead::read_until` on an unrestricted stream and only checks `max_bytes` after a whole line has been allocated. A stream without a newline can grow the `Vec` far past the promised output limit. The parent deadline limits time but not this allocation, and detached pipe-reader threads can keep it alive if descendants retain the pipe.
- Why it matters: A misbehaving provider/override executable can exhaust memory during a supposedly bounded, non-billable version or session probe. Output-size enforcement should stop consumption immediately after a small margin beyond the limit.
- Concrete failure scenario: A probe writes many megabytes without a newline. With `max_bytes = 1024`, Forge allocates/consumes the entire line before reporting `OutputTooLarge`.
- Recommended fix: Apply `Read::take(remaining + 1)` around each `read_until` call, including a cumulative budget, and distinguish a bounded nonterminated fragment from a complete line. Tear down the process tree/readers when the cap is reached. Reuse the already bounded `account_usage.rs::drain_stdout_lines` strategy.
- Example patch/code where useful: `reader.by_ref().take(u64::try_from(max_bytes.saturating_sub(total).saturating_add(1))?).read_until(b'\n', &mut line)` followed by deliberate oversize/unterminated handling.
- Tests that should be added: An endless/large no-newline stdout and stderr stream must consume at most `max_bytes + margin`, return promptly and leave no child/reader. Cover exact-bound newline, fragmented UTF-8, cumulative many-line overflow and a child retaining stdout after parent exit.
- Validation evidence: A standalone Rust harness imports the **actual** production `codex/process.rs` and supplies a finite counting `Read` implementation. Output: `configured_max_bytes=1024 actual_reader_bytes=8388608 event_too_large=true`. The harness and binary are temporary files under `/tmp/artisan-review-codex-pipe-bound*`; no repository source was changed.

### B12: Local project-icon files can block uncancellable worker threads
- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/backend/src/project_icon_service.rs::resolve` 39-49 and `local_icon` 55-74; invocation `modules/backend/src/project_subtitles.rs::refresh_icon` 292-335; runtime shutdown ownership `modules/backend/src/lib.rs::run` 133-153.
- What is wrong: The local icon scan opens repository-controlled paths with ordinary blocking `File::open`, follows symlinks and does not require a regular file. It awaits the blocking task before starting the only timeout, which covers the later remote lookup. `take(INPUT_LIMIT + 1)` bounds bytes but cannot bound waiting to open/read a FIFO or special file.
- Why it matters: A malformed local project can permanently occupy a blocking worker, keep its icon refresh pending and delay Tokio runtime teardown. Cancelling the outer detached task does not cancel a running `spawn_blocking` closure.
- Concrete failure scenario: On Unix a project contains `.artisan/icon.png` as a named pipe with no writer, or a symlink to an existing pipe. Opening the project schedules `local_icon`, which blocks in `File::open` and never reaches fallback or the 8-second remote timeout. Git cannot directly store a FIFO, so this scenario requires local filesystem/tool-created state or a link to an existing special file.
- Recommended fix: Open through a regular-file-only, no-follow boundary (with nonblocking open where supported), validate the opened file's metadata, and handle slow/network filesystems under a supervised cancellable subprocess or an intentionally bounded filesystem service. Use the existing native-files discipline where appropriate.
- Example patch/code where useful: Use `OpenOptions` with `O_NOFOLLOW | O_NONBLOCK` on Unix, verify `metadata().is_file()`, and then perform the bounded read; supply equivalent Windows reparse/special-file handling.
- Tests that should be added: Unix FIFO and symlink-to-FIFO are rejected before blocking; directory and growing file; regular valid icon still resolves; shutdown after an icon refresh remains within budget. Include platform-specific tests rather than assuming Unix flags exist everywhere.

### B13: Small metadata lookups perform unbounded or synchronous whole-file reads
- Severity: Low
- Confidence: High
- Exact files/functions/lines: `modules/backend/src/account_usage_cursor.rs::read_cursor_access_token` 219-232; `modules/backend/src/engine_owner/claude/launch.rs::read_claude_session_title` 471-481; async invocation `modules/backend/src/engine_owner/operation/claude.rs::settle_claude_terminal_title` 400-414, called at 340 and 379.
- What is wrong: Cursor's token reader calls unrestricted `fs::read` before checking `max_bytes`. Claude checks a path's size, then calls unrestricted `read_to_string` on a separately opened file, checks length only after allocation, collects every line in a vector, and performs all of this synchronously on the async executor. A growth race defeats the size check; even an allowed 64 MiB file creates avoidable full-text plus line-vector allocations.
- Why it matters: Corrupt/growing metadata can exceed intended memory ceilings. Every Claude terminal result can pause the current-thread owner while reading a large transcript for a single small title, delaying unrelated cancellation/heartbeat work and generating extra memory traffic.
- Concrete failure scenario: A large valid transcript accumulates through many resumed runs; each completed turn rereads all 64 MiB synchronously to find its last title. A local auth file or transcript grows between metadata and read and allocates beyond its declared maximum before being rejected.
- Recommended fix: Read from a single opened, verified regular-file handle with `take(maximum + 1)`. Move transcript I/O off the async thread and find the newest title from a bounded tail or streaming scan without collecting every line. Keep a bounded incremental title cache if the transcript format permits it.
- Example patch/code where useful: `file.take(max_bytes as u64 + 1).read_to_end(&mut bytes)`; for transcript titles, a bounded reverse/tail reader or `BufReader` with per-line caps and one latest-title variable.
- Tests that should be added: Oversized/growing token file stops allocation at the cap; transcript with many short lines; regular-file checks; large transcript title lookup does not stall an unrelated Tokio timer/cancellation task.

### B14: Tiny attachment chunks cause quadratic database work and huge row counts
- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/domain/src/composer_attachment.rs::ComposerAttachmentChunk::new` 54-81; `modules/database/src/repository/composer_attachment_upload.rs::apply_chunk` 155-240, especially whole-upload UPDATE 201-207 and whole-upload SELECT 208-235; `assemble` 245-278.
- What is wrong: Valid chunks may contain one byte at arbitrary offsets. There is no per-upload chunk-count/alignment/minimum-size limit. Every new chunk updates the timestamp of every stored chunk and rereads/checks all prior rows inside the write transaction. A 32 MiB payload can therefore legally create tens of millions of rows and quadratic total work despite its small byte bound.
- Why it matters: The byte budget does not bound database/CPU overhead or how long the SQLite writer is occupied. Repeated tiny chunks can interfere with durable run observations, draft saves and lease renewals on the same database. The current frontend's ordinary 4 MiB splitting is efficient; this is a boundary-hardening/performance failure for alternate or malformed clients.
- Concrete failure scenario: An authenticated client uploads a valid digest's bytes one at a time. At chunk N, Forge rewrites/checks approximately N upload rows. Before completion the client has caused much more work/storage than the 32 MiB image budget suggests.
- Recommended fix: Define a fixed/aligned chunk protocol (except the final chunk), or enforce a small maximum chunk count and track upload metadata/progress in a separate row. Refresh one upload-level timestamp and validate adjacent interval overlap with indexed queries instead of rescanning all intervals each time.
- Example patch/code where useful: Admit `offset % CHUNK_SIZE == 0` and exact expected size for each non-final chunk, with total chunk count `ceil(total_bytes / CHUNK_SIZE)` bounded by the image ceiling. If variable chunks are required, enforce `MAX_CHUNKS_PER_UPLOAD` before insertion and add upload-level accounting.
- Tests that should be added: One-byte chunk attempts are rejected or bounded; fixed out-of-order chunks complete; duplicates remain idempotent; total chunk count cannot exceed the cap; measure rows/statements touched at increasing upload sizes and verify near-linear behavior.

### B15: Questionnaire reads decode the entire thread history for one group
- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/database/src/repository/questionnaire.rs::read_thread_questionnaire` 43-48 and `scan_questionnaire` 53-105, especially ledger query 62-68, full payload decode 74-79 and retained resolved IDs 83-84.
- What is wrong: A lookup for one questionnaire scans every observation row in the thread from sequence zero, decodes every payload, and collects all question-resolution IDs, including unrelated groups. Paging bounds one query but never bounds total work. The ledger already has an `observation_tag` column that this path does not filter.
- Why it matters: A small question form becomes slower as unrelated tool/activity history grows. Long conversations can time out in the request handler while allocating/decoding megabytes for information belonging to a handful of rows. This is separate from the frontend's cumulative activity projection costs.
- Concrete failure scenario: A long agent thread accumulates 100,000 tool/terminal observations, then asks a two-question form. Each form read walks/decode all 100,000 rows before returning those two questions, and each later read repeats the full scan.
- Recommended fix: Immediately filter to question rows using `observation_tag = 'question'`. Longer term, persist/query questionnaire group and target identifiers in indexed relational columns or a bounded projection table, and read a stable high-water snapshot instead of chasing a growing tail indefinitely.
- Example patch/code where useful: Add the typed question-tag filter to the ORM query; create/query an index keyed by `(thread_id, group_id, delivery_sequence)` once group attribution is projected explicitly.
- Tests that should be added: Large unrelated history does not increase decoded rows for a small questionnaire; multiple groups and requested/resolved chronology; a writer appending concurrently cannot cause an unbounded scan; request remains within its deadline.

### FQ-01. Failed autosave lets Send submit an older prompt
- Severity: High
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/native_application/composer_drafts.rs::sync_composer_draft`, lines 115-120; `submit_draft_save`, lines 307-318; `receive_draft_event`, lines 146-161. `modules/frontend/src/composer_draft_sync.rs::begin_submit`, lines 208-226; `settle`, lines 259-280. `modules/frontend/src/native_application/draft_send.rs::drive_draft_submission`, lines 31-58.
- What is wrong: a rejected autosave is treated as definitively settled and its body is discarded. The composer change counter has already been marked consumed, so no retry occurs. `begin_submit` ignores the supplied body whenever any previous revision is known and there is no pending/in-flight save. It consequently calls the old revision ready even though that revision contains different text.
- Why it matters: the app can send instructions the user did not intend, then clear the newer prompt. This is more serious than a missing retry or a misleading error banner.
- Concrete failure scenario: Forge stores "old prompt" at revision 1. The user types "new prompt". The 64-command queue rejects that save as Busy, or the Forge returns a non-connection SaveFailed. The user presses Send after capacity returns. No save is queued; the request names revision 1 and sends "old prompt". The submission token still describes the current draft, so accepting the receipt can consume the newer text.
- Validation: `.dist/review/draft-sync-repro.rs` imports the actual `composer_draft_sync.rs` with minimal domain type stubs. `rustc --edition=2024 ...` succeeded; the reproduction asserts that, after the new-body save settles unsuccessfully, `begin_submit("new prompt")` returns no save and `submit_readiness` returns Ready(revision 1).
- Recommended fix: retain the latest unsaved body on admission/backpressure failure and distinguish failed-to-admit from definitive refusal. Track the acknowledged body together with its revision. `begin_submit` may reuse that revision only when it names the supplied body; otherwise save the supplied body first. Keep a user-visible unsaved/error state for definitive failures.
- Example code: `if acknowledged.as_ref().is_some_and(|saved| saved.body == body) { Ready(saved.revision) } else { save(body) }`.
- Tests to add: successful revision 1, rejected/failed save of body 2, then Send must save body 2 and await its acknowledgement. Run both Busy admission and a non-connection SaveFailed. Ensure failed autosaves remain pending across idle polling, host switch and shutdown.

### FQ-02. Failed image upload permanently suppresses retry
- Severity: High
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/native_application/composer_drafts.rs::sync_composer_draft`, lines 92-113; `receive_draft_event` Uploaded arm, lines 181-210; `flush_composer_drafts`, lines 245-257. `modules/frontend/src/native_application/impl_message_flight.rs::begin_message_submission`, lines 199-209. `modules/frontend/src/native_composer/forge_draft.rs::unstored_attachments`, lines 213-228; `draft_body_with_uploads`, lines 188-216.
- What is wrong: the attachment ID is inserted into `uploads` before a request ID is minted or transport admission succeeds. A Busy/Stopped admission finishes the hold but leaves the ID in this dedup set. Uploaded Err also leaves it there without setting a retryable attachment failure. Later syncs skip that same unstored attachment forever until connection bookkeeping resets.
- Why it matters: a transient queue-full or upload failure makes a usable image impossible to send. Closing the app treats the same ID as a live upload and can synthesize a draft reference for an object the Forge never stored.
- Concrete failure scenario: a ready screenshot enters the tray while the command queue is full. Its upload is refused once. The queue drains; repeated sync/Send skips the upload and reports "Images are still uploading" forever. The user must remove/re-add the image or reconnect. Shutdown can save the digest of the never-uploaded image.
- Recommended fix: model attachment upload state explicitly as queued/in-flight/stored/failed, with correlation and a retry action. Mark in-flight only after admission. Roll back on admission failure; terminal Uploaded errors must leave a visible failure and allow retry. Only acknowledged or genuinely admitted uploads may enter the shutdown draft reference set.
- Example code: keep the ID in a local variable; on `submit_draft` Err, `uploads.remove(&attachment_id)` and retain pending work. On Uploaded Err, remove it and set a typed failure rather than silently finishing.
- Tests to add: Busy admission then capacity recovery uploads the same image exactly once; request-ID failure does not poison dedup; Uploaded Err becomes retryable; shutdown never references an upload refused at admission. Current `submit_draft` test path (`composer_drafts.rs`, lines 329-332) bypasses the scripted command outcomes and always returns Ok, so include a realistic bounded service or a failure-capable draft sink.

### FQ-03. Rich-link retention can create a recurring fetch/repaint loop
- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/rich_link_titles.rs::queue`, lines 108-113; `insert`, lines 192-206; `modules/frontend/src/conversation_surface.rs::SurfaceRichLinkTitles::resolved_title`, lines 460-473; `modules/frontend/src/conversation_surface/interactions.rs::flush_rich_link_requests`, lines 493-512.
- What is wrong: the table's 64-record LRU evicts Pending and Failed entries alike. Every missing link rendered next frame becomes a new candidate. A visible reply with at least 65 distinct links exceeds retention, so newly queued misses evict other still-visible pending/resolved links. `flush_rich_link_requests` then notifies another paint.
- Why it matters: entry count is bounded, but total admitted work is not. Many links can repeatedly refill the 128-preview lane, re-fetch evicted URLs, churn favicon/title state and repaint continuously. A generated answer can trigger this without user clicks.
- Concrete failure scenario: one on-screen Markdown body contains 65 distinct HTTP(S) links. First flush queues all 65 and evicts the first. The following paint sees the first as missing and evicts the second when queueing it; further frames cycle. Preview completions also notify and reorder retention.
- Recommended fix: separate durable per-surface attempted/in-flight identity from the LRU display cache; never evict in-flight requests as if they were never attempted. Cap admissions per surface or by the visible set, and reconcile visible references before adding work. Prefer a generation-aware bounded coordinator that knows Pending/Resolved/Failed states and admission acknowledgements.
- Tests to add: render 65 and 128 distinct URLs for several unchanged frames and drain completions; request count must stop growing and each URL may have at most one live request. Verify failed URLs stay suppressed after other URLs are retained.

### FQ-04. Evaluation-only fonts are part of every production binary
- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/assets/src/fonts.rs::ALL`, lines 105-116; `bundled_fonts`, lines 133-137; `modules/assets/licenses/founders-grotesk-test.txt`, lines 1-3; `modules/frontend/src/native_application/app_entry.rs::run`, lines 112-115.
- What is wrong: the production catalog unconditionally embeds and registers both Test Founders Grotesk OTFs. The checked-in provenance says they are included for local evaluation and explicitly does not grant redistribution or production rights. There is no evaluation-only feature/asset boundary.
- Why it matters: packaging a production editor redistributes these bytes unless separate rights have been obtained outside this tree. This is a release-readiness and provenance gap, not a claim that a particular license has been legally violated.
- Concrete failure scenario: build the default production editor and publish its payload; both test-font byte arrays are compiled into the binary as startup registration inputs.
- Recommended fix: gate evaluation assets and typography behind a development-only feature excluded by packaging, or replace them with a licensed production face. Document acquired rights if the asset is meant to ship.
- Tests to add: production asset inventory must exclude evaluation-only font IDs/bytes; debug evaluation builds may include them. Include a provenance/license allowlist check for shipped assets.

### FQ-05. Shared controls and the composer omit existing accessibility semantics
- Severity: High
- Confidence: High
- Exact files/functions/lines: `modules/ui/src/button.rs::Button::render`, lines 493-575; `modules/frontend/src/native_composer/render.rs::NativeComposer::render`, lines 321-348 and 436-461; `modules/ui/src/input.rs`, lines 349-364; `modules/ui/src/textarea.rs`, lines 339-354.
- What is wrong: the application has a GPUI fork with AccessKit support and several custom controls call `.role/.aria_label`, but the shared Button renderer drops the accessible label when rendering icon-only content and sets no role/label/disabled state. The real composer text editor tracks focus and renders StyledText without a textbox role, label, value, selection or accessibility actions. Surrounding wrappers do not add these semantics to those controls.
- Why it matters: the core input and shared actions do not become actionable, named accessibility nodes. Keyboard focus and retained test metadata cannot substitute for platform semantics.
- Concrete failure scenario: a screen-reader user tries to find and edit the composer textbox or activate an icon-only Send/code-copy button. The native input handler serves IME/keyboard input, but no accessible textbox/value is published, and the shared button's validated AccessibleLabel never reaches AccessKit.
- Validation: inspected exact pinned GPUI commit `8d8c07348532be87463aa666a01c1dbcb28651df` through the local sibling repository `/home/sander/projects/artisanstreet/gpui-ce-fallback`. Its `_accessibility.rs`, lines 64-68, requires both an element ID and role for a node to be reported. `elements/div.rs::a11y_role`, lines 1913-1919, uses only the explicit override_role. This rejects the initial broad candidate that GPUI has no accessibility tree. Several module comments claiming no tree are stale.
- Recommended fix: connect the existing metadata to the fork's role/label/value/disabled and text-selection APIs. Add textbox semantics and edit/selection actions for composer input. Update stale GPUI capability comments.
- Example code: shared Button root should include `.role(gpui::Role::Button).aria_label(self.content.accessible_label())` and the disabled state; the composer needs the TextInput role plus the fork's value/selection/action integration.
- Tests to add: assert platform accessibility-tree nodes for shared buttons, composer text/value/selection, and disabled state. Run manual Narrator/NVDA/VoiceOver smoke tests on shipped platforms.

### FQ-06. Typing copies every pending image before upload deduplication
- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/native_composer/forge_draft.rs::unstored_attachments`, lines 213-228; `modules/frontend/src/native_application/composer_drafts.rs::sync_composer_draft`, lines 73-94; `modules/frontend/src/native_application/impl_message_flight.rs::observe_composer_change`, lines 252-255 and `begin_message_submission`, lines 199-200; subscription in `modules/frontend/src/native_application/impl_lifecycle.rs::new`, lines 112-114.
- What is wrong: `unstored_attachments` deep-copies each encoded image into a new `ComposerImage`. It is called for every composer observation before the already-uploading IDs are filtered, so keystrokes and selection changes copy buffers that the caller immediately discards. Send also copies every buffer merely to test whether the vector is empty.
- Why it matters: these allocations/copies occur on the GPUI thread. The per-image 32 MiB and per-draft 320 MiB bounds prevent unlimited single-draft input, but they permit large repeated copies during ordinary typing while an upload is outstanding.
- Concrete failure scenario: attach a 32 MiB image over a slow connection and keep typing. Each composer notification clones the 32 MiB payload even though `uploads` already contains its ID; ten ready pending images permit about 320 MiB of copying per observation. This increases latency, allocator pressure and peak memory without doing new work.
- Recommended fix: identify/filter pending upload IDs before materializing payloads. Return borrowed/shared attachment descriptors from the composer and clone bytes only once at transport admission if the protocol requires owned bytes. Add a cheap `has_unstored_attachments` predicate for Send.
- Example code: `composer.unstored_attachments_excluding(&uploads)` should skip known IDs before constructing `ComposerImage`; a boolean scan of `stored.is_none() && bytes.is_some()` replaces the allocation used for `.is_empty()`.
- Tests to add: a large pending attachment followed by many text/caret edits must not construct or copy additional upload payloads. Benchmark UI-side allocation/copy volume, including multiple concurrent pending attachments.

### FQ-07. Remembered drafts retain image buffers without a window-wide budget
- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/native_composer.rs::NativeComposer::scope_drafts`, lines 188-192; `switch_thread`, lines 573-599. `modules/frontend/src/native_composer/forge_draft.rs::ScopeDraft`, lines 44-50; `remember_scope_draft`, lines 71-95; `restore_scope_draft`, lines 107-113. Attachment buffers in `modules/frontend/src/native_composer_attachments.rs::ComposerAttachment`, lines 227-240.
- What is wrong: leaving each scope inserts a clone of its ready attachments into a HashMap. Those clones share `Arc<Vec<u8>>` payloads and thumbnails, so clearing the visible tray does not free them. Restoring clones the entry again without removing it. There is no entry-count, aggregate-byte, age or completed-upload eviction; the map persists for the whole host-scoped composer lifetime.
- Why it matters: limits apply to one draft, not all remembered drafts. A long session can retain gigabytes of already-persisted image data even when only an empty/new composer is shown.
- Concrete failure scenario: create unsent image drafts in ten different threads and navigate away from each. Each can retain up to the 320 MiB domain payload allowance; the same window can keep about 3.2 GiB of encoded bytes plus preview data until it is dropped or those individual entries are replaced.
- Recommended fix: introduce a byte/count budget for remembered scopes, pinning unsaved/pending data until it is acknowledged. Store lightweight Forge attachment references for saved inactive scopes and reload their bytes on demand. Evict clean least-recently-used drafts while preserving authoritative saved text/references.
- Tests to add: visit more scopes than the retention budget with distinct large image drafts; retained bytes stay bounded, dirty/pending scopes remain recoverable, and revisiting an evicted clean scope re-reads the Forge draft.

### FQ-08. Observation storage grows while every replay processes the full history
- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/engine_observation_state/state.rs::EngineObservationState`, lines 82-134; `apply_attributed`, lines 478-528; `pair_tool`, lines 906-946; `push_timeline`, lines 1163-1184. `modules/frontend/src/conversation_observation_projection.rs::project_activities`, lines 210-315 and 527-537. `modules/frontend/src/native_application/impl_service_events.rs::replay_observation_activity`, lines 874-914. `modules/frontend/src/native_application/history_paging.rs::apply_turn_work`, lines 394-410.
- What is wrong: seen event IDs/sequences and paired activity/timeline rows accumulate for the selected thread without pruning. Each replay walks retained rows, constructs/clones candidate bodies, sorts all candidates, and only then discards entries outside the scene window. The tool/terminal ordering vectors also use linear membership scans per new row despite the accompanying maps.
- Why it matters: a bounded scene and bounded per-commit/channel batch do not bound retained source state or work per frame. A tool-heavy thread gradually uses more memory and CPU. Per-new-row linear membership plus repeated full-history projection makes cumulative processing quadratic or worse across a long stream.
- Concrete failure scenario: leave a long run with thousands of distinct tool/terminal events selected, or expand a large settled turn's work pages. The transcript renders at most its configured window, but every later observation still rebuilds/sorts all retained activity candidates and event dedup sets keep growing.
- Safeguards checked: held-back settled turns delay loading; earlier transcript pages have a count limit; live thinking retains one block; replay is coalesced once per UI drain. None bounds cumulative live activity or already-expanded work. The backend reviewer confirmed that the 4,096 observation bound is channel capacity and 64 is per-commit batching, not cumulative retained run history.
- Recommended fix: bound retained presentation rows by loaded turns/recent activity and keep unsettled approvals/questions pinned. Preserve replay correctness using a durable watermark plus bounded out-of-order gaps instead of storing every identity indefinitely. Select the visible candidate window before cloning bodies, and update changed facts incrementally. Use map Entry occupancy to maintain insertion order without vector scans.
- Tests to add: stream a large distinct-event run and repeatedly update one existing tool; retained bytes and projection time must stay bounded by the loaded window, with unchanged facts left untouched. Verify reconnect dedup, late history pages and pending interactions after pruning.

### FQ-09. One UI service poll can drain an unbounded batch
- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/native_application/impl_service_events.rs::poll_service`, lines 9-38; `modules/frontend/src/native_transport_service/service_lifecycle.rs::try_recv`, lines 230-235. The producer bridge has capacity 64 in `spawn_for_host`, lines 79-80.
- What is wrong: the polling loop keeps receiving until the channel happens to be empty, appending to an unconstrained Vec, then handles the entire collected batch synchronously. Channel capacity bounds instantaneous queued events, but a concurrent producer can refill it while the UI drains, so it does not impose a per-poll work limit.
- Why it matters: sustained delivery or replay bursts can monopolize the GPUI thread, defer rendering/input and allocate a batch larger than the channel capacity. Coalescing activity replay helps only the final projection, not event collection and dispatch.
- Concrete failure scenario: a fast Forge replay emits updates while the background service continually refills the channel. One UI tick handles hundreds/thousands of updates before other interaction runs, even though the channel advertises a 64-event bound.
- Recommended fix: cap events and/or elapsed work per UI tick; handle a bounded batch, leave the remainder queued, and promptly schedule another tick. Apply the same budget to shutdown-drain loops if they run on the application executor.
- Example code: receive at most `MAX_EVENTS_PER_TICK` rather than `loop`, then let the next polling wake continue; use a time budget if handling costs vary.
- Tests to add: a producer that refills faster than dispatch must not grow one batch beyond the cap; keyboard/render progress continues while a large history replay is drained in order.

### FQ-10. Settings persistence is unsafe between Editor processes
- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/editor_settings/storage.rs::persist`, lines 150-161; process-local load/update contract in `modules/frontend/src/editor_settings.rs`, lines 3-9 and `update` (lines 233-247).
- What is wrong: every writer uses the same `settings.json.pending` sibling via truncate/write/rename, without interprocess locking or unique temporary files. Each process also persists its startup snapshot of the whole settings object, so independent updates overwrite other processes' values and unknown keys.
- Why it matters: two running Editor binaries may share one installation root. The fixed temporary path breaks the documented complete-document atomic-write guarantee under concurrent writers; stale snapshots lose preferences even when no corruption occurs.
- Concrete failure scenario: process A opens/truncates the pending file; B opens/truncates the same inode; A renames it over settings while B is still writing. B can write to the inode now visible at settings and its own rename fails. Separately, A changes the frame limit and B changes the reopen host from an old snapshot; B overwrites A's frame limit.
- Recommended fix: serialize update transactions with an install-root file lock, re-read/merge the current object under that lock, and use a unique same-directory create-new temporary file for each write. Preserve unknown keys from the latest on-disk object. If crash durability is promised, sync the file and parent directory as appropriate.
- Tests to add: synchronized concurrent writers cannot expose partial JSON, both independent changes survive, and failed temp-file operations cannot remove another writer's pending document. Include Windows replacement/rename behavior.

### FQ-11. A panicked transport thread never reports completion
- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/native_transport_service/service_lifecycle.rs::spawn_for_host`, lines 85-112; `is_finished`, lines 239-242; `join`, lines 245-259. `modules/frontend/src/native_application/workspace.rs::close_connection`, lines 425-453; `modules/frontend/src/native_application/app_entry.rs::request_app_shutdown`, lines 38-51.
- What is wrong: `finished` is set only after the service closure returns normally. With panic unwinding, the thread exits without setting it. `join` refuses to inspect the completed JoinHandle until this flag is true, so its advertised Panicked result becomes unreachable for that case. Quit passes no stop deadline and waits forever on the false flag.
- Why it matters: a recoverable worker panic in development/debug builds prevents service replacement and graceful application exit; shutdown/recovery state no longer reflects the actual worker lifetime.
- Concrete failure scenario: in an unwind-enabled development build, service handling panics. The event channel closes and the UI notices failure, but the completion bit remains false. Switching/recovery reaches its deadline without joining; pressing Quit enters the unbounded close loop although the worker is already dead.
- Scope limit: the custom production and production-debug profiles use/inherit `panic = "abort"`, so a panic there terminates the process instead of taking this hang path. This finding concerns unwind-enabled ordinary dev/test builds; it does not assert panic recovery in either packaged stage.
- Recommended fix: use a completion guard whose Drop sets the bit, or derive completion from JoinHandle::is_finished and allow joining a finished panicked thread. Give Quit a bounded stop policy consistent with the module's documented bounded wait, and surface worker panic distinctly from generic channel closure.
- Tests to add: deliberately panic a test service thread and verify completion is observed, join returns Panicked, recovery may replace it, and Quit completes within its deadline.

### FQ-12. The window root still owns unrelated state machines and presentation internals
- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/native_application.rs::NativeApplication`, lines 337-563; `modules/frontend/src/native_application/impl_lifecycle.rs::new`, lines 21-293; `modules/frontend/src/native_application/impl_service_events.rs::handle_service_event`, lines 66-323 and subscription/switch machinery, lines 401-737 and 1115-1517; `modules/frontend/src/native_application/impl_profile_menu.rs` (1,641 lines).
- What is wrong: NativeApplication owns transport/subscription flights, drafts/uploads, per-scope policy, catalogs, queue/run decisions, route caches, project intake, profile provider usage, and detailed hover/tween/focus/scroll state in one mutable object. Numerous `impl_*` files import `super::*` and add parent-owned methods; splitting those methods into files has not established independent state ownership or narrower mutation interfaces.
- Why it matters: a change in one feature must account for root-wide lifecycle/reset ordering and cross-feature fields. Review/test boundaries are difficult to identify, and admission/reset invariants can be bypassed by nearby methods. The draft admission bugs above are examples of failure-state coordination being spread across root methods and helper state.
- Concrete failure scenario: adding a new profile-menu refresh animation or host-scoped action requires initializing fields in the large root constructor and knowing which shutdown/switch paths should cancel them. A forgotten clear/fence can retain old host state or affect the newly mounted view; file-level separation alone cannot enforce that invariant.
- Recommended fix: move complete state and lifetime ownership into focused entities/coordinators (profile menu, draft/upload submission, selected-thread subscription/navigation). Expose typed events and narrow commands to the root. Preserve NativeWorkspace's useful host-lifetime boundary and the existing pure state machines; avoid a mechanical file split or generic callback/event bus that leaves the root coupled.
- Tests to add: owner-level transition/admission/reset tests and a small set of root integration tests for host switch, send and navigation. Each owner should have one explicit drop/cancel/reset contract.

### FQ-13. Context-window formatting overflows for accepted u64 values
- Severity: Low
- Confidence: High
- Exact files/functions/lines: `modules/catalog/rust/manifest.rs::context_window_label`, lines 154-162; `NativeContextWindowOption::display_label`, lines 138-143. Input parsing in `modules/catalog/rust/wire/decode.rs`, lines 307-320, and validation in `modules/catalog/rust/wire/validate.rs::validate_model_capabilities`, lines 336-349.
- What is wrong: million-token rounding adds 500,000 to an unrestricted u64. The wire accepts values near u64::MAX and capability validation imposes no token upper bound, so the formatter's arithmetic can overflow.
- Why it matters: malformed provider/Forge catalog data can panic an overflow-checked editor or show a wrapped nonsense label in release. This is a robustness edge case, not a claim that realistic model capacities approach the limit.
- Concrete failure scenario: a context option carries tokens = u64::MAX; opening its picker calls display_label and panics in debug, or produces `0M` with wrapping arithmetic in release.
- Validation: `.dist/review/catalog-max-repro.rs` links the existing compiled catalog crate, modifies a real discovery fixture's context option to u64::MAX, successfully encodes and decodes it through the production wire API, and asserts that calling the production formatter panics with overflow checks. Reproduction passed.
- Recommended fix: round with quotient/remainder (`tokens / 1_000_000 + u64::from(tokens % 1_000_000 >= 500_000)`), and optionally validate an explicit supported capacity range at the protocol boundary.
- Tests to add: u64::MAX and values just above u64::MAX - 500,000; normal thousand/million rounding boundary cases must stay unchanged.

### FQ-14. Resolved rich-link titles never enter the expiry-refresh path
- Severity: Low
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/conversation_surface.rs::SurfaceRichLinkTitles::resolved_title`, lines 461-472; `modules/frontend/src/rich_link_titles.rs::lookup`, lines 76-83; `request_candidate`, lines 88-104; `queue`, lines 108-113.
- What is wrong: resolved_title returns the cached title before consulting request_candidate. lookup deliberately returns expired resolved titles, so normal rendering never requests a refresh for an expired entry. If a caller does queue an expired entry, queue replaces it with Pending and erases the displayed title, contradicting the stale-title/no-flicker contract in the module documentation.
- Why it matters: retained previews ignore the backend expiry and can show obsolete titles for the entire surface lifetime. A future refresh integration would introduce label flicker unless the state model changes too.
- Concrete failure scenario: a link title resolves with a short expiry and the linked page changes. Re-rendering after expiry returns the old title indefinitely and sends no new request.
- Recommended fix: check refresh candidacy independently of whether a display title exists, and retain the last resolved title while a refresh is pending. Model cached display data separately from request state.
- Tests to add: keep a title visible while one refresh is admitted after expiry; repeated frames admit no duplicate refresh, and failure retains the stale title according to an explicit policy.

### FQ-15. Transient error notices lack announcement and keyboard lifetime control
- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/window_notice.rs::render_window_notice`, lines 95-177; `WindowNotices::push`, lines 315-335; `set_hovered`, lines 363-379; `tick`, lines 384-402. `modules/frontend/src/native_application/impl_window_notices.rs::window_notice_stack`, lines 45-65; callers `impl_recent_threads.rs`, line 248 and `impl_machines.rs`, line 94.
- What is wrong: the in-app display of failed-host/open-thread messages is a notice whose lifetime is four seconds. The card has no alert/status role or announcement text, its close button has no keyboard focus handle, and expiry pauses only while the pointer hovers. The application Tab route uses focus traversal, so it cannot enter or pause these controls.
- Why it matters: keyboard and screen-reader users can miss the reason an operation failed. Even a slow reader cannot retain the error using the available input path. The close button's existing Button role/label is a partial safeguard, but it does not make the surrounding message an announcement or provide ordinary keyboard lifetime control.
- Concrete failure scenario: a keyboard user opens a recent thread whose subscription is refused. The message disappears four seconds later while focus stays in the picker/composer. No announced error or persistent in-app error record is available to inspect the cause. Stderr and optional flight-recorder diagnostics do not provide this user-facing interaction.
- Recommended fix: publish alert/status semantics through the existing GPUI accessibility API, add focusable close/review controls, pause while keyboard focus is inside, and provide persistent error details or a repeatable status surface. Treat errors requiring user action differently from short informational notices.
- Tests to add: tree contains a named alert/status for the message; Tab reaches dismiss/review, focus pauses expiry, Escape/dismiss works, and an important error remains inspectable after the timer.

### FQ-16. Rich-link work is marked pending before its action is admitted
- Severity: Medium
- Confidence: High
- Exact files/functions/lines: `modules/frontend/src/conversation_surface/interactions.rs::flush_rich_link_requests`, lines 493-512; `enqueue_action`, lines 1344-1349; `modules/frontend/src/rich_link_titles.rs::request_candidate`, lines 88-104 and `queue`, lines 108-113.
- What is wrong: queue mutates every URL to Pending before enqueue_action checks the bounded surface action queue. If that queue is full, the request vector is dropped while Pending remains. Future frames suppress the URLs as already in-flight even though no host/service ever received them.
- Why it matters: transient local backpressure permanently loses preview work for otherwise ordinary links. This is distinct from FQ-03: it affects a single link below the retention limit and does not require LRU churn.
- Concrete failure scenario: a backed-up host effect outbox leaves the surface's action queue full. A new assistant message renders a link and the render-tail flush fails admission once. Once other actions drain, that link still reports Pending forever and never resolves.
- Recommended fix: admit one action transactionally before committing Pending, or retain the failed action for retry. Roll back only the entries owned by the failed admission, taking care not to undo an older live request.
- Tests to add: fill the surface action queue, render one new link, then drain actions and repaint; exactly one resolve action must eventually be emitted and Pending must correspond to admitted work.

## Final verification checkpoint

- Rust/Python declaration inventory: 1,360 files / 33,931 named callables / 40,700 declarations. Broad semantic flow review is separate from this inventory; generated methods, anonymous closures and every branch were not individually proved correct.
- Root transport/protocol/installer/CLI tests: 716 passed / 3 failed. Targeted database draft/upload/dispatch/interaction run: 32 passed. Provider unit run: 489 passed / 7 failed / 1 ignored. A broader owned-crate run reached stale migration expectations: 77 passed / 4 failed before stopping; overlapping runs are not summed as unique tests.
- Catalog tests: 19 passed. Assets: 24 passed. Python ratchet regressions: 5 passed. External Rust target registration: passed.
- Whole workspace all-target check: failed with three E0063 missing-decision initializers in two frontend test targets (R20); a successful complete workspace build/test run is not claimed.
- Formatter: failed on six files. File-size ratchet: failed on 72 files. Enabled default NixOS package reference: missing attribute reproduced by Nix evaluation.
- Actual-source reproductions: stale draft submission, ZIP executable-mode loss, Codex no-newline byte-budget bypass, and accepted u64 catalog formatting overflow. Small shims isolate the original modules; no provider side-effect or screen-reader runtime exploit was exercised.
- No published-release installation, native-window rendering, real-provider integration, platform assistive-technology run or Windows production execution was performed. All-target optional dependency reachability remains incomplete. RustSec matching was a primary-database version scan, not an official cargo-audit report.
- Source/user changes were preserved. This task writes review documentation and ignored review-only reproduction artifacts; it does not repair, format, stage or commit application code.
