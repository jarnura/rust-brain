//! Per-key rate limiter using fixed-window token buckets.
//!
//! Each API key gets its own bucket with the rate limit configured in the
//! `api_keys` table. Supports the three headers required by ADR-007:
//! - `X-RateLimit-Limit`: max requests per minute
//! - `X-RateLimit-Remaining`: remaining requests in current window
//! - `X-RateLimit-Reset`: Unix timestamp when the window resets

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

/// Result of a rate limit check.
#[derive(Debug)]
pub struct RateLimitResult {
    /// Whether the request is allowed.
    pub allowed: bool,
    /// Maximum requests per minute for this key.
    pub limit: u32,
    /// Remaining requests in the current window.
    pub remaining: u32,
    /// Unix timestamp (seconds) when the current window resets.
    pub reset_at_secs: u64,
}

/// Per-key token bucket state using a fixed-window algorithm.
struct KeyBucket {
    /// Configured rate limit (requests per minute).
    rate_limit_per_minute: u32,
    /// Remaining tokens in the current window.
    remaining: u32,
    /// Start of the current rate-limit window.
    window_start: Instant,
}

impl KeyBucket {
    fn new(rate_limit_per_minute: u32) -> Self {
        Self {
            rate_limit_per_minute,
            remaining: rate_limit_per_minute,
            window_start: Instant::now(),
        }
    }
}

/// Per-key rate limiter using fixed-window token buckets.
///
/// Each API key ID maps to its own bucket. Buckets are created on first
/// access and reset every 60 seconds. When a key's rate limit changes
/// (e.g., via PATCH /api/keys), the next check picks up the new value.
pub struct PerKeyRateLimiter {
    buckets: RwLock<HashMap<String, KeyBucket>>,
}

impl PerKeyRateLimiter {
    /// Creates a new empty rate limiter.
    pub fn new() -> Self {
        Self {
            buckets: RwLock::new(HashMap::new()),
        }
    }

    /// Checks whether a request from the given key is allowed.
    ///
    /// Returns a [`RateLimitResult`] with remaining capacity and reset time
    /// regardless of whether the request is allowed or denied.
    pub fn check(&self, key_id: &str, rate_limit_per_minute: u32) -> RateLimitResult {
        let now = Instant::now();
        let window_duration = Duration::from_secs(60);

        let mut buckets = self.buckets.write().unwrap();

        let bucket = buckets.entry(key_id.to_string()).or_insert_with(|| {
            tracing::debug!(
                key_id = key_id,
                rate_limit = rate_limit_per_minute,
                "Creating new rate limit bucket"
            );
            KeyBucket::new(rate_limit_per_minute)
        });

        if now.duration_since(bucket.window_start) >= window_duration {
            bucket.window_start = now;
            bucket.remaining = bucket.rate_limit_per_minute;
        }

        if bucket.rate_limit_per_minute != rate_limit_per_minute {
            tracing::debug!(
                key_id = key_id,
                old_limit = bucket.rate_limit_per_minute,
                new_limit = rate_limit_per_minute,
                "Rate limit changed for key"
            );
            bucket.rate_limit_per_minute = rate_limit_per_minute;
            bucket.remaining = bucket.remaining.min(rate_limit_per_minute);
        }

        let reset_at_secs = bucket.window_start.elapsed().as_secs();
        let reset_at_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + (60 - reset_at_secs.min(60));

        if bucket.remaining > 0 {
            bucket.remaining -= 1;
            RateLimitResult {
                allowed: true,
                limit: rate_limit_per_minute,
                remaining: bucket.remaining,
                reset_at_secs: reset_at_unix,
            }
        } else {
            RateLimitResult {
                allowed: false,
                limit: rate_limit_per_minute,
                remaining: 0,
                reset_at_secs: reset_at_unix,
            }
        }
    }

    /// Removes a key's bucket. Called when a key is revoked.
    pub fn remove_key(&self, key_id: &str) {
        let mut buckets = self.buckets.write().unwrap();
        buckets.remove(key_id);
    }

    /// Returns the number of active buckets (for diagnostics).
    #[allow(dead_code)]
    pub fn active_keys(&self) -> usize {
        self.buckets.read().unwrap().len()
    }
}

impl Default for PerKeyRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allows_within_limit() {
        let limiter = PerKeyRateLimiter::new();
        let result = limiter.check("key1", 5);
        assert!(result.allowed);
        assert_eq!(result.limit, 5);
        assert_eq!(result.remaining, 4);
    }

    #[test]
    fn test_denies_at_limit() {
        let limiter = PerKeyRateLimiter::new();
        for _ in 0..5 {
            let result = limiter.check("key1", 5);
            assert!(result.allowed);
        }
        let result = limiter.check("key1", 5);
        assert!(!result.allowed);
        assert_eq!(result.remaining, 0);
    }

    #[test]
    fn test_different_keys_independent() {
        let limiter = PerKeyRateLimiter::new();
        for _ in 0..3 {
            assert!(limiter.check("key1", 3).allowed);
        }
        assert!(!limiter.check("key1", 3).allowed);
        assert!(limiter.check("key2", 3).allowed);
    }

    #[test]
    fn test_per_key_rate_limits() {
        let limiter = PerKeyRateLimiter::new();

        // Admin key: 120/min
        let result = limiter.check("admin-key", 120);
        assert!(result.allowed);
        assert_eq!(result.limit, 120);

        // Readonly key: 30/min
        let result = limiter.check("readonly-key", 30);
        assert!(result.allowed);
        assert_eq!(result.limit, 30);
    }

    #[test]
    fn test_rate_limit_change_picked_up() {
        let limiter = PerKeyRateLimiter::new();

        // Start with 5/min
        for _ in 0..5 {
            assert!(limiter.check("key1", 5).allowed);
        }
        assert!(!limiter.check("key1", 5).allowed);

        // Change to 10/min - remaining is capped at current (0), but new window
        // would give 10. Since we're in the same window, remaining stays 0.
        let result = limiter.check("key1", 10);
        assert!(!result.allowed);
        assert_eq!(result.limit, 10);
    }

    #[test]
    fn test_remove_key() {
        let limiter = PerKeyRateLimiter::new();
        assert!(limiter.check("key1", 5).allowed);
        assert_eq!(limiter.active_keys(), 1);

        limiter.remove_key("key1");
        assert_eq!(limiter.active_keys(), 0);

        // Key can be recreated
        let result = limiter.check("key1", 5);
        assert!(result.allowed);
    }

    #[test]
    fn test_reset_at_is_unix_timestamp() {
        let limiter = PerKeyRateLimiter::new();
        let result = limiter.check("key1", 60);

        // Should be a reasonable Unix timestamp (after year 2020)
        assert!(result.reset_at_secs > 1577836800);
    }

    #[test]
    fn test_default() {
        let limiter = PerKeyRateLimiter::default();
        assert_eq!(limiter.active_keys(), 0);
    }
}
