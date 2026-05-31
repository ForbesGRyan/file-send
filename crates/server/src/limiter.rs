//! Per-IP rate limiter for room-join attempts, to make online enumeration of
//! short room codes infeasible. Pure bookkeeping over a caller-supplied clock
//! (`now_ms`), so it is fully unit-testable.

use std::collections::HashMap;
use std::net::IpAddr;

/// Tracks recent failed join attempts per source IP within a sliding window.
pub struct JoinLimiter {
    window_ms: u64,
    max_failures: usize,
    /// Per-IP timestamps (ms) of failures still within the window.
    failures: HashMap<IpAddr, Vec<u64>>,
}

impl JoinLimiter {
    pub fn new(window_ms: u64, max_failures: usize) -> Self {
        Self { window_ms, max_failures, failures: HashMap::new() }
    }

    /// Drop timestamps older than the window for `ip`, removing the entry if it
    /// becomes empty (keeps the map from growing without bound).
    fn prune(&mut self, ip: IpAddr, now_ms: u64) {
        if let Some(v) = self.failures.get_mut(&ip) {
            v.retain(|&t| now_ms.saturating_sub(t) < self.window_ms);
            if v.is_empty() {
                self.failures.remove(&ip);
            }
        }
    }

    /// Whether `ip` may attempt a join now (i.e. is under the failure cap).
    /// Read-only: checking does not consume budget.
    pub fn allowed(&mut self, ip: IpAddr, now_ms: u64) -> bool {
        self.prune(ip, now_ms);
        self.failures.get(&ip).map_or(0, |v| v.len()) < self.max_failures
    }

    /// Record one failed join attempt from `ip`.
    pub fn record_failure(&mut self, ip: IpAddr, now_ms: u64) {
        self.prune(ip, now_ms);
        self.failures.entry(ip).or_default().push(now_ms);
    }
}

#[cfg(test)]
mod tests {
    use super::JoinLimiter;
    use std::net::{IpAddr, Ipv4Addr};

    fn ip(n: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, n))
    }

    #[test]
    fn allows_up_to_max_then_blocks_within_window() {
        // 3 failures allowed per 1000ms window.
        let mut l = JoinLimiter::new(1000, 3);
        let a = ip(1);
        // First 3 attempts are allowed; record a failure each time.
        for t in 0..3 {
            assert!(l.allowed(a, t), "attempt {t} should be allowed");
            l.record_failure(a, t);
        }
        // 4th attempt within the window is blocked.
        assert!(!l.allowed(a, 3), "4th attempt should be blocked");
    }

    #[test]
    fn window_expiry_clears_old_failures() {
        let mut l = JoinLimiter::new(1000, 3);
        let a = ip(1);
        for t in 0..3 {
            l.record_failure(a, t);
        }
        assert!(!l.allowed(a, 500), "still within window -> blocked");
        // Once all failures age out past the window, attempts are allowed again.
        assert!(l.allowed(a, 1000), "failures aged out -> allowed");
    }

    #[test]
    fn limits_are_per_ip() {
        let mut l = JoinLimiter::new(1000, 2);
        let a = ip(1);
        let b = ip(2);
        l.record_failure(a, 0);
        l.record_failure(a, 0);
        assert!(!l.allowed(a, 0), "ip a is over its limit");
        assert!(l.allowed(b, 0), "ip b is unaffected");
    }

    #[test]
    fn successful_attempts_do_not_count() {
        // Only record_failure consumes budget; merely checking allowed does not.
        let mut l = JoinLimiter::new(1000, 1);
        let a = ip(1);
        assert!(l.allowed(a, 0));
        assert!(l.allowed(a, 0), "checking does not consume budget");
    }
}
