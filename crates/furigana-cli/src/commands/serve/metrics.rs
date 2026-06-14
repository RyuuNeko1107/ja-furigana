//! 軽量 Prometheus 互換 metrics (= 配信運用の trace + 簡易 alert 用)
//!
//! 外部 crate を増やさず、 std::sync::atomic + 固定 bucket histogram で実装。
//! `/metrics` endpoint で text format を返す。

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};

/// histogram の境界 (ms 単位、 8 bucket + +Inf)
const LATENCY_BUCKETS_MS: [f64; 8] = [5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0];

/// 1 process あたり 1 個生成、 Arc で共有
#[derive(Default)]
pub(super) struct ServerMetrics {
    // mode 別 request count
    req_tts: AtomicU64,
    req_hiragana: AtomicU64,
    req_ruby: AtomicU64,
    req_kanji: AtomicU64,
    req_romaji: AtomicU64,
    req_romaji_kunrei: AtomicU64,
    req_analyze: AtomicU64,
    req_accent: AtomicU64,
    // latency histogram (= total ms、 bucket cumulative count)
    latency_buckets: [AtomicU64; 9],
    latency_sum_ms: AtomicU64,
    latency_count: AtomicU64,
    // 各種 counter
    auth_failures: AtomicU64,
    rate_limited: AtomicU64,
    slow_requests: AtomicU64,
    failed_resolution: AtomicU64,
    reloads: AtomicU64,
    // gauge
    dict_size: AtomicUsize,
}

impl ServerMetrics {
    pub(super) fn record_request(&self, mode: &str, total_ms: f64) {
        let counter = match mode {
            "tts" => &self.req_tts,
            "hiragana" => &self.req_hiragana,
            "ruby" => &self.req_ruby,
            "kanji" => &self.req_kanji,
            "romaji" => &self.req_romaji,
            "romaji-kunrei" => &self.req_romaji_kunrei,
            "analyze" => &self.req_analyze,
            "accent" => &self.req_accent,
            _ => return,
        };
        counter.fetch_add(1, Relaxed);
        // histogram (= 各 bucket は「<= 境界」 の cumulative count を保持)
        for (i, b) in LATENCY_BUCKETS_MS.iter().enumerate() {
            if total_ms <= *b {
                self.latency_buckets[i].fetch_add(1, Relaxed);
            }
        }
        self.latency_buckets[8].fetch_add(1, Relaxed); // +Inf bucket
        self.latency_sum_ms
            .fetch_add(total_ms.round() as u64, Relaxed);
        self.latency_count.fetch_add(1, Relaxed);
    }

    pub(super) fn record_auth_failure(&self) {
        self.auth_failures.fetch_add(1, Relaxed);
    }

    pub(super) fn record_rate_limited(&self) {
        self.rate_limited.fetch_add(1, Relaxed);
    }

    pub(super) fn record_slow_request(&self) {
        self.slow_requests.fetch_add(1, Relaxed);
    }

    pub(super) fn record_failed_resolution(&self) {
        self.failed_resolution.fetch_add(1, Relaxed);
    }

    pub(super) fn record_reload(&self, new_dict_size: usize) {
        self.reloads.fetch_add(1, Relaxed);
        self.dict_size.store(new_dict_size, Relaxed);
    }

    pub(super) fn set_dict_size(&self, size: usize) {
        self.dict_size.store(size, Relaxed);
    }

    /// Prometheus text exposition format で render
    pub(super) fn render(&self) -> String {
        let mut out = String::with_capacity(2048);

        // requests_total per mode
        out.push_str("# HELP furigana_requests_total Total requests served, partitioned by mode\n");
        out.push_str("# TYPE furigana_requests_total counter\n");
        for (mode, c) in [
            ("tts", &self.req_tts),
            ("hiragana", &self.req_hiragana),
            ("ruby", &self.req_ruby),
            ("kanji", &self.req_kanji),
            ("romaji", &self.req_romaji),
            ("romaji-kunrei", &self.req_romaji_kunrei),
            ("analyze", &self.req_analyze),
            ("accent", &self.req_accent),
        ] {
            out.push_str(&format!(
                "furigana_requests_total{{mode=\"{}\"}} {}\n",
                mode,
                c.load(Relaxed)
            ));
        }

        // latency histogram
        out.push_str(
            "# HELP furigana_request_duration_ms Request total latency (process() 内の total_ms)\n",
        );
        out.push_str("# TYPE furigana_request_duration_ms histogram\n");
        for (i, b) in LATENCY_BUCKETS_MS.iter().enumerate() {
            out.push_str(&format!(
                "furigana_request_duration_ms_bucket{{le=\"{}\"}} {}\n",
                b,
                self.latency_buckets[i].load(Relaxed)
            ));
        }
        out.push_str(&format!(
            "furigana_request_duration_ms_bucket{{le=\"+Inf\"}} {}\n",
            self.latency_buckets[8].load(Relaxed)
        ));
        out.push_str(&format!(
            "furigana_request_duration_ms_sum {}\n",
            self.latency_sum_ms.load(Relaxed)
        ));
        out.push_str(&format!(
            "furigana_request_duration_ms_count {}\n",
            self.latency_count.load(Relaxed)
        ));

        // 各種 counter
        for (name, help, value) in [
            (
                "furigana_auth_failures_total",
                "Authentication failures (= token mismatch)",
                self.auth_failures.load(Relaxed),
            ),
            (
                "furigana_rate_limited_total",
                "Requests rejected by rate limiter (= HTTP 429)",
                self.rate_limited.load(Relaxed),
            ),
            (
                "furigana_slow_requests_total",
                "Requests whose total_ms exceeded slow threshold (= 100ms default)",
                self.slow_requests.load(Relaxed),
            ),
            (
                "furigana_failed_resolution_total",
                "Requests whose result indicated reading resolution failure",
                self.failed_resolution.load(Relaxed),
            ),
            (
                "furigana_reloads_total",
                "Dictionary reload events (SIGHUP / admin / auto_update / startup)",
                self.reloads.load(Relaxed),
            ),
        ] {
            out.push_str(&format!("# HELP {} {}\n", name, help));
            out.push_str(&format!("# TYPE {} counter\n", name));
            out.push_str(&format!("{} {}\n", name, value));
        }

        // gauge: dict_size
        out.push_str("# HELP furigana_dict_size Current dictionary size (entries)\n");
        out.push_str("# TYPE furigana_dict_size gauge\n");
        out.push_str(&format!(
            "furigana_dict_size {}\n",
            self.dict_size.load(Relaxed)
        ));

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_requests_are_counted() {
        // 回帰: record_request に "accent" arm が無く、accent mode が per-mode counter /
        // latency histogram から欠落していた (handlers.rs:245 が "accent" で呼ぶ)。
        let m = ServerMetrics::default();
        m.record_request("accent", 12.0);
        m.record_request("accent", 30.0);
        let rendered = m.render();
        assert!(
            rendered.contains("furigana_requests_total{mode=\"accent\"} 2"),
            "accent counter missing/zero:\n{rendered}"
        );
        // latency にも計上される (count が 2)
        assert!(
            rendered.contains("furigana_request_duration_ms_count 2"),
            "accent not counted in latency:\n{rendered}"
        );
    }

    #[test]
    fn unknown_mode_is_dropped_not_counted() {
        // 未知 mode は従来どおり無視 (latency も増えない)。
        let m = ServerMetrics::default();
        m.record_request("bogus", 10.0);
        assert!(m.render().contains("furigana_request_duration_ms_count 0"));
    }
}
