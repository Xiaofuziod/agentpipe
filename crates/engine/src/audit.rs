use chrono::{DateTime, Utc};

/// run-id = <UTC 紧凑时间戳>-<slug(name)>;构造即保证只含 [A-Za-z0-9_-]。
pub fn run_id(name: &str, started: DateTime<Utc>) -> String {
    let ts = started.format("%Y%m%dT%H%M%SZ");
    let s = slug(name);
    if s.is_empty() {
        ts.to_string()
    } else {
        format!("{ts}-{s}")
    }
}

/// name → 小写、非字母数字折成单个 `-`、去首尾 `-`、截断 ≤40 字符(截断后再去尾 `-`)。
fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in name.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            // 注:_ 不是 alphanumeric → 折成 -;is_valid_run_id 另行允许 _(外部传入 id)
            out.push(ch);
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let mut s: String = out.trim_matches('-').chars().take(40).collect();
    while s.ends_with('-') {
        s.pop();
    }
    s
}

/// 校验外部传入的 run-id:只允许 [A-Za-z0-9_-],非空。防路径穿越。
pub fn is_valid_run_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn run_id_combines_timestamp_and_slug() {
        let t = Utc.with_ymd_and_hms(2026, 6, 18, 14, 22, 33).unwrap();
        assert_eq!(run_id("Add Verify Gate!", t), "20260618T142233Z-add-verify-gate");
    }

    #[test]
    fn run_id_is_always_valid() {
        let t = Utc.with_ymd_and_hms(2026, 6, 18, 0, 0, 0).unwrap();
        assert!(is_valid_run_id(&run_id("名字 with 中文 & symbols///", t)));
    }

    #[test]
    fn allowlist_rejects_path_traversal() {
        assert!(!is_valid_run_id("../etc/passwd"));
        assert!(!is_valid_run_id("a/b"));
        assert!(!is_valid_run_id(""));
        assert!(is_valid_run_id("20260618T142233Z-ok_id-1"));
    }

    #[test]
    fn slug_retrim_after_truncation() {
        // 39 个 'a' + '!' + 'b' → 截断前 slug "aaa…a-b"(41 字符)
        // take(40) → "aaa…a-",再去尾 '-' → "aaa…a"(39 字符)
        let name = "a".repeat(39) + "!b";
        let t = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let id = run_id(&name, t);
        assert!(!id.contains("--"), "不应有连续破折号: {id}");
        assert!(!id.ends_with('-'), "不应以破折号结尾: {id}");
        assert!(is_valid_run_id(&id), "应是合法 run-id: {id}");
    }
}
