/**
 * In-process flag evaluation engine.
 *
 * @internal
 */

export type FlagValue = boolean | string | number | Record<string, unknown> | unknown[];
export type EvaluationReason = 'STATIC' | 'TARGETING_MATCH' | 'DEFAULT' | 'DISABLED' | 'SPLIT' | 'ERROR';
export type EvaluationContextValue =
	| string
	| number
	| boolean
	| null
	| EvaluationContextValue[]
	| { [key: string]: EvaluationContextValue };
export type EvaluationContext = Record<string, EvaluationContextValue>;

/** Thrown when persisted flag config is internally inconsistent. */
export class FlagConfigError extends Error {
	constructor(flagKey: string, message: string) {
		super(`Flag '${flagKey}' ${message}`);
		this.name = 'FlagConfigError';
		Object.setPrototypeOf(this, new.target.prototype);
	}
}

/** Evaluation-only rule — priority is stripped at write time after sorting. */
export interface EvalRule {
	conditions: Condition[];
	serve_variation: string;
	rollout?: { percentage: number; attribute?: string };
}

export interface EvalFlag {
	key: string;
	enabled: boolean;
	default_variation: string;
	variations: Record<string, unknown>;
	/** Rules pre-sorted by priority at write time. Priority field stripped — iteration order is evaluation order. */
	rules: EvalRule[];
}

export type Definitions = {
	flags: Record<string, EvalFlag>;
};

export type Operator =
	| 'equals'
	| 'not_equals'
	| 'greater_than'
	| 'less_than'
	| 'greater_than_or_equals'
	| 'less_than_or_equals'
	| 'contains'
	| 'starts_with'
	| 'ends_with'
	| 'in'
	| 'not_in'
	| 'has'
	| 'not_has';

export interface BaseCondition {
	attribute: string;
	operator: Operator;
	value: unknown;
}

export interface LogicalCondition {
	logical_operator: 'AND' | 'OR';
	clauses: Condition[];
}

export type Condition = BaseCondition | LogicalCondition;

const ISO_8601_REGEX = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})$/;
const MAX_PATH_DEPTH = 5;

const encoder = new TextEncoder();
const randomBuf = new Uint32Array(1);
const MAX_RETAINED_HASH_BYTES = 32 * 1024;
const HASH_QUOTIENT_RANGE = Math.ceil(2 ** 32 / 100);
let hashBuf = new Uint8Array(512);

function isPrimitive(value: unknown): boolean {
	return value === null || (typeof value !== 'object' && typeof value !== 'function');
}

function isScalar(value: unknown): value is string | number | boolean {
	return typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean';
}

/** Own-property check compatible with the package's ES2021 lib target. */
function hasOwn(obj: object, key: PropertyKey): boolean {
	return Object.prototype.hasOwnProperty.call(obj, key);
}

function getContextValue(context: EvaluationContext, attribute: string): unknown {
	if (hasOwn(context, attribute)) return context[attribute];
	if (attribute.indexOf('.') === -1) return undefined;

	const path = attribute.split('.');
	if (path.length > MAX_PATH_DEPTH) return undefined;

	let value: unknown = context;
	for (const segment of path) {
		if (isPrimitive(value) || !hasOwn(value as object, segment)) return undefined;
		value = (value as Record<string, unknown>)[segment];
	}
	return value;
}

type StringOperator = 'equals' | 'not_equals' | 'contains' | 'starts_with' | 'ends_with';
type OrderingOperator = 'greater_than' | 'less_than' | 'greater_than_or_equals' | 'less_than_or_equals';

function evaluateStringOperator(operator: StringOperator, attrValue: unknown, target: unknown): boolean {
	if (attrValue === null) return operator === 'not_equals';
	if (!isScalar(attrValue) || !isPrimitive(target)) return false;
	const actual = String(attrValue);
	const expected = String(target);

	switch (operator) {
		case 'equals':
			return actual === expected;
		case 'not_equals':
			return actual !== expected;
		case 'contains':
			return actual.includes(expected);
		case 'starts_with':
			return actual.startsWith(expected);
		case 'ends_with':
			return actual.endsWith(expected);
	}
}

function toNumber(value: string | number | boolean): number {
	return typeof value === 'string' && value.trim() === '' ? NaN : Number(value);
}

function compareValues(attrValue: unknown, target: unknown): number | undefined {
	if (!isScalar(attrValue) || !isScalar(target)) return undefined;

	let actual: number;
	let expected: number;

	if (typeof target === 'string' && ISO_8601_REGEX.test(target) && typeof attrValue === 'string') {
		actual = Date.parse(attrValue);
		if (Number.isNaN(actual)) {
			actual = toNumber(attrValue);
			expected = toNumber(target);
		} else {
			expected = Date.parse(target);
		}
	} else {
		actual = toNumber(attrValue);
		expected = toNumber(target);
	}

	if (Number.isNaN(actual) || Number.isNaN(expected)) return undefined;
	if (actual === expected) return 0;
	return actual < expected ? -1 : 1;
}

function evaluateOrderingOperator(operator: OrderingOperator, attrValue: unknown, target: unknown): boolean {
	const comparison = compareValues(attrValue, target);
	if (comparison === undefined) return false;

	switch (operator) {
		case 'greater_than':
			return comparison > 0;
		case 'less_than':
			return comparison < 0;
		case 'greater_than_or_equals':
			return comparison >= 0;
		case 'less_than_or_equals':
			return comparison <= 0;
	}
}

function containsValue(values: unknown[], target: unknown): boolean {
	const expected = String(target);
	for (const value of values) {
		if (value !== null && String(value) === expected) return true;
	}
	return false;
}

function containsPrimitiveValue(values: unknown[], target: unknown): boolean {
	const expected = String(target);
	for (const value of values) {
		if (isScalar(value) && String(value) === expected) return true;
	}
	return false;
}

/** MurmurHash3 (32-bit) over UTF-8 bytes. Returns the unsigned 32-bit hash. */
export function murmurhash3(str: string, seed: number): number {
	const requiredBytes = str.length * 3;
	let b: Uint8Array;
	let n: number;
	if (requiredBytes > MAX_RETAINED_HASH_BYTES) {
		b = encoder.encode(str);
		n = b.byteLength;
	} else {
		if (hashBuf.byteLength < requiredBytes) hashBuf = new Uint8Array(requiredBytes);
		b = hashBuf;
		n = encoder.encodeInto(str, b).written;
	}
	let h = seed >>> 0;
	let i = 0;
	while (i + 4 <= n) {
		let k = b[i]! | (b[i + 1]! << 8) | (b[i + 2]! << 16) | (b[i + 3]! << 24);
		k = Math.imul(k, 0xcc9e2d51) >>> 0;
		k = ((k << 15) | (k >>> 17)) >>> 0;
		k = Math.imul(k, 0x1b873593) >>> 0;
		h ^= k;
		h = ((h << 13) | (h >>> 19)) >>> 0;
		h = (Math.imul(h, 5) + 0xe6546b64) >>> 0;
		i += 4;
	}
	let k = 0;
	if (n - i >= 3) k ^= b[i + 2]! << 16;
	if (n - i >= 2) k ^= b[i + 1]! << 8;
	if (n > i) {
		k ^= b[i]!;
		k = Math.imul(k, 0xcc9e2d51) >>> 0;
		k = ((k << 15) | (k >>> 17)) >>> 0;
		k = Math.imul(k, 0x1b873593) >>> 0;
		h ^= k;
	}
	h ^= n;
	h ^= h >>> 16;
	h = Math.imul(h, 0x85ebca6b) >>> 0;
	h ^= h >>> 13;
	h = Math.imul(h, 0xc2b2ae35) >>> 0;
	h ^= h >>> 16;
	return h >>> 0;
}

function evaluateCondition(condition: Condition, context: EvaluationContext): boolean {
	if ('logical_operator' in condition) {
		const { logical_operator, clauses } = condition;
		if (logical_operator === 'AND') {
			for (const clause of clauses) {
				if (!evaluateCondition(clause, context)) return false;
			}
			return true;
		}
		for (const clause of clauses) {
			if (evaluateCondition(clause, context)) return true;
		}
		return false;
	}

	const { attribute, operator, value: target } = condition as BaseCondition;
	const attrValue = getContextValue(context, attribute);
	if (attrValue === undefined) return false;

	switch (operator) {
		case 'equals':
		case 'not_equals':
		case 'contains':
		case 'starts_with':
		case 'ends_with':
			return evaluateStringOperator(operator, attrValue, target);
		case 'greater_than':
		case 'less_than':
		case 'greater_than_or_equals':
		case 'less_than_or_equals':
			return evaluateOrderingOperator(operator, attrValue, target);
		case 'in':
		case 'not_in':
			if (!Array.isArray(target)) return false;
			if (attrValue === null) return operator === 'not_in';
			if (!isScalar(attrValue)) return false;
			return operator === 'in' ? containsValue(target, attrValue) : !containsValue(target, attrValue);
		case 'has':
		case 'not_has':
			if (!Array.isArray(attrValue) || !isPrimitive(target)) return false;
			return operator === 'has' ? containsPrimitiveValue(attrValue, target) : !containsPrimitiveValue(attrValue, target);
		default:
			return false;
	}
}

/**
 * Evaluate a single flag definition against the provided context.
 * Pure function — no I/O.
 */
export function evaluateFlag(
	flagDef: EvalFlag,
	context: EvaluationContext,
	accountId: string,
): { value: FlagValue; variant: string; reason: EvaluationReason } {
	const serve = (variant: string, reason: EvaluationReason) => {
		if (!hasOwn(flagDef.variations, variant)) {
			throw new FlagConfigError(flagDef.key, `variation '${variant}' is not defined`);
		}
		return { value: flagDef.variations[variant] as FlagValue, variant, reason };
	};

	if (!flagDef.enabled) return serve(flagDef.default_variation, 'DISABLED');
	if (flagDef.rules.length === 0) return serve(flagDef.default_variation, 'STATIC');

	// Seed derived from accountId+flagKey so the same targetingKey lands in different buckets
	// across different flags and accounts, preventing correlated rollouts.
	let seed: number | undefined;

	for (let i = 0; i < flagDef.rules.length; i++) {
		const rule = flagDef.rules[i]!;
		let ruleMatches = true;

		for (const condition of rule.conditions) {
			if (!evaluateCondition(condition, context)) {
				ruleMatches = false;
				break;
			}
		}

		const { rollout } = rule;
		const isSplit = rollout !== undefined && rollout.percentage < 100;
		if (ruleMatches && rollout) {
			const attr = getContextValue(context, rollout.attribute || 'targetingKey');
			if (attr !== null && attr !== undefined && !isScalar(attr)) {
				ruleMatches = false;
			} else if (isSplit) {
				// % 100 must stay: changing the seed would reshuffle every existing bucket.
				seed ??= murmurhash3(`${accountId}:${flagDef.key}`, 0) % 100;
				let bucket: number;
				if (isScalar(attr)) {
					const h = murmurhash3(String(attr), seed);
					bucket = (h % 100) + Math.floor(h / 100) / HASH_QUOTIENT_RANGE;
				} else {
					bucket = (globalThis.crypto.getRandomValues(randomBuf)[0]! / 0x100000000) * 100;
				}
				if (bucket >= rollout.percentage) ruleMatches = false;
			}
		}

		if (ruleMatches) {
			return serve(rule.serve_variation, isSplit ? 'SPLIT' : 'TARGETING_MATCH');
		}
	}

	return serve(flagDef.default_variation, 'DEFAULT');
}
