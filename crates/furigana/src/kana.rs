//! ひらがな⇄カタカナ変換、漢字判定、Unicode 正規化ユーティリティ
//!
//! データに依存しない純粋関数のみ。
//! `normalize_text` だけ [`CompatData`](crate::rules::CompatData) を引数に取り、
//! 異体字置換を行う。

use crate::char_class::{self, HIRAGANA_END, HIRAGANA_START, KATAKANA_END, KATAKANA_START};
use crate::rules::CompatData;
use once_cell::sync::Lazy;
use regex::Regex;
use unicode_normalization::UnicodeNormalization;

// ─── 範囲定数 ────────────────────────────────────────────────────────────────

// Unicode range 定数は crate::char_class に集約 (本 module は変換 offset のみ持つ)。

/// ひら⇄カタ オフセット
const KATA_HIRA_OFFSET: u32 = 0x60;

// ─── 単文字判定 ──────────────────────────────────────────────────────────────
//
// 実装は crate::char_class に集約、 本 module は公開 API として delegate を維持。

/// ひらがな 1 文字か (ぁ〜ん + ゔ)
#[must_use]
pub fn is_hiragana_char(c: char) -> bool {
    char_class::is_hiragana_char(c)
}

/// カタカナ 1 文字か (ァ〜ン + ヴ)
#[must_use]
pub fn is_katakana_char(c: char) -> bool {
    char_class::is_katakana_char(c)
}

/// 漢字 1 文字か (CJK 統合漢字 + 拡張 A + 互換 + 々〆ヶ)
#[must_use]
pub fn is_kanji_char(c: char) -> bool {
    char_class::is_kanji_char(c)
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

/// カタカナを 1 文字でも含むか (長音 ー / 半角カナ 含む)
#[must_use]
pub fn has_katakana(s: &str) -> bool {
    s.chars().any(char_class::is_katakana_loose_char)
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

/// 異体字セレクタ (variation selector) か。
///
/// NFKC/NFC では剥がれないが、 dict lookup では base char に揃えたいので除去する。
/// - U+FE00〜U+FE0F: Variation Selectors (VS1〜VS16、 BMP)
/// - U+E0100〜U+E01EF: Variation Selectors Supplement (IVS)
#[inline]
fn is_variation_selector(c: char) -> bool {
    matches!(c, '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}')
}

/// テキスト正規化: 異体字セレクタ除去 → NFKC → 異体字置換 → NFC
///
/// `compat_map` の variant → canonical 変換を、NFKC の後に適用する。
/// 入力が空なら空文字列を返す。
///
/// 表示 surface も正規化したい呼び出し側はこちら、 lookup だけ正規化して表示は
/// 原文を保つ場合は [`normalize_text_aligned`] を使う (production の `to_*` / `analyze`
/// は後者)。 両者の正規化テキストは一致する (本関数は aligned 版の `.text` を返す)。
#[must_use]
pub fn normalize_text(s: &str, compat: &CompatData) -> String {
    normalize_text_aligned(s, compat).text
}

/// 1 文字を NFKC → compat 置換 → NFC した断片を返す。
///
/// [`normalize_text_aligned`] が unit ごとに呼ぶ。 NFKC は **1 文字単位** で適用する
/// (旧 `normalize_text` の whole-string NFKC と、 結合文字列を跨がない実用入力では
/// 一致する)。 compat は variant → canonical の先頭 1 文字で置換 (= 旧実装と同挙動)。
fn normalize_char_piece(ch: char, compat: &CompatData) -> String {
    let nfkc: String = ch.to_string().nfkc().collect();
    let replaced: String = nfkc
        .chars()
        .map(|c| {
            compat
                .lookup(&c.to_string())
                .and_then(|canonical| canonical.chars().next())
                .unwrap_or(c)
        })
        .collect();
    replaced.nfc().collect()
}

/// 正規化テキスト + 原文への char 単位 alignment。
///
/// `text` は dict lookup / 形態素解析に渡す正規化形 ([`normalize_text`] と一致)。
/// `units` で 「正規化後 byte 位置 → 原文 byte 範囲」 を保持し、 解析後に各 token の
/// surface を原文 (= 異体字 / IVS / 全角を保ったまま) へ戻せる。
///
/// production の `to_ruby` 等は 「lookup は正規化形、 表示は原文 surface」 を満たすため
/// これを使う (例: `髙田` → 高田 で lookup → 表示は `{髙田|たかだ}`)。
pub(crate) struct NormalizedText {
    /// lookup 用の正規化テキスト
    pub text: String,
    /// 正規化が原文と異なるか。 false なら remap 不要 (fast path)。
    pub changed: bool,
    /// 正規化 unit 列 (`norm_start` 昇順、 隙間なく原文・正規化テキスト両方を覆う)。
    units: Vec<NormUnit>,
}

/// 原文 1 文字 (+ 後続の異体字セレクタ) → 正規化テキスト上の断片 1 つの対応。
struct NormUnit {
    /// 正規化テキスト上の開始 byte 位置 (この unit が生成した断片の先頭)
    norm_start: usize,
    /// 原文上の byte 範囲
    orig: std::ops::Range<usize>,
}

impl NormalizedText {
    /// 正規化テキスト ([`Self::text`]) を連結した token surface 列を受け取り、
    /// 各 token を原文 surface + 原文 byte range に対応付けて返す (token と同順)。
    ///
    /// 各 unit は 「正規化後 `norm_start` を含む token」 に割り当てる。 token 境界が
    /// 多文字展開 unit (`㍻` → `平成` 等) の途中に落ちた場合、 その原文 1 文字は
    /// 先行 token に丸ごと付き、 後続 token の原文 surface は空になる。 これにより
    /// 「原文 surface の連結 == 原文」 の不変条件が常に保たれる。
    pub(crate) fn remap<'a>(
        &self,
        orig: &str,
        surfaces: impl IntoIterator<Item = &'a str>,
    ) -> Vec<(String, std::ops::Range<usize>)> {
        let mut out = Vec::new();
        let mut cursor_norm = 0usize;
        let mut cursor_orig = 0usize;
        let mut ui = 0usize;
        for surf in surfaces {
            let te = cursor_norm + surf.len();
            let o_start = cursor_orig;
            let mut o_end = cursor_orig;
            while ui < self.units.len() && self.units[ui].norm_start < te {
                o_end = self.units[ui].orig.end;
                ui += 1;
            }
            out.push((orig[o_start..o_end].to_string(), o_start..o_end));
            cursor_orig = o_end;
            cursor_norm = te;
        }
        out
    }
}

/// テキストを正規化しつつ、 原文 surface へ戻すための alignment を保持する。
///
/// 異体字セレクタ (IVS / VS) は除去して直前 unit の原文範囲に併合 (= norm へは寄与
/// しない)。 それ以外は 1 文字ごとに [`normalize_char_piece`] を適用し、 原文 1 文字 ↔
/// 正規化断片 1 つの unit を作る。
#[must_use]
pub(crate) fn normalize_text_aligned(s: &str, compat: &CompatData) -> NormalizedText {
    let mut text = String::new();
    let mut units: Vec<NormUnit> = Vec::new();
    for (orig_off, ch) in s.char_indices() {
        let orig_end = orig_off + ch.len_utf8();
        if is_variation_selector(ch) {
            // VS は norm に出さず、 直前 unit の原文範囲を伸ばして吸収する。
            if let Some(last) = units.last_mut() {
                last.orig.end = orig_end;
            } else {
                // 先頭 orphan VS: norm 寄与ゼロの unit として記録 (表示時に原文へ復元)
                units.push(NormUnit {
                    norm_start: text.len(),
                    orig: orig_off..orig_end,
                });
            }
            continue;
        }
        let norm_start = text.len();
        text.push_str(&normalize_char_piece(ch, compat));
        units.push(NormUnit {
            norm_start,
            orig: orig_off..orig_end,
        });
    }
    let changed = text != s;
    NormalizedText {
        text,
        changed,
        units,
    }
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
    fn normalize_text_strips_variation_selectors() {
        let compat = make_compat(&[]);
        // IVS / 異体字セレクタ付き漢字は base char に正規化され dict lookup できる。
        // NFKC/NFC は VS を剥がさないので明示除去が要る。人名 (葛飾/辻 等) で頻出。
        assert_eq!(normalize_text("葛\u{E0100}飾", &compat), "葛飾"); // IVS (VS Supplement)
        assert_eq!(normalize_text("辻\u{FE00}", &compat), "辻"); // VS1 (BMP)
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

    #[test]
    fn voice_first_kana_covers_rendaku_table() {
        // 先頭かなの連濁 (清音→濁音)。カタカナ・ひらがな両方の各 arm を網羅。
        let pairs = [
            ("カ", "ガ"),
            ("キ", "ギ"),
            ("ク", "グ"),
            ("ケ", "ゲ"),
            ("コ", "ゴ"),
            ("サ", "ザ"),
            ("シ", "ジ"),
            ("ス", "ズ"),
            ("セ", "ゼ"),
            ("ソ", "ゾ"),
            ("タ", "ダ"),
            ("チ", "ヂ"),
            ("ツ", "ヅ"),
            ("テ", "デ"),
            ("ト", "ド"),
            ("ハ", "バ"),
            ("ヒ", "ビ"),
            ("フ", "ブ"),
            ("ヘ", "ベ"),
            ("ホ", "ボ"),
            ("か", "が"),
            ("き", "ぎ"),
            ("く", "ぐ"),
            ("け", "げ"),
            ("こ", "ご"),
            ("さ", "ざ"),
            ("し", "じ"),
            ("す", "ず"),
            ("せ", "ぜ"),
            ("そ", "ぞ"),
            ("た", "だ"),
            ("ち", "ぢ"),
            ("つ", "づ"),
            ("て", "で"),
            ("と", "ど"),
            ("は", "ば"),
            ("ひ", "び"),
            ("ふ", "ぶ"),
            ("へ", "べ"),
            ("ほ", "ぼ"),
        ];
        for (clean, voiced) in pairs {
            assert_eq!(
                voice_first_kana(clean).as_deref(),
                Some(voiced),
                "{clean}→{voiced}"
            );
        }
        // 2 文字目以降は保持
        assert_eq!(voice_first_kana("カミ").as_deref(), Some("ガミ"));
        assert_eq!(voice_first_kana("ひと").as_deref(), Some("びと")); // 人々→ひとびと
                                                                       // 連濁不可 (母音 / ナマヤラワ / 既濁音 / 半濁音 / 空) は None
        for none in [
            "ア", "ナ", "マ", "ヤ", "ラ", "ワ", "ン", "ガ", "パ", "あ", "な", "ん", "",
        ] {
            assert_eq!(voice_first_kana(none), None, "{none} は連濁不可");
        }
    }

    #[test]
    fn normalize_long_vowel_expands_by_vowel_grade() {
        // ー を直前かなの母音段で展開: ア段→ア / イ段→イ / ウ段→ウ / エ段→イ / オ段→ウ。
        assert_eq!(normalize_long_vowel("カー"), "カア"); // a
        assert_eq!(normalize_long_vowel("キー"), "キイ"); // i
        assert_eq!(normalize_long_vowel("クー"), "クウ"); // u
        assert_eq!(normalize_long_vowel("ケー"), "ケイ"); // e → イ
        assert_eq!(normalize_long_vowel("コー"), "コウ"); // o → ウ
        assert_eq!(normalize_long_vowel("サー"), "サア");
        assert_eq!(normalize_long_vowel("スー"), "スウ");
        assert_eq!(normalize_long_vowel("ソー"), "ソウ");
        assert_eq!(normalize_long_vowel("ネー"), "ネイ");
        assert_eq!(normalize_long_vowel("ガッコー"), "ガッコウ"); // 実例
                                                                  // 先頭の ー は展開しない (i>0 ガード)、 ー 以外は不変
        assert_eq!(normalize_long_vowel("ーカ"), "ーカ");
        assert_eq!(normalize_long_vowel("ネコ"), "ネコ");
    }

    #[test]
    fn zen_to_han_converts_fullwidth() {
        // 全角英数記号 → 半角 (各 arm を個別に固定)。
        assert_eq!(zen_to_han("ＡＢＣ"), "ABC");
        assert_eq!(zen_to_han("ａｂｃ"), "abc");
        assert_eq!(zen_to_han("１２３"), "123");
        // 記号 arm を網羅: － \u{2212} ＋ ～ 〜 ％ ． ， ／
        assert_eq!(zen_to_han("－"), "-");
        assert_eq!(zen_to_han("\u{2212}"), "-");
        assert_eq!(zen_to_han("＋"), "+");
        assert_eq!(zen_to_han("～"), "~");
        assert_eq!(zen_to_han("〜"), "~");
        assert_eq!(zen_to_han("％"), "%");
        assert_eq!(zen_to_han("．"), ".");
        assert_eq!(zen_to_han("，"), ",");
        assert_eq!(zen_to_han("／"), "/");
        assert_eq!(zen_to_han("漢字"), "漢字"); // 非対象は不変
    }
}
