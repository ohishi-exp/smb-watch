//! アップロード可能なサイズの上限。

/// 生バイト数の上限。base64 (4/3 倍) にしても auth-worker の上限 16 MiB を超えない値。
pub const MAX_RAW_BYTES: u64 = 12 * 1024 * 1024;

/// 上限を超えているか (上限ちょうどは許容)。
pub fn too_large(size: u64) -> bool {
    size > MAX_RAW_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary() {
        assert!(!too_large(0));
        assert!(!too_large(MAX_RAW_BYTES));
        assert!(too_large(MAX_RAW_BYTES + 1));
    }

    #[test]
    fn base64_of_max_stays_within_16_mib() {
        let encoded = MAX_RAW_BYTES.div_ceil(3) * 4;
        assert!(encoded <= 16 * 1024 * 1024);
    }
}
