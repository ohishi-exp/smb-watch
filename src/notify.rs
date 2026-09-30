//! 走行結果の LINE WORKS 通知 (auth-worker `POST /device-notify` 経由)。
//!
//! carins の車検証アップロードが 7 週間 55 件落ち続けたのに誰も気づかなかった
//! (nuxt-pwa-carins#54)。`last_run.txt` は誰も見ていないので、**失敗を能動的に
//! 押し出す**のがこのモジュールの役割。成功通知はおまけ。
//!
//! `scanner.rs` / `state.rs` と同じく、判定 (`should_notify`) と文面組み立て
//! (`build_message`) を純粋関数に切り出し、副作用は `notify_run_result` だけが持つ。
//!
//! 宛先はこのバイナリでは決めない。auth-worker が KV で固定するので、body は
//! `{"text": ...}` だけを送る。宛先を指定するフィールドを足すと 400 で弾かれる。

use serde::Serialize;
use tracing::{info, warn};

use crate::cli::Config;
use crate::source::file_name_of;

/// 通知文 1 行目の件名。開かずに何の通知か分かるようにする。
const SUBJECT: &str = "[carins 車検証]";

/// 失敗ファイル名を並べる最大件数。超えた分は「ほか N 件」に畳む
/// (55 件失敗しても 55 行送らない)。
const MAX_FAILED_LINES: usize = 5;

/// auth-worker `/device-notify` の `text` 上限。超えると 400 で弾かれる。
/// 検証側は JS の `String.length` = UTF-16 code unit 数なので、こちらも
/// UTF-16 長で数える (`utf16_len`)。
const MAX_TEXT_UNITS: usize = 1000;

/// 1 ファイル名あたりの表示上限。異常に長い id 1 個で全体が上限を超えるのを防ぐ。
const MAX_NAME_UNITS: usize = 120;

#[derive(Serialize)]
struct NotifyRequest<'a> {
    text: &'a str,
}

/// 通知文 2 行目に出す「どこを見ていたか」の表記を、実際の設定から導出する (純粋)。
///
/// `--smb-host` / `--smb-share` / `--smb-path` はいずれも既定値を持つだけの
/// **設定可能な項目**なので、固定文字列で持つと設定を変えた瞬間に通知文だけが
/// 嘘になる。設定を写さず毎回ここで組み立てる。
///
/// `--local-path` 指定時 (ローカルモード) はそのパスを出す。
pub fn source_label(config: &Config) -> String {
    if let Some(local) = &config.local_path {
        return truncate_utf16(&format!("ローカル {}", local.display()), MAX_NAME_UNITS);
    }

    let mut label = config.smb_host().trim().to_string();
    for seg in [config.smb_share(), config.smb_path()] {
        let seg = seg.trim().trim_matches(['/', '\\']);
        if seg.is_empty() {
            continue;
        }
        if !label.is_empty() {
            label.push('/');
        }
        label.push_str(seg);
    }
    if label.is_empty() {
        label.push_str("SMB");
    }
    truncate_utf16(&label, MAX_NAME_UNITS)
}

/// 通知を出すべきか。
///
/// - `failed >= 1` … 必ず送る (これが本体)
/// - `failed == 0 && uploaded >= 1` … 送る (成功通知)
/// - `files_found == 0` … **送らない**
///
/// 平日 9〜17 時の毎正時 = 1 日 9 回走るので、「変化なし」を無音にしないと
/// 本当の失敗がその通知に埋もれる。再発防止という目的そのものを損なう。
///
/// `files_found > 0 && uploaded == 0 && failed == 0` (読み取り前に全件消えた等) も
/// 送らない。main の アップロードループは 1 件ごとに必ず `uploaded` か `new_failed`
/// のどちらかを増やすので実際には起きないが、起きたとしても報告する事象が無い。
pub fn should_notify(files_found: usize, uploaded: usize, failed: usize) -> bool {
    if files_found == 0 {
        return false;
    }
    failed >= 1 || uploaded >= 1
}

/// 通知文を組み立てる (純粋)。
///
/// 1 行目だけで失敗の有無が分かること、`MAX_TEXT_UNITS` を超えないことが要件。
/// `source_label` は 2 行目に出す出所表記 (`source_label()` で設定から導出する)。
/// `failed` の要素は `Entry.id` (local: 絶対パス / SMB: 共有相対パス) なので、
/// 表示は `file_name_of` を通した basename にする。
pub fn build_message(
    source_label: &str,
    files_found: usize,
    uploaded: usize,
    failed: &[String],
) -> String {
    let failed_count = failed.len();

    let mut head = if failed_count > 0 {
        format!("{} 失敗 {} / 成功 {}", SUBJECT, failed_count, uploaded)
    } else {
        format!("{} 成功 {}", SUBJECT, uploaded)
    };
    // 検出数と 成功+失敗 が合わない = どこかで数が落ちている。1 行目に出す。
    if uploaded + failed_count != files_found {
        head.push_str(&format!(" (検出 {})", files_found));
    }

    let mut lines = vec![head, source_label.to_string()];

    let shown = failed_count.min(MAX_FAILED_LINES);
    for id in &failed[..shown] {
        lines.push(truncate_utf16(&file_name_of(id), MAX_NAME_UNITS));
    }
    if failed_count > shown {
        lines.push(format!("ほか {} 件", failed_count - shown));
    }

    // 畳んだ結果は通常 700 units 程度に収まるが、最後に上限を保証する。
    truncate_utf16(&lines.join("\n"), MAX_TEXT_UNITS)
}

/// 実際に送る。**失敗しても `Err` を返さない** (fail-open)。
///
/// 通知が落ちてアップロード本体が落ちたら本末転倒なので、`?` で早期 return しない。
/// ただし無言にもしない — status と body を必ず `warn!` に残す (今回の 502 調査で
/// 「詳細が出ない」ことに時間を取られた)。
pub async fn notify_run_result(client: &reqwest::Client, auth_url: &str, token: &str, text: &str) {
    let url = format!("{}/device-notify", auth_url.trim_end_matches('/'));

    match client
        .post(&url)
        .bearer_auth(token)
        .json(&NotifyRequest { text })
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                info!("Run result notified (HTTP {})", status);
            } else {
                let body = resp
                    .text()
                    .await
                    .unwrap_or_else(|_| "(unreadable body)".to_string());
                warn!(
                    "Notification failed (HTTP {}): {} — upload result itself is unaffected",
                    status,
                    body.trim()
                );
            }
        }
        Err(e) => {
            warn!(
                "Notification request to {} failed: {} — upload result itself is unaffected",
                url, e
            );
        }
    }
}

/// JS の `String.length` と同じ数え方 (UTF-16 code unit 数)。
fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// UTF-16 長で `max` を超える場合だけ末尾を `…` に置き換えて切り詰める。
fn truncate_utf16(s: &str, max: usize) -> String {
    if utf16_len(s) <= max {
        return s.to_string();
    }
    // 末尾の '…' (1 unit) の分を残す。
    let budget = max.saturating_sub(1);
    let mut out = String::new();
    let mut used = 0usize;
    for c in s.chars() {
        let w = c.len_utf16();
        if used + w > budget {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 指示の例と同じ出所表記。const ではなく `source_label()` の出力を渡す前提。
    const LABEL: &str = "box-01 dir-a";

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    /// テスト用の SMB 接続先 (既定値は無いので毎回渡す)。
    const SMB: [&str; 6] = [
        "--smb-host",
        "host-01",
        "--smb-share",
        "share-a",
        "--smb-path",
        "dir-a",
    ];

    /// 引数から Config を組み立てる。
    fn config(args: &[&str]) -> Config {
        let mut argv = vec!["smb-watch"];
        argv.extend_from_slice(args);
        <Config as clap::Parser>::parse_from(argv)
    }

    // --- source_label ---

    #[test]
    fn source_label_is_derived_from_smb_config() {
        assert_eq!(source_label(&config(&SMB)), "host-01/share-a/dir-a");
    }

    #[test]
    fn source_label_follows_config_changes() {
        // ★ ここが const 化を防ぐ番人。設定を変えたら通知文も変わること。
        let c = config(&[
            "--smb-host",
            "host-02",
            "--smb-share",
            "share2",
            "--smb-path",
            "dir-b",
        ]);
        assert_eq!(source_label(&c), "host-02/share2/dir-b");
    }

    #[test]
    fn source_label_handles_local_mode_and_empty_segments() {
        assert_eq!(
            source_label(&config(&["--local-path", "/mnt/carins"])),
            "ローカル /mnt/carins"
        );
        // 共有直下を見る設定 (path 空) でも読める文字列になること。
        assert_eq!(
            source_label(&config(&[
                "--smb-host",
                "host-01",
                "--smb-share",
                "share-a",
                "--smb-path",
                ""
            ])),
            "host-01/share-a"
        );
    }

    // --- should_notify ---

    #[test]
    fn notifies_when_anything_failed() {
        assert!(should_notify(55, 53, 2));
        // 全滅 (今回の 403 の型) も当然送る。
        assert!(should_notify(55, 0, 55));
    }

    #[test]
    fn notifies_on_success_only_run() {
        assert!(should_notify(55, 55, 0));
    }

    #[test]
    fn stays_silent_when_nothing_found() {
        // 1 日 9 回の「変化なし」を無音にするのが要点。failed が立っていても
        // files_found == 0 なら送らない (そもそも起きない組み合わせ)。
        assert!(!should_notify(0, 0, 0));
        assert!(!should_notify(0, 0, 1));
    }

    #[test]
    fn stays_silent_when_found_but_nothing_happened() {
        // main のループでは起きない端。報告する事象が無いので送らない。
        assert!(!should_notify(3, 0, 0));
    }

    // --- build_message ---

    #[test]
    fn message_with_failures_leads_with_failure_count() {
        let msg = build_message(
            LABEL,
            55,
            53,
            &ids(&[
                "share-a/dir-a/20260807140512_長崎100か3822.json",
                "share-a/dir-a/20260706112854_長崎100え428.json",
            ]),
        );
        assert_eq!(
            msg,
            "[carins 車検証] 失敗 2 / 成功 53\n\
             box-01 dir-a\n\
             20260807140512_長崎100か3822.json\n\
             20260706112854_長崎100え428.json"
        );
        // 1 行目だけで失敗が分かること。
        assert!(msg.lines().next().unwrap().contains("失敗 2"));
    }

    #[test]
    fn message_for_success_only_run() {
        let msg = build_message(LABEL, 55, 55, &[]);
        assert_eq!(msg, "[carins 車検証] 成功 55\nbox-01 dir-a");
        assert!(!msg.contains("失敗"));
    }

    #[test]
    fn message_uses_basename_not_full_path() {
        let msg = build_message(LABEL, 1, 0, &ids(&["/mnt/share-a/dir-a/a.json"]));
        assert!(msg.contains("a.json"));
        assert!(!msg.contains("/mnt/"));
    }

    #[test]
    fn message_shows_the_source_label_it_was_given() {
        // const に戻す変更が入ったら落ちる。設定由来のラベルがそのまま 2 行目に出ること。
        let label = source_label(&config(&[
            "--smb-host",
            "host-02",
            "--smb-share",
            "share-a",
            "--smb-path",
            "dir-b",
        ]));
        let msg = build_message(&label, 2, 1, &ids(&["a.json"]));
        assert_eq!(msg.lines().nth(1).unwrap(), "host-02/share-a/dir-b");
        assert!(!msg.contains("dir-a"));
    }

    #[test]
    fn message_folds_more_than_five_failures() {
        let names: Vec<String> = (1..=6).map(|i| format!("f{}.json", i)).collect();
        let msg = build_message(LABEL, 6, 0, &names);
        assert!(msg.contains("f5.json"));
        assert!(!msg.contains("f6.json"));
        assert!(msg.contains("ほか 1 件"));
        // 失敗行は 5 件 + 畳み 1 行 = ヘッダ 2 行と合わせて 8 行。
        assert_eq!(msg.lines().count(), 8);
    }

    #[test]
    fn message_folds_the_55_file_outage() {
        // 今回の再発時に 55 行送らないことの担保。
        let names: Vec<String> = (1..=55).map(|i| format!("f{}.json", i)).collect();
        let msg = build_message(LABEL, 55, 0, &names);
        assert!(msg.starts_with("[carins 車検証] 失敗 55 / 成功 0"));
        assert!(msg.contains("ほか 50 件"));
        assert_eq!(msg.lines().count(), 8);
    }

    #[test]
    fn message_notes_count_mismatch() {
        // 成功 + 失敗 が検出数に足りない場合だけ (検出 N) を出す。
        let msg = build_message(LABEL, 10, 3, &ids(&["a.json"]));
        assert!(msg.starts_with("[carins 車検証] 失敗 1 / 成功 3 (検出 10)"));
        assert!(!build_message(LABEL, 4, 3, &ids(&["a.json"])).contains("検出"));
    }

    #[test]
    fn message_never_exceeds_the_1000_char_limit() {
        // 極端に長いファイル名 x 大量失敗でも 400 にならないこと。
        let long: Vec<String> = (0..99)
            .map(|i| format!("{}_{}.json", "長".repeat(400), i))
            .collect();
        let msg = build_message(LABEL, 99, 0, &long);
        assert!(utf16_len(&msg) <= MAX_TEXT_UNITS, "len={}", utf16_len(&msg));
        assert!(msg.starts_with("[carins 車検証] 失敗 99 / 成功 0"));

        // 通常ケースも当然収まる。
        let normal: Vec<String> = (1..=55)
            .map(|i| format!("20260807140512_長崎100か{:04}.json", i))
            .collect();
        assert!(utf16_len(&build_message(LABEL, 55, 0, &normal)) <= MAX_TEXT_UNITS);
    }

    #[test]
    fn truncate_keeps_short_strings_untouched() {
        assert_eq!(truncate_utf16("abc", 10), "abc");
        assert_eq!(truncate_utf16("あいう", 3), "あいう");
        assert_eq!(truncate_utf16("あいうえお", 3), "あい…");
    }

    #[test]
    fn utf16_len_counts_like_javascript() {
        assert_eq!(utf16_len("abc"), 3);
        assert_eq!(utf16_len("長崎"), 2);
        // サロゲートペアは JS では 2 units。
        assert_eq!(utf16_len("🚚"), 2);
    }
}
