//! fetch ハンドラ (`POST /run`) の純粋な判定と応答の組み立て。
//!
//! 応答に件数・ファイル名・共有名・パス・エラーの生文言を入れない。本文は下の 3 形 (+ 409 / 500) だけ。

/// リクエストの振り分け結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// `POST /run`。`force_dry_run` は `?dry_run=1` の有無。
    Run { force_dry_run: bool },
    /// パスが `/run` ではない (404)。
    NotFound,
    /// `/run` だが POST ではない (405)。
    MethodNotAllowed,
}

/// メソッド (大文字) とパス・クエリから振り分ける。パスは末尾の `/` を許さない。
pub fn route(method: &str, path: &str, query: Option<&str>) -> Route {
    if path != "/run" {
        return Route::NotFound;
    }
    if method != "POST" {
        return Route::MethodNotAllowed;
    }
    let force_dry_run = query
        .into_iter()
        .flat_map(|q| q.split('&'))
        .any(|pair| pair == "dry_run=1");
    Route::Run { force_dry_run }
}

/// 実効の dry-run。Worker 全体の `DRY_RUN` が `"0"` 以外なら常に dry-run (fail-safe)。
/// `?dry_run=1` は dry-run を強めることしかできない。
pub fn effective_dry_run(env_value: Option<&str>, force_dry_run: bool) -> bool {
    env_value != Some("0") || force_dry_run
}

/// エラー応答 (500) の `reason` に使う定型の種類名。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// lease を取る DO 呼び出しが失敗した。
    LeaseUnavailable,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LeaseUnavailable => "lease_unavailable",
        }
    }
}

/// HTTP 応答 (ステータスと JSON 本文)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub body: String,
}

/// 404 / 405。本文は空。
pub fn reply_for_unrouted(route: Route) -> Option<Reply> {
    let status = match route {
        Route::NotFound => 404,
        Route::MethodNotAllowed => 405,
        Route::Run { .. } => return None,
    };
    Some(Reply {
        status,
        body: String::new(),
    })
}

/// lease が取れなかった (他の run が保持中) → 409。
pub fn busy() -> Reply {
    Reply {
        status: 409,
        body: r#"{"status":"busy"}"#.to_string(),
    }
}

/// lease が取れた → 202。本体はこの応答の後ろで走る。
pub fn accepted(dry_run: bool) -> Reply {
    Reply {
        status: 202,
        body: format!(r#"{{"status":"accepted","dry_run":{dry_run}}}"#),
    }
}

/// 失敗 → 500。
pub fn error(kind: ErrorKind) -> Reply {
    Reply {
        status: 500,
        body: format!(r#"{{"status":"error","reason":"{}"}}"#, kind.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_post_run_is_accepted() {
        assert_eq!(
            route("POST", "/run", None),
            Route::Run {
                force_dry_run: false
            }
        );
        assert_eq!(route("POST", "/x", None), Route::NotFound);
        assert_eq!(route("GET", "/x", None), Route::NotFound);
        assert_eq!(route("POST", "/", None), Route::NotFound);
        assert_eq!(route("POST", "/run/", None), Route::NotFound);
        assert_eq!(route("POST", "/Run", None), Route::NotFound);
    }

    #[test]
    fn non_post_on_run_is_405() {
        for m in ["GET", "PUT", "DELETE", "HEAD", "OPTIONS", "PATCH"] {
            assert_eq!(route(m, "/run", None), Route::MethodNotAllowed, "{m}");
        }
    }

    #[test]
    fn dry_run_query() {
        let forced = Route::Run {
            force_dry_run: true,
        };
        let plain = Route::Run {
            force_dry_run: false,
        };
        assert_eq!(route("POST", "/run", Some("dry_run=1")), forced);
        assert_eq!(route("POST", "/run", Some("a=b&dry_run=1")), forced);
        assert_eq!(route("POST", "/run", Some("dry_run=1&a=b")), forced);
        assert_eq!(route("POST", "/run", Some("dry_run=0")), plain);
        assert_eq!(route("POST", "/run", Some("dry_run=10")), plain);
        assert_eq!(route("POST", "/run", Some("dry_run")), plain);
        assert_eq!(route("POST", "/run", Some("")), plain);
    }

    #[test]
    fn env_dry_run_is_fail_safe() {
        assert!(effective_dry_run(None, false));
        assert!(effective_dry_run(Some("1"), false));
        assert!(effective_dry_run(Some(""), false));
        assert!(!effective_dry_run(Some("0"), false));
        assert!(effective_dry_run(Some("0"), true));
        assert!(effective_dry_run(Some("1"), true));
    }

    #[test]
    fn unrouted_replies() {
        assert_eq!(reply_for_unrouted(Route::NotFound).unwrap().status, 404);
        assert_eq!(
            reply_for_unrouted(Route::MethodNotAllowed).unwrap().status,
            405
        );
        assert_eq!(
            reply_for_unrouted(Route::Run {
                force_dry_run: false
            }),
            None
        );
    }

    #[test]
    fn reply_bodies_are_fixed_shapes() {
        assert_eq!(busy().status, 409);
        assert_eq!(busy().body, r#"{"status":"busy"}"#);
        assert_eq!(accepted(true).status, 202);
        assert_eq!(
            accepted(true).body,
            r#"{"status":"accepted","dry_run":true}"#
        );
        assert_eq!(
            accepted(false).body,
            r#"{"status":"accepted","dry_run":false}"#
        );
        let e = error(ErrorKind::LeaseUnavailable);
        assert_eq!(e.status, 500);
        assert_eq!(e.body, r#"{"status":"error","reason":"lease_unavailable"}"#);
    }
}
