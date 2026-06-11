//! ひらがな⇄カタカナ変換、漢字判定、Unicode 正規化ユーティリティ
//!
//! データに依存しない純粋関数のみ。
//! `normalize_text` だけ [`CompatData`](crate::rules::CompatData) を引数に取り、
//! 異体字置換を行う。

use crate::rules::CompatData;
use once_cell::sync::Lazy;
use regex::Regex;
use unicode_normalization::UnicodeNormalization;

// ─── 範囲定数 ────────────────────────────────────────────────────────────────

/// ひらがな範囲: ぁ(0x3041) 〜 ん(0x3093)
const HIRAGANA_START: u32 = 0x3041;
const HIRAGANA_END: u32 = 0x3093;

/// カタカナ範囲: ァ(0x30A1) 〜 ン(0x30F3)
const KATAKANA_START: u32 = 0x30A1;
const KATAKANA_END: u32 = 0x30F3;

/// ひら⇄カタ オフセット
const KATA_HIRA_OFFSET: u32 = 0x60;

// ─── 単文字判定 ──────────────────────────────────────────────────────────────

/// ひらがな 1 文字か (ぁ〜ん + ゔ)
#[must_use]
pub fn is_hiragana_char(c: char) -> bool {
    let cp = c as u32;
    (HIRAGANA_START..=HIRAGANA_END).contains(&cp) || c == 'ゔ'
}

/// カタカナ 1 文字か (ァ〜ン + ヴ)
#[must_use]
pub fn is_katakana_char(c: char) -> bool {
    let cp = c as u32;
    (KATAKANA_START..=KATAKANA_END).contains(&cp) || c == 'ヴ'
}

/// 漢字 1 文字か (CJK 統合漢字 + 拡張 A + 互換 + 々〆ヶ)
#[must_use]
pub fn is_kanji_char(c: char) -> bool {
    matches!(c,
        '\u{3400}'..='\u{4DBF}' |   // CJK 拡張 A
        '\u{4E00}'..='\u{9FFF}' |   // CJK 統合漢字
        '\u{F900}'..='\u{FAFF}' |   // CJK 互換
        '々' | '〆' | 'ヶ'
    )
}

// ─── 文字列単位 ──────────────────────────────────────────────────────────────

/// カタカナ→ひらがな
#[must_use]
pub fn kata_to_hira(s: &str) -> String {
    s.chars()
        .map(|c| {
            let cp = c as u32;
            if (KATAKANA_START..=KATAKANA_END).contains(&cp) {
                char::from_u32(cp - KATA_HIRA_OFFSET).unwrap_or(c)
            } else if c == 'ヴ' {
                'ゔ'
            } else {
                c
            }
        })
        .collect()
}

/// UniDic 発音形 (pron) の長音符 「ー」 を 表記読み に正規化する。
///
/// UniDic は 「学校 = ガッコー」 「大きい = オオキー」 「美しい = ウツクシー」 のように
/// 長音を 「ー」 で表記するため、 ja-furigana のルビ振り用途 (= 表記読み 「ガッコウ」
/// 「オオキイ」 「ウツクシイ」) に変換する必要がある。
///
/// 変換規則 (直前 kana の母音段で展開):
/// - ア段 + ー → ア段 + ア (= カー → カア)
/// - イ段 + ー → イ段 + イ (= シー → シイ、 キー → キイ)
/// - ウ段 + ー → ウ段 + ウ (= スー → スウ、 ツー → ツウ)
/// - エ段 + ー → エ段 + イ (= ケー → ケイ、 セー → セイ、 漢語慣習)
/// - オ段 + ー → オ段 + ウ (= コー → コウ、 ジョー → ジョウ、 漢語慣習)
///
/// 注意: 外来語 surface (= カタカナ表記、 「コーヒー」 「ボール」 等) で 「ー」 が
/// **そのまま使うべき** ケースがあるが、 本関数では一律展開する (= 外来語かどうか
/// は形態素レベルでは判定不能、 caller が surface 種別で post-process する想定)。
/// UniDic でも 外来語 entry は元々 ー で登録されてるので、 lForm 経由なら ー 保持可能。
///
/// IPADIC は元々 表記読み を返すので影響なし、 UniDic 環境でのみ意味を持つ。
#[must_use]
pub fn normalize_long_vowel(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == 'ー' && i > 0 {
            let prev = chars[i - 1];
            if let Some(vowel) = vowel_of_kana(prev) {
                // オ段は ウ、 エ段は イ、 他は同じ母音で展開
                let expanded = match vowel {
                    'o' => 'ウ',
                    'e' => 'イ',
                    'a' => 'ア',
                    'i' => 'イ',
                    'u' => 'ウ',
                    _ => 'ー',
                };
                if expanded != 'ー' {
                    out.push(expanded);
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

/// カタカナ 1 字の母音段を返す ('a' / 'i' / 'u' / 'e' / 'o')。
/// 該当しない場合 None (= 撥音 / 促音 / 拗音 / 長音 / 他文字)。
fn vowel_of_kana(c: char) -> Option<char> {
    // ア段 (= 母音 a)
    if matches!(
        c,
        'ア' | 'カ'
            | 'サ'
            | 'タ'
            | 'ナ'
            | 'ハ'
            | 'マ'
            | 'ヤ'
            | 'ラ'
            | 'ワ'
            | 'ガ'
            | 'ザ'
            | 'ダ'
            | 'バ'
            | 'パ'
            | 'ャ'
            | 'ァ'
            | 'ヮ'
    ) {
        return Some('a');
    }
    // イ段 (= 母音 i)
    if matches!(
        c,
        'イ' | 'キ'
            | 'シ'
            | 'チ'
            | 'ニ'
            | 'ヒ'
            | 'ミ'
            | 'リ'
            | 'ヰ'
            | 'ギ'
            | 'ジ'
            | 'ヂ'
            | 'ビ'
            | 'ピ'
            | 'ィ'
    ) {
        return Some('i');
    }
    // ウ段 (= 母音 u)
    if matches!(
        c,
        'ウ' | 'ク'
            | 'ス'
            | 'ツ'
            | 'ヌ'
            | 'フ'
            | 'ム'
            | 'ユ'
            | 'ル'
            | 'グ'
            | 'ズ'
            | 'ヅ'
            | 'ブ'
            | 'プ'
            | 'ュ'
            | 'ゥ'
            | 'ヴ'
    ) {
        return Some('u');
    }
    // エ段 (= 母音 e)
    if matches!(
        c,
        'エ' | 'ケ'
            | 'セ'
            | 'テ'
            | 'ネ'
            | 'ヘ'
            | 'メ'
            | 'レ'
            | 'ヱ'
            | 'ゲ'
            | 'ゼ'
            | 'デ'
            | 'ベ'
            | 'ペ'
            | 'ェ'
    ) {
        return Some('e');
    }
    // オ段 (= 母音 o)
    if matches!(
        c,
        'オ' | 'コ'
            | 'ソ'
            | 'ト'
            | 'ノ'
            | 'ホ'
            | 'モ'
            | 'ヨ'
            | 'ロ'
            | 'ヲ'
            | 'ゴ'
            | 'ゾ'
            | 'ド'
            | 'ボ'
            | 'ポ'
            | 'ョ'
            | 'ォ'
    ) {
        return Some('o');
    }
    None
}

/// ひらがな→カタカナ
#[must_use]
pub fn hira_to_kata(s: &str) -> String {
    s.chars()
        .map(|c| {
            let cp = c as u32;
            if (HIRAGANA_START..=HIRAGANA_END).contains(&cp) {
                char::from_u32(cp + KATA_HIRA_OFFSET).unwrap_or(c)
            } else if c == 'ゔ' {
                'ヴ'
            } else {
                c
            }
        })
        .collect()
}

/// 漢字を 1 文字でも含むか
#[must_use]
pub fn has_kanji(s: &str) -> bool {
    s.chars().any(is_kanji_char)
}

/// カタカナを 1 文字でも含むか (長音 ー 含む)
#[must_use]
pub fn has_katakana(s: &str) -> bool {
    s.chars()
        .any(|c| is_katakana_char(c) || c == 'ー' || c == 'ヴ')
}

/// kana reading の **第 1 音を連濁化** する (踊り字 「々」 展開で使用)。
///
/// カタカナ / ひらがな の両方で同 logic 動作。 カ/サ/タ/ハ 行の清音 → 対応する濁音
/// (ハ 行は半濁音前の濁音) に変換し、 第 1 音を置き換えた新文字列を返す。 連濁対象外
/// (ア/ナ/マ/ヤ/ラ/ワ 行 + 既に濁音 + ハ 行半濁音) は `None` を返し、 caller は
/// 「清音のまま複製」 にフォールバックする想定。
///
/// Smart engine の踊り字処理 (`scoring::odoriji` の `RendakuPass`) から使う。
/// (旧 Strict engine の `expand_odoriji_inplace` と共有していた logic、 alpha.15 の
/// Strict 撤廃後は Smart 側のみが caller。)
///
/// ## 例
///
/// ```
/// use furigana::kana::voice_first_kana;
///
/// assert_eq!(voice_first_kana("カミ").as_deref(), Some("ガミ"));   // 神々 → カミ + ガミ
/// assert_eq!(voice_first_kana("ヒト").as_deref(), Some("ビト"));   // 人々 → ヒト + ビト
/// assert_eq!(voice_first_kana("ひと").as_deref(), Some("びと"));   // ひらがなも対応 ★round 48
/// assert_eq!(voice_first_kana("ワレ"), None);                       // 我々 → ワレワレ (連濁なし)
/// assert_eq!(voice_first_kana("ヤマ"), None);                       // 山々 → ヤマヤマ (連濁なし)
/// ```
#[must_use]
pub fn voice_first_kana(reading: &str) -> Option<String> {
    let mut chars = reading.chars();
    let first = chars.next()?;
    let voiced = match first {
        // ─── カタカナ ───────────────────────────────────────────────────────
        'カ' => 'ガ',
        'キ' => 'ギ',
        'ク' => 'グ',
        'ケ' => 'ゲ',
        'コ' => 'ゴ',
        'サ' => 'ザ',
        'シ' => 'ジ',
        'ス' => 'ズ',
        'セ' => 'ゼ',
        'ソ' => 'ゾ',
        'タ' => 'ダ',
        'チ' => 'ヂ',
        'ツ' => 'ヅ',
        'テ' => 'デ',
        'ト' => 'ド',
        'ハ' => 'バ',
        'ヒ' => 'ビ',
        'フ' => 'ブ',
        'ヘ' => 'ベ',
        'ホ' => 'ボ',
        // ─── ひらがな (★round 48、 unihan/joyo の ひらがな default で「人々→ひとひと」
        //     になっていた問題を fix。 「人/ひと」 + 「々/ひと」 → 「ひと/びと」 連濁) ──
        'か' => 'が',
        'き' => 'ぎ',
        'く' => 'ぐ',
        'け' => 'げ',
        'こ' => 'ご',
        'さ' => 'ざ',
        'し' => 'じ',
        'す' => 'ず',
        'せ' => 'ぜ',
        'そ' => 'ぞ',
        'た' => 'だ',
        'ち' => 'ぢ',
        'つ' => 'づ',
        'て' => 'で',
        'と' => 'ど',
        'は' => 'ば',
        'ひ' => 'び',
        'ふ' => 'ぶ',
        'へ' => 'べ',
        'ほ' => 'ぼ',
        _ => return None,
    };
    let mut out = String::new();
    out.push(voiced);
    out.push_str(chars.as_str());
    Some(out)
}

/// 純カタカナ文字列か (長音 ー / 中点 ・ も許容)
#[must_use]
pub fn is_pure_katakana(s: &str) -> bool {
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[゠-ヿー・]+$").unwrap());
    !s.is_empty() && RE.is_match(s)
}

/// 純ひらがな文字列か (ゔ 含む、その他記号は不可)
#[must_use]
pub fn is_pure_hiragana(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_hiragana_char)
}

// ─── 全角→半角 ──────────────────────────────────────────────────────────────

/// 全角英数字・記号 → 半角
#[must_use]
pub fn zen_to_han(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '０'..='９' => char::from_u32(c as u32 - '０' as u32 + '0' as u32).unwrap_or(c),
            'Ａ'..='Ｚ' => char::from_u32(c as u32 - 'Ａ' as u32 + 'A' as u32).unwrap_or(c),
            'ａ'..='ｚ' => char::from_u32(c as u32 - 'ａ' as u32 + 'a' as u32).unwrap_or(c),
            '－' | '\u{2212}' => '-',
            '＋' => '+',
            '～' | '〜' => '~',
            '％' => '%',
            '．' => '.',
            '，' => ',',
            '／' => '/',
            _ => c,
        })
        .collect()
}

// ─── 正規化 ──────────────────────────────────────────────────────────────────

/// テキスト正規化: NFKC → 異体字置換 → NFC
///
/// `compat_map` の variant → canonical 変換を、NFKC の後に適用する。
/// 入力が空なら空文字列を返す。
#[must_use]
pub fn normalize_text(s: &str, compat: &CompatData) -> String {
    if s.is_empty() {
        return String::new();
    }
    // NFKC で結合・互換正規化
    let nfkc: String = s.nfkc().collect();
    // 異体字置換 (1 文字単位)
    let replaced: String = nfkc
        .chars()
        .map(|c| {
            let cs = c.to_string();
            if let Some(canonical) = compat.lookup(&cs) {
                canonical.chars().next().unwrap_or(c)
            } else {
                c
            }
        })
        .collect();
    // 安全のため NFC で正規化結合
    replaced.nfc().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // ─── kata_to_hira / hira_to_kata ──────────────────────────────

    #[test]
    fn kata_to_hira_basic() {
        assert_eq!(kata_to_hira("ヨム"), "よむ");
        assert_eq!(kata_to_hira("トウキョウ"), "とうきょう");
        assert_eq!(kata_to_hira("ヴァイオリン"), "ゔぁいおりん");
    }

    #[test]
    fn kata_to_hira_passthrough() {
        assert_eq!(kata_to_hira("漢字"), "漢字");
        assert_eq!(kata_to_hira("hello123"), "hello123");
        assert_eq!(kata_to_hira(""), "");
    }

    #[test]
    fn kata_to_hira_keeps_long_mark_and_punct() {
        assert_eq!(kata_to_hira("コーヒー・ラテ"), "こーひー・らて");
    }

    #[test]
    fn hira_to_kata_basic() {
        assert_eq!(hira_to_kata("よむ"), "ヨム");
        assert_eq!(hira_to_kata("とうきょう"), "トウキョウ");
        assert_eq!(hira_to_kata("ゔぁ"), "ヴァ");
    }

    #[test]
    fn round_trip_kata_hira() {
        let original = "アイウエオカキクケコ";
        assert_eq!(hira_to_kata(&kata_to_hira(original)), original);
    }

    // ─── 単文字判定 ───────────────────────────────────────────────

    #[test]
    fn is_hiragana_char_works() {
        assert!(is_hiragana_char('あ'));
        assert!(is_hiragana_char('ん'));
        assert!(is_hiragana_char('ゔ'));
        assert!(!is_hiragana_char('ア'));
        assert!(!is_hiragana_char('a'));
    }

    #[test]
    fn is_katakana_char_works() {
        assert!(is_katakana_char('ア'));
        assert!(is_katakana_char('ン'));
        assert!(is_katakana_char('ヴ'));
        assert!(!is_katakana_char('あ'));
        assert!(!is_katakana_char('a'));
    }

    #[test]
    fn is_kanji_char_works() {
        assert!(is_kanji_char('漢'));
        assert!(is_kanji_char('東'));
        assert!(is_kanji_char('々'));
        assert!(is_kanji_char('〆'));
        assert!(is_kanji_char('ヶ'));
        assert!(!is_kanji_char('あ'));
        assert!(!is_kanji_char('a'));
    }

    // ─── has_kanji / has_katakana ───────────────────────────────

    #[test]
    fn has_kanji_works() {
        assert!(has_kanji("読む"));
        assert!(has_kanji("東京タワー"));
        assert!(has_kanji("々"));
        assert!(!has_kanji("よむ"));
        assert!(!has_kanji("カタカナ"));
        assert!(!has_kanji(""));
    }

    #[test]
    fn has_katakana_works() {
        assert!(has_katakana("カタカナ"));
        assert!(has_katakana("漢字とカナ"));
        assert!(has_katakana("コーヒー"));
        assert!(!has_katakana("ひらがな"));
        assert!(!has_katakana("漢字"));
    }

    // ─── pure 判定 ────────────────────────────────────────────────

    #[test]
    fn is_pure_katakana_works() {
        assert!(is_pure_katakana("カタカナ"));
        assert!(is_pure_katakana("タワー"));
        assert!(is_pure_katakana("コーヒー・ラテ"));
        assert!(!is_pure_katakana("漢字"));
        assert!(!is_pure_katakana("ひらがな"));
        assert!(!is_pure_katakana(""));
        assert!(!is_pure_katakana("カナと漢字"));
    }

    #[test]
    fn is_pure_hiragana_works() {
        assert!(is_pure_hiragana("ひらがな"));
        assert!(is_pure_hiragana("ゔぁい"));
        assert!(!is_pure_hiragana("カタカナ"));
        assert!(!is_pure_hiragana(""));
        assert!(!is_pure_hiragana("ひらと漢字"));
    }

    // ─── zen_to_han ──────────────────────────────────────────────

    #[test]
    fn zen_to_han_digits_and_symbols() {
        assert_eq!(zen_to_han("１２３"), "123");
        assert_eq!(zen_to_han("５０％"), "50%");
        assert_eq!(zen_to_han("Ａ＋Ｂ"), "A+B");
        assert_eq!(zen_to_han("ｈｅｌｌｏ"), "hello");
        assert_eq!(zen_to_han("１．５"), "1.5");
    }

    #[test]
    fn zen_to_han_passthrough() {
        // 漢字・カナはそのまま
        assert_eq!(zen_to_han("漢字"), "漢字");
        assert_eq!(zen_to_han("カナ"), "カナ");
    }

    // ─── normalize_text ──────────────────────────────────────────

    fn make_compat(pairs: &[(&str, &str)]) -> CompatData {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(v, c)| ((*v).to_string(), (*c).to_string()))
            .collect();
        CompatData { map }
    }

    #[test]
    fn normalize_text_replaces_variants() {
        let compat = make_compat(&[("髙", "高"), ("﨑", "崎")]);
        assert_eq!(normalize_text("髙﨑", &compat), "高崎");
    }

    #[test]
    fn normalize_text_keeps_unmapped() {
        let compat = make_compat(&[]);
        assert_eq!(normalize_text("こんにちは", &compat), "こんにちは");
    }

    #[test]
    fn normalize_text_applies_nfkc() {
        let compat = make_compat(&[]);
        // 全角数字 NFKC → 半角
        assert_eq!(normalize_text("１２３", &compat), "123");
    }

    #[test]
    fn normalize_text_empty() {
        let compat = make_compat(&[]);
        assert_eq!(normalize_text("", &compat), "");
    }
}
