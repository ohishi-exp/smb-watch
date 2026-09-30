//! 今回の走行で処理する id の決定。

use std::collections::BTreeSet;

/// 走査で見つかった 1 ファイルのメタデータ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// 共有相対パス。
    pub id: String,
    /// 最終更新時刻 (UNIX ミリ秒)。
    pub mtime_ms: u64,
    /// バイト数。
    pub size: u64,
}

/// 処理対象の id を返す。`mtime_ms > since_ms` の id と、前回失敗した `retry` のうち
/// `entries` に今も在るものを、重複なく id 順にマージする。
///
/// retry のうち `entries` に無い id (その後消えたファイル) は落とす。
pub fn candidates(entries: &[Entry], since_ms: u64, retry: &[String]) -> Vec<String> {
    let retry: BTreeSet<&str> = retry.iter().map(String::as_str).collect();
    entries
        .iter()
        .filter(|e| e.mtime_ms > since_ms || retry.contains(e.id.as_str()))
        .map(|e| e.id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(id: &str, mtime_ms: u64) -> Entry {
        Entry {
            id: id.to_string(),
            mtime_ms,
            size: 1,
        }
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn since_itself_is_excluded() {
        let entries = [e("a", 99), e("b", 100), e("c", 101)];
        assert_eq!(candidates(&entries, 100, &[]), ids(&["c"]));
    }

    #[test]
    fn retry_is_added_even_when_old() {
        let entries = [e("a", 10), e("b", 200)];
        assert_eq!(candidates(&entries, 100, &ids(&["a"])), ids(&["a", "b"]));
    }

    #[test]
    fn retry_overlapping_changed_is_not_duplicated() {
        let entries = [e("a", 200), e("b", 200)];
        let retry = ids(&["a", "a", "b"]);
        assert_eq!(candidates(&entries, 100, &retry), ids(&["a", "b"]));
    }

    #[test]
    fn retry_missing_from_entries_is_dropped() {
        let entries = [e("a", 200)];
        assert_eq!(candidates(&entries, 100, &ids(&["gone"])), ids(&["a"]));
    }

    #[test]
    fn result_is_sorted_by_id() {
        let entries = [e("c", 200), e("a", 300), e("b", 10)];
        assert_eq!(
            candidates(&entries, 100, &ids(&["b"])),
            ids(&["a", "b", "c"])
        );
    }

    #[test]
    fn empty_inputs_give_empty() {
        assert!(candidates(&[], 0, &ids(&["x"])).is_empty());
        assert!(candidates(&[e("a", 5)], 5, &[]).is_empty());
    }
}
