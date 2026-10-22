//! Internal `MurmurHash3` support for deterministic rollout bucketing.

const BLOCK_MULTIPLIER: u32 = 0xcc9e_2d51;
const HASH_MULTIPLIER: u32 = 0x1b87_3593;
const HASH_OFFSET: u32 = 0xe654_6b64;
const FINAL_MULTIPLIER_ONE: u32 = 0x85eb_ca6b;
const FINAL_MULTIPLIER_TWO: u32 = 0xc2b2_ae35;
const BUCKET_COUNT: u32 = 100;
const HASH_QUOTIENT_RANGE: u32 = 42_949_673; // ceil(2^32 / 100)

/// Computes canonical `MurmurHash3` `x86_32` over the input's exact UTF-8 bytes.
///
/// Blocks are decoded as little-endian words. Every multiplication and addition
/// wraps modulo 2^32, including seed mixing and finalization.
///
/// The input contract is valid Rust UTF-8. Rust strings cannot contain lone
/// UTF-16 surrogates; a caller converting JavaScript strings must replace lone
/// surrogates with `U+FFFD` first, matching `TextEncoder` behavior.
pub(crate) fn murmurhash3(input: &str, seed: u32) -> u32 {
    let bytes = input.as_bytes();
    let (chunks, remaining) = bytes.as_chunks::<4>();
    let mut hash = seed;

    for chunk in chunks {
        let mut block = u32::from_le_bytes(*chunk);
        block = block.wrapping_mul(BLOCK_MULTIPLIER);
        block = block.rotate_left(15);
        block = block.wrapping_mul(HASH_MULTIPLIER);

        hash ^= block;
        hash = hash.rotate_left(13);
        hash = hash.wrapping_mul(5).wrapping_add(HASH_OFFSET);
    }

    let mut tail = 0_u32;
    if remaining.len() >= 3 {
        tail ^= u32::from(remaining[2]) << 16;
    }
    if remaining.len() >= 2 {
        tail ^= u32::from(remaining[1]) << 8;
    }
    if let Some(first) = remaining.first() {
        tail ^= u32::from(*first);
        tail = tail.wrapping_mul(BLOCK_MULTIPLIER);
        tail = tail.rotate_left(15);
        tail = tail.wrapping_mul(HASH_MULTIPLIER);
        hash ^= tail;
    }

    #[allow(
        clippy::cast_possible_truncation,
        reason = "MurmurHash3 x86_32 mixes the byte length modulo 2^32"
    )]
    let byte_length = bytes.len() as u32;
    hash ^= byte_length;
    hash ^= hash >> 16;
    hash = hash.wrapping_mul(FINAL_MULTIPLIER_ONE);
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(FINAL_MULTIPLIER_TWO);
    hash ^= hash >> 16;

    hash
}

/// Derives the stable seed for one account and flag.
///
/// This is `murmurhash3("{account_id}:{flag_key}", 0) % 100`. The modulo and
/// input format are compatibility requirements: changing either reshuffles
/// existing users across rollout buckets.
pub(crate) fn rollout_seed(account_id: &str, flag_key: &str) -> u32 {
    let seed_input = format!("{account_id}:{flag_key}");

    murmurhash3(&seed_input, 0) % BUCKET_COUNT
}

/// Returns the deterministic Flagship rollout bucket in the half-open range `[0, 100)`.
///
/// `attribute` must already use JavaScript string coercion semantics; this
/// function hashes it exactly as supplied with a seed from [`rollout_seed`]. It
/// then computes `(hash % 100) + floor(hash / 100) / ceil(2^32 / 100)`.
/// Changing any part of this formula reshuffles existing users.
/// Missing and null attributes must not be passed here: they require a random
/// bucket instead of deterministic hashing.
pub(crate) fn rollout_bucket(attribute: &str, seed: u32) -> f64 {
    let hash = murmurhash3(attribute, seed);

    bucket_from_hash(hash)
}

fn bucket_from_hash(hash: u32) -> f64 {
    let base = f64::from(hash % BUCKET_COUNT);
    let quotient = f64::from(hash / BUCKET_COUNT);

    base + quotient / f64::from(HASH_QUOTIENT_RANGE)
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::{bucket_from_hash, murmurhash3, rollout_bucket, rollout_seed};

    const ACCOUNT_ID: &str = "aaaabbbbccccdddd1111222233334444";
    const FLAG_KEY: &str = "rollout_test";

    #[derive(Deserialize)]
    struct VectorSuite {
        murmur: Vec<MurmurVector>,
    }

    #[derive(Deserialize)]
    struct MurmurVector {
        name: String,
        input: String,
        seed: u32,
        expected: u32,
    }

    #[test]
    fn matches_all_shared_hash_vectors() {
        let suite: VectorSuite = serde_json::from_str(include_str!("cases.json"))
            .expect("packaged local-evaluation vectors must be valid JSON");

        assert!(!suite.murmur.is_empty());
        for vector in suite.murmur {
            assert_eq!(
                murmurhash3(&vector.input, vector.seed),
                vector.expected,
                "{}",
                vector.name
            );
        }
    }

    #[test]
    fn hashes_exact_utf8_for_empty_unicode_and_long_inputs() {
        assert_eq!(murmurhash3("", 0), 0);
        assert_eq!(murmurhash3("🚀🎉", 0), 727_794_551);
        assert_eq!(murmurhash3("日本語", 45), 1_482_885_209);
        assert_eq!(murmurhash3(&"a".repeat(1_000), 7), 4_253_212_744);
    }

    #[test]
    fn hashes_every_tail_length_exactly() {
        let cases = [
            ("", 0),
            ("a", 1_009_084_850),
            ("ab", 2_613_040_991),
            ("abc", 3_017_643_002),
            ("abcd", 1_139_631_978),
        ];

        for (input, expected) in cases {
            assert_eq!(murmurhash3(input, 0), expected, "input {input:?}");
        }
    }

    #[test]
    fn computes_known_rollout_buckets() {
        let cases = [
            ("0", 0x402e_5124_345e_bb6f),
            ("1", 0x4021_b3dd_38f1_308b),
            ("2", 0x4056_d5ba_c794_a915),
            ("user-a", 0x404b_294b_192b_5ad4),
            ("日本語", 0x4022_b0c6_109d_3ce8),
            ("héllo", 0x4052_68fd_a56f_5c09),
            ("🚀", 0x4040_0c8e_c5a3_cdc5),
            ("", 0x4049_7cae_f32a_0d44),
        ];
        let seed = rollout_seed(ACCOUNT_ID, FLAG_KEY);

        assert_eq!(seed, 45);

        for (attribute, expected_bits) in cases {
            let actual = rollout_bucket(attribute, seed);

            assert_eq!(actual.to_bits(), expected_bits, "bucket for {attribute:?}");
        }
    }

    #[test]
    fn bucket_boundaries_remain_in_the_required_range() {
        assert_eq!(bucket_from_hash(0).to_bits(), 0x0000_0000_0000_0000);
        assert_eq!(bucket_from_hash(99).to_bits(), 0x4058_c000_0000_0000);

        // Largest u32 with remainder 99, and therefore the true maximum bucket.
        assert_eq!(
            bucket_from_hash(4_294_967_199).to_bits(),
            0x4058_ffff_ffce_0000
        );

        // This quotient distinguishes division from multiplication by a reciprocal.
        assert_eq!(
            bucket_from_hash(33_554_700).to_bits(),
            0x3f80_0008_5fbf_ffdf
        );
    }
}
