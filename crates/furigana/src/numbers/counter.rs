//! 数値 + 助数詞 の読み解決 (data-driven)
//!
//! [`euphonic_counter_read`] が中核。`CountersData` と `DaysData` を引数に
//! 取り、助数詞の連濁・促音化・kana 末尾置換・特殊読み・末尾「目」再帰
//! までをルールデータに従って処理する。

use super::helpers::{last_digit, norm_num, sokuonize_last, to_int};
use crate::rules::{CountersData, DaysData};

/// 百の位で終わる数 (100 / 300 / 600 …) の促音化。
///
/// [`sokuonize_last`] は イチ / ロク / ハチ / ジュウ だけを扱うので、 促音化する助数詞でも
/// 100回 = ヒャクカイ / 100本 = ヒャクポン になっていた。 カ行・パ行で始まる助数詞の前でだけ
/// ヒャク / ピャク / ビャク を促音にする (100回 = ヒャッカイ / 100本 = ヒャッポン)。
/// サ行・タ行 (100歳 / 100点) は ヒャク読みが一般的なので変えない。
/// (★2026-09-15 助数詞スイープで検出)
fn sokuonize_hundred(body: &str, suffix: &str) -> String {
    if !suffix.starts_with(['カ', 'キ', 'ク', 'ケ', 'コ', 'パ', 'ピ', 'プ', 'ペ', 'ポ']) {
        return body.to_string();
    }
    for (src, dst) in [
        ("ヒャク", "ヒャッ"),
        ("ピャク", "ピャッ"),
        ("ビャク", "ビャッ"),
    ] {
        if let Some(stripped) = body.strip_suffix(src) {
            return format!("{stripped}{dst}");
        }
    }
    body.to_string()
}

/// 連濁・促音化の判定に使う末尾の数字。
///
/// 数の読みが「ん」(千 / 万) で終わる時は、 3 (さん) と同じ連濁にする
/// (1000本 = せんぼん / 3000匹 = さんぜんびき / 5万本 = ごまんぼん / 100万本 = ひゃくまんぼん)。
/// 対象は、 末尾の 0 が 3〜7 個 (1,000 〜 9,999,0000 台) の数と、 末尾が 万 / 千 の表記。
/// 10 / 100 (じゅっぽん / ひゃっぽん) と、 億以上 (末尾の 0 が 8 個以上) は従来どおり。
fn effective_last_digit(raw_num: &str) -> u32 {
    let norm = norm_num(raw_num);
    let t = norm.trim();
    if t.ends_with('万') || t.ends_with('千') {
        return 3;
    }
    let zeros = t.chars().rev().take_while(|&c| c == '0').count();
    if (3..=7).contains(&zeros) && t.chars().all(|c| c.is_ascii_digit()) && t.len() > zeros {
        return 3;
    }
    last_digit(raw_num)
}

/// 「数値 + 助数詞」の読みを構築する (data-driven)
///
/// 優先順位:
/// 1. counter が `simple` 表にあれば: `num_kata + simple[counter]`
/// 2. counter が `日` なら: days.toml の特殊読み (該当しなければ「ニチ」)
/// 3. counter が `counter` 表 (CounterRule) にあれば、specials → replacements
///    → rules (last_digit + sokuonize) → default の順で評価
/// 4. counter が末尾「目」を持つなら: base counter で再帰、最後に「メ」
/// 5. fallback: num_kata + counter (そのまま連結)
///
/// 0 は連濁・促音化を抑制する (例: ゼロ本 → ゼロホン)。
#[must_use]
pub fn euphonic_counter_read(
    num_kata: &str,
    counter: &str,
    raw_num: &str,
    counters: &CountersData,
    days: &DaysData,
) -> String {
    // 0 (ゼロ/レイ) は連濁・促音化対象外 — 99 を番兵として全ルールから外す
    let sd = if norm_num(raw_num) == "0" {
        99
    } else {
        effective_last_digit(raw_num)
    };

    // ─── 1. simple 表 ──────────────────────────────────────────────────────
    if let Some(suffix) = counters.simple.get(counter) {
        return format!("{num_kata}{suffix}");
    }

    // ─── 2. 日: days.toml 優先 ─────────────────────────────────────────────
    if counter == "日" {
        if let Some(n) = to_int(raw_num) {
            // `as u32` だと 4294967297 日が 1 日 (ついたち) に化けるので、 範囲外は表を引かない
            if let Some(day) = u32::try_from(n).ok().and_then(|d| days.get(d)) {
                return day.to_string();
            }
        }
        if let Some(rule) = counters.counter.get("日") {
            if let Some(default) = &rule.default {
                return format!("{num_kata}{default}");
            }
        }
        return format!("{num_kata}ニチ");
    }

    // ─── 3. counter 表 ─────────────────────────────────────────────────────
    if let Some(rule) = counters.counter.get(counter) {
        // 3a. 数値 specials (full override)
        let raw_normalized = norm_num(raw_num);
        if let Some(special) = rule.specials.get(&raw_normalized) {
            return special.clone();
        }

        // 3b. kana 末尾置換 (replacements、例: 時の 4→ヨン→ヨ)
        let mut adjusted_kana = num_kata.to_string();
        if sd != 99 {
            for repl in &rule.replacements {
                if repl.last_digit.contains(&sd) && adjusted_kana.ends_with(&repl.from) {
                    let cut = adjusted_kana.len() - repl.from.len();
                    adjusted_kana.truncate(cut);
                    adjusted_kana.push_str(&repl.to);
                    break;
                }
            }
        }

        // 3c. rules (last_digit + 連濁/促音化)
        if sd != 99 {
            for r in &rule.rules {
                if r.last_digit.contains(&sd) {
                    let body = if r.sokuonize {
                        sokuonize_hundred(&sokuonize_last(&adjusted_kana), &r.suffix)
                    } else {
                        adjusted_kana.clone()
                    };
                    return format!("{}{}", body, r.suffix);
                }
            }
        }

        // 3d. default
        if let Some(default) = &rule.default {
            return format!("{adjusted_kana}{default}");
        }
    }

    // ─── 4. 末尾「目」: 既存助数詞末尾の再帰モード ──────────────────────────
    if let Some(base_counter) = counter.strip_suffix('目') {
        if !base_counter.is_empty() {
            let base_read = euphonic_counter_read(num_kata, base_counter, raw_num, counters, days);
            return format!("{base_read}メ");
        }
        // base が空 = 「目」 単体。 これは scale 経由 (= 「100万目」 の 「100万」 を
        // scale で読んだ後に counter = 「目」 だけが残る) で起きる。 recursive rule の
        // suffix (= メ) を直接使う。 これが無いと step 5 の fallback に落ちて
        // **「目」 を surface のまま読みに混ぜてしまう** (「100万目指せる」 →
        // 「ヒャクマン目サセル」、 ★2026-09-11 実コーパス 704 万行で検出)。
        if let Some(rule) = counters.counter.get("目") {
            if let Some(suffix) = &rule.suffix {
                return format!("{num_kata}{suffix}");
            }
        }
    }

    // ─── 5. fallback ───────────────────────────────────────────────────────
    format!("{num_kata}{counter}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::parse_toml;

    fn load_counters() -> CountersData {
        let raw = include_str!("../../tests/fixtures/rules/counters.toml");
        parse_toml(raw, "counters.toml").unwrap()
    }

    fn load_days() -> DaysData {
        let raw = include_str!("../../tests/fixtures/rules/days.toml");
        parse_toml(raw, "days.toml").unwrap()
    }

    #[test]
    fn huge_day_count_does_not_wrap_into_a_calendar_day() {
        let c = load_counters();
        let d = load_days();
        // 2^32 + 1 日が `as u32` で 1 日 (ついたち) になっていた
        let big = "4294967297";
        assert_ne!(
            euphonic_counter_read("ヨンジュウ", "日", big, &c, &d),
            "ツイタチ"
        );
        assert_eq!(euphonic_counter_read("イチ", "日", "1", &c, &d), "ツイタチ");
    }

    #[test]
    fn basic_hon() {
        let c = load_counters();
        let d = load_days();
        assert_eq!(euphonic_counter_read("イチ", "本", "1", &c, &d), "イッポン");
        assert_eq!(euphonic_counter_read("サン", "本", "3", &c, &d), "サンボン");
        assert_eq!(euphonic_counter_read("ニ", "本", "2", &c, &d), "ニホン");
    }

    #[test]
    fn thousand_and_ten_thousand_endings_voice_like_three() {
        // 数の読みが「ん」(千 / 万) で終わる時は 3 と同じ連濁 (1000本 = せんぼん / 5万本 = ごまんぼん)
        let c = load_counters();
        let d = load_days();
        for (kata, raw, want) in [
            ("セン", "1000", "センボン"),
            ("サンゼン", "3000", "サンゼンボン"),
            ("サンゼン", "3,000", "サンゼンボン"),
            ("イチマン", "10000", "イチマンボン"),
            ("ゴマン", "5万", "ゴマンボン"),
            ("ゴセン", "5千", "ゴセンボン"),
            ("ヒャクマン", "1000000", "ヒャクマンボン"),
            ("センマン", "10000000", "センマンボン"),
        ] {
            assert_eq!(
                euphonic_counter_read(kata, "本", raw, &c, &d),
                want,
                "{raw}"
            );
        }
        // 境界: 末尾の 0 が 2 個以下 (10 / 100) と 8 個以上 (億) は従来どおり
        assert_eq!(
            euphonic_counter_read("ヒャク", "本", "100", &c, &d),
            "ヒャッポン"
        );
        assert_eq!(
            euphonic_counter_read("ジュウ", "本", "10", &c, &d),
            "ジュッポン"
        );
        assert_eq!(
            euphonic_counter_read("イチオク", "本", "100000000", &c, &d),
            "イチオクポン"
        );
        // 全部が 0 の列 (0000) は 千 / 万 ではないので 3 扱いにしない (末尾の 0 で判定する)
        assert_eq!(
            euphonic_counter_read("ゼロ", "本", "0000", &c, &d),
            "ゼロポン"
        );
        // 0 が 3 個でも 1 桁だけ (1000) でなく 1001 のような数は末尾の 1 で判定する
        assert_eq!(
            euphonic_counter_read("センイチ", "本", "1001", &c, &d),
            "センイッポン"
        );
    }

    #[test]
    fn hundred_sokuon_before_k_and_p_row_counters() {
        // 百の位で終わる数も、 カ行・パ行で始まる助数詞の前では促音化する。
        let c = load_counters();
        let d = load_days();
        assert_eq!(
            euphonic_counter_read("ヒャク", "本", "100", &c, &d),
            "ヒャッポン"
        );
        assert_eq!(
            euphonic_counter_read("ヒャク", "回", "100", &c, &d),
            "ヒャッカイ"
        );
        assert_eq!(
            euphonic_counter_read("ロッピャク", "本", "600", &c, &d),
            "ロッピャッポン"
        );
        assert_eq!(
            euphonic_counter_read("サンビャク", "回", "300", &c, &d),
            "サンビャッカイ"
        );
        // サ行 (歳) は ヒャクサイ のまま。 促音化しない末尾 (ニ) にも効かない。
        assert_eq!(
            euphonic_counter_read("ヒャク", "歳", "100", &c, &d),
            "ヒャクサイ"
        );
        assert_eq!(
            euphonic_counter_read("ジュウ", "本", "10", &c, &d),
            "ジュッポン"
        );
    }

    #[test]
    fn fun_pun() {
        let c = load_counters();
        let d = load_days();
        assert_eq!(euphonic_counter_read("イチ", "分", "1", &c, &d), "イップン");
        assert_eq!(euphonic_counter_read("ニ", "分", "2", &c, &d), "ニフン");
        assert_eq!(euphonic_counter_read("サン", "分", "3", &c, &d), "サンプン");
    }

    #[test]
    fn person_specials() {
        let c = load_counters();
        let d = load_days();
        assert_eq!(euphonic_counter_read("イチ", "人", "1", &c, &d), "ヒトリ");
        assert_eq!(euphonic_counter_read("ニ", "人", "2", &c, &d), "フタリ");
        assert_eq!(euphonic_counter_read("サン", "人", "3", &c, &d), "サンニン");
    }

    #[test]
    fn day_uses_days_table() {
        let c = load_counters();
        let d = load_days();
        assert_eq!(euphonic_counter_read("イチ", "日", "1", &c, &d), "ツイタチ");
        assert_eq!(euphonic_counter_read("ニ", "日", "2", &c, &d), "フツカ");
        assert_eq!(
            euphonic_counter_read("ニジュウ", "日", "20", &c, &d),
            "ハツカ"
        );
    }

    #[test]
    fn hour_replacements() {
        let c = load_counters();
        let d = load_days();
        assert_eq!(euphonic_counter_read("ヨン", "時", "4", &c, &d), "ヨジ");
        assert_eq!(euphonic_counter_read("ナナ", "時", "7", &c, &d), "シチジ");
        assert_eq!(euphonic_counter_read("キュウ", "時", "9", &c, &d), "クジ");
        assert_eq!(
            euphonic_counter_read("ジュウヨン", "時", "14", &c, &d),
            "ジュウヨジ"
        );
        assert_eq!(euphonic_counter_read("ゼロ", "時", "0", &c, &d), "レイジ");
    }

    /// replacement の適用条件は「digit 一致 **かつ** kana が from で終わる」(両方)で
    /// なければならない。数値カナと raw_num が整合する正規呼び出しでは 2 条件は同値だが、
    /// ends_with guard は「digit 一致だけで誤って truncate し、from を含まない kana を
    /// 破壊する」のを防ぐ防御契約。digit=4 (repl[4] from=ヨン) でも kana が ヨン で
    /// 終わらなければ置換してはならない。
    /// (故障モデル: guard を緩めると「ナナ」を ヨン 長で切って「ヨ」へ誤置換 = ヨジ)
    #[test]
    fn replacement_needs_kana_suffix_not_just_digit() {
        let c = load_counters();
        let d = load_days();
        // raw=4 は repl[4] の digit に一致するが num_kata="ナナ" は ヨン で終わらない。
        // 正しくは無置換で default ジ を付けるのみ → "ナナジ"。
        assert_eq!(euphonic_counter_read("ナナ", "時", "4", &c, &d), "ナナジ");
    }

    #[test]
    fn month_specials() {
        let c = load_counters();
        let d = load_days();
        assert_eq!(euphonic_counter_read("ヨン", "月", "4", &c, &d), "シガツ");
        assert_eq!(euphonic_counter_read("ナナ", "月", "7", &c, &d), "シチガツ");
        assert_eq!(euphonic_counter_read("キュウ", "月", "9", &c, &d), "クガツ");
        assert_eq!(euphonic_counter_read("イチ", "月", "1", &c, &d), "イチガツ");
    }

    #[test]
    fn recursive_me() {
        let c = load_counters();
        let d = load_days();
        // 「3 回目」→ サンカイメ (回 default + メ)
        assert_eq!(
            euphonic_counter_read("サン", "回目", "3", &c, &d),
            "サンカイメ"
        );
        // 「2 人目」→ フタリメ (人 special 2 → フタリ + メ)
        assert_eq!(euphonic_counter_read("ニ", "人目", "2", &c, &d), "フタリメ");
    }

    #[test]
    fn zero_no_sokuon() {
        let c = load_counters();
        let d = load_days();
        // 0 は連濁・促音化対象外
        assert_eq!(euphonic_counter_read("ゼロ", "本", "0", &c, &d), "ゼロホン");
        assert_eq!(euphonic_counter_read("ゼロ", "匹", "0", &c, &d), "ゼロヒキ");
        assert_eq!(euphonic_counter_read("ゼロ", "杯", "0", &c, &d), "ゼロハイ");
        assert_eq!(euphonic_counter_read("ゼロ", "分", "0", &c, &d), "ゼロフン");
        assert_eq!(euphonic_counter_read("ゼロ", "回", "0", &c, &d), "ゼロカイ");
    }

    #[test]
    fn juu_sokuon() {
        let c = load_counters();
        let d = load_days();
        // 10 + 助数詞 は促音化する
        assert_eq!(
            euphonic_counter_read("ジュウ", "本", "10", &c, &d),
            "ジュッポン"
        );
    }

    #[test]
    fn simple_suffix_passthrough() {
        let c = load_counters();
        let d = load_days();
        assert_eq!(
            euphonic_counter_read("ヒャク", "円", "100", &c, &d),
            "ヒャクエン"
        );
        assert_eq!(
            euphonic_counter_read("イチ", "ヶ月", "1", &c, &d),
            "イチカゲツ"
        );
    }

    #[test]
    fn unknown_counter_passthrough() {
        let c = load_counters();
        let d = load_days();
        assert_eq!(euphonic_counter_read("イチ", "謎", "1", &c, &d), "イチ謎");
    }
}
