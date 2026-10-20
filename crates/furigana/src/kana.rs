//! ひらがな⇄カタカナ変換、漢字判定、Unicode 正規化ユーティリティ
//!
//! データに依存しない純粋関数のみ。
//! `normalize_text` だけ [`CompatData`](crate::rules::CompatData) を引数に取り、
//! 異体字置換を行う。

use crate::char_class::{self, HIRAGANA_END, HIRAGANA_START, KATAKANA_END, KATAKANA_START};
use once_cell::sync::Lazy;
use regex::Regex;

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
            } else if c == 'ヵ' {
                // ★2026-09-13: カタカナ範囲定数は ン (0x30F3) までなので
                // ヴ / ヵ / ヶ は個別に対応する。 未対応だと romaji 出力に
                // カタカナのまま残っていた (実ログ 88 万行で 9 件)。
                // ★2026-10-18: 小書きの ゕ / ゖ のままだと TTS が読めない (実コメント 300 万行に 265 行。
                // スヶベ / マイヶル のように ケ の代用が大半) ので、 大きい か / け にする。
                'か'
            } else if c == 'ヶ' {
                'け'
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

mod normalize;

pub use normalize::{normalize_text, zen_to_han};
pub(crate) use normalize::{normalize_text_aligned, normalize_text_aligned_with};

#[cfg(test)]
mod tests;
