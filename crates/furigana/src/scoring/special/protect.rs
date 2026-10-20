//! 保護トークン (URL / Email / 絵文字 / 顔文字 / マークアップ) の抽出と [`ProtectTokenProvider`]

use crate::char_class;
use crate::scoring::candidate::{
    CandidateProvider, RawCandidate, Score, ScoringContext, BAND_PROTECTED,
};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

// ─── 保護トークン定義 ────────────────────────────────────────────────────────

/// 保護対象 token の種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProtectedKind {
    /// URL (http / https / ftp / file / IP / domain.tld)
    Url,
    /// Email アドレス
    Email,
    /// 絵文字 (Unicode emoji range)
    Emoji,
    /// 括弧で囲んだ顔文字 ((´・ω・`) / (;´Д`) / ヽ(^o^)ノ)。 読みは空 (丸ごと除外)
    Kaomoji,
    /// HTML 文字参照 (`&#8230;` / `&#x2026;` / `&amp;` / `&lt;`)。 bot の投稿や RSS 由来で混ざる。
    /// 数字の羅列として読まれるのを避けるため読みは空 (`&` 単体も読まないので揃う)
    Entity,
    /// アスキーアート (`/:::::{:::/ }:}` / `|---|---|---|`)。 記号の連なりで、 文字・数字を含まない。 読みは空
    /// (「すらっしゅ」 などの連呼になるので丸ごと除外)
    AsciiArt,
    /// マークアップの記法部分 (HTML 風タグ `<center>` / `<small>` / `</i>`、 Misskey の MFM `$[x2 ` と対応する `]`)。
    /// 中身の文章は残し、 記法だけを読まない (読みは空)
    Markup,
}

/// 入力中の保護対象 1 件 (range + 種別)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectedToken {
    /// 入力 byte range
    pub range: Range<usize>,
    /// 種別
    pub kind: ProtectedKind,
}

// ─── regex (URL / Email) ─────────────────────────────────────────────────────

static ENTITY_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"&(?:#[0-9]{1,7}|#[xX][0-9a-fA-F]{1,6}|amp|lt|gt|quot|apos|nbsp);")
        .expect("ENTITY_RE compile")
});

/// HTML 風のタグ (既知のタグ名だけ。 `<Team>` のような山括弧の使い方は対象外)。 開き・閉じ・自己閉じ
static MARKUP_TAG_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)</?(?:center|plain|small|tiny|big|i|b|s|u|sub|sup|br|li|ul|ol|a|mark|strike|em|strong|p|div|span)(?:\s[^<>]*)?/?>",
    )
    .expect("MARKUP_TAG_RE compile")
});

static URL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?xi)(?:(?:https?://|ftp://|file://|www\.)[^\s<>"'\(\)\{\}\[\]]+|(?:[A-Za-z0-9\-]+\.)+[A-Za-z]{2,}(?::\d+)?(?:/[^\s<>"'\(\)\{\}\[\]]*)?|\d{1,3}(?:\.\d{1,3}){3}(?::\d+)?(?:/[^\s<>"'\(\)\{\}\[\]]*)?)"#,
    )
    .expect("scoring URL regex build failed")
});

static EMAIL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}")
        .expect("scoring email regex build failed")
});

/// 括弧 1 組 (前後に腕 ヽ ＼ ノ ／ や、 走る顔文字の ε=ε= ┌ ┘ が付くことがある。 中に空白があってもよい `( ^ω^)`)。 中身が顔かどうかは [`is_kaomoji_inner`] で見る
static KAOMOJI_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?:ε[=＝])*[ヽヾ＼\\┌┏└┗╭]?[(（]([^()（）\n]{1,14})[)）][ノﾉ／/v┘┛┐┓╮]?")
        .expect("scoring kaomoji regex build failed")
});

/// 括弧のない顔文字 (`^^` / `^_^` / `^o^` / `T_T` / `;_;` / `>_<` / `orz` / `XD` / `:)`)、 と閉じ括弧のない汗の顔 (`(^^;` / `(;^_^A`)。
/// 猫の腕 `∩^ω^∩` / `∩´ω`∩`、 `=ω=` / `=^ω^=` のように両側を記号で挟んだ顔も取る (腕ごと 1 つにしないと 腕の手前で経路が切れて ω だけ読まれる)。
/// 英数字との境目の判定は [`bare_kaomoji_boundary_ok`] で行う (Rust の regex は先読み・後読みが使えない)
static BARE_KAOMOJI_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"∩?[(（]?[;；]?[\^＾][_\-.oO・‿ω]?[\^＾][;；ゞ]?A?[)）]?∩?|∩[´`'・゜ﾟ°][ωДд∀▽][´`'・゜ﾟ°]∩|[=＝][´`'・゜ﾟ°\^＾]?[ωДд∀▽][´`'・゜ﾟ°\^＾]?[=＝]|[TＴ;；][_\-ー^ωд][TＴ;；]|[>＞][_ωД][<＜]|T{2,}|(?i:orz)|(?i:xd)|[:：][-']?[)(DPp]",
    )
    .expect("scoring bare kaomoji regex build failed")
});

/// 括弧のない顔文字の前後の境目。 `^` 始まり (目が閉じた顔) は直後が数字なら累乗 (2^3^4) なので除く。
/// 英字・`:` 始まりは、 前後が英数字なら語の一部 (Corzo / box:D) なので除く
fn bare_kaomoji_boundary_ok(input: &str, range: &Range<usize>) -> bool {
    let m = &input[range.clone()];
    let prev = input[..range.start].chars().next_back();
    let next = input[range.end..].chars().next();
    let first = m.chars().next().unwrap_or(' ');
    let word = |c: char| c.is_ascii_alphanumeric();
    if matches!(first, '^' | '＾' | '(' | '（' | ';' | '；') && m.contains(['^', '＾']) {
        return !next.is_some_and(|c| c.is_ascii_digit())
            && !prev.is_some_and(|c| c.is_ascii_digit());
    }
    // T-Tシャツ (Tシャツの T を並べた語) は顔ではない
    if input[range.end..].starts_with("シャツ") {
        return false;
    }
    !prev.is_some_and(|c| word(c) || c == ':') && !next.is_some_and(word)
}

/// 顔文字の中身か。 かな・漢字を含まず、 顔のパーツ (目・口・頬) を含み、 英数字は 2 つまで。
/// 点 (`・`) や記号だけのものは、 英数字が無い時だけ顔とみなす ((A・B) は顔ではない)
fn is_kaomoji_inner(inner: &str) -> bool {
    // 引用符で挟んだ目 ( '-' ) / ( '.' )
    let t: Vec<char> = inner.trim().chars().collect();
    if t.len() == 3
        && matches!(t[0], '\'' | '`' | '´' | '′')
        && matches!(t[2], '\'' | '`' | '´' | '′')
        && matches!(t[1], '-' | '.' | '_' | 'ー' | '・')
    {
        return true;
    }
    // 括弧のない顔文字の形そのもの ((T-T) / (;_;) / (XD))
    if BARE_KAOMOJI_RE
        .find(inner.trim())
        .is_some_and(|m| m.start() == 0 && m.end() == inner.trim().len())
    {
        return true;
    }
    let mut strong = false;
    let mut weak = false;
    let mut alnum = 0;
    for c in inner.chars() {
        let kana_or_kanji = crate::char_class::is_kanji_char(c)
            || crate::char_class::is_hiragana_char(c)
            || (crate::char_class::is_katakana_loose_char(c)
                && !matches!(c, 'ロ' | 'ー' | 'ノ' | 'ツ' | 'ン'));
        if kana_or_kanji {
            return false;
        }
        if c.is_ascii_alphanumeric() {
            alnum += 1;
        }
        if matches!(
            c,
            '´' | '｀'
                | '`'
                | 'ω'
                | '∀'
                | 'Д'
                | 'д'
                | '°'
                | '▽'
                | '∇'
                | 'ﾟ'
                | '゜'
                | '≧'
                | '≦'
                | '◕'
                | '＾'
                | '^'
                | '_'
                | '＿'
                | ';'
                | '；'
                | '‿'
                | '⌒'
                | 'ᴗ'
                | '˘'
                | '′'
                | 'ᐢ'
        ) {
            strong = true;
        }
        if matches!(c, '・' | '･' | 'ｰ' | '＞' | '＜') {
            weak = true;
        }
    }
    alnum <= 2 && (strong || (weak && alnum == 0))
}

// ─── 絵文字判定 (char-based) ─────────────────────────────────────────────────

// 絵文字 range 判定は crate::char_class に集約。 既存 caller 向けに従来 path を維持。
pub use crate::char_class::is_emoji_char;

// ─── 抽出 logic ──────────────────────────────────────────────────────────────

/// Misskey の MFM (`$[x2 文章]` / `$[font.serif $[x2 **文章**]]`) の記法部分の range を返す。
/// 開き `$[名前 ` (名前は英数字と `. , = -` と `_`) と、 それに対応する閉じ `]` (入れ子と `[` `]` の対を数える)。
/// 閉じが見つからない時は開きだけを返す。 中身の文章は含めない
fn mfm_ranges(input: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = input[from..].find("$[") {
        let open = from + rel;
        let name_start = open + 2;
        let name_len = input[name_start..]
            .bytes()
            .take_while(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'.' | b',' | b'=' | b'-' | b'_')
            })
            .count();
        let after_name = name_start + name_len;
        if name_len == 0 || bytes.get(after_name) != Some(&b' ') {
            from = name_start;
            continue;
        }
        let open_end = after_name + 1;
        out.push(open..open_end);
        // 対応する閉じ `]` (深さ 1 から数える)
        let mut depth = 1usize;
        for (i, b) in input[open_end..].bytes().enumerate() {
            match b {
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        out.push(open_end + i..open_end + i + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        from = open_end;
    }
    out
}

/// 記法の直前に隣り合っていたら、 記法と同じ保護範囲に含める記号か (ASCII の記号・括弧類)。
/// 形態素解析が記号の連なり (`...</` / `)</` / `「<`) を 1 語にまとめて、 その途中の記法へ経路が届かなくなるのを防ぐ。
/// `。` `、` は読み上げの間になるので含めない
fn is_markup_adjacent_symbol(c: char) -> bool {
    c.is_ascii_punctuation()
        || matches!(
            c,
            '「' | '」'
                | '『'
                | '』'
                | '（'
                | '）'
                | '【'
                | '】'
                | '〈'
                | '〉'
                | '《'
                | '》'
                | '｢'
                | '｣'
        )
}

/// アスキーアートの部品になる文字か (ASCII の記号・罫線ブロック・全角の線)。 文字・数字・仮名・漢字は含まない
fn is_ascii_art_char(c: char) -> bool {
    matches!(
        c,
        '|' | '\\'
            | '\''
            | '`'
            | '-'
            | '='
            | '^'
            | '('
            | ')'
            | '['
            | ']'
            | '{'
            | '}'
            | '<'
            | '>'
            | ';'
            | ','
            | '/'
            | '*'
            | '_'
            | ':'
            | '"'
            | '.'
            | '!'
            | '?'
            | '~'
            | '\u{FF0F}'
            | '\u{FF3C}'
            | '\u{FFE3}'
            | '\u{FF3F}'
    ) || ('\u{2500}'..='\u{259F}').contains(&c)
}

/// 強調・間の記号 (アスキーアートの種類には数えない): `. ! ? ~ ,`
fn is_ascii_art_filler(c: char) -> bool {
    matches!(c, '.' | '!' | '?' | '~' | ',')
}

/// 入力中のアスキーアートらしい記号の連なりの byte range を列挙する。
///
/// 記号 (空白をはさんでよい) だけが並び、 空白を除いて 6 文字以上で、 強調記号 (`. ! ? ~ ,`) を除いた記号の種類が
/// 3 以上 (2 種類なら 12 文字以上)。 `!!!!!` / `......` / `?!?!?!` のような同じ記号・強調の連続は対象外。
pub(super) fn ascii_art_runs(input: &str) -> Vec<Range<usize>> {
    fn judge(chars: &[char]) -> bool {
        let mut kinds: Vec<char> = chars
            .iter()
            .copied()
            .filter(|&c| !is_ascii_art_filler(c))
            .collect();
        kinds.sort_unstable();
        kinds.dedup();
        chars.len() >= 6 && (kinds.len() >= 3 || (kinds.len() >= 2 && chars.len() >= 12))
    }
    // 線に見える 1 文字の英字 (l i j r ...) が記号にはさまれていれば、 連なりを途切れさせない (数え方には入れない)
    let is_bridge = |c: char| {
        matches!(
            c,
            'l' | 'i' | 'j' | 'r' | 'f' | 't' | 'v' | 'V' | 'T' | 'Y' | 'L' | 'I' | '1'
        )
    };
    let all: Vec<(usize, char)> = input.char_indices().collect();
    let mut runs = Vec::new();
    let mut start: Option<usize> = None;
    let mut last_end = 0usize;
    let mut art: Vec<char> = Vec::new();
    for (k, &(idx, c)) in all.iter().enumerate() {
        let next = all.get(k + 1).map(|&(_, n)| n);
        let prev_is_art = k > 0 && (is_ascii_art_char(all[k - 1].1) || all[k - 1].1 == ' ');
        let next_is_art = next.is_some_and(|n| is_ascii_art_char(n) || n == ' ');
        if is_ascii_art_char(c) {
            if start.is_none() {
                start = Some(idx);
            }
            last_end = idx + c.len_utf8();
            art.push(c);
        } else if c == ' ' && start.is_some() {
            // 空白は記号のあいだだけ許す (run の末尾には含めない: last_end は更新しない)
        } else if is_bridge(c) && start.is_some() && prev_is_art && next_is_art {
            last_end = idx + c.len_utf8();
        } else {
            if let Some(s) = start.take() {
                if judge(&art) {
                    runs.push(s..last_end);
                }
            }
            art.clear();
        }
    }
    if let Some(s) = start {
        if judge(&art) {
            runs.push(s..last_end);
        }
    }
    runs
}

/// 入力から保護トークン (URL / Email / 絵文字) を全列挙して返す。
///
/// 重なる token は **start 位置順で先行を優先** (= 先頭 URL がカバーする range 内に
/// email や絵文字があっても URL を優先)。 scoring engine の DP 前段で呼ぶ用途。
#[must_use]
pub fn extract_protected_tokens(input: &str) -> Vec<ProtectedToken> {
    let mut tokens: Vec<ProtectedToken> = Vec::new();

    // 各 regex が必ず要求する字が無い入力 (大半の行) は、 regex を走らせずに飛ばす
    // URL は `.` か `://` を、 メールは `@` を、 文字参照は `&` を、 括弧つき顔文字は `(` `（` を要求する。

    // URL
    if input.contains('.') || input.contains("://") {
        for m in URL_RE.find_iter(input) {
            tokens.push(ProtectedToken {
                range: m.range(),
                kind: ProtectedKind::Url,
            });
        }
    }

    // Email
    if input.contains('@') {
        for m in EMAIL_RE.find_iter(input) {
            tokens.push(ProtectedToken {
                range: m.range(),
                kind: ProtectedKind::Email,
            });
        }
    }

    // HTML 文字参照。 URL の中 (`?a=1&amp;b=2`) のものは URL が取る
    let entity_matches = if input.contains('&') {
        ENTITY_RE.find_iter(input).collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    for m in entity_matches {
        let range = m.range();
        if tokens
            .iter()
            .any(|t| t.range.start < range.end && range.start < t.range.end)
        {
            continue;
        }
        tokens.push(ProtectedToken {
            range,
            kind: ProtectedKind::Entity,
        });
    }

    // マークアップの記法 (HTML 風タグ / Misskey の MFM)。 URL・メール・文字参照と重なるものは取らない
    let mut markup: Vec<Range<usize>> = MARKUP_TAG_RE.find_iter(input).map(|m| m.range()).collect();
    markup.extend(mfm_ranges(input));
    let all_markup = markup.clone();
    for mut range in markup {
        // 記法の直前に隣り合う記号 (ASCII の記号・括弧類。 `。` `、` は間なので除く) も同じ範囲に含める。 形態素解析が `**</` や `]</` を
        // 1 語にまとめて、 その途中の `<` / `$` の位置へ経路が届かず、 記法の候補が評価されなくなるため。
        // 閉じ `]` 自身は対象外 (`]]` が互いに重なるのを避ける)
        if &input[range.clone()] != "]" {
            while let Some(c) = input[..range.start].chars().next_back() {
                let at = range.start - c.len_utf8();
                // 隣の記法 (`</i><b>` の `>`) には食い込まない
                let in_other = all_markup.iter().any(|r| r.start <= at && at < r.end);
                if is_markup_adjacent_symbol(c) && !in_other {
                    range.start -= c.len_utf8();
                } else {
                    break;
                }
            }
        }
        // 記法の中にすっぽり入る URL 風の語 (`$[font.serif ` の font.serif) は誤検出なので記法を優先する
        tokens.retain(|t| {
            !(t.kind == ProtectedKind::Url
                && range.start <= t.range.start
                && t.range.end <= range.end)
        });
        if tokens
            .iter()
            .any(|t| t.range.start < range.end && range.start < t.range.end)
        {
            continue;
        }
        tokens.push(ProtectedToken {
            range,
            kind: ProtectedKind::Markup,
        });
    }

    // 顔文字
    if input.contains(['(', '（']) {
        for caps in KAOMOJI_RE.captures_iter(input) {
            if is_kaomoji_inner(&caps[1]) {
                let m = caps.get(0).unwrap();
                tokens.push(ProtectedToken {
                    range: m.range(),
                    kind: ProtectedKind::Kaomoji,
                });
            }
        }
    }

    // 括弧のない顔文字。 括弧つきの顔文字や URL・メールと重なるものは取らない
    for m in BARE_KAOMOJI_RE.find_iter(input) {
        let range = m.range();
        if tokens
            .iter()
            .any(|t| t.range.start < range.end && range.start < t.range.end)
        {
            continue;
        }
        if bare_kaomoji_boundary_ok(input, &range) {
            tokens.push(ProtectedToken {
                range,
                kind: ProtectedKind::Kaomoji,
            });
        }
    }

    // アスキーアート。 URL・メール・顔文字などと重なるものは取らない
    for range in ascii_art_runs(input) {
        if tokens
            .iter()
            .any(|t| t.range.start < range.end && range.start < t.range.end)
        {
            continue;
        }
        tokens.push(ProtectedToken {
            range,
            kind: ProtectedKind::AsciiArt,
        });
    }

    // 絵文字 (連続範囲を 1 つの token に集約)。 VS16 / 肌色 / ZWJ / keycap (1️⃣) / 国旗も同じ token に含める
    let mut current_emoji_start: Option<usize> = None;
    let mut current_emoji_end: usize = 0;
    // ASCII だけの入力に絵文字は無い (keycap の 1️⃣ も ASCII 以外の続きを要る)
    let scan_emoji = !input.is_ascii();
    for (idx, c) in input.char_indices().filter(|_| scan_emoji) {
        let char_end = idx + c.len_utf8();
        let keycap_base = matches!(c, '0'..='9' | '#' | '*') && {
            let mut next = input[char_end..].chars();
            match next.next() {
                Some('\u{20E3}') => true,
                Some('\u{FE0F}') => next.next() == Some('\u{20E3}'),
                _ => false,
            }
        };
        let joiner_in_run = current_emoji_start.is_some() && crate::char_class::is_emoji_joiner(c);
        if is_emoji_char(c) || keycap_base || joiner_in_run {
            if current_emoji_start.is_none() {
                current_emoji_start = Some(idx);
            }
            current_emoji_end = char_end;
        } else if let Some(start) = current_emoji_start.take() {
            tokens.push(ProtectedToken {
                range: start..current_emoji_end,
                kind: ProtectedKind::Emoji,
            });
        }
    }
    if let Some(start) = current_emoji_start {
        tokens.push(ProtectedToken {
            range: start..current_emoji_end,
            kind: ProtectedKind::Emoji,
        });
    }

    // 重複排除: start 位置順 + range 包含チェック
    tokens.sort_by_key(|t| (t.range.start, std::cmp::Reverse(t.range.end)));
    let mut filtered: Vec<ProtectedToken> = Vec::new();
    for t in tokens {
        // 既出 token に完全包含されていれば skip
        let contained = filtered
            .iter()
            .any(|prev| prev.range.start <= t.range.start && t.range.end <= prev.range.end);
        if !contained {
            filtered.push(t);
        }
    }
    filtered.sort_by_key(|t| t.range.start);
    filtered
}

// ─── 絵文字の読み ───────────────────────────────────────────────────────────

/// 絵文字の連なりを 1 まとまり (cluster) ごとに分ける。 1 つの cluster は
/// 基本の絵文字 + (VS16 / 肌色 / keycap / タグ) と、 ZWJ で繋いだ続きの絵文字、 国旗 (地域指示記号 2 つ)。
pub(super) fn emoji_clusters(run: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = run.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let start = chars[i].0;
        let mut j = i + 1;
        if char_class::is_regional_indicator(chars[i].1)
            && chars
                .get(j)
                .is_some_and(|&(_, c)| char_class::is_regional_indicator(c))
        {
            j += 1;
        }
        loop {
            while chars
                .get(j)
                .is_some_and(|&(_, c)| char_class::is_emoji_joiner(c) && c != '\u{200D}')
            {
                j += 1;
            }
            if chars.get(j).is_some_and(|&(_, c)| c == '\u{200D}') {
                j += 1;
                if chars.get(j).is_some() {
                    j += 1;
                    continue;
                }
            }
            break;
        }
        let end = chars.get(j).map_or(run.len(), |&(idx, _)| idx);
        out.push(&run[start..end]);
        i = j;
    }
    out
}

/// 絵文字の連なりの読み。 cluster ごとに 完全一致 → VS16 と肌色を除いた形 の順で表を引く。
///
/// 表の値は `読み|別表記|別表記…` (別表記は省略可)。
/// - 読みが空 (`""`) の絵文字は常に 「読まない」
/// - 先頭を `!` にすると、 前後を問わず常に読む (`"!めざましどけい"`)
/// - 読みがある絵文字 (名詞: ⏰ = めざましどけい) は、 文の中で名詞として使われている時だけ読む。 見出し・文末・語の後ろに
///   添える装飾 (`アイス🍨楽しみー` / `🌸本日の配信🌸`) まで読むと 「アイスアイス」 になるため、 次の全部を満たす時:
///   1. 直後が **格助詞** (を が は の に で と も へ や)、 または **語が続く** (かな・漢字・カタカナ: `🐯さんが` / `⚾見に行く`)。
///      語が続く場合は、 行頭・空白・開き括弧の直後ではなく (見出しの装飾)、 直前が文末の形 (ます・です・た・よ・ね…) でもない
///   2. 直前が名詞 (漢字・カタカナ・英数字・絵文字) ではない (`ラーメン🍜が` / `猫🐱が` はその語の装飾)。 綴りの揺れに頼らない
///   3. 直前・直後の語が、 同じ読み・別表記ではない (`ねこ🐱` / `🦀蟹座`。 別表記は `|` の後ろ)
///   4. 行末に同じ絵文字がもう一度付いていない (`🐸…🐸` の挟み装飾)
/// - 載っていない cluster は表記のまま。 表が空・1 つも読みに変わらなかった時は `None` (= 全体を表記のまま)
pub(super) fn emoji_reading(
    run: &str,
    table: &HashMap<String, crate::rules::emoji::EmojiEntry>,
    prev_text: &str,
    after_text: &str,
) -> Option<String> {
    if table.is_empty() {
        return None;
    }
    // 文脈で変わる読み (match の input_contains_any) に使う文全体
    let input = format!("{prev_text}{run}{after_text}");
    let next = after_text.chars().next();
    let particle_follows = next.is_some_and(|c| "をがはのにでともへや".contains(c));
    // 語が続く (かな・漢字・カタカナ)。 数字・英字・記号・空白・文末は装飾とみなす
    let word_follows = next.is_some_and(|c| {
        char_class::is_kanji_char(c)
            || char_class::is_hiragana_char(c)
            || (char_class::is_katakana_loose_char(c) && c != 'ー')
    });
    let prev_char = prev_text.chars().next_back();
    // 行頭・空白・開き括弧の直後 = 見出しの装飾
    let prev_at_boundary =
        prev_char.is_none_or(|c| c.is_whitespace() || "【「『（(［[<＜・、。!！?？…".contains(c));
    // 直前が名詞 (漢字・カタカナ・英数字・絵文字) なら、 その語に添えた装飾
    let prev_is_noun = prev_char.is_some_and(|c| {
        char_class::is_kanji_char(c)
            || (char_class::is_katakana_loose_char(c) && c != 'ー')
            || (c.is_alphanumeric() && !char_class::is_hiragana_char(c))
            || is_emoji_char(c)
    });
    // 直前が文末の形なら、 次の文との区切りに置いた装飾
    let prev_is_sentence_end = [
        "ます",
        "です",
        "した",
        "ない",
        "ました",
        "でした",
        "ください",
        "よ",
        "ね",
        "わ",
        "ぞ",
        "ぜ",
        "か",
        "な",
        "さ",
        "い",
        "う",
        "た",
        "だ",
        "る",
        "。",
        "!",
        "！",
        "?",
        "？",
        "…",
        "w",
        "ｗ",
    ]
    .iter()
    .any(|e| prev_text.ends_with(e));
    let prev_hira = crate::kana::kata_to_hira(prev_text);
    let after_hira = crate::kana::kata_to_hira(after_text);
    let tail = after_text.trim_end();
    let clusters = emoji_clusters(run);
    let last = clusters.len().saturating_sub(1);
    let mut any = false;
    let mut out = String::new();
    for (i, cluster) in clusters.into_iter().enumerate() {
        let hit = table
            .get(cluster)
            .or_else(|| table.get(&crate::rules::emoji::strip_variation(cluster)));
        let Some(entry) = hit else {
            out.push_str(cluster);
            continue;
        };
        // 文脈で変わる読み (上から見て最初に当たったもの。 空なら読まない)。 連なりの途中の絵文字は前後が絵文字なので文脈を持たない
        let cprev = if i == 0 { prev_text } else { "\u{E000}" };
        let cafter = if i == last { after_text } else { "\u{E000}" };
        if let Some(r) = entry.matched(cprev, cafter, &input) {
            any = true;
            out.push_str(r.strip_prefix('!').unwrap_or(r));
            continue;
        }
        // どの match にも当たらず既定の読みも無い絵文字は、 辞書に載っていないのと同じ (表記のまま)
        let Some(value) = entry.base() else {
            out.push_str(cluster);
            continue;
        };
        if value.is_empty() {
            any = true;
            continue;
        }
        if let Some(always) = value.strip_prefix('!') {
            any = true;
            out.push_str(always.split('|').next().unwrap_or(""));
            continue;
        }
        let mut parts = value.split('|');
        let reading = parts.next().unwrap_or("");
        let words: Vec<String> = std::iter::once(reading)
            .chain(parts)
            // かな 1 文字の読み・別表記 (き / は / め) は、 前後の別の語の頭や末尾と偶然一致するので重複判定に使わない
            .filter(|w| {
                !w.is_empty()
                    && !(w.chars().count() == 1 && w.chars().all(char_class::is_hiragana_char))
            })
            .map(crate::kana::kata_to_hira)
            .collect();
        // 連なりの先頭は直前の語、 末尾は直後の語と同じなら装飾 (ねこ🐱 / 🦀蟹座)
        let duplicates_neighbour = (i == 0
            && words.iter().any(|w| prev_hira.ends_with(w.as_str())))
            || (i == last && words.iter().any(|w| after_hira.starts_with(w.as_str())));
        // 行末に同じ絵文字がもう一度ある挟み装飾 (🐸…🐸)
        let paired = i == last && !tail.is_empty() && tail.ends_with(cluster);
        let in_sentence =
            particle_follows || (word_follows && !prev_at_boundary && !prev_is_sentence_end);
        if i == last && in_sentence && !prev_is_noun && !duplicates_neighbour && !paired {
            any = true;
            out.push_str(reading);
        } else {
            out.push_str(cluster);
        }
    }
    any.then_some(out)
}

// ─── ProtectTokenProvider ────────────────────────────────────────────────────

/// 保護 token を candidate として供給する [`CandidateProvider`] 実装。
///
/// 構築時に input を 1 度 scan して保護 token を pre-compute、 `candidates_at(pos)` で
/// 各位置からの候補を返す。 candidate の score は band [`BAND_PROTECTED`] (= 2000、
/// 全 band を上回る) で、 path 選択で必ず採用される。
///
/// reading = surface で透過 (= URL や 絵文字を ひらがな化しない)。
#[derive(Debug, Clone)]
pub struct ProtectTokenProvider {
    tokens: Vec<ProtectedToken>,
    /// 絵文字の読み (emoji.toml)。 空なら絵文字は表記のまま通す
    emoji: Arc<HashMap<String, crate::rules::emoji::EmojiEntry>>,
}

impl ProtectTokenProvider {
    /// 入力 string を scan して保護 token を抽出、 provider を構築する (絵文字の読み表なし。 テスト用)。
    #[cfg(test)]
    #[must_use]
    pub fn new(input: &str) -> Self {
        Self::with_emoji(input, Arc::new(HashMap::new()))
    }

    /// 絵文字の読み表つきで構築する。 表のキーは [`crate::rules::emoji::strip_variation`] を通した形で持つこと
    #[must_use]
    pub fn with_emoji(
        input: &str,
        emoji: Arc<HashMap<String, crate::rules::emoji::EmojiEntry>>,
    ) -> Self {
        Self {
            tokens: extract_protected_tokens(input),
            emoji,
        }
    }

    /// pre-computed 保護 token 一覧を返す (debug / 解析用途)。
    #[cfg(test)]
    #[must_use]
    pub fn tokens(&self) -> &[ProtectedToken] {
        &self.tokens
    }
}

impl CandidateProvider for ProtectTokenProvider {
    fn candidates_at<'b>(
        &'b self,
        ctx: &ScoringContext<'b>,
        pos: usize,
        out: &mut Vec<RawCandidate<'b>>,
    ) {
        // tokens は start の昇順 (extract_protected_tokens が並べる)。 位置ごとに全件を舐めると、
        // 絵文字・URL の多い長い入力で 「位置数 × 件数」 になるので、 二分探索で pos の先頭へ飛ぶ
        let first = self.tokens.partition_point(|t| t.range.start < pos);
        for token in self.tokens[first..]
            .iter()
            .take_while(|t| t.range.start <= pos)
        {
            if token.range.start == pos {
                let surface = &ctx.input[token.range.clone()];
                let char_count = surface.chars().count();
                let length = u8::try_from(char_count).unwrap_or(u8::MAX);
                // 顔文字は読みを空にして丸ごと除外、 絵文字は辞書の読み (無ければ表記のまま)、 それ以外は表記のまま素通し
                let reading: std::borrow::Cow<'_, str> = match token.kind {
                    ProtectedKind::Kaomoji
                    | ProtectedKind::Entity
                    | ProtectedKind::AsciiArt
                    | ProtectedKind::Markup => "".into(),
                    ProtectedKind::Emoji => emoji_reading(
                        surface,
                        &self.emoji,
                        &ctx.input[..token.range.start],
                        &ctx.input[token.range.end..],
                    )
                    .map_or_else(|| surface.into(), Into::into),
                    _ => surface.into(),
                };
                out.push(RawCandidate::new(
                    reading,
                    token.range.clone(),
                    Score::new(BAND_PROTECTED, length, 0),
                ));
            }
        }
    }
}
