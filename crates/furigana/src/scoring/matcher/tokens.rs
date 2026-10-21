//! 文字種境界で切る 「pseudo-token」 の走査 (前後の token を MatchContext に渡すための軽量 helper)。

use crate::char_class::{classify_char, CharType};

/// pseudo-token の走査上限 (文字数)。
///
/// 同一 char-class が延々続く入力 (例: 同じ漢字 ×10000) で `next_logical_token` /
/// `prev_logical_token` が run 全体を走査すると、 per-position 呼び出しと合わせて
/// **O(N²)** になり request amplification DoS の梃子になる。 走査をこの長さで
/// 打ち切って線形化する。
///
/// 正しさ: dict の match condition 値は [`crate::sanitize::MAX_DICT_VALUE_CHARS`] 以下
/// (sanitize で保証)。 これを **超える** 長さの token は如何なる `*_eq` / `*_eq_any`
/// とも一致せず、 `*_starts_any` は先頭から最大 condition 長、 `*_char_type` は
/// 境界 1 文字しか見ない。 よって上限 = `MAX_DICT_VALUE_CHARS + 1` で打ち切れば
/// 「condition より確実に長い」 ことが判別でき、 match 結果は一切変わらない。
pub(super) const MAX_LOGICAL_TOKEN_CHARS: usize = crate::sanitize::MAX_DICT_VALUE_CHARS + 1;

/// 文字種が連続する範囲を 「pseudo-token」 として切り出す helper。
///
/// Smart engine の `DictBridgeProvider` / `KanjiProvider` が path 構築中に
/// MatchContext を build するために使う。 Lindera token segmentation を使わず
/// **文字種境界** (= 漢字 / ひらがな / カタカナ / 英数 / 記号) で切る軽量実装。
///
/// 走査は [`MAX_LOGICAL_TOKEN_CHARS`] 文字で打ち切る (O(N²) 回避、 match 結果不変)。
///
/// 日本語の助詞 / 助動詞 (= ひらがな連続)、 漢字熟語 (= 漢字連続)、 数字
/// (= 英数連続) は概ね同 char_type で連続するため、 token-level segmentation
/// と概ね一致する。 完全一致が要る用途 (= POS-aware tokenization) には不適。
///
/// # 例
///
/// 「上手から登場」 の pos 6 (= 「上手」 の直後) から `next_logical_token` →
/// `"から"` (= ひらがな連続、 「登」 漢字で切れる)。
#[must_use]
pub fn next_logical_token(input: &str, start: usize) -> &str {
    let tail = &input[start..];
    let mut first_class: Option<CharType> = None;
    let mut end = start;
    for (n_chars, (idx, c)) in tail.char_indices().enumerate() {
        if n_chars >= MAX_LOGICAL_TOKEN_CHARS {
            break; // 異常に長い run を打ち切り (O(N²) 回避、 match 結果は不変)
        }
        let class = classify_char(c);
        match (first_class, class) {
            (None, Some(cls)) => {
                first_class = Some(cls);
                end = start + c.len_utf8(); // 最初の 1 字 (idx は常に 0)
            }
            (Some(fc), Some(cls)) if fc == cls => {
                end = start + idx + c.len_utf8();
            }
            _ => break,
        }
    }
    &input[start..end]
}

/// 「pseudo-token」 の **更にその次** を切り出す (= next_logical_token を 2 回適用)。
///
/// 「人気が無い」 で pos 0 (= 「人気」 後の文脈) → next = 「が」、 next2 = 「無い」 のような
/// 1 飛ばし lookup 用。 next_logical_token と同じ char-class 境界 logic。
#[must_use]
pub fn next2_logical_token(input: &str, start: usize) -> &str {
    let next1 = next_logical_token(input, start);
    let next1_end = start + next1.len();
    next_logical_token(input, next1_end)
}

/// 直前の pseudo-token を切り出す (= byte 位置 `end` の手前から **文字種が連続する範囲**)。
///
/// 「東方の上手」 の pos 9 (= 「上手」 の直前) から `prev_logical_token` → `"の"`
/// (= ひらがな 1 文字、 「方」 漢字で切れる)。 行末 (= end が input.len()) でも動作。
#[must_use]
pub fn prev_logical_token(input: &str, end: usize) -> &str {
    // 英数字の直後の空白 (「Python 版」 「32bit 版」 「1000 分の」) は読み飛ばして英数字の run を前文脈にする。
    // 漢字 / かなの後の空白は句の区切りなので飛ばさない (「全部 白」 を 漢字 + 白 と見ると音読み規則が誤爆する)
    let trimmed = input[..end].trim_end_matches([' ', '\u{3000}']);
    // trimmed.len() <= end なので、 末尾に空白が無ければ end のまま (= 代入しても同じ)
    let end = if trimmed
        .chars()
        .next_back()
        .is_some_and(|c| classify_char(c) == Some(CharType::Alphanumeric))
    {
        trimmed.len()
    } else {
        end
    };
    let head = &input[..end];
    let mut last_class: Option<CharType> = None;
    let mut start = end;
    for (n_chars, (idx, c)) in head.char_indices().rev().enumerate() {
        if n_chars >= MAX_LOGICAL_TOKEN_CHARS {
            break; // 異常に長い run を打ち切り (O(N²) 回避、 match 結果は不変)
        }
        let class = classify_char(c);
        match (last_class, class) {
            (None, Some(cls)) => {
                last_class = Some(cls);
                start = idx;
            }
            (Some(lc), Some(cls)) if lc == cls => {
                start = idx;
            }
            _ => break,
        }
    }
    &input[start..end]
}
