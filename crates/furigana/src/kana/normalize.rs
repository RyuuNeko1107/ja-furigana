//! 入力の正規化 (全角→半角・NFKC・異体字・不可視文字・上付き下付き) と、 元の位置との対応付け

use crate::rules::CompatData;
use unicode_normalization::UnicodeNormalization;

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

/// 幅ゼロの不可視文字 (ZWSP / ZWNJ / WORD JOINER / BOM / soft hyphen)。 漢字の間に挟まれると辞書の語に当たらなくなる
/// (NG ワード回避のスパムでも使われる) ので、 異体字セレクタと同じく正規化テキストからは除く。 ZWJ は絵文字の連結に使うので除かない
#[inline]
fn is_invisible_char(c: char) -> bool {
    matches!(
        c,
        '\u{200B}' | '\u{200C}' | '\u{2060}' | '\u{FEFF}' | '\u{AD}'
    )
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

/// ローマ数字 1 文字 (Ⅰ〜Ⅻ = U+2160..=U+216B、 ⅰ〜ⅻ = U+2170..=U+217B) の値 (1〜12)
fn roman_numeral_value(ch: char) -> Option<u32> {
    let cp = u32::from(ch);
    match cp {
        0x2160..=0x216B => Some(cp - 0x2160 + 1),
        0x2170..=0x217B => Some(cp - 0x2170 + 1),
        _ => None,
    }
}

/// 丸数字 ①〜⑳ (U+2460..=U+2473)
fn is_circled_number(ch: char) -> bool {
    matches!(ch, '\u{2460}'..='\u{2473}')
}

/// 上付き数字 ⁰¹²³⁴⁵⁶⁷⁸⁹ の値 (累乗の指数)
fn superscript_digit(ch: char) -> Option<char> {
    match ch {
        '\u{B2}' => Some('2'),
        '\u{B3}' => Some('3'),
        '\u{B9}' => Some('1'),
        '\u{2070}' => Some('0'),
        '\u{2074}'..='\u{2079}' => char::from_digit(u32::from(ch) - 0x2070, 10),
        _ => None,
    }
}

/// 下付き数字 ₀〜₉
fn is_subscript_digit(c: char) -> bool {
    matches!(c, '\u{2080}'..='\u{2089}')
}

/// 下付き数字の直前が 「添え字を付ける底」 か: 前に英字が続かない小文字 1 字 (a₁ / x₂)、 または log / ln
fn subscript_base_ok(prefix: &str) -> bool {
    // ∫₀¹ / Σ₁ の範囲
    if prefix.ends_with(['∫', 'Σ']) {
        return true;
    }
    for func in ["log", "ln"] {
        if let Some(rest) = prefix.strip_suffix(func) {
            return !rest
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphabetic());
        }
    }
    let mut it = prefix.chars().rev();
    matches!(it.next(), Some(c) if c.is_ascii_lowercase())
        && !it.next().is_some_and(|c| c.is_ascii_alphabetic())
}

/// 1 文字を NFKC → compat 置換 → NFC した断片を返す。
///
/// [`normalize_text_aligned`] が unit ごとに呼ぶ。 NFKC は **1 文字単位** で適用する
/// (旧 `normalize_text` の whole-string NFKC と、 結合文字列を跨がない実用入力では
/// 一致する)。 compat は variant → canonical の **全体** で置換する (= `廿 → 二十`
/// `卅 → 三十` のような旧字漢数字の複数文字 canonical も完全展開。 旧実装は先頭 1 文字
/// しか採らず `廿 → 二` と十を落としていた)。 1 文字が複数文字に展開されても、
/// `normalize_text_aligned` の alignment は unit 単位 (= 原文 1 文字 ↔ 正規化断片) で
/// 扱うため surface 保持に影響しない (`㍻ → 平成` と同じ扱い)。
/// 結果は `out` へ直接 push する (= 戻り値 `String` を作らない)。
///
/// 旧実装は 1 文字あたり `ch.to_string()` / NFKC collect / `String::with_capacity` /
/// compat lookup 用の `c.to_string()` / NFC collect で **5 回前後の確保**が走っていた。
/// これが [`normalize_text_aligned`] から入力 1 文字ごとに呼ばれるため、 通常の
/// 日本語入力でも文字数 × 5 の確保が解析のたびに乗っていた。
///
/// 通常の文字 (= NFKC で変化せず compat にも無い) は **確保ゼロでそのまま push** する。
/// 単一文字が NFKC で不変なら合成の余地が無いので NFC でも不変 = 旧実装と完全に等価。
/// 変換が要る文字だけ従来どおりの経路に落とす。
fn normalize_char_piece_into(ch: char, compat: &CompatData, out: &mut String) {
    // 顔文字の頬・口 (´ ¨ ¯ ˘) は NFKC で 「空白 + 結合記号」 に分解されて顔文字が崩れるので、 そのまま残す
    if matches!(ch, '\u{B4}' | '\u{A8}' | '\u{AF}' | '\u{2D8}') {
        out.push(ch);
        return;
    }
    // ローマ数字 Ⅰ〜Ⅻ / ⅰ〜ⅻ は NFKC で I / II / V のような英字列になり、 数 (ドラクエⅤ / Ⅱ世 / 第Ⅲ期) として
    // 読めなくなるので、 アラビア数字にして lookup する (表示は原文の surface のまま)
    if let Some(n) = roman_numeral_value(ch) {
        out.push_str(&n.to_string());
        return;
    }
    // compat lookup の key は 1 文字。 スタック上の buffer から &str を作れば確保不要。
    let mut buf = [0u8; 4];
    let ch_str: &str = ch.encode_utf8(&mut buf);

    // fast path: NFKC が 「自分自身 1 文字」 かつ compat 未登録
    let mut nfkc_iter = std::iter::once(ch).nfkc();
    if let Some(first) = nfkc_iter.next() {
        if first == ch && nfkc_iter.next().is_none() && compat.lookup(ch_str).is_none() {
            out.push(ch);
            return;
        }
    }

    // slow path: NFKC → compat 置換 → NFC (挙動は旧実装と同一)
    let nfkc: String = std::iter::once(ch).nfkc().collect();
    let mut replaced = String::with_capacity(nfkc.len());
    for c in nfkc.chars() {
        let mut cbuf = [0u8; 4];
        match compat.lookup(c.encode_utf8(&mut cbuf)) {
            Some(canonical) => replaced.push_str(canonical),
            None => replaced.push(c),
        }
    }
    out.extend(replaced.nfc());
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
/// しない)。 それ以外は 1 文字ごとに [`normalize_char_piece_into`] を適用し、 原文 1 文字 ↔
/// 正規化断片 1 つの unit を作る。
#[must_use]
pub(crate) fn normalize_text_aligned(s: &str, compat: &CompatData) -> NormalizedText {
    normalize_text_aligned_with(s, compat, None)
}

/// 1 文字が 「正規化で何も起きない」 普通の文字か (NFKC で不変・compat 未登録・VS / 不可視文字でない)。
/// 結果は文字ごとに覚える (BMP のみ。 NFKC の判定が 1 文字あたり数十 ns かかるため)。
pub(super) fn is_plain_char(
    c: char,
    compat: &CompatData,
    compat_chars: Option<&crate::fxmap::FxSet<char>>,
) -> bool {
    use std::sync::atomic::{AtomicU8, Ordering::Relaxed};
    static NFKC_SELF: [AtomicU8; 0x1_0000] = [const { AtomicU8::new(0) }; 0x1_0000];
    let nfkc_self = |c: char| {
        let mut it = std::iter::once(c).nfkc();
        it.next() == Some(c) && it.next().is_none()
    };
    let cp = c as u32;
    let identity = if cp < 0x1_0000 {
        let slot = &NFKC_SELF[cp as usize];
        match slot.load(Relaxed) {
            1 => true,
            2 => false,
            _ => {
                let r = nfkc_self(c);
                slot.store(if r { 1 } else { 2 }, Relaxed);
                r
            }
        }
    } else {
        nfkc_self(c)
    };
    identity
        && !is_variation_selector(c)
        && !is_invisible_char(c)
        && if let Some(set) = compat_chars {
            !set.contains(&c)
        } else {
            let mut buf = [0u8; 4];
            compat.lookup(c.encode_utf8(&mut buf)).is_none()
        }
}

/// [`normalize_text_aligned`] の本体。 `compat_chars` は compat の 1 文字 key の集合 (呼び出し側が持っていれば
/// 渡す。 無ければ compat を直接引く)。
pub(crate) fn normalize_text_aligned_with(
    s: &str,
    compat: &CompatData,
    compat_chars: Option<&crate::fxmap::FxSet<char>>,
) -> NormalizedText {
    // 全部が普通の文字なら、 正規化は何もしない (大半の入力)。 原文との対応表も要らない
    // (対応が要るのは changed の時だけ)。
    if s.chars().all(|c| is_plain_char(c, compat, compat_chars)) {
        return NormalizedText {
            text: s.to_string(),
            changed: false,
            units: Vec::new(),
        };
    }
    normalize_text_aligned_slow(s, compat)
}

/// 1 文字ずつ正規化して原文との対応表を作る本来の経路 ([`normalize_text_aligned_with`] の素通りの外)。
pub(super) fn normalize_text_aligned_slow(s: &str, compat: &CompatData) -> NormalizedText {
    let mut text = String::new();
    let mut units: Vec<NormUnit> = Vec::new();
    // 数式 (log₂8=3 / a₁+a₂=b) の下付き数字は NFKC で数になって添え字だと分からなくなるので、 そのまま残す
    // (式の記号がある行の、 小文字 1 字 / log / ln の直後だけ。 H₂O や CO₂ は従来どおり)
    let math_line = s.chars().any(is_subscript_digit)
        && s.contains([
            '=', '＝', '<', '>', '≦', '≧', '≠', '≒', '≈', '≤', '≥', '√', '^', '²', '³',
        ]);
    let mut in_subscript = false;
    for (orig_off, ch) in s.char_indices() {
        let orig_end = orig_off + ch.len_utf8();
        if math_line
            && is_subscript_digit(ch)
            && (in_subscript || subscript_base_ok(&s[..orig_off]))
        {
            let norm_start = text.len();
            text.push(ch);
            units.push(NormUnit {
                norm_start,
                orig: orig_off..orig_end,
            });
            in_subscript = s[orig_end..].chars().next().is_some_and(is_subscript_digit);
            continue;
        }
        in_subscript = false;
        // 半角の濁点・半濁点 (ﾋﾞ → ビ): 1 文字ずつの NFKC だと結合記号が別に残って 「ひ゛」 になるので、 直前の半角カナと合成する
        if matches!(ch, '\u{FF9E}' | '\u{FF9F}')
            && s[..orig_off]
                .chars()
                .next_back()
                .is_some_and(|c| matches!(c, '\u{FF66}'..='\u{FF9D}'))
        {
            let mark = if ch == '\u{FF9E}' {
                '\u{3099}'
            } else {
                '\u{309A}'
            };
            if let Some(last) = text.chars().next_back() {
                let composed: String = [last, mark].iter().collect::<String>().nfc().collect();
                if composed.chars().count() == 1 {
                    text.pop();
                    text.push_str(&composed);
                    if let Some(u) = units.last_mut() {
                        u.orig.end = orig_end;
                    }
                    continue;
                }
            }
        }
        if is_variation_selector(ch) || is_invisible_char(ch) {
            // VS / 不可視文字は norm に出さず、 直前 unit の原文範囲を伸ばして吸収する。
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
        // 丸数字が続く時 (①②③) は 123 に合体して 「ひゃくにじゅうさん」 になるので、 間に読みが空の中黒を入れて 1 つずつ読ませる (空白だと 2 つ目以降の読みの先頭に空白が残る)
        if is_circled_number(ch)
            && s[..orig_off]
                .chars()
                .next_back()
                .is_some_and(is_circled_number)
        {
            text.push('・');
        }
        // 10⁻³ / x⁻¹: 上付きのマイナスと、 それに続く上付き数字は 「^-3」 にして累乗として読む
        if ch == '\u{207B}'
            && s[orig_end..]
                .chars()
                .next()
                .is_some_and(|c| superscript_digit(c).is_some())
            && s[..orig_off]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, ')' | '）'))
        {
            text.push_str("^-");
            units.push(NormUnit {
                norm_start: text.len() - 2,
                orig: orig_off..orig_end,
            });
            continue;
        }
        // 上付き数字 (10² / x³) は 「^2」 にして累乗として読む (直前が数字・英字・閉じ括弧の時だけ。 注記の ¹ は除く)
        if let Some(d) = superscript_digit(ch) {
            // ∫₀¹ の ¹ (下付き数字の直後の上付き数字) は上端: 「^1」
            if math_line
                && s[..orig_off]
                    .chars()
                    .next_back()
                    .is_some_and(is_subscript_digit)
            {
                let norm_start = text.len();
                text.push('^');
                text.push(d);
                units.push(NormUnit {
                    norm_start,
                    orig: orig_off..orig_end,
                });
                continue;
            }
            // ⁻ や上付き数字の直後の上付き数字 (10⁻³ の ³ / x¹⁰ の ⁰) は、 数字だけ続ける
            if s[..orig_off]
                .chars()
                .next_back()
                .is_some_and(|c| c == '\u{207B}' || superscript_digit(c).is_some())
            {
                let norm_start = text.len();
                text.push(d);
                units.push(NormUnit {
                    norm_start,
                    orig: orig_off..orig_end,
                });
                continue;
            }
            if s[..orig_off]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, ')' | '）'))
            {
                text.push('^');
                text.push(d);
                units.push(NormUnit {
                    norm_start,
                    orig: orig_off..orig_end,
                });
                continue;
            }
            // 身長² / 半径³: 漢字・カタカナの直後の ² ³ は 「の2乗」 「の3乗」
            if matches!(ch, '\u{B2}' | '\u{B3}')
                && s[..orig_off].chars().next_back().is_some_and(|c| {
                    (('\u{4E00}'..='\u{9FFF}').contains(&c) && !matches!(c, '注' | '註'))
                        || ('ァ'..='ヶ').contains(&c)
                })
            {
                text.push('の');
                text.push(d);
                text.push('乗');
                units.push(NormUnit {
                    norm_start,
                    orig: orig_off..orig_end,
                });
                continue;
            }
            // 底が切れた ² ³ (「²-5x+b=0」 = x² の x が無い式) は、 後ろが演算子なら指数として残す (数 2 にしない)
            if matches!(ch, '\u{B2}' | '\u{B3}')
                && s[orig_end..].chars().next().is_some_and(|c| {
                    matches!(
                        c,
                        '+' | '-'
                            | '='
                            | '*'
                            | '/'
                            | ')'
                            | '×'
                            | '÷'
                            | '\u{2212}'
                            | '＋'
                            | '－'
                            | '＝'
                    )
                })
            {
                text.push('^');
                text.push(d);
                units.push(NormUnit {
                    norm_start,
                    orig: orig_off..orig_end,
                });
                continue;
            }
        }
        normalize_char_piece_into(ch, compat, &mut text);
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
