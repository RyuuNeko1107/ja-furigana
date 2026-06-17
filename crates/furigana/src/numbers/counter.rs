//! 数値 + 助数詞 の読み解決 (data-driven)
//!
//! [`euphonic_counter_read`] が中核。`CountersData` と `DaysData` を引数に
//! 取り、助数詞の連濁・促音化・kana 末尾置換・特殊読み・末尾「目」再帰
//! までをルールデータに従って処理する。

use super::helpers::{last_digit, norm_num, sokuonize_last, to_int};
use crate::rules::{CountersData, DaysData};

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
        last_digit(raw_num)
    };

    // ─── 1. simple 表 ──────────────────────────────────────────────────────
    if let Some(suffix) = counters.simple.get(counter) {
        return format!("{num_kata}{suffix}");
    }

    // ─── 2. 日: days.toml 優先 ─────────────────────────────────────────────
    if counter == "日" {
        if let Some(n) = to_int(raw_num) {
            if n >= 0 {
                if let Some(day) = days.get(n as u32) {
                    return day.to_string();
                }
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
                        sokuonize_last(&adjusted_kana)
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
    fn basic_hon() {
        let c = load_counters();
        let d = load_days();
        assert_eq!(euphonic_counter_read("イチ", "本", "1", &c, &d), "イッポン");
        assert_eq!(euphonic_counter_read("サン", "本", "3", &c, &d), "サンボン");
        assert_eq!(euphonic_counter_read("ニ", "本", "2", &c, &d), "ニホン");
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
