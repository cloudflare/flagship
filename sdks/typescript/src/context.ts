import type { EvaluationContext } from '@openfeature/server-sdk';

/**
 * Utility for transforming OpenFeature evaluation context
 */
export class ContextTransformer {
	/**
	 * Transform OpenFeature evaluation context to query parameters
	 * for the Flagship API.
	 *
	 * Primitive values (`string`, `number`, `boolean`) and `Date` objects are
	 * serialized directly. Nested objects and arrays cannot be expressed as query
	 * parameters and are skipped.
	 *
	 * When a `droppedKeys` collector array is provided, skipped key names are
	 * pushed into it and **no** console warning is emitted — the caller is
	 * expected to handle the situation (e.g. throw `INVALID_CONTEXT`).
	 * When no collector is provided, a `console.warn` is emitted for each
	 * skipped key so the issue is still surfaced in development.
	 *
	 * @param context - OpenFeature evaluation context
	 * @param droppedKeys - Optional collector array; skipped key names are pushed here
	 */
	static toQueryParams(context: EvaluationContext, droppedKeys?: string[]): Record<string, string> {
		return Object.fromEntries(toSearchParams(context, droppedKeys));
	}

	/**
	 * Build URL with query parameters from context.
	 *
	 * @param baseUrl - The base evaluation endpoint URL
	 * @param flagKey - The flag key to evaluate
	 * @param context - OpenFeature evaluation context
	 * @param droppedKeys - Optional collector array; skipped context key names are pushed here
	 */
	static buildUrl(baseUrl: string, flagKey: string, context: EvaluationContext, droppedKeys?: string[]): string {
		return buildEvaluationUrl(new URL(baseUrl).toString(), flagKey, context, droppedKeys);
	}
}

export function buildEvaluationUrl(baseUrl: string, flagKey: string, context: EvaluationContext, droppedKeys?: string[]): string {
	const params = toSearchParams(context, droppedKeys, flagKey);

	if (!baseUrl.includes('?') && !baseUrl.includes('#')) return `${baseUrl}?${params}`;

	const url = new URL(baseUrl);
	for (const [key, value] of params) url.searchParams.set(key, value);
	return url.toString();
}

function toSearchParams(context: EvaluationContext, droppedKeys?: string[], flagKey?: string): URLSearchParams {
	const params = new URLSearchParams();
	if (flagKey !== undefined) params.set('flagKey', flagKey);

	for (const [key, value] of Object.entries(context)) {
		if (value === undefined || value === null) continue;

		if (value instanceof Date) {
			params.set(key, value.toISOString());
			continue;
		}

		if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') {
			params.set(key, String(value));
			continue;
		}

		if (typeof value === 'object') {
			if (droppedKeys) {
				droppedKeys.push(key);
			} else {
				console.warn(
					`[Flagship] Context key "${key}" is a complex object/array and cannot be serialized to a query parameter. ` +
						'This value will be ignored during flag evaluation.',
				);
			}
		}
	}

	return params;
}
