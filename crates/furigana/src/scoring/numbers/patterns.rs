//! 数字系 candidate の **regex 定義 + 構築** (matching logic は `mod.rs` 側)。
//!
//! - 静的 pattern (data 非依存): [`TIME_COLON_RE`] / [`TIME_JP_FULL_RE`] /
//!   [`DATE_KANJI_FULL_RE`] / [`DATE_KANJI_MD_RE`] / [`DIGIT_RE`]
//! - 動的 builder (rules data 依存、 [`super::NumberCandidateProvider::new`] で 1 回だけ
//!   compile): [`build_counter_regexes`] / [`build_scale_regex`] / [`build_si_unit_regex`]
//!
//! builder は空 rules で `None` を返す (= never-match pattern を作らない、
//! 旧 NumberChunker の 51 GB alloc 暴走の教訓)。

use crate::rules::{CounterMode, CountersData, ScalesData, UnitsData};
use once_cell::sync::Lazy;
use regex::{Captures, Regex};

// ─── 静的 regex (data 非依存) ────────────────────────────────────────────────

/// 数値パターン (符号付き、 カンマ・小数対応)。
pub(super) const NUM_PAT: &str =
    r"[+\-\u{2212}\u{FF0D}\u{FF0B}]?[0-9０-９]+(?:,[0-9０-９]{3})*(?:\.[0-9０-９]+)?";

/// 日付・月日 用の 「Arabic 1〜4 桁 OR 漢数字 1〜3 文字」 pattern。
const DATE_NUM_PAT: &str = r"(?:[0-9０-９]{1,4}|[一二三四五六七八九十〇零]{1,3})";

/// 漢数字 pattern (= 末尾再帰助数詞 「N 個目」 の漢数字版用)。 `kansuji_to_arabic` が
/// 解釈できる範囲 (一〜九十百千万億 additive + 〇零 positional)。 「一個目」 「十二回目」
/// 「三百回目」 「一万回目」 等を catch する。
const KANJI_NUM_PAT: &str = r"[一二三四五六七八九十百千万億〇零]{1,16}";

pub(super) static TIME_COLON_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"([0-9０-９]{1,2})[:：]([0-9０-９]{2})(?:[:：]([0-9０-９]{2}))?")
        .expect("scoring TIME_COLON regex build failed")
});

/// 比 / スコア (1:1 / 50:50 / 3:7)。 時刻として読めない `数字:数字`
pub(super) static RATIO_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"([0-9０-９]{1,3})[:：]([0-9０-９]{1,3})")
        .expect("scoring RATIO regex build failed")
});

pub(super) static TIME_JP_FULL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"([0-9０-９]{1,2})時(?:([0-9０-９]{1,2})分)?(?:([0-9０-９]{1,2})秒)?")
        .expect("scoring TIME_JP regex build failed")
});

/// 漢数字の和式時刻 `三時五分` / `零時三十分二十秒`。 分 を必須にする (`三時` 単独は 時 の助数詞側、
/// `十分` / `五分` / `三分の一` は前に 時 が無いので当たらない)。 議事録の 「午後零時三分散会」 が
/// 三 + 分 (ぶん) + 散会 に割れていた (2026-09-28)
pub(super) static TIME_JP_KANJI_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"([〇零一二三四五六七八九十]{1,3})時([〇一二三四五六七八九十]{1,4})分(?:([〇一二三四五六七八九十]{1,4})秒)?")
        .expect("scoring TIME_JP_KANJI regex build failed")
});

pub(super) static DATE_KANJI_FULL_RE: Lazy<Regex> = Lazy::new(|| {
    let pat = format!(r"({DATE_NUM_PAT})年({DATE_NUM_PAT})月({DATE_NUM_PAT})日");
    Regex::new(&pat).expect("scoring DATE_KANJI_FULL regex build failed")
});

pub(super) static DATE_KANJI_MD_RE: Lazy<Regex> = Lazy::new(|| {
    let pat = format!(r"({DATE_NUM_PAT})月({DATE_NUM_PAT})日");
    Regex::new(&pat).expect("scoring DATE_KANJI_MD regex build failed")
});

/// 分数 「N分のM」 (間の半角空白は任意: Qiita 等の 「33 分の 10」 / 「14 分の 1」)。 2026-09-27
/// 空白が入ると 「33 分」 が助数詞 (ふん) と区切られず 分の = ふんの になっていた。
pub(super) static FRACTION_RE: Lazy<Regex> = Lazy::new(|| {
    let pat = format!(r"({NUM_PAT}) ?分の ?({NUM_PAT})");
    Regex::new(&pat).expect("scoring FRACTION regex build failed")
});

/// 位取りの漢数字の小数 (一・〇〇七 / 三・〇〇)。 小数部 2 桁以上に限る (一・二・三 のような列挙を拾わない)
pub(super) static KANJI_DECIMAL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new("^([〇一二三四五六七八九]{1,6})・([〇一二三四五六七八九]{2,12})")
        .expect("scoring KANJI_DECIMAL regex build failed")
});

/// 小数部 1 桁の漢数字の小数 (三八・四)。 整数部が位取り 2 桁の時だけ使う (try_kanji_decimal)
pub(super) static KANJI_DECIMAL_SHORT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new("^([〇一二三四五六七八九]{2})・([〇一二三四五六七八九])")
        .expect("scoring KANJI_DECIMAL_SHORT regex build failed")
});

/// 1 桁・1 桁の漢数字の小数で、 直後に単位が付く形 (二・五％ / 一・八倍 / 〇・七ポイント / 一・五万円)。
/// 列挙 (一・二・三) と区別するため、 直後の単位を必須にする (単位は数の一部ではないので取らない側の group 3)
pub(super) static KANJI_DECIMAL_UNIT_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        "^([〇一二三四五六七八九])・([〇一二三四五六七八九])(％|%|倍|割|万|億|兆|円|パーセント|ポイント|キロ|ミリ|センチ|メートル|ヶ月|か月|ヵ月|カ月)",
    )
    .expect("scoring KANJI_DECIMAL_UNIT regex build failed")
});

/// ドット区切り 3 要素以上の数字列 (バージョン `1.2.3` / IP `192.168.0.1`)。
/// 小数としては読めない (1.2 と .3 に割れる) ので、 各要素を数として読み 「テン」 で繋ぐ。
pub(super) static VERSION_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"[0-9０-９]+(?:\.[0-9０-９]+){2,}").expect("scoring VERSION regex build failed")
});

/// 電話番号の形。 区切りは `-` か空白。
/// - 国際: `+81-90-1234-5678` / `+1 555-123-4567` / `+44 7911 123456` (桁数の検査は呼び出し側)
/// - 国内: `03-1234-5678` / `090 1234 5678` / `0120-123-456`
/// - ハイフンなしの携帯: `09012345678`
pub(super) static PHONE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"\+[0-9]{1,3}[- ]?[0-9]{1,5}(?:[- ][0-9]{1,6}){1,3}|0[0-9]{1,3}[- ][0-9]{1,4}[- ][0-9]{3,4}|0[5789]0[0-9]{8}",
    )
    .expect("scoring PHONE regex build failed")
});

/// `N/M` (日付 12/25 か分数 1/2 か。 文脈で決める。 3 桁まで: 1/144 (縮尺) / 50/100 は分数)
pub(super) static SLASH_PAIR_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"([0-9]{1,3})[/\u{2044}]([0-9]{1,3})")
        .expect("scoring SLASH_PAIR regex build failed")
});

/// `N/M` の直後が日付の手がかり: 曜日の括弧 `(日)` / `（月・祝）`、 時刻 `20:00` / `20時`、 日付の言い方 (まで / から / 発売 …)
pub(super) static SLASH_DATE_AFTER_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"^(?:[ 　]?[(（][月火水木金土日祝]|[ 　]?[0-9]{1,2}(?::[0-9]{2}|時)|まで|から|より|発売|開催|配信|開始|開幕|公開|実施|予定|締切|締め切り|〆|終了|スタート|誕生日|記念日|時点|更新)",
    )
    .expect("scoring SLASH_DATE_AFTER regex build failed")
});

/// `N/M` の直後が分数の手がかり
pub(super) static SLASH_FRACTION_AFTER_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(?:サイズ|倍|割|以上|以下|程度|ほど|くらい|ぐらい|弱|強|ずつ)")
        .expect("scoring SLASH_FRACTION_AFTER regex build failed")
});

/// `N/M` の直前が分数の手がかり (HP1/10 / 確率 1/3)。 数値の途中は除く
pub(super) static SLASH_FRACTION_BEFORE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?:HP|MP|SP|LP|hp|mp|sp|確率|割合|体力|ゲージ|ライフ)[ 　]?$")
        .expect("scoring SLASH_FRACTION_BEFORE regex build failed")
});

/// `N/M` の直前が日付の手がかり (本日 / 明日 / 締切 / 発売日 …)
pub(super) static SLASH_DATE_BEFORE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?:本日|明日|明後日|来月|今月|締切|締め切り|〆切|期限|発売日|開催日|配信日|更新日|公開日|開始日|予定日|日程|日時)[:：は ]?[ 　]?$")
        .expect("scoring SLASH_DATE_BEFORE regex build failed")
});

/// 範囲 `N/M〜N/M` の途中: 直前が `N/M〜` (後ろの日付)
pub(super) static SLASH_RANGE_BEFORE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"([0-9]{1,2})/([0-9]{1,2})(?:\([^)]*\)|（[^）]*）)?[ 　]?[〜~～\-－ー–][ 　]?$")
        .expect("scoring SLASH_RANGE_BEFORE regex build failed")
});

/// 範囲 `N/M〜N/M` の先頭: 直後が `〜N/M`
pub(super) static SLASH_RANGE_AFTER_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(?:\([^)]*\)|（[^）]*）)?[ 　]?[〜~～\-－ー–][ 　]?([0-9]{1,2})/([0-9]{1,2})(?:[^0-9/]|$)")
        .expect("scoring SLASH_RANGE_AFTER regex build failed")
});

/// 文全体に日付の話題の語がある (配信 / イベント / 予定 / 発売 …)。 `N/M` の手がかりが近くに無い時の判定に使う
pub(super) static SLASH_SENTENCE_DATE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        "配信|ライブ|イベント|予定|発売|開催|通販|お品書き|試合|締切|〆切|スケジュール|日程|曜日|放送|公開|リリース|アップデート|メンテ|開幕|開始|終了|誕生日|記念日|ガチャ|限定|コミケ|参加|出演|チケット|販売|受付|応募|当選|ご案内|お知らせ|開館|休業|営業|開店",
    )
    .expect("scoring SLASH_SENTENCE_DATE regex build failed")
});

/// 文全体に分数・得点・進捗の話題の語がある。 あれば文脈だけでは日付にしない
pub(super) static SLASH_SENTENCE_FRACTION_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        "(?i)スコア|score|HP|MP|ページ|page|回目|分の|進捗|wordle|ターン|達成|確率|割合|ダメージ|%|％|スケール|比率|打率|安打|奪三振|第[0-9]",
    )
    .expect("scoring SLASH_SENTENCE_FRACTION regex build failed")
});

/// 年付きの日付 `2025/10/13` / `2000-10-12` (年 4 桁。 区切りは `/` か `-` で、 2 つとも同じ)
pub(super) static YMD_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"([0-9]{4})([/\-.])([0-9]{1,2})([/\-.])([0-9]{1,2})")
        .expect("scoring YMD regex build failed")
});

/// 郵便番号 `〒100-0001` / `〒1000001` / `郵便番号 100-0001` の数字部分 (3 桁 + 4 桁、 間のハイフンは任意)
pub(super) static POSTAL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"([0-9]{3})[-\u2010-\u2015\u2212ー]?([0-9]{4})")
        .expect("scoring POSTAL regex build failed")
});

pub(super) static DIGIT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(NUM_PAT).expect("scoring DIGIT regex build failed"));

// ─── 動的 regex builders (data 依存) ─────────────────────────────────────────

/// 助数詞 list から 2 本の regex を構築:
/// - **arabic**: `(NUM)( )?(base)(recursive)?` (= 算用 / 全角数字 + 助数詞、 recursive 任意。
///   数字と助数詞の間の半角空白 1 つを許す = 「5 分」 「3 人」、 2026-09-27)
/// - **kanji_recursive**: `(KANJI_NUM)(base)(recursive)` (= 漢数字 + 助数詞 + 「目」、 recursive **必須**)
///
/// recursive-mode の助数詞 (= 「目」) は base alternation に混ぜず trailing group にする。
/// こうしないと 「2 個目」 が 「2 個」 で止まり、 残った 「目」 が単漢字 fallback で
/// 「モク」 と誤読される (= 「ニコモク」)。group が match したら counter section で
/// `base + recursive` を合成して `euphonic_counter_read` の末尾再帰 (strip_suffix '目')
/// に委ねる。
///
/// 漢数字側は **recursive を必須** にする (= 「一個目」 のみ対象、 「一個」 「一日」 等の
/// bare 漢数字 + 助数詞 は従来通り Lindera / chunker に委ねる)。 こうしないと
/// 「一日中」 の 「一日」 が counter candidate 化して chunker 互換が崩れる
/// (= `tests::single_counter_kansuji_only_in_date_pattern`)。
pub(super) fn build_counter_regexes(counters: &CountersData) -> (Option<Regex>, Option<Regex>) {
    let mut base: Vec<String> = counters.simple.keys().cloned().collect();
    let mut recursive: Vec<String> = Vec::new();
    // 漢数字 bare 対応に opt-in した助数詞 (= `kanji_numeral = true`)。
    let mut kanji_optin = false;
    for (key, rule) in &counters.counter {
        if rule.mode == Some(CounterMode::Recursive) {
            recursive.push(key.clone());
        } else {
            base.push(key.clone());
        }
        if rule.kanji_numeral {
            kanji_optin = true;
        }
    }
    if base.is_empty() {
        return (None, None);
    }
    // 長い順に並べて alternation で longest-first match (= 「番目」 を 「番」 より先に)
    base.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));
    let base_alts: Vec<String> = base.iter().map(|s| regex::escape(s)).collect();
    let base_joined = base_alts.join("|");

    // 漢数字 regex は recursive 助数詞 (「目」) か opt-in 助数詞のどちらかが在れば構築する。
    // recursive group は optional 化し、 bare match (recursive 無し) の採否は matcher 側で
    // `kanji_numeral` フラグにより gate する (= 「一日中」 の 「一日」 等の誤 counter 化を防ぐ)。
    if recursive.is_empty() {
        let arabic = Regex::new(&format!(r"({NUM_PAT})(?: )?({base_joined})"))
            .expect("scoring counter regex build failed");
        let kanji = if kanji_optin {
            Some(
                Regex::new(&format!(r"({KANJI_NUM_PAT})({base_joined})"))
                    .expect("scoring kanji counter regex build failed"),
            )
        } else {
            None
        };
        return (Some(arabic), kanji);
    }

    recursive.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));
    let rec_alts: Vec<String> = recursive.iter().map(|s| regex::escape(s)).collect();
    let rec_joined = rec_alts.join("|");

    let arabic = Regex::new(&format!(r"({NUM_PAT})(?: )?({base_joined})({rec_joined})?"))
        .expect("scoring counter regex build failed");
    // recursive group を optional 化 (= 「一個目」 の recursive 形 + opt-in 助数詞の bare 形 両対応)。
    let kanji = Regex::new(&format!(r"({KANJI_NUM_PAT})({base_joined})({rec_joined})?"))
        .expect("scoring kanji counter regex build failed");
    (Some(arabic), Some(kanji))
}

/// 大数スケール (+ optional 末尾漢字 unit / counter) の regex を構築。 空 list なら `None`。
///
/// trailing 候補 (= 末尾の 1 字漢字 unit) は:
/// - units.toml の漢字 1 字 entries (= 円 / 度 等)
/// - counters の漢字 1 字 entries (= 個 / 歩 / 回 等) ★alpha.21 round 7 拡張
///
/// 「1 万歩 → イチマンポ」 「3 千個 → サンゼンコ」 のような scale + counter 連結を
/// scale section で 1 token 化するため、 counter キーも trailing pattern に含める。
pub(super) fn build_scale_regex(
    scales: &ScalesData,
    units: &UnitsData,
    counters: &CountersData,
) -> Option<Regex> {
    let kanjis: Vec<String> = scales.entries.iter().map(|e| e.kanji.clone()).collect();
    if kanjis.is_empty() {
        return None;
    }
    let mut sorted_scales = kanjis;
    sorted_scales.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));
    let scale_alts: Vec<String> = sorted_scales.iter().map(|s| regex::escape(s)).collect();

    let is_single_non_ascii_kanji = |s: &str| {
        s.chars().count() == 1 && s.chars().next().is_some_and(|c| !c.is_ascii_alphanumeric())
    };

    // unit + counter 両方から漢字 1 字 trailing を集めて merge (dedupe)。
    let mut trailing_set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for u in units
        .entries
        .keys()
        .filter(|s| is_single_non_ascii_kanji(s))
    {
        trailing_set.insert(u.clone());
    }
    for (c, _) in counters
        .counter
        .iter()
        .filter(|(s, r)| is_single_non_ascii_kanji(s) && r.scale_trailing)
    {
        trailing_set.insert(c.clone());
    }
    let trailing: Vec<String> = trailing_set.iter().map(|s| regex::escape(s)).collect();

    let pat = if trailing.is_empty() {
        format!(r"({NUM_PAT})({})", scale_alts.join("|"))
    } else {
        format!(
            r"({NUM_PAT})({})({})?",
            scale_alts.join("|"),
            trailing.join("|")
        )
    };
    Some(Regex::new(&pat).expect("scoring scale regex build failed"))
}

/// SI 単位の regex を構築 (case-insensitive)。 空 list なら `None`。
pub(super) fn build_si_unit_regex(units: &UnitsData) -> Option<Regex> {
    let symbols: Vec<String> = units.entries.keys().cloned().collect();
    if symbols.is_empty() {
        return None;
    }
    let mut sorted = symbols;
    sorted.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));
    let alts: Vec<String> = sorted.iter().map(|s| regex::escape(s)).collect();
    let pat = format!(r"(?i)({NUM_PAT})({})", alts.join("|"));
    Some(Regex::new(&pat).expect("scoring si_unit regex build failed"))
}

/// `re` が `hay` の先頭から (start == 0) match した場合のみ Captures を返す。
///
/// `re.captures` は先頭から離れた位置でも探すので、 長い入力の全位置でこれを呼ぶと入力長の 2 乗に比例する
/// (`1.2.3.4.5.…` を 3 万文字並べた入力で 45 秒かかった)。 数や日付・型番の形は 256 byte より長くならないので、
/// 先頭 256 byte (文字の途中では切らない) だけを見る
pub(super) fn at_start<'h>(re: &Regex, hay: &'h str) -> Option<Captures<'h>> {
    const WINDOW: usize = 256;
    let mut end = hay.len().min(WINDOW);
    while !hay.is_char_boundary(end) {
        end -= 1;
    }
    re.captures(&hay[..end])
        .filter(|c| c.get(0).is_some_and(|m| m.start() == 0))
}

/// 読点区切りの漢数字 + 助数詞 (`一、〇〇〇円` / `三、五〇〇円` / `一二、三四五人`)。 法令・判例の金額表記。
/// 先頭は 1〜3 桁 (1 桁目は 一〜九、 2 桁目以降は 〇 も可 = 一〇、〇〇〇)、 読点の後ろは **ちょうど 3 桁** の位取り漢数字 (〇〜九) の繰り返し。 counter が空なら `None`
pub(super) fn build_grouped_kanji_counter_regex(counters: &CountersData) -> Option<Regex> {
    let mut base: Vec<String> = counters.simple.keys().cloned().collect();
    for (key, rule) in &counters.counter {
        if rule.mode != Some(CounterMode::Recursive) {
            base.push(key.clone());
        }
    }
    if base.is_empty() {
        return None;
    }
    base.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));
    let base_joined = base
        .iter()
        .map(|s| regex::escape(s))
        .collect::<Vec<_>>()
        .join("|");
    Some(
        Regex::new(&format!(r"([一二三四五六七八九][〇一二三四五六七八九]{{0,2}}(?:、[〇一二三四五六七八九]{{3}})+)([万億兆千])?({base_joined})"))
            .expect("scoring grouped kanji counter regex build failed"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::RulesData;

    #[test]
    fn empty_counters_yield_none() {
        let rules = RulesData::default();
        let (arabic, kanji) = build_counter_regexes(&rules.counters);
        assert!(
            arabic.is_none(),
            "空 counters は None (never-match を作らない)"
        );
        assert!(kanji.is_none());
    }

    #[test]
    fn empty_scales_and_units_yield_none() {
        let rules = RulesData::default();
        assert!(build_scale_regex(&rules.scales, &rules.units, &rules.counters).is_none());
        assert!(build_si_unit_regex(&rules.units).is_none());
    }

    #[test]
    fn kanji_optin_builds_bare_kanji_regex_without_recursive() {
        // recursive 助数詞 (「目」) が無くても、 kanji_numeral opt-in 助数詞が在れば
        // 漢数字 regex が構築され bare match する。
        let toml_str = r#"
            [counter."匹"]
            default = "ヒキ"
            kanji_numeral = true
        "#;
        let counters: crate::rules::CountersData = toml::from_str(toml_str).unwrap();
        let (arabic, kanji) = build_counter_regexes(&counters);
        assert!(arabic.is_some());
        let kanji = kanji.expect("opt-in 助数詞で漢数字 regex が構築される");
        assert!(
            at_start(&kanji, "五匹").is_some(),
            "五匹 が bare match する"
        );
    }

    #[test]
    fn non_optin_counter_yields_no_kanji_regex_without_recursive() {
        // opt-in も recursive も無ければ漢数字 regex は None (従来挙動)。
        let toml_str = r#"
            [counter."杯"]
            default = "ハイ"
        "#;
        let counters: crate::rules::CountersData = toml::from_str(toml_str).unwrap();
        let (_arabic, kanji) = build_counter_regexes(&counters);
        assert!(kanji.is_none(), "opt-in 無しは漢数字 regex を作らない");
    }

    #[test]
    fn at_start_looks_only_at_the_head_window() {
        // 3 万文字続く版番号でも、 見るのは先頭 256 byte だけ (全位置で全体を舐めると 2 乗で遅くなる)
        let long = "1.2.3.4.5.".repeat(3000);
        let m = at_start(&VERSION_RE, &long).unwrap();
        assert!(m.get(0).unwrap().end() <= 256);
        // 文字の途中では切らない (全角数字は 3 byte)
        let wide = "１．２．３".repeat(200);
        assert!(at_start(&Regex::new(r"[０-９]").unwrap(), &wide).is_some());
    }

    #[test]
    fn at_start_rejects_mid_string_match() {
        let re = Regex::new(r"[0-9]+").unwrap();
        assert!(at_start(&re, "12x").is_some());
        assert!(
            at_start(&re, "x12").is_none(),
            "先頭以外の match は採用しない"
        );
    }
}
