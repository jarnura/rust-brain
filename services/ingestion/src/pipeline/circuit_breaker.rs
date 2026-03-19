//! Circuit breaker pattern for external service calls (Neo4j, Ollama, Qdrant).
//!
//! Trips after consecutive failures, enters half-open after a cooldown,
//! and resets on success. When open, callers skip the operation and mark
//! items for retry.

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Circuit breaker states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CircuitState {
    /// Normal operation — all calls pass through
    Closed = 0,
    /// Breaker tripped — calls are rejected immediately
    Open = 1,
    /// Cooldown expired — one probe call allowed to test recovery
    HalfOpen = 2,
}

impl From<u8> for CircuitState {
    fn from(v: u8) -> Self {
        match v {
            0 => Self::Closed,
            1 => Self::Open,
            2 => Self::HalfOpen,
            _ => Self::Closed,
        }
    }
}

impl std::fmt::Display for CircuitState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => write!(f, "closed"),
            Self::Open => write!(f, "open"),
            Self::HalfOpen => write!(f, "half-open"),
        }
    }
}

/// Configuration for a circuit breaker instance
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before tripping
    pub failure_threshold: u32,
    /// How long to wait before allowing a probe call
    pub cooldown: Duration,
    /// Human-readable name for logging
    pub name: String,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            cooldown: Duration::from_secs(30),
            name: "unnamed".to_string(),
        }
    }
}

/// Thread-safe circuit breaker using atomics for hot-path reads.
///
/// State transitions:
///   Closed  --[failure_threshold failures]--> Open
///   Open    --[cooldown elapsed]-----------> HalfOpen
///   HalfOpen --[probe succeeds]------------> Closed
///   HalfOpen --[probe fails]--------------> Open
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    /// Atomic state for lock-free reads on the hot path
    state: AtomicU8,
    /// Consecutive failure count
    consecutive_failures: AtomicU64,
    /// Total failure count (monotonic, for metrics)
    total_failures: AtomicU64,
    /// Total success count (monotonic, for metrics)
    total_successes: AtomicU64,
    /// Mutex-protected timestamp of last state transition to Open.
    /// We use a Mutex<Option<Instant>> because Instant is not atomic.
    last_failure_time: Mutex<Option<Instant>>,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            state: AtomicU8::new(CircuitState::Closed as u8),
            consecutive_failures: AtomicU64::new(0),
            total_failures: AtomicU64::new(0),
            total_successes: AtomicU64::new(0),
            last_failure_time: Mutex::new(None),
        }
    }

    /// Convenience constructors for the services we protect.
    pub fn neo4j() -> Self {
        Self::new(CircuitBreakerConfig {
            failure_threshold: 5,
            cooldown: Duration::from_secs(30),
            name: "neo4j".to_string(),
        })
    }

    pub fn ollama() -> Self {
        Self::new(CircuitBreakerConfig {
            failure_threshold: 5,
            cooldown: Duration::from_secs(30),
            name: "ollama".to_string(),
        })
    }

    pub fn qdrant() -> Self {
        Self::new(CircuitBreakerConfig {
            failure_threshold: 5,
            cooldown: Duration::from_secs(30),
            name: "qdrant".to_string(),
        })
    }

    /// Current state (lock-free read).
    pub fn state(&self) -> CircuitState {
        CircuitState::from(self.state.load(Ordering::Acquire))
    }

    /// Check if a call is allowed. If the breaker is Open and cooldown has
    /// elapsed, transitions to HalfOpen and allows exactly one probe.
    pub async fn allow_call(&self) -> bool {
        let current = self.state();

        match current {
            CircuitState::Closed => true,
            CircuitState::Open => {
                // Check cooldown
                let guard = self.last_failure_time.lock().await;
                if let Some(ts) = *guard {
                    if ts.elapsed() >= self.config.cooldown {
                        drop(guard);
                        // Transition to half-open
                        self.state
                            .store(CircuitState::HalfOpen as u8, Ordering::Release);
                        info!(
                            "Circuit breaker [{}] transitioning Open -> HalfOpen (cooldown elapsed)",
                            self.config.name
                        );
                        true
                    } else {
                        false
                    }
                } else {
                    // No failure timestamp recorded — shouldn't happen, but allow
                    true
                }
            }
            CircuitState::HalfOpen => {
                // Only one probe at a time; subsequent callers are rejected
                // The probe caller already got `true` from the Open->HalfOpen transition
                false
            }
        }
    }

    /// Record a successful call. Resets consecutive failures and closes the breaker.
    pub async fn record_success(&self) {
        self.total_successes.fetch_add(1, Ordering::Relaxed);
        let prev_failures = self.consecutive_failures.swap(0, Ordering::Release);
        let prev_state = self.state();

        if prev_state == CircuitState::HalfOpen {
            self.state
                .store(CircuitState::Closed as u8, Ordering::Release);
            info!(
                "Circuit breaker [{}] HalfOpen -> Closed (probe succeeded after {} failures)",
                self.config.name, prev_failures
            );
        }
    }

    /// Record a failed call. Increments consecutive failures and may trip the breaker.
    pub async fn record_failure(&self) {
        self.total_failures.fetch_add(1, Ordering::Relaxed);
        let failures = self.consecutive_failures.fetch_add(1, Ordering::AcqRel) + 1;

        let current_state = self.state();

        match current_state {
            CircuitState::HalfOpen => {
                // Probe failed — go back to Open
                self.state
                    .store(CircuitState::Open as u8, Ordering::Release);
                let mut guard = self.last_failure_time.lock().await;
                *guard = Some(Instant::now());
                warn!(
                    "Circuit breaker [{}] HalfOpen -> Open (probe failed)",
                    self.config.name
                );
            }
            CircuitState::Closed => {
                if failures >= self.config.failure_threshold as u64 {
                    self.state
                        .store(CircuitState::Open as u8, Ordering::Release);
                    let mut guard = self.last_failure_time.lock().await;
                    *guard = Some(Instant::now());
                    warn!(
                        "Circuit breaker [{}] Closed -> Open ({} consecutive failures >= threshold {})",
                        self.config.name, failures, self.config.failure_threshold
                    );
                }
            }
            CircuitState::Open => {
                // Already open — just update timestamp
                let mut guard = self.last_failure_time.lock().await;
                *guard = Some(Instant::now());
            }
        }
    }

    /// Execute an async operation through the circuit breaker.
    ///
    /// Returns `Err(CircuitBreakerError::Open)` if the breaker rejects the call,
    /// otherwise returns the inner result and records success/failure.
    pub async fn call<F, Fut, T, E>(&self, f: F) -> Result<T, CircuitBreakerError<E>>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, E>>,
        E: std::fmt::Display,
    {
        if !self.allow_call().await {
            return Err(CircuitBreakerError::Open {
                service: self.config.name.clone(),
                consecutive_failures: self.consecutive_failures.load(Ordering::Relaxed),
            });
        }

        match f().await {
            Ok(val) => {
                self.record_success().await;
                Ok(val)
            }
            Err(e) => {
                self.record_failure().await;
                Err(CircuitBreakerError::Inner(e))
            }
        }
    }

    /// Force-reset the breaker to Closed (for admin/testing).
    pub async fn reset(&self) {
        self.state
            .store(CircuitState::Closed as u8, Ordering::Release);
        self.consecutive_failures.store(0, Ordering::Release);
        let mut guard = self.last_failure_time.lock().await;
        *guard = None;
        info!("Circuit breaker [{}] force-reset to Closed", self.config.name);
    }

    /// Snapshot of metrics for observability.
    pub fn metrics(&self) -> CircuitBreakerMetrics {
        CircuitBreakerMetrics {
            name: self.config.name.clone(),
            state: self.state(),
            consecutive_failures: self.consecutive_failures.load(Ordering::Relaxed),
            total_failures: self.total_failures.load(Ordering::Relaxed),
            total_successes: self.total_successes.load(Ordering::Relaxed),
        }
    }
}

/// Error returned by `CircuitBreaker::call`.
#[derive(Debug)]
pub enum CircuitBreakerError<E> {
    /// The breaker is open — call was not attempted.
    Open {
        service: String,
        consecutive_failures: u64,
    },
    /// The inner operation failed.
    Inner(E),
}

impl<E: std::fmt::Display> std::fmt::Display for CircuitBreakerError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open {
                service,
                consecutive_failures,
            } => write!(
                f,
                "circuit breaker [{}] is open ({} consecutive failures)",
                service, consecutive_failures
            ),
            Self::Inner(e) => write!(f, "{}", e),
        }
    }
}

/// Metrics snapshot for observability.
#[derive(Debug, Clone)]
pub struct CircuitBreakerMetrics {
    pub name: String,
    pub state: CircuitState,
    pub consecutive_failures: u64,
    pub total_failures: u64,
    pub total_successes: u64,
}

impl std::fmt::Display for CircuitBreakerMetrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] state={}, consecutive_failures={}, total_ok={}, total_err={}",
            self.name, self.state, self.consecutive_failures, self.total_successes, self.total_failures
        )
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_breaker() -> CircuitBreaker {
        CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 3,
            cooldown: Duration::from_millis(100),
            name: "test".to_string(),
        })
    }

    #[tokio::test]
    async fn test_starts_closed() {
        let cb = test_breaker();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.allow_call().await);
    }

    #[tokio::test]
    async fn test_trips_after_threshold() {
        let cb = test_breaker();
        // 3 failures to trip
        for _ in 0..3 {
            cb.record_failure().await;
        }
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.allow_call().await);
    }

    #[tokio::test]
    async fn test_does_not_trip_below_threshold() {
        let cb = test_breaker();
        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.allow_call().await);
    }

    #[tokio::test]
    async fn test_success_resets_counter() {
        let cb = test_breaker();
        cb.record_failure().await;
        cb.record_failure().await;
        cb.record_success().await;
        // Counter reset, need 3 more failures to trip
        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[tokio::test]
    async fn test_half_open_after_cooldown() {
        let cb = test_breaker();
        for _ in 0..3 {
            cb.record_failure().await;
        }
        assert_eq!(cb.state(), CircuitState::Open);

        // Wait for cooldown
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Should transition to HalfOpen and allow one probe
        assert!(cb.allow_call().await);
        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    #[tokio::test]
    async fn test_half_open_success_closes() {
        let cb = test_breaker();
        for _ in 0..3 {
            cb.record_failure().await;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(cb.allow_call().await); // transitions to HalfOpen

        cb.record_success().await;
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[tokio::test]
    async fn test_half_open_failure_reopens() {
        let cb = test_breaker();
        for _ in 0..3 {
            cb.record_failure().await;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(cb.allow_call().await); // transitions to HalfOpen

        cb.record_failure().await;
        assert_eq!(cb.state(), CircuitState::Open);
    }

    #[tokio::test]
    async fn test_call_returns_open_error() {
        let cb = test_breaker();
        for _ in 0..3 {
            cb.record_failure().await;
        }

        let result: Result<(), CircuitBreakerError<String>> =
            cb.call(|| async { Ok::<(), String>(()) }).await;

        assert!(matches!(result, Err(CircuitBreakerError::Open { .. })));
    }

    #[tokio::test]
    async fn test_call_propagates_inner_error() {
        let cb = test_breaker();
        let result: Result<(), CircuitBreakerError<String>> = cb
            .call(|| async { Err::<(), String>("boom".to_string()) })
            .await;

        assert!(matches!(result, Err(CircuitBreakerError::Inner(_))));
    }

    #[tokio::test]
    async fn test_call_records_success() {
        let cb = test_breaker();
        cb.record_failure().await;
        cb.record_failure().await;

        let _: Result<i32, CircuitBreakerError<String>> = cb.call(|| async { Ok(42) }).await;

        // Success should have reset the counter
        let m = cb.metrics();
        assert_eq!(m.consecutive_failures, 0);
        assert_eq!(m.total_successes, 1);
    }

    #[tokio::test]
    async fn test_reset() {
        let cb = test_breaker();
        for _ in 0..3 {
            cb.record_failure().await;
        }
        assert_eq!(cb.state(), CircuitState::Open);

        cb.reset().await;
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.allow_call().await);
    }

    #[tokio::test]
    async fn test_metrics() {
        let cb = test_breaker();
        cb.record_success().await;
        cb.record_success().await;
        cb.record_failure().await;

        let m = cb.metrics();
        assert_eq!(m.name, "test");
        assert_eq!(m.state, CircuitState::Closed);
        assert_eq!(m.consecutive_failures, 1);
        assert_eq!(m.total_successes, 2);
        assert_eq!(m.total_failures, 1);
    }
}
