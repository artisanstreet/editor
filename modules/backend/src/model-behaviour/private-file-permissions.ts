/**
 * Compatibility surface for the Model Behaviour adapter.
 *
 * Private-file permission capture and restoration is not specific to Model
 * Behaviour: every harness config write needs it. The implementation now lives
 * in `harness-config/private-file-permissions.ts`; this module keeps the
 * original import path working for existing callers and suites.
 */
export * from "../harness-config/private-file-permissions";
