/**
 * Flag metadata helpers shared by the server and client providers.
 *
 * @internal
 */

/**
 * Converts a metadata payload into a frozen OpenFeature `flagMetadata` record.
 * Absent or malformed payloads yield `{}`, and non-primitive entries are dropped.
 */
export function toFlagMetadata(value: unknown): Record<string, string | number | boolean> {
	const out: Record<string, string | number | boolean> = {};
	if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
		for (const [key, entry] of Object.entries(value)) {
			if (typeof entry === 'string' || typeof entry === 'boolean' || (typeof entry === 'number' && Number.isFinite(entry))) {
				// defineProperty keeps a `__proto__` key as an own property instead of setting the prototype.
				Object.defineProperty(out, key, { value: entry, enumerable: true });
			}
		}
	}
	return Object.freeze(out);
}
