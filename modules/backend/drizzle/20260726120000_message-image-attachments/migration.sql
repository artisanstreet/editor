CREATE TABLE `message_image_attachments` (
	`attachment_id` text PRIMARY KEY NOT NULL,
	`message_id` text NOT NULL,
	`name` text NOT NULL,
	`media_type` text NOT NULL,
	`size_bytes` integer NOT NULL,
	`content` blob NOT NULL,
	`position` integer NOT NULL,
	CONSTRAINT `message_image_attachments_media_type_check` CHECK (`message_image_attachments`.`media_type` IN ('image/gif', 'image/jpeg', 'image/png', 'image/webp'))
);
--> statement-breakpoint
CREATE INDEX `message_image_attachments_message_index` ON `message_image_attachments` (`message_id`,`position`);
