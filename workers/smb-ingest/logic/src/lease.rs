//! 走行の排他 (lease) 判定。

/// lease の有効時間 (ミリ秒)。
pub const LEASE_MS: u64 = 14 * 60 * 1000;

/// lease の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseState {
    /// lease が無い (前回は正常終了した)。
    Free,
    /// 他の走行が保持中 (期限内)。
    Held,
    /// 期限切れの lease が残っている = 前回の走行が途中で止まった印。
    Expired,
}

/// `lease_until_ms` (期限の UNIX ミリ秒) と現在時刻から状態を決める。
/// 期限ちょうど (`now_ms == lease_until_ms`) は `Expired`。
pub fn lease_state(lease_until_ms: Option<u64>, now_ms: u64) -> LeaseState {
    match lease_until_ms {
        None => LeaseState::Free,
        Some(until) if now_ms < until => LeaseState::Held,
        Some(_) => LeaseState::Expired,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_states() {
        assert_eq!(lease_state(None, 1_000), LeaseState::Free);
        assert_eq!(lease_state(Some(2_000), 1_000), LeaseState::Held);
        assert_eq!(lease_state(Some(500), 1_000), LeaseState::Expired);
    }

    #[test]
    fn boundary() {
        assert_eq!(lease_state(Some(1_000), 999), LeaseState::Held);
        assert_eq!(lease_state(Some(1_000), 1_000), LeaseState::Expired);
    }

    #[test]
    fn lease_is_14_minutes() {
        assert_eq!(LEASE_MS, 840_000);
    }
}
