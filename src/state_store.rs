use std::{collections::{HashMap, VecDeque}, sync::{Mutex, MutexGuard}, time::{Duration, Instant}};

use thiserror::Error;
use uuid::Uuid;

#[derive(Debug)]
pub(crate) struct ExpiringStore<T> {
    prefix: &'static str,
    capacity: usize,
    ttl: Duration,
    state: Mutex<StoreState<T>>,
}

impl<T> ExpiringStore<T> {
    pub(crate) fn new(prefix: &'static str, capacity: usize, ttl: Duration) -> Result<Self, StateStoreError> {
        if prefix.is_empty() || capacity == 0 || ttl == Duration::ZERO {
            return Err(StateStoreError::InvalidConfiguration);
        }
        Ok(Self { prefix, capacity, ttl, state: Mutex::new(StoreState::default()) })
    }

    pub(crate) fn insert(&self, value: T) -> Result<String, StateStoreError> {
        let now = Instant::now();
        let expires_at = now.checked_add(self.ttl).ok_or(StateStoreError::ExpirationOverflow)?;
        let mut state = self.lock_state();
        state.purge_expired(now);
        while state.entries.len() >= self.capacity {
            state.evict_oldest();
        }
        let token = format!("{}{}", self.prefix, Uuid::new_v4().simple());
        state.order.push_back(token.clone());
        state.entries.insert(token.clone(), StoredValue { value, expires_at });
        Ok(token)
    }

    pub(crate) fn get_cloned(&self, token: &str) -> Result<T, StateStoreError>
    where
        T: Clone,
    {
        self.validate_token(token)?;
        let now = Instant::now();
        let mut state = self.lock_state();
        state.purge_expired(now);
        state.entries.get(token).map(|entry| entry.value.clone()).ok_or(StateStoreError::UnknownOrExpired)
    }

    pub(crate) fn take(&self, token: &str) -> Result<T, StateStoreError> {
        self.validate_token(token)?;
        let now = Instant::now();
        let mut state = self.lock_state();
        state.purge_expired(now);
        state.entries.remove(token).map(|entry| entry.value).ok_or(StateStoreError::UnknownOrExpired)
    }

    pub(crate) fn len(&self) -> usize {
        let mut state = self.lock_state();
        state.purge_expired(Instant::now());
        state.entries.len()
    }

    fn validate_token(&self, token: &str) -> Result<(), StateStoreError> {
        let suffix = token.strip_prefix(self.prefix).ok_or(StateStoreError::UnknownOrExpired)?;
        if suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            Ok(())
        } else {
            Err(StateStoreError::UnknownOrExpired)
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, StoreState<T>> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug)]
struct StoreState<T> {
    entries: HashMap<String, StoredValue<T>>,
    order: VecDeque<String>,
}

impl<T> Default for StoreState<T> {
    fn default() -> Self {
        Self { entries: HashMap::new(), order: VecDeque::new() }
    }
}

impl<T> StoreState<T> {
    fn purge_expired(&mut self, now: Instant) {
        while let Some(token) = self.order.front().cloned() {
            match self.entries.get(&token) {
                Some(entry) if entry.expires_at <= now => {
                    self.order.pop_front();
                    self.entries.remove(&token);
                }
                None => { self.order.pop_front(); }
                Some(_) => break,
            }
        }
    }

    fn evict_oldest(&mut self) {
        while let Some(token) = self.order.pop_front() {
            if self.entries.remove(&token).is_some() { return; }
        }
    }
}

#[derive(Debug)]
struct StoredValue<T> { value: T, expires_at: Instant }

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StateStoreError {
    #[error("invalid state store configuration")]
    InvalidConfiguration,
    #[error("state expiration cannot be represented")]
    ExpirationOverflow,
    #[error("unknown or expired state token")]
    UnknownOrExpired,
}

#[cfg(test)]
mod tests {
    use std::thread;
    use super::*;

    #[test]
    fn resolves_evicts_and_expires_values() {
        let store = ExpiringStore::new("test_", 1, Duration::from_millis(5)).expect("store");
        let first = store.insert(1_u8).expect("first");
        let second = store.insert(2_u8).expect("second");
        assert!(store.get_cloned(&first).is_err());
        assert_eq!(store.get_cloned(&second).expect("second"), 2);
        thread::sleep(Duration::from_millis(20));
        assert!(store.get_cloned(&second).is_err());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn take_is_single_use() {
        let store = ExpiringStore::new("test_", 1, Duration::from_secs(60)).expect("store");
        let token = store.insert(String::from("value")).expect("insert");
        assert_eq!(store.take(&token).expect("take"), "value");
        assert!(store.take(&token).is_err());
    }
}
