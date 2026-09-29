import type { EvaluationContext } from '@openfeature/server-sdk';
import { FlagshipError, FlagshipErrorCode } from './types.js';

export type NormalizedContextValue =
	| string
	| number
	| boolean
	| null
	| NormalizedContextValue[]
	| { [key: string]: NormalizedContextValue };

export interface NormalizedEvaluationContext {
	context: Record<string, NormalizedContextValue>;
	requiresPost: boolean;
}

export function normalizeEvaluationContext(context: EvaluationContext): NormalizedEvaluationContext {
	const normalized: Record<string, NormalizedContextValue> = {};
	let requiresPost = false;
	const ancestors = new WeakSet<object>();

	for (const [key, value] of Object.entries(context)) {
		if (value === undefined) continue;
		const result = normalizeContextValue(value, key, ancestors);
		setOwn(normalized, key, result.value);
		requiresPost ||= result.structured;
	}

	return { context: normalized, requiresPost };
}

function normalizeContextValue(
	value: unknown,
	path: string,
	ancestors: WeakSet<object>,
): { value: NormalizedContextValue; structured: boolean } {
	if (value === null) return { value: null, structured: true };
	if (typeof value === 'string' || typeof value === 'boolean') return { value, structured: false };
	if (typeof value === 'number') {
		if (Number.isFinite(value)) return { value, structured: false };
		throw invalidContext(path, 'numbers must be finite');
	}
	if (value instanceof Date) {
		if (Number.isNaN(value.getTime())) throw invalidContext(path, 'date is invalid');
		return { value: value.toISOString(), structured: false };
	}
	if (typeof value !== 'object') throw invalidContext(path, `unsupported value type '${typeof value}'`);
	if (ancestors.has(value)) throw invalidContext(path, 'cyclic values are not supported');

	ancestors.add(value);
	try {
		if (Array.isArray(value)) {
			return {
				value: value.map((item, index) => {
					if (item === undefined) throw invalidContext(`${path}[${index}]`, 'undefined is not supported inside arrays');
					return normalizeContextValue(item, `${path}[${index}]`, ancestors).value;
				}),
				structured: true,
			};
		}

		const prototype = Object.getPrototypeOf(value);
		if (prototype !== Object.prototype && prototype !== null) throw invalidContext(path, 'only plain objects are supported');

		const object: Record<string, NormalizedContextValue> = {};
		for (const [key, item] of Object.entries(value)) {
			if (item === undefined) throw invalidContext(`${path}.${key}`, 'undefined is not supported inside objects');
			setOwn(object, key, normalizeContextValue(item, `${path}.${key}`, ancestors).value);
		}
		return { value: object, structured: true };
	} finally {
		ancestors.delete(value);
	}
}

function setOwn(target: Record<string, NormalizedContextValue>, key: string, value: NormalizedContextValue): void {
	Object.defineProperty(target, key, { value, enumerable: true, writable: true, configurable: true });
}

function invalidContext(path: string, detail: string): FlagshipError {
	return new FlagshipError(`Evaluation context key "${path}" is invalid: ${detail}`, FlagshipErrorCode.INVALID_CONTEXT);
}

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
