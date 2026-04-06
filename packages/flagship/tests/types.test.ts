import { describe, it, expect } from 'vitest';
import { FlagshipError, FlagshipErrorCode, FLAGSHIP_DEFAULT_BASE_URL } from '../src/types.js';

describe('FlagshipError', () => {
	it('is an instance of Error', () => {
		const err = new FlagshipError('oops', FlagshipErrorCode.NETWORK_ERROR);
		expect(err).toBeInstanceOf(Error);
	});

	it('is an instance of FlagshipError', () => {
		const err = new FlagshipError('oops', FlagshipErrorCode.NETWORK_ERROR);
		expect(err).toBeInstanceOf(FlagshipError);
	});

	it('sets the message correctly', () => {
		const err = new FlagshipError('something failed', FlagshipErrorCode.PARSE_ERROR);
		expect(err.message).toBe('something failed');
	});

	it('sets the name to FlagshipError', () => {
		const err = new FlagshipError('oops', FlagshipErrorCode.TIMEOUT_ERROR);
		expect(err.name).toBe('FlagshipError');
	});

	it('sets the code property', () => {
		expect(new FlagshipError('x', FlagshipErrorCode.NETWORK_ERROR).code).toBe(FlagshipErrorCode.NETWORK_ERROR);
		expect(new FlagshipError('x', FlagshipErrorCode.TIMEOUT_ERROR).code).toBe(FlagshipErrorCode.TIMEOUT_ERROR);
		expect(new FlagshipError('x', FlagshipErrorCode.PARSE_ERROR).code).toBe(FlagshipErrorCode.PARSE_ERROR);
		expect(new FlagshipError('x', FlagshipErrorCode.INVALID_CONTEXT).code).toBe(FlagshipErrorCode.INVALID_CONTEXT);
	});

	it('stores cause when provided', () => {
		const cause = new Response(null, { status: 404 });
		const err = new FlagshipError('not found', FlagshipErrorCode.NETWORK_ERROR, cause);
		expect(err.cause).toBe(cause);
	});

	it('cause is undefined when not provided', () => {
		const err = new FlagshipError('oops', FlagshipErrorCode.NETWORK_ERROR);
		expect(err.cause).toBeUndefined();
	});

	it('instanceof check works after prototype fix', () => {
		function throwFlagshipError() {
			throw new FlagshipError('test', FlagshipErrorCode.NETWORK_ERROR);
		}
		try {
			throwFlagshipError();
		} catch (e) {
			expect(e instanceof FlagshipError).toBe(true);
			expect(e instanceof Error).toBe(true);
		}
	});
});

describe('FlagshipErrorCode', () => {
	it('has all expected string values', () => {
		expect(FlagshipErrorCode.NETWORK_ERROR).toBe('NETWORK_ERROR');
		expect(FlagshipErrorCode.TIMEOUT_ERROR).toBe('TIMEOUT_ERROR');
		expect(FlagshipErrorCode.PARSE_ERROR).toBe('PARSE_ERROR');
		expect(FlagshipErrorCode.INVALID_CONTEXT).toBe('INVALID_CONTEXT');
	});
});

describe('FLAGSHIP_DEFAULT_BASE_URL', () => {
	it('is a valid URL', () => {
		expect(() => new URL(FLAGSHIP_DEFAULT_BASE_URL)).not.toThrow();
	});

	it('uses https', () => {
		expect(FLAGSHIP_DEFAULT_BASE_URL.startsWith('https://')).toBe(true);
	});
});
