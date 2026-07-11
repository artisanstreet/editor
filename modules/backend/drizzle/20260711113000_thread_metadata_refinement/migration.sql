CREATE UNIQUE INDEX `journal_commands_refinement_source_unique` ON `journal_commands` (`thread_id`, `causation_id`) WHERE `origin` = 'backend' AND `payload_type` = 'thread.metadata.refine';
