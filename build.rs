use std::collections::HashMap;
use std::process::Command;

fn main() {
    // .env.build (Git 管理外) を fallback として読む。
    let mut file_env: HashMap<String, String> = HashMap::new();
    if let Ok(content) = std::fs::read_to_string(".env.build") {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                file_env.insert(key.trim().to_string(), value.trim().to_string());
            }
        }
    }

    // OAuth 既定値: 環境変数 (CI/CD) > .env.build > 空文字列。
    // 未設定でも空文字列を必ず emit して、cli.rs の env!() がコンパイルエラーに
    // ならないようにする (= secret 未投入でも build / test は通る)。
    for key in ["DEFAULT_GOOGLE_CLIENT_ID", "DEFAULT_GOOGLE_CLIENT_SECRET"] {
        let val = std::env::var(key)
            .ok()
            .or_else(|| file_env.get(key).cloned())
            .unwrap_or_default();
        println!("cargo:rustc-env={}={}", key, val);
        println!("cargo:rerun-if-env-changed={}", key);
    }

    // BUILD_SHA: deploy 検証で「焼き込んだ binary == deploy した commit」を突合する。
    // CI は GITHUB_SHA、ローカルは git、どちらも無ければ "unknown"。
    let sha = std::env::var("GITHUB_SHA")
        .ok()
        .map(|s| s.chars().take(7).collect::<String>())
        .or_else(|| {
            Command::new("git")
                .args(["rev-parse", "--short", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=BUILD_SHA={}", sha);

    // BUILD_TIME: 可読な UTC (date -u)、無ければ unix epoch 秒。
    let time = Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs().to_string())
                .unwrap_or_else(|_| "unknown".to_string())
        });
    println!("cargo:rustc-env=BUILD_TIME={}", time);

    println!("cargo:rerun-if-changed=.env.build");
    println!("cargo:rerun-if-changed=.git/HEAD");
}
