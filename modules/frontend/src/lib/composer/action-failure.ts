/**
 * What a composer action refused, in the words the reader gets.
 *
 * A title naming the action and a description carrying the cause, which is the
 * shape the removed notification service took and the shape the surface that
 * replaced it still reads.
 */
export interface ComposerActionFailure {
	readonly description: string;
	readonly title: string;
}
