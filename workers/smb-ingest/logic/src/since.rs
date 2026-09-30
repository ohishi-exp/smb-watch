//! 前回走行時刻 (since) の決定。

/// `resolve_since` が since を決められなかった。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SinceError {
    /// 保存値も初期値も無い。
    Unset,
}

impl std::fmt::Display for SinceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SinceError::Unset => {
                write!(f, "since is unset (no stored value and no initial value)")
            }
        }
    }
}

impl std::error::Error for SinceError {}

/// 保存値があればそれ、無ければ初期値。どちらも無ければ `Unset`。
///
/// 0 にフォールバックしない: 全件を上げ直す事故を防ぐため、loud に失敗させる。
/// 初期値 (RFC3339) の解釈は呼び出し側 (Worker) で行い、ここには UNIX ミリ秒で渡す。
pub fn resolve_since(
    stored_ms: Option<u64>,
    initial_since_ms: Option<u64>,
) -> Result<u64, SinceError> {
    stored_ms.or(initial_since_ms).ok_or(SinceError::Unset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_wins_over_initial() {
        assert_eq!(resolve_since(Some(5), Some(9)), Ok(5));
    }

    #[test]
    fn stored_zero_is_still_stored() {
        assert_eq!(resolve_since(Some(0), Some(9)), Ok(0));
    }

    #[test]
    fn falls_back_to_initial() {
        assert_eq!(resolve_since(None, Some(9)), Ok(9));
    }

    #[test]
    fn neither_is_an_error_not_zero() {
        assert_eq!(resolve_since(None, None), Err(SinceError::Unset));
    }
}
