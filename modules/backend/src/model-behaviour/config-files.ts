/**
 * Compatibility surface for the Model Behaviour adapter.
 *
 * The byte-exact config-file plane moved to `harness-config/file-store.ts` so
 * guidance, Model Behaviour, Marketplace mirrors, and future harness settings
 * publish through one implementation instead of three. These aliases preserve
 * the original vocabulary at the original import path; the classes are the
 * same identities, so `instanceof` checks are unaffected.
 */
export {
	ConfigFileBackupError as ModelBehaviourConfigFileBackupError,
	ConfigFileReadError as ModelBehaviourConfigFileReadError,
	ConfigFileReplaceError as ModelBehaviourConfigFileReplaceError,
	ConfigFileRestoreError as ModelBehaviourConfigFileRestoreError,
	ConfigFileStore as ModelBehaviourConfigFiles,
	ConfigFileStoreLive as ModelBehaviourConfigFilesLive,
	ConfigFileWriteError as ModelBehaviourConfigFileWriteError,
	make_config_file_store_layer as make_model_behaviour_config_files_layer,
	make_config_file_store_platform_layer as make_model_behaviour_config_files_platform_layer,
	type ConfigFileHooks as ModelBehaviourConfigFileHooks,
	type ConfigFileReplaceOptions as ModelBehaviourConfigFileReplaceOptions,
	type ConfigFileReplaceResult as ModelBehaviourConfigFileReplaceResult,
	type ConfigFileSnapshot as ModelBehaviourConfigFileSnapshot,
} from "../harness-config/file-store";
