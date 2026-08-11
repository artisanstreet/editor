ALTER TABLE `threads` ADD `reader_activity_at` text DEFAULT '1970-01-01T00:00:00.000Z' NOT NULL;
UPDATE `threads` SET `reader_activity_at` = `last_activity_at`;
