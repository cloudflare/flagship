//! In-memory evaluation response cache.
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::context::{ExpectedFlagType, NormalizedContext};
use crate::error::FlagshipError;
use crate::types::FlagshipEvaluationResponse;

trait Clock: Send + Sync {
    fn now(&self) -> Duration;
}

struct MonotonicClock {
    origin: Instant,
}

impl MonotonicClock {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Clock for MonotonicClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

pub(crate) struct CacheToken {
    key: String,
    epoch: u64,
}

impl fmt::Debug for CacheToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CacheToken")
            .field("key_length", &self.key.len())
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub(crate) struct CacheLookup {
    pub(crate) response: Option<Arc<FlagshipEvaluationResponse>>,
    pub(crate) token: CacheToken,
}

struct Entry {
    response: Arc<FlagshipEvaluationResponse>,
    expires_at: Duration,
    last_access: u64,
}

#[derive(Default)]
struct State {
    items: HashMap<String, Entry>,
    recency: BTreeMap<u64, String>,
    access_sequence: u64,
    epoch: u64,
}

impl State {
    fn next_access(&mut self) -> u64 {
        self.access_sequence += 1;

        self.access_sequence
    }

    fn get_live(&mut self, key: &str, now: Duration) -> Option<Arc<FlagshipEvaluationResponse>> {
        let entry = self.items.get_mut(key)?;

        if now >= entry.expires_at {
            let expired_access = entry.last_access;
            self.recency.remove(&expired_access);
            self.items.remove(key);

            return None;
        }

        self.access_sequence += 1;
        let previous_access = std::mem::replace(&mut entry.last_access, self.access_sequence);
        let response = Arc::clone(&entry.response);
        if let Some(indexed_key) = self.recency.remove(&previous_access) {
            self.recency.insert(self.access_sequence, indexed_key);
        }

        Some(response)
    }

    fn insert(
        &mut self,
        key: String,
        response: Arc<FlagshipEvaluationResponse>,
        expires_at: Duration,
    ) {
        let last_access = self.next_access();
        let entry = Entry {
            response,
            expires_at,
            last_access,
        };

        if let Some(replaced) = self.items.insert(key.clone(), entry) {
            self.recency.remove(&replaced.last_access);
        }

        self.recency.insert(last_access, key);
    }

    fn evict_to_capacity(&mut self, capacity: usize) {
        while self.items.len() > capacity
            && let Some((_, key)) = self.recency.pop_first()
        {
            self.items.remove(&key);
        }
    }

    #[cfg(test)]
    fn indexes_are_consistent(&self) -> bool {
        self.items.len() == self.recency.len()
            && self.recency.iter().all(|(access, key)| {
                self.items
                    .get(key)
                    .is_some_and(|entry| entry.last_access == *access)
            })
            && self.items.iter().all(|(key, entry)| {
                self.recency
                    .get(&entry.last_access)
                    .is_some_and(|indexed_key| indexed_key == key)
            })
    }
}

/// Fixed-TTL LRU response cache. Reads don't extend expiry; expired entries are
/// removed lazily on lookup.
///
/// Clearing bumps an epoch so evaluations started before the clear cannot
/// repopulate the cache.
pub(crate) struct ResponseCache {
    ttl: Duration,
    capacity: NonZeroUsize,
    clock: Box<dyn Clock>,
    state: Mutex<State>,
}

impl ResponseCache {
    /// Creates a cache, or returns `None` when a zero TTL disables caching.
    pub(crate) fn new(ttl: Duration, capacity: NonZeroUsize) -> Option<Self> {
        Self::with_clock(ttl, capacity, Box::new(MonotonicClock::new()))
    }

    fn with_clock(ttl: Duration, capacity: NonZeroUsize, clock: Box<dyn Clock>) -> Option<Self> {
        if ttl.is_zero() {
            return None;
        }

        Some(Self {
            ttl,
            capacity,
            clock,
            state: Mutex::new(State::default()),
        })
    }

    pub(crate) const fn capacity(&self) -> NonZeroUsize {
        self.capacity
    }

    /// Looks up an evaluation and returns a token for a later insert.
    ///
    /// # Errors
    ///
    /// Returns an error if the context cannot be serialized into a cache key.
    pub(crate) fn lookup(
        &self,
        flag_key: &str,
        expected_type: ExpectedFlagType,
        context: &NormalizedContext,
    ) -> Result<CacheLookup, FlagshipError> {
        let key = context.cache_key(flag_key, expected_type)?;
        let mut state = self.lock_state();
        let now = self.clock.now();
        let epoch = state.epoch;

        let response = state.get_live(&key, now);
        drop(state);

        Ok(CacheLookup {
            response,
            token: CacheToken { key, epoch },
        })
    }

    /// Inserts a response; returns `false` if the cache was cleared since the token was issued.
    pub(crate) fn insert(&self, token: CacheToken, response: FlagshipEvaluationResponse) -> bool {
        let response = Arc::new(response);
        let mut state = self.lock_state();

        if token.epoch != state.epoch {
            return false;
        }

        let now = self.clock.now();
        let expires_at = now.saturating_add(self.ttl);
        state.insert(token.key, response, expires_at);
        state.evict_to_capacity(self.capacity.get());

        true
    }

    /// Removes every entry and invalidates outstanding lookup tokens.
    pub(crate) fn clear(&self) {
        let mut state = self.lock_state();
        state.items.clear();
        state.recency.clear();
        state.access_sequence = 0;
        state.epoch = state.epoch.wrapping_add(1);
    }

    fn lock_state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|_| panic!("response cache mutex poisoned"))
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;
    use std::time::Duration;

    use open_feature::EvaluationContext;
    use serde_json::{Value, json};

    use super::{CacheLookup, Clock, ResponseCache};
    use crate::context::{ExpectedFlagType, NormalizedContext, normalize_context};
    use crate::types::{EvaluationReason, FlagshipEvaluationResponse};

    #[derive(Default)]
    struct TestClock {
        now: Mutex<Duration>,
    }

    impl TestClock {
        fn set(&self, now: Duration) {
            *self.now.lock().unwrap() = now;
        }

        fn advance(&self, duration: Duration) {
            let mut now = self.now.lock().unwrap();
            *now = now.saturating_add(duration);
        }
    }

    impl Clock for Arc<TestClock> {
        fn now(&self) -> Duration {
            *self.now.lock().unwrap()
        }
    }

    struct BlockingClock {
        now: Duration,
        should_block: AtomicBool,
        entered: SyncSender<()>,
        release: Mutex<Receiver<()>>,
    }

    impl Clock for Arc<BlockingClock> {
        fn now(&self) -> Duration {
            if self.should_block.load(Ordering::SeqCst) {
                self.entered.send(()).unwrap();
                self.release.lock().unwrap().recv().unwrap();
            }

            self.now
        }
    }

    fn capacity(value: usize) -> NonZeroUsize {
        NonZeroUsize::new(value).unwrap()
    }

    fn cache(ttl: Duration, max_size: usize, clock: &Arc<TestClock>) -> ResponseCache {
        ResponseCache::with_clock(ttl, capacity(max_size), Box::new(Arc::clone(clock))).unwrap()
    }

    fn empty_context() -> NormalizedContext {
        normalize_context(&EvaluationContext::default()).unwrap()
    }

    fn lookup(
        cache: &ResponseCache,
        flag_key: &str,
        expected_type: ExpectedFlagType,
        context: &NormalizedContext,
    ) -> CacheLookup {
        cache.lookup(flag_key, expected_type, context).unwrap()
    }

    fn response(flag_key: &str, value: Value) -> FlagshipEvaluationResponse {
        FlagshipEvaluationResponse::new(flag_key, value)
    }

    fn insert(
        cache: &ResponseCache,
        flag_key: &str,
        expected_type: ExpectedFlagType,
        context: &NormalizedContext,
        response: FlagshipEvaluationResponse,
    ) {
        let token = lookup(cache, flag_key, expected_type, context).token;

        assert!(cache.insert(token, response));
    }

    fn contains(cache: &ResponseCache, flag_key: &str) -> bool {
        lookup(cache, flag_key, ExpectedFlagType::Bool, &empty_context())
            .response
            .is_some()
    }

    fn assert_consistent(cache: &ResponseCache) {
        assert!(cache.lock_state().indexes_are_consistent());
    }

    #[test]
    fn zero_ttl_disables_cache() {
        assert!(ResponseCache::new(Duration::ZERO, capacity(1)).is_none());
    }

    #[test]
    fn exact_ttl_boundary_is_expired_but_one_nanosecond_before_is_fresh() {
        let clock = Arc::new(TestClock::default());
        let cache = cache(Duration::from_secs(10), 2, &clock);
        let context = empty_context();
        insert(
            &cache,
            "flag",
            ExpectedFlagType::Bool,
            &context,
            response("flag", json!(true)),
        );

        clock.advance(Duration::from_secs(10) - Duration::from_nanos(1));
        assert!(
            lookup(&cache, "flag", ExpectedFlagType::Bool, &context)
                .response
                .is_some()
        );
        clock.advance(Duration::from_nanos(1));

        assert!(
            lookup(&cache, "flag", ExpectedFlagType::Bool, &context)
                .response
                .is_none()
        );
        assert!(cache.lock_state().items.is_empty());
        assert_consistent(&cache);
    }

    #[test]
    fn reads_do_not_slide_ttl() {
        let clock = Arc::new(TestClock::default());
        let cache = cache(Duration::from_secs(10), 2, &clock);
        let context = empty_context();
        insert(
            &cache,
            "flag",
            ExpectedFlagType::Bool,
            &context,
            response("flag", json!(true)),
        );

        clock.advance(Duration::from_secs(6));
        assert!(
            lookup(&cache, "flag", ExpectedFlagType::Bool, &context)
                .response
                .is_some()
        );
        clock.advance(Duration::from_secs(4));

        assert!(
            lookup(&cache, "flag", ExpectedFlagType::Bool, &context)
                .response
                .is_none()
        );
    }

    #[test]
    fn replacement_resets_ttl_and_recency() {
        let clock = Arc::new(TestClock::default());
        let cache = cache(Duration::from_secs(10), 2, &clock);
        let context = empty_context();
        insert(
            &cache,
            "a",
            ExpectedFlagType::Int,
            &context,
            response("a", json!(1)),
        );
        insert(
            &cache,
            "b",
            ExpectedFlagType::Int,
            &context,
            response("b", json!(1)),
        );

        clock.advance(Duration::from_secs(9));
        insert(
            &cache,
            "a",
            ExpectedFlagType::Int,
            &context,
            response("a", json!(2)),
        );
        insert(
            &cache,
            "c",
            ExpectedFlagType::Int,
            &context,
            response("c", json!(3)),
        );
        clock.advance(Duration::from_secs(9));

        assert_eq!(
            lookup(&cache, "a", ExpectedFlagType::Int, &context)
                .response
                .unwrap()
                .value,
            json!(2)
        );
        assert!(
            lookup(&cache, "b", ExpectedFlagType::Int, &context)
                .response
                .is_none()
        );
        assert_consistent(&cache);
    }

    #[test]
    fn reads_update_lru_order() {
        let clock = Arc::new(TestClock::default());
        let cache = cache(Duration::from_secs(10), 2, &clock);
        let context = empty_context();
        insert(
            &cache,
            "a",
            ExpectedFlagType::Bool,
            &context,
            response("a", json!(true)),
        );
        insert(
            &cache,
            "b",
            ExpectedFlagType::Bool,
            &context,
            response("b", json!(true)),
        );
        assert!(contains(&cache, "a"));
        insert(
            &cache,
            "c",
            ExpectedFlagType::Bool,
            &context,
            response("c", json!(true)),
        );

        assert!(contains(&cache, "a"));
        assert!(!contains(&cache, "b"));
        assert!(contains(&cache, "c"));
        assert_consistent(&cache);
    }

    #[test]
    fn capacity_one_replaces_the_only_entry() {
        let clock = Arc::new(TestClock::default());
        let cache = cache(Duration::from_secs(10), 1, &clock);
        let context = empty_context();
        insert(
            &cache,
            "a",
            ExpectedFlagType::Bool,
            &context,
            response("a", json!(true)),
        );
        insert(
            &cache,
            "b",
            ExpectedFlagType::Bool,
            &context,
            response("b", json!(true)),
        );

        assert!(!contains(&cache, "a"));
        assert!(contains(&cache, "b"));
        assert_consistent(&cache);
    }

    #[test]
    fn misses_do_not_advance_the_access_sequence() {
        let clock = Arc::new(TestClock::default());
        let cache = cache(Duration::from_secs(10), 2, &clock);
        let context = empty_context();
        insert(
            &cache,
            "flag",
            ExpectedFlagType::Bool,
            &context,
            response("flag", json!(true)),
        );
        let before = cache.lock_state().access_sequence;

        assert!(!contains(&cache, "missing"));
        assert_eq!(cache.lock_state().access_sequence, before);
        assert!(contains(&cache, "flag"));
        assert_eq!(cache.lock_state().access_sequence, before + 1);
        assert_eq!(cache.capacity().get(), 2);
    }

    #[test]
    fn huge_ttl_saturates_expiry() {
        let clock = Arc::new(TestClock::default());
        clock.set(Duration::MAX.checked_sub(Duration::from_secs(1)).unwrap());
        let cache = cache(Duration::MAX, 1, &clock);
        let context = empty_context();
        insert(
            &cache,
            "flag",
            ExpectedFlagType::Bool,
            &context,
            response("flag", json!(true)),
        );

        assert!(
            lookup(&cache, "flag", ExpectedFlagType::Bool, &context)
                .response
                .is_some()
        );
    }

    #[test]
    fn recency_index_stays_consistent_across_mixed_operations() {
        let clock = Arc::new(TestClock::default());
        let cache = cache(Duration::from_secs(10), 2, &clock);
        let context = empty_context();
        insert(
            &cache,
            "a",
            ExpectedFlagType::Bool,
            &context,
            response("a", json!(true)),
        );
        assert_consistent(&cache);
        insert(
            &cache,
            "b",
            ExpectedFlagType::Bool,
            &context,
            response("b", json!(true)),
        );
        assert_consistent(&cache);

        assert!(contains(&cache, "a"));
        assert_consistent(&cache);
        insert(
            &cache,
            "a",
            ExpectedFlagType::Bool,
            &context,
            response("a", json!(false)),
        );
        assert_consistent(&cache);
        insert(
            &cache,
            "c",
            ExpectedFlagType::Bool,
            &context,
            response("c", json!(true)),
        );
        assert_consistent(&cache);

        clock.advance(Duration::from_secs(10));
        assert!(!contains(&cache, "a"));
        assert!(!contains(&cache, "c"));
        assert_consistent(&cache);

        cache.clear();
        assert_consistent(&cache);
    }

    #[test]
    fn clear_rejects_stale_insertions() {
        let clock = Arc::new(TestClock::default());
        let cache = cache(Duration::from_secs(10), 2, &clock);
        let context = empty_context();
        let stale = lookup(&cache, "flag", ExpectedFlagType::Bool, &context).token;

        cache.clear();

        assert!(!cache.insert(stale, response("flag", json!(true))));
        assert!(!contains(&cache, "flag"));
        assert_consistent(&cache);
    }

    #[test]
    fn token_debug_redacts_canonical_key() {
        let clock = Arc::new(TestClock::default());
        let cache = cache(Duration::from_secs(10), 1, &clock);
        let context = normalize_context(
            &EvaluationContext::default().with_custom_field("secret", "must-not-leak"),
        )
        .unwrap();
        let token = lookup(&cache, "sensitive-flag", ExpectedFlagType::String, &context).token;
        let debug = format!("{token:?}");

        assert!(debug.contains("key_length"));
        assert!(debug.contains("epoch"));
        assert!(!debug.contains("must-not-leak"));
        assert!(!debug.contains("sensitive-flag"));
    }

    #[test]
    fn keys_distinguish_flag_type_and_structured_context() {
        let clock = Arc::new(TestClock::default());
        let cache = cache(Duration::from_secs(10), 8, &clock);
        let empty = empty_context();
        let array = normalize_context(
            &EvaluationContext::default().with_custom_field("items", Arc::new(json!([1, null]))),
        )
        .unwrap();
        let reversed = normalize_context(
            &EvaluationContext::default().with_custom_field("items", Arc::new(json!([null, 1]))),
        )
        .unwrap();
        let null = normalize_context(
            &EvaluationContext::default().with_custom_field("items", Arc::new(Value::Null)),
        )
        .unwrap();
        insert(
            &cache,
            "flag",
            ExpectedFlagType::Object,
            &array,
            response("flag", json!({"ok": true})),
        );

        assert!(
            lookup(&cache, "other", ExpectedFlagType::Object, &array)
                .response
                .is_none()
        );
        assert!(
            lookup(&cache, "flag", ExpectedFlagType::String, &array)
                .response
                .is_none()
        );
        assert!(
            lookup(&cache, "flag", ExpectedFlagType::Object, &empty)
                .response
                .is_none()
        );
        assert!(
            lookup(&cache, "flag", ExpectedFlagType::Object, &reversed)
                .response
                .is_none()
        );
        assert!(
            lookup(&cache, "flag", ExpectedFlagType::Object, &null)
                .response
                .is_none()
        );
        assert!(
            lookup(&cache, "flag", ExpectedFlagType::Object, &array)
                .response
                .is_some()
        );
    }

    #[test]
    fn hits_share_cached_storage_without_cloning() {
        let clock = Arc::new(TestClock::default());
        let cache = cache(Duration::from_secs(10), 2, &clock);
        let context = empty_context();
        insert(
            &cache,
            "flag",
            ExpectedFlagType::Object,
            &context,
            response("flag", json!({"nested": {"value": 1}})),
        );

        let first = lookup(&cache, "flag", ExpectedFlagType::Object, &context)
            .response
            .unwrap();
        let second = lookup(&cache, "flag", ExpectedFlagType::Object, &context)
            .response
            .unwrap();

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(second.value, json!({"nested": {"value": 1}}));
        assert_eq!(second.reason, EvaluationReason::DEFAULT);
    }

    #[test]
    fn clock_is_sampled_after_acquiring_cache_lock() {
        let (entered_sender, entered_receiver) = sync_channel(0);
        let (release_sender, release_receiver) = sync_channel(0);
        let clock = Arc::new(BlockingClock {
            now: Duration::ZERO,
            should_block: AtomicBool::new(false),
            entered: entered_sender,
            release: Mutex::new(release_receiver),
        });
        let cache = Arc::new(
            ResponseCache::with_clock(
                Duration::from_secs(10),
                capacity(1),
                Box::new(Arc::clone(&clock)),
            )
            .unwrap(),
        );
        let context = Arc::new(empty_context());
        let token = lookup(&cache, "flag", ExpectedFlagType::Bool, &context).token;
        assert!(cache.insert(token, response("flag", json!(true))));
        clock.should_block.store(true, Ordering::SeqCst);

        let lookup_cache = Arc::clone(&cache);
        let lookup_context = Arc::clone(&context);
        let lookup_thread = thread::spawn(move || {
            lookup(
                &lookup_cache,
                "flag",
                ExpectedFlagType::Bool,
                &lookup_context,
            )
        });
        entered_receiver.recv().unwrap();

        let start_clear = Arc::new(Barrier::new(2));
        let clear_barrier = Arc::clone(&start_clear);
        let clear_cache = Arc::clone(&cache);
        let (cleared_sender, cleared_receiver) = sync_channel(0);
        let clear_thread = thread::spawn(move || {
            clear_barrier.wait();
            clear_cache.clear();
            cleared_sender.send(()).unwrap();
        });
        start_clear.wait();
        assert!(
            cleared_receiver
                .recv_timeout(Duration::from_millis(20))
                .is_err()
        );

        release_sender.send(()).unwrap();
        assert!(lookup_thread.join().unwrap().response.is_some());
        cleared_receiver.recv().unwrap();
        clear_thread.join().unwrap();
        clock.should_block.store(false, Ordering::SeqCst);
        assert!(!contains(&cache, "flag"));
    }

    #[test]
    fn poisoned_mutex_is_not_recovered() {
        let clock = Arc::new(TestClock::default());
        let cache = Arc::new(cache(Duration::from_secs(10), 2, &clock));
        let poison_cache = Arc::clone(&cache);

        assert!(
            thread::spawn(move || {
                let _guard = poison_cache.state.lock().unwrap();
                panic!("poison cache mutex");
            })
            .join()
            .is_err()
        );

        let operation_cache = Arc::clone(&cache);
        assert!(
            thread::spawn(move || operation_cache.clear())
                .join()
                .is_err()
        );
    }

    #[test]
    fn concurrent_access_preserves_indexes_and_expires_all_entries() {
        let clock = Arc::new(TestClock::default());
        let cache = Arc::new(cache(Duration::from_secs(60), 16, &clock));
        let context = Arc::new(empty_context());
        insert(
            &cache,
            "shared",
            ExpectedFlagType::Int,
            &context,
            response("shared", json!(true)),
        );
        let start = Arc::new(Barrier::new(9));
        let hit_seen = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();

        for worker in 0..8 {
            let cache = Arc::clone(&cache);
            let context = Arc::clone(&context);
            let start = Arc::clone(&start);
            let hit_seen = Arc::clone(&hit_seen);
            threads.push(thread::spawn(move || {
                start.wait();
                if lookup(&cache, "shared", ExpectedFlagType::Int, &context)
                    .response
                    .is_some()
                {
                    hit_seen.store(true, Ordering::SeqCst);
                }

                for iteration in 0..500 {
                    let key = format!("flag-{}", (worker + iteration) % 24);
                    let result = lookup(&cache, &key, ExpectedFlagType::Int, &context);

                    if result.response.is_none() {
                        let _ = cache.insert(result.token, response(&key, json!(iteration)));
                    }

                    if iteration % 137 == 0 {
                        cache.clear();
                    }
                }
            }));
        }

        start.wait();

        for thread in threads {
            thread.join().unwrap();
        }

        assert!(hit_seen.load(Ordering::SeqCst));
        assert_consistent(&cache);
        assert!(cache.lock_state().items.len() <= 16);
        clock.advance(Duration::from_secs(60));

        for index in 0..24 {
            let key = format!("flag-{index}");
            assert!(
                lookup(&cache, &key, ExpectedFlagType::Int, &context)
                    .response
                    .is_none()
            );
        }

        let state = cache.lock_state();
        assert!(state.items.is_empty());
        assert!(state.recency.is_empty());
        assert!(state.indexes_are_consistent());
    }

    #[test]
    fn duplicate_miss_tokens_may_both_insert() {
        let clock = Arc::new(TestClock::default());
        let cache = cache(Duration::from_secs(10), 2, &clock);
        let context = empty_context();
        let first = lookup(&cache, "flag", ExpectedFlagType::Bool, &context).token;
        let second = lookup(&cache, "flag", ExpectedFlagType::Bool, &context).token;

        assert!(cache.insert(first, response("flag", json!(false))));
        assert!(cache.insert(second, response("flag", json!(true))));
        assert_eq!(
            lookup(&cache, "flag", ExpectedFlagType::Bool, &context)
                .response
                .unwrap()
                .value,
            json!(true)
        );
        assert_consistent(&cache);
    }
}
