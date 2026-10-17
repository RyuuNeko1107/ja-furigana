//! 特殊処理 (cross-cutting) candidate provider 群。
//!
//! 詳細仕様: `docs/PROPOSALS/scoring-engine.md` §5.6
//!
//! ## 含まれる provider
//!
//! - [`ProtectTokenProvider`]: URL / Email / 絵文字 を保護 token として抽出 (band 2000、 必ず勝つ)
//! - 今後追加: アルファベット passthrough / 数字+助数詞 / 漢数字 / 数字読み / 踊り字
//!
//! ## 設計方針
//!
//! 保護トークンは scoring-engine pipeline の **前段** で抽出し、 高 band candidate として
//! `solve_path` に流す。 surface = reading で透過 (= URL や 絵文字をひらがな化しない)、
//! path 選択時に必ず採用される (band 2000 が他の全 band を上回る)。
//!
//! URL_RE / EMAIL_RE は本 module が唯一の実装 (旧 `chunks::regex` の重複実装は
//! alpha.15 の chunks 削除で解消済)。

use crate::char_class::{self, is_digit_char};
use crate::scoring::candidate::{
    CandidateProvider, RawCandidate, Score, ScoringContext, BAND_DICT_EXACT, BAND_KANJI,
    BAND_PROTECTED,
};
use crate::scoring::digit_reading::DigitStyle;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;
use std::collections::HashSet;
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
fn emoji_clusters(run: &str) -> Vec<&str> {
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
fn emoji_reading(
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
        for token in &self.tokens {
            if token.range.start == pos {
                let surface = &ctx.input[token.range.clone()];
                let char_count = surface.chars().count();
                let length = u8::try_from(char_count).unwrap_or(u8::MAX);
                // 顔文字は読みを空にして丸ごと除外、 絵文字は辞書の読み (無ければ表記のまま)、 それ以外は表記のまま素通し
                let reading: std::borrow::Cow<'_, str> = match token.kind {
                    ProtectedKind::Kaomoji | ProtectedKind::Entity => "".into(),
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

// ─── アルファベット (英語) passthrough (C2) ─────────────────────────────────

/// 英字 / 全角英数字 / ASCII whitespace を 1 文字単位で判定。
///
/// = 「英数」 文字種 ([`char_class::is_alphanumeric_char`]) + **半角 space / tab**
/// (= 「猫 犬」 のような ASCII whitespace 含み input で path 構築失敗を防ぐ、
/// passthrough として扱う)。 whitespace 込みは本 provider 固有の判断で、
/// range 知識自体は char_class と共有 (= 手動同期不要)。
#[must_use]
pub fn is_alphabet_char(c: char) -> bool {
    c == ' ' || c == '\t' || char_class::is_alphanumeric_char(c)
}

/// 入力中の連続英字 byte range を全列挙。
///
/// 連続する [`is_alphabet_char`] 文字を 1 つの range に集約。 例: `"APIサーバー"`
/// → `[0..3]` (= "API")。 1 文字単独でも range として扱う。
#[must_use]
pub fn find_alphabet_ranges(input: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start: Option<usize> = None;
    let mut end: usize = 0;
    let chars: Vec<(usize, char)> = input.char_indices().collect();
    for (i, &(idx, c)) in chars.iter().enumerate() {
        let char_end = idx + c.len_utf8();
        // 英字語内の連結記号 (Wi-Fi / Blu-ray / don't) は語の一部として range に含める。
        // 前後が **英字 (数字・空白を除く)** の時だけ。 「3-1」 のような数値式や
        // 「A - B」 のような空白挟みは対象外 (= 従来通り記号 provider に流れる)。
        let joins_word = is_alphabet_connector(c)
            && i > 0
            && is_alphabet_letter(chars[i - 1].1)
            && chars
                .get(i + 1)
                .is_some_and(|&(_, n)| is_alphabet_letter(n));
        // 空白は 英字どうしの間 (Hello World) を繋ぐときだけ range に含める。
        // 数字に接する空白まで含めると 「2 M3.7」 が 1 つの passthrough に覆われ、
        // 数値側の読みが負けて 「2 M3。 なな」 になる (★2026-09-15 精度評価で検出)。
        let is_space = c == ' ' || c == '\t';
        let space_joins = is_space
            && i > 0
            && is_alphabet_letter(chars[i - 1].1)
            && chars
                .get(i + 1)
                .is_some_and(|&(_, n)| is_alphabet_letter(n));
        let in_range = if is_space {
            space_joins
        } else {
            is_alphabet_char(c)
        };
        if in_range || joins_word {
            if start.is_none() {
                start = Some(idx);
            }
            end = char_end;
        } else if let Some(s) = start.take() {
            ranges.push(s..end);
        }
    }
    if let Some(s) = start {
        ranges.push(s..end);
    }
    // range の先頭 / 末尾の空白を落とす。
    //
    // `is_alphabet_char` は 「Hello World」 を 1 range に保つため空白を含むが、
    // そのままだと 「優勝 5回」 の 「 5」 が range になり、 **数字を passthrough が覆って
    // 助数詞が効かなくなる** (「 5かい」。 ★2026-09-11 文脈不変検査で検出)。
    ranges
        .into_iter()
        .filter_map(|r| {
            let trimmed = input[r.clone()].trim_matches(|c: char| c.is_whitespace());
            if trimmed.is_empty() {
                return None;
            }
            let offset = input[r.clone()].find(trimmed).unwrap_or(0);
            let start = r.start + offset;
            Some(start..start + trimmed.len())
        })
        .collect()
}

/// 英字語の内部連結記号 (ハイフン類 / アポストロフィ類)。
fn is_alphabet_connector(c: char) -> bool {
    matches!(
        c,
        '-' | '\u{2010}' | '\u{2011}' | '\u{2212}' | '\u{FF0D}' | '\'' | '\u{2019}'
    )
}

/// 英字 (ASCII / 全角 A-Z a-z)。 数字・空白は含まない。
fn is_alphabet_letter(c: char) -> bool {
    c.is_ascii_alphabetic() || matches!(c, '\u{FF21}'..='\u{FF3A}' | '\u{FF41}'..='\u{FF5A}')
}

/// 連結記号を除いた lookup key (= `wi-fi` → `wifi`)。 記号を含まなければ `None`。
fn strip_alphabet_connectors(normalized: &str) -> Option<String> {
    if !normalized.chars().any(is_alphabet_connector) {
        return None;
    }
    Some(
        normalized
            .chars()
            .filter(|&c| !is_alphabet_connector(c))
            .collect(),
    )
}

/// 英字 surface を 「全角→半角 + case-fold」 で正規化。
///
/// dict 完全一致 lookup 前段の normalize。 旧 `loanwords::Loanwords::normalize`
/// (alpha.15 で削除済) と同等の挙動、 現在は本関数が唯一の実装。
#[must_use]
pub fn normalize_alphabet(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let half = match c {
            'Ａ'..='Ｚ' => char::from_u32((c as u32) - 0xFF21 + 0x41).unwrap_or(c),
            'ａ'..='ｚ' => char::from_u32((c as u32) - 0xFF41 + 0x61).unwrap_or(c),
            '０'..='９' => char::from_u32((c as u32) - 0xFF10 + 0x30).unwrap_or(c),
            _ => c,
        };
        let folded = if half.is_ascii_uppercase() {
            half.to_ascii_lowercase()
        } else {
            half
        };
        out.push(folded);
    }
    out
}

/// lookup map 内で `[digits]` table の項目 (語 → 数字の読み方) を区別する key 接頭辞。
/// `#` は英字 range に入らないので surface の key とは衝突しない。
pub const DIGITS_KEY_PREFIX: &str = "#digits:";

/// 英文の中でだけ使う語 (loanwords の `[sentence_words]`) の lookup key 接頭辞。
pub const SENTENCE_KEY_PREFIX: &str = "#sw:";

/// 区切りを探す語の最大長 / 連結を試す英字の最大長
const MAX_WORD_LEN: usize = 20;
const MAX_SEGMENT_INPUT: usize = 40;
/// 連結を試す英字の最小長 (これ未満は区切らない)
const MIN_SEGMENT_INPUT: usize = 6;
/// `[digits]` の値で 「読み方」 と 「語 + 数字の時だけ使う読み」 を区切る文字。
pub const DIGITS_SEP: char = '\u{1F}';

/// 英字 token の lookup と passthrough を扱う [`CandidateProvider`]。
///
/// 構築時に input を scan して英字 range を pre-compute。 各 range で:
/// - 正規化 ([`normalize_alphabet`]) 後 lookup map に hit → band 1000 candidate (= dict 適用)
/// - miss → passthrough candidate (= surface = output、 reading 振らない、 band [`BAND_KANJI`])
///
/// **band 設計** (proposal §5.6 「band 比較対象外」 を fallback と解釈):
///
/// hit (= 辞書経由 reading 確定) は band 1000 で他 provider と勝負、 miss (= 入力素通し) は
/// band 100 で fallback 扱い。 これにより数字系 [`crate::scoring::numbers::NumberCandidateProvider`]
/// (band 950) が `100km` のような alphanumeric mixed surface で SI reading を勝たせられる。
/// 単独 ASCII (例: `API`) は他 provider に競合候補が無いので miss band 100 でも path に採用される。
#[derive(Debug, Clone)]
pub struct AlphabetPassthroughProvider {
    /// 入力中の英字 byte range (pre-computed)
    ranges: Vec<Range<usize>>,
    /// 正規化済 surface → reading の lookup map (Arc 共有、 caller が pre-populate)
    lookup: Arc<HashMap<String, String>>,
    /// SI 単位記号 (小文字化済)。 空なら従来どおり全 span を emit する。
    unit_symbols: Arc<HashSet<String>>,
    /// 直後に型番が続く range の終端 (byte)。 `split_at_model_starts` が切った range
    before_model: Vec<usize>,
}

impl AlphabetPassthroughProvider {
    /// 入力 + lookup map で provider 構築。
    #[must_use]
    /// 単位情報なしで構築する (test / 単体利用)。 本番 pipeline は
    /// [`Self::with_units`] を使うこと (= 「3km」 span の重複を避けるため)。
    #[cfg(test)]
    pub fn new(input: &str, lookup: Arc<HashMap<String, String>>) -> Self {
        Self {
            ranges: find_alphabet_ranges(input),
            lookup,
            unit_symbols: Arc::new(HashSet::new()),
            before_model: Vec::new(),
        }
    }

    /// SI 単位記号の集合を渡して構築する。
    ///
    /// 「3km」 のような **数字 + 単位** の span は、 数値側 (band 950) が読むべきで、
    /// ここで passthrough 候補 (band 100) を出すと path が同点になり、 文中では
    /// passthrough が勝って単位が読まれない (★2026-09-11 文脈不変検査で検出:
    /// 「130km」 は読めるのに 「あ130km」 が読めなかった)。
    pub fn with_units(
        input: &str,
        lookup: Arc<HashMap<String, String>>,
        unit_symbols: Arc<HashSet<String>>,
    ) -> Self {
        Self {
            ranges: find_alphabet_ranges(input),
            lookup,
            unit_symbols,
            before_model: Vec::new(),
        }
    }

    /// 1 語の読み。 外来語表の語か、 英文の中でだけ使う語 (a / is / to …)。
    /// 2 文字以下の外来語表の語は 「英文の語」 としては使わない (表にある語でも短いものは語の一部になりやすい)
    fn word_reading(&self, norm: &str) -> Option<&str> {
        self.lookup
            .get(norm)
            .or_else(|| self.lookup.get(&format!("{SENTENCE_KEY_PREFIX}{norm}")))
            .map(String::as_str)
    }

    /// 空白なしでつながった英単語 (thankyou / iloveyou / GameOver) を、 辞書の語に区切って読む。
    ///
    /// 全部の部分が辞書 (3 文字以上の外来語表の語) か英文の語表にあり、 2 部分以上に分かれ、
    /// 辞書の語を 1 つ以上含む時だけ。 全部大文字 (略語) / 数字や記号を含むものは対象外。
    /// 部分の数が最も少ない区切りを選ぶ
    fn segmented_reading(&self, surface: &str) -> Option<String> {
        let n = surface.len();
        if !(MIN_SEGMENT_INPUT..=MAX_SEGMENT_INPUT).contains(&n)
            || !surface.bytes().all(|b| b.is_ascii_alphabetic())
            || surface.bytes().all(|b| b.is_ascii_uppercase())
        {
            return None;
        }
        let mut out = String::new();
        let mut parts = 0;
        let mut content = 0;
        for chunk in camel_chunks(surface) {
            let (reading, p, c) = self.word_break(&chunk.to_ascii_lowercase())?;
            out.push_str(&reading);
            parts += p;
            content += c;
        }
        (parts >= 2 && content >= 1).then_some(out)
    }

    /// 小文字の英字列を辞書の語に区切る (部分の数が最少のもの)。 (読み, 部分の数, 辞書の語の数)
    ///
    /// 短い部分 (2 文字以下の英文の語) は、 隣り合わせにしない / 1 文字は先頭の `i` `a` だけ。
    /// さもないと固有名詞 (Aniimo = an + i + …) が細かい語に割られて読まれてしまう
    fn word_break(&self, s: &str) -> Option<(String, usize, usize)> {
        /// 位置 i の状態 (k = 0: 直前の部分が長い / 1: 短い) に至る最良の経路: (部分の数, 辞書の語の数, 直前の位置, 直前の状態)
        type Entry = (usize, usize, usize, usize);
        let n = s.len();
        let mut best: Vec<[Option<Entry>; 2]> = vec![[None, None]; n + 1];
        best[0][0] = Some((0, 0, 0, 0));
        // 文章用の語表の key を作り直さないための作業領域
        let mut key = String::from(SENTENCE_KEY_PREFIX);
        for i in 0..n {
            for k in 0..2 {
                let Some((count, content, _, _)) = best[i][k] else {
                    continue;
                };
                for j in (i + 1)..=n.min(i + MAX_WORD_LEN) {
                    let word = &s[i..j];
                    let short = word.len() <= 2;
                    if (short && k == 1)
                        || (word.len() == 1 && !(i == 0 && matches!(word, "i" | "a")))
                    {
                        continue;
                    }
                    let is_content = word.len() >= 3 && self.lookup.contains_key(word);
                    if !is_content {
                        key.truncate(SENTENCE_KEY_PREFIX.len());
                        key.push_str(word);
                        if !self.lookup.contains_key(&key) {
                            continue;
                        }
                    }
                    let cand = (count + 1, content + usize::from(is_content), i, k);
                    let slot = &mut best[j][usize::from(short)];
                    if slot.is_none_or(|b| cand.0 < b.0) {
                        *slot = Some(cand);
                    }
                }
            }
        }
        let end = best[n];
        let (mut k, (count, content, _, _)) = end
            .iter()
            .enumerate()
            .filter_map(|(k, e)| e.map(|e| (k, e)))
            .min_by_key(|(_, e)| e.0)?;
        // 経路を逆にたどって、 部分の読みを前から並べる
        let mut pieces: Vec<&str> = Vec::with_capacity(count);
        let mut j = n;
        while j > 0 {
            let (_, _, i, pk) = best[j][k]?;
            let word = &s[i..j];
            let reading = self
                .lookup
                .get(word)
                .filter(|_| word.len() >= 3)
                .or_else(|| {
                    key.truncate(SENTENCE_KEY_PREFIX.len());
                    key.push_str(word);
                    self.lookup.get(&key)
                })?;
            pieces.push(reading.as_str());
            j = i;
            k = pk;
        }
        pieces.reverse();
        Some((pieces.concat(), count, content))
    }

    /// 空白区切りの 2 語以上で、 外来語表にある語を 「語の読み」 に置き換えて空白でつないだもの
    /// (末尾の空白は残す)。 全部の語が表にある時だけ。 `allow_partial` なら 1 語でもあれば読む
    fn known_words_reading(&self, surface: &str, allow_partial: bool) -> Option<String> {
        let body = surface.trim_end_matches(' ');
        // 単独の語 (型番の前の 1 語) には英文用の語表を使わない (単独の so / to を読まない)
        let sentence = body.contains(' ');
        let mut hits = 0;
        let mut content = false;
        let mut out: Vec<&str> = Vec::new();
        for word in body.split(' ').filter(|w| !w.is_empty()) {
            let norm = normalize_alphabet(word);
            let found = if sentence {
                self.word_reading(&norm)
            } else {
                self.lookup.get(&norm).map(String::as_str)
            };
            if norm.len() >= 3 && self.lookup.contains_key(&norm) {
                content = true;
            }
            match found {
                Some(r) => {
                    hits += 1;
                    out.push(r);
                }
                None => out.push(word),
            }
        }
        let all_known = hits == out.len();
        // 複数語は、 外来語表の語 (3 文字以上) を 1 つ以上含む時だけ (a a a a a / go up のような機能語だけは読まない)
        if sentence && !content {
            return None;
        }
        (hits > 0 && (all_known || allow_partial)).then(|| out.join(" ") + &surface[body.len()..])
    }

    /// 型番 (`i9-13900K` / `RTX 4090`) の開始位置で英字 range を切る。
    ///
    /// 前に英単語が付くと (`Hello i9-13900K`)、 range が型番の頭を飲み込んで型番候補の前に
    /// 辺が無くなり、 型番が読めなくなる。 開始位置より後ろは型番側の候補が覆う。
    #[must_use]
    pub fn split_at_model_starts(mut self, starts: &[usize]) -> Self {
        for r in &mut self.ranges {
            if let Some(&s) = starts.iter().filter(|&&s| r.start < s && s < r.end).min() {
                r.end = s;
                self.before_model.push(s);
            }
        }
        self
    }

    /// span が 「先頭の数字 + 既知の単位記号」 の形か (= 数値側に任せるべきか)。
    fn is_number_with_unit(&self, surface: &str) -> bool {
        if self.unit_symbols.is_empty() {
            return false;
        }
        let digits: String = surface.chars().take_while(|c| is_digit_char(*c)).collect();
        if digits.is_empty() {
            return false;
        }
        let rest = &surface[digits.len()..];
        !rest.is_empty() && self.unit_symbols.contains(&rest.to_ascii_lowercase())
    }

    /// 正規化済 surface が 「英字 + ASCII 数字」 で、 英字部分に reading と `[digits]` の
    /// 読み方がどちらも登録されていれば、 連結した reading を返す。
    fn letters_with_digits_reading(&self, normalized: &str) -> Option<String> {
        let letters_len = normalized.find(|c: char| !c.is_ascii_alphabetic())?;
        let (letters, digits) = normalized.split_at(letters_len);
        if letters.is_empty() {
            return None;
        }
        let (style, base) = self.digits_style_and_base(letters)?;
        Some(format!("{base}{}", style.render(digits)?))
    }

    /// `[digits]` に載った語の (読み方, 語の読み)。 語の読みは `reading` の上書きが
    /// あればそれ、 無ければ `[entries]` の読み。
    fn digits_style_and_base(&self, letters: &str) -> Option<(DigitStyle, &str)> {
        let raw = self.lookup.get(&format!("{DIGITS_KEY_PREFIX}{letters}"))?;
        let (style, over) = match raw.split_once(DIGITS_SEP) {
            Some((s, r)) => (s, Some(r)),
            None => (raw.as_str(), None),
        };
        let base = over.or_else(|| self.lookup.get(letters).map(String::as_str))?;
        Some((DigitStyle::parse(style)?, base))
    }

    /// 英字だけの range の直後が 「区切り (空白 / ハイフン) + 数字」 で、 その語が `[digits]` に
    /// 載っていれば (数字の終端の byte 位置, 連結 reading) を返す。 数字の直後が `.数字`
    /// (iOS 17.2) の時は小数として別に読むので対象外。
    fn separated_digits_reading(
        &self,
        input: &str,
        range: &Range<usize>,
        normalized: &str,
    ) -> Option<(usize, String)> {
        if !normalized.chars().all(|c| c.is_ascii_alphabetic()) {
            return None;
        }
        let (style, base) = self.digits_style_and_base(normalized)?;
        let rest = &input[range.end..];
        let sep = rest.chars().next()?;
        if !matches!(
            sep,
            ' ' | '-' | '.' | '\u{2010}' | '\u{2011}' | '\u{2212}' | '\u{FF0D}' | '\u{3000}'
        ) {
            return None;
        }
        let after_sep = &rest[sep.len_utf8()..];
        let digits_len = after_sep
            .find(|c: char| !is_digit_char(c))
            .unwrap_or(after_sep.len());
        if digits_len == 0 {
            return None;
        }
        let tail = &after_sep[digits_len..];
        if tail.starts_with('.') && tail[1..].chars().next().is_some_and(is_digit_char) {
            return None;
        }
        let digits = normalize_alphabet(&after_sep[..digits_len]);
        let reading = format!("{base}{}", style.render(&digits)?);
        Some((range.end + sep.len_utf8() + digits_len, reading))
    }

    /// lookup 不要 (= 全 passthrough) で構築。 test / 簡易用途。
    #[cfg(test)]
    #[must_use]
    pub fn passthrough_only(input: &str) -> Self {
        Self::new(input, Arc::new(HashMap::new()))
    }

    /// pre-computed 英字 range を返す (debug / 解析用途)。
    #[cfg(test)]
    #[must_use]
    pub fn ranges(&self) -> &[Range<usize>] {
        &self.ranges
    }
}

impl CandidateProvider for AlphabetPassthroughProvider {
    fn candidates_at<'b>(
        &'b self,
        ctx: &ScoringContext<'b>,
        pos: usize,
        out: &mut Vec<RawCandidate<'b>>,
    ) {
        for range in &self.ranges {
            if range.start != pos {
                continue;
            }
            let surface = &ctx.input[range.clone()];

            // ★alpha.19: surface が全て数字 (= 「2」「100」 等) なら skip。
            // 数字単独の reading 化は NumberCandidateProvider (band 950) の責務、
            // ここで passthrough band 100 で 「2」 surface "2" として emit すると、
            // 「2〜3回」 のような range 内で NumberCandidateProvider の "ニ" 候補と
            // path tie になり、 provider 列挙順で Alphabet が勝つ問題が出る。
            if surface.chars().all(is_digit_char) {
                continue;
            }

            let char_count = surface.chars().count();
            let length = u8::try_from(char_count).unwrap_or(u8::MAX);

            let normalized = normalize_alphabet(surface);
            // 完全一致 → 連結記号除去 (Wi-Fi → wifi) の順で lookup。
            let hit = self.lookup.get(&normalized).or_else(|| {
                strip_alphabet_connectors(&normalized).and_then(|k| self.lookup.get(&k))
            });

            // 単独の w の直後が助詞 (キーwを押す / wは小文字) なら、 笑いではなく文字そのもの。
            // 笑いの w は文末・読点・絵文字の前など助詞の前には来ない。 外来語表の ワラ を使わず
            // 文字名で読む。 www... や ww は従来どおり笑い
            // 笑いの w は仮名・漢字の後ろにも普通に来る (なぁwとは / 対応wといい) ので、 文字名にするのは
            // 大文字の W (ガンダムWが) か、 小文字なら文頭・空白・括弧・引用符・長音 (キー) の後ろだけ
            let prev = ctx.input[..range.start].chars().next_back();
            let w_is_letter = surface == "W"
                || (surface == "w"
                    && prev.is_none_or(|c| {
                        c.is_whitespace()
                            || matches!(c, '「' | '『' | '(' | '（' | '【' | '[' | '"' | '“' | 'ー')
                    }));
            // 式の中の w (w=2 / x+w / a*w) も文字そのもの。 演算子は笑いの w の前後には付かない (> < ^ は顔文字なので除く)
            let operator = |c: char| matches!(c, '=' | '+' | '*');
            let w_in_formula = surface == "w"
                && (prev.is_some_and(operator)
                    || ctx.input[range.end..].chars().next().is_some_and(operator));
            let letter_name = ((w_is_letter
                && ctx.input[range.end..].chars().next().is_some_and(|c| {
                    matches!(c, 'は' | 'を' | 'が' | 'の' | 'も' | 'に' | 'で' | 'と')
                }))
                || w_in_formula)
                .then_some("ダブリュー");
            let hit = if letter_name.is_some() { None } else { hit };

            // 「3km」 のような 数字 + 単位 は NumberCandidateProvider の責務。
            // ただし辞書に完全一致がある語 (5G = ファイブジー。 G は SI 単位のグラムでもある) は
            // 辞書を優先する。 小文字を含む表記 (4g = 4 グラム) は単位として読むので除く。
            let dict_wins = hit.is_some() && !surface.chars().any(|c| c.is_ascii_lowercase());
            if !dict_wins && self.is_number_with_unit(surface) {
                continue;
            }
            // 「Windows11」 「RTX4090」 のような 英字 + 数字 は全体が辞書に無いと丸ごと
            // passthrough になる。 英字部分が loanwords の `[digits]` に載っている語だけ、
            // 「英字の読み + 宣言された読み方での数字」 を読みにする (数字の読みは語ごとに
            // 違うので、 未登録の語は従来どおり素通し)。 同じ range に素通し候補も重ねると
            // 前が仮名の時 (とGPT5) に素通しが残るので、 どちらか 1 つだけ出す。
            // 数字の直後が `.数字` (Ver2.0 / iOS17.2) なら小数 / バージョンとして別に読む。
            let after_range = &ctx.input[range.end..];
            let decimal_follows = after_range.starts_with('.')
                && after_range[1..].chars().next().is_some_and(is_digit_char);
            let with_digits = if hit.is_none() && !decimal_follows {
                self.letters_with_digits_reading(&normalized)
            } else {
                None
            }
            // 空白でつながった複数語 (Hello World / Intel Core) は、 全部の語が外来語表にあれば語ごとに
            // カタカナで読む。 型番の直前 (Intel Core i7-...) なら未登録の語が混じっても、 登録のある語は読む。
            // 一部しか知らない英文 (thank you so much) は英字のまま素通し (2026-09-15 の決定を保つ)
            .or_else(|| {
                if hit.is_none() {
                    self.known_words_reading(surface, self.before_model.contains(&range.end))
                } else {
                    None
                }
            })
            // 空白なしの連結 (thankyou / GameOver) は、 辞書の語に区切れる時だけ読む
            .or_else(|| {
                if hit.is_none() && !surface.contains(' ') {
                    self.segmented_reading(surface)
                } else {
                    None
                }
            });
            let (reading, band): (std::borrow::Cow<'_, str>, _) =
                match (letter_name, hit, with_digits) {
                    (Some(r), _, _) => (r.into(), BAND_DICT_EXACT),
                    (None, Some(r), _) => (r.as_str().into(), BAND_DICT_EXACT),
                    (None, None, Some(r)) => (r.into(), BAND_DICT_EXACT),
                    (None, None, None) => (surface.into(), BAND_KANJI), // passthrough miss は fallback band
                };

            out.push(RawCandidate::new(
                reading,
                range.clone(),
                Score::new(band, length, 0),
            ));

            // 英字の直後に # / + が続く語 (C# / F# / C++ / g++) は、 外来語表に載っている時だけ
            // 記号込みで引く (英字の範囲は記号を含まないので、 そのままでは C + # に割れて # が素通しになる)。 2026-09-27
            let tail: String = ctx.input[range.end..]
                .chars()
                .take_while(|c| matches!(c, '#' | '＃' | '+' | '＋'))
                .collect();
            if !tail.is_empty() {
                let end = range.end + tail.len();
                let key = normalize_alphabet(&ctx.input[range.start..end]);
                if let Some(r) = self.lookup.get(&key) {
                    let length = u8::try_from(ctx.input[range.start..end].chars().count())
                        .unwrap_or(u8::MAX);
                    out.push(RawCandidate::new(
                        r.as_str(),
                        range.start..end,
                        Score::new(BAND_DICT_EXACT, length, 0),
                    ));
                }
            }

            // 「Windows 10」 「GPT-5」 のように 空白 / ハイフンを挟んだ 語 + 数字。 語が `[digits]` に
            // 載っていれば、 くっついた形 (Windows11) と同じ読みにする。
            if let Some((end, reading)) =
                self.separated_digits_reading(ctx.input, range, &normalized)
            {
                let l =
                    u8::try_from(ctx.input[range.start..end].chars().count()).unwrap_or(u8::MAX);
                out.push(RawCandidate::new(
                    reading,
                    range.start..end,
                    Score::new(BAND_DICT_EXACT, l, 0),
                ));
            }

            // 「SSR3体」 「CD2枚」 のように 英字 + 数字 + 日本語 が続く時、 全体を 1 つの
            // passthrough が覆うと 数字 + 助数詞 を数値側が読めない (SSR3からだ)。 英字だけの
            // passthrough も選べるようにして、 数字は NumberCandidateProvider に譲れる形にする。
            if band == BAND_KANJI {
                if let Some(letters_len) = letters_before_digits(surface) {
                    let after = &ctx.input[range.end..];
                    let counter_like = after.chars().next().is_some_and(|c| !c.is_ascii());
                    // バージョン表記 (v1.2.3 / Ver2.0): 数字の続きが `.数字`。 この時だけ
                    // 英字部分の辞書 reading も使う (v = ブイ / Ver = バージョン)。
                    let version_like = after.starts_with('.')
                        && after[1..].chars().next().is_some_and(is_digit_char);
                    if counter_like || version_like {
                        let l =
                            u8::try_from(surface[..letters_len].chars().count()).unwrap_or(u8::MAX);
                        let letters = &surface[..letters_len];
                        let dict_reading = if version_like {
                            self.lookup.get(&normalize_alphabet(letters))
                        } else {
                            None
                        };
                        out.push(RawCandidate::new(
                            dict_reading.map_or(letters, String::as_str),
                            range.start..range.start + letters_len,
                            Score::new(
                                if dict_reading.is_some() {
                                    BAND_DICT_EXACT
                                } else {
                                    BAND_KANJI
                                },
                                l,
                                0,
                            ),
                        ));
                    }
                }
            }
        }
    }
}

/// surface が 「英字 + 数字だけ」 (例: `SSR3`) なら 英字部分の byte 長。
/// camelCase / TitleCase の切れ目で分ける (`GameOver` → `Game` `Over`、 `iLoveYou` → `i` `Love` `You`)。
/// 切れ目は 「小文字の直後の大文字」 だけ。 全部小文字ならそのまま 1 つ
fn camel_chunks(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    for i in 1..bytes.len() {
        if bytes[i].is_ascii_uppercase() && bytes[i - 1].is_ascii_lowercase() {
            out.push(&s[start..i]);
            start = i;
        }
    }
    out.push(&s[start..]);
    out
}

fn letters_before_digits(surface: &str) -> Option<usize> {
    let letters_len = surface.find(|c: char| !is_alphabet_letter(c))?;
    let digits = &surface[letters_len..];
    (letters_len > 0 && digits.chars().all(is_digit_char)).then_some(letters_len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::emoji::EmojiEntry;
    use crate::scoring::boundary::BoundaryAnalysis;

    fn ctx(input: &str) -> ScoringContext<'_> {
        let boundary = Box::leak(Box::new(BoundaryAnalysis::empty()));
        ScoringContext::new(input, boundary)
    }

    // (is_emoji_char の unit test は crate::char_class 側に移動)

    // ─── URL extraction ──────────────────────────────────────────────────────

    #[test]
    fn extract_simple_https_url() {
        let tokens = extract_protected_tokens("見て https://example.com/path です");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, ProtectedKind::Url);
        let s = "見て https://example.com/path です";
        let extracted = &s[tokens[0].range.clone()];
        assert_eq!(extracted, "https://example.com/path");
    }

    #[test]
    fn extract_www_url_without_scheme() {
        let s = "www.example.com を見て";
        let tokens = extract_protected_tokens(s);
        // 「種別が含まれる」だけだと over-capture (末尾の space や「を」 を飲む) を
        // 見逃す。抽出 range の中身を完全一致で固定する。
        let url = tokens
            .iter()
            .find(|t| t.kind == ProtectedKind::Url)
            .expect("url token");
        assert_eq!(&s[url.range.clone()], "www.example.com");
    }

    #[test]
    fn extract_email() {
        let s = "foo@bar.com にメール";
        let tokens = extract_protected_tokens(s);
        let email = tokens
            .iter()
            .find(|t| t.kind == ProtectedKind::Email)
            .expect("email token");
        assert_eq!(&s[email.range.clone()], "foo@bar.com");
    }

    // ─── Emoji extraction ────────────────────────────────────────────────────

    #[test]
    fn extract_single_emoji() {
        let tokens = extract_protected_tokens("こんにちは😀");
        let emoji = tokens
            .iter()
            .find(|t| t.kind == ProtectedKind::Emoji)
            .expect("emoji token");
        let s = "こんにちは😀";
        let extracted = &s[emoji.range.clone()];
        assert_eq!(extracted, "😀");
    }

    #[test]
    fn extract_consecutive_emoji_as_single_token() {
        let tokens = extract_protected_tokens("🎉🎊✨ パーティー");
        let emoji_tokens: Vec<&ProtectedToken> = tokens
            .iter()
            .filter(|t| t.kind == ProtectedKind::Emoji)
            .collect();
        assert_eq!(
            emoji_tokens.len(),
            1,
            "consecutive emoji が 1 token に集約される"
        );
        let s = "🎉🎊✨ パーティー";
        let extracted = &s[emoji_tokens[0].range.clone()];
        assert_eq!(extracted, "🎉🎊✨");
    }

    #[test]
    fn extract_emoji_separated_by_text() {
        let tokens = extract_protected_tokens("こんにちは🎉お元気ですか🚀");
        let emoji_tokens: Vec<&ProtectedToken> = tokens
            .iter()
            .filter(|t| t.kind == ProtectedKind::Emoji)
            .collect();
        assert_eq!(emoji_tokens.len(), 2);
    }

    // ─── 重複削除 ────────────────────────────────────────────────────────────

    #[test]
    fn extract_no_duplicate_tokens_for_overlapping_url_email() {
        // URL の中に email っぽい substring がある場合、 広い range の URL が勝ち、
        // 包含される email token は dedup で消える。「URL が 1 つ以上」 だけだと
        // dedup ロジック自体を検証できないので、 token 総数と内訳・range を固定する。
        let s = "https://user@example.com/page";
        let tokens = extract_protected_tokens(s);
        assert_eq!(
            tokens.len(),
            1,
            "email は URL に包含され dedup される: {tokens:?}"
        );
        assert_eq!(tokens[0].kind, ProtectedKind::Url);
        assert_eq!(&s[tokens[0].range.clone()], s, "URL は全体を覆う");
        assert_eq!(
            tokens
                .iter()
                .filter(|t| t.kind == ProtectedKind::Email)
                .count(),
            0,
            "包含 email token は残らない"
        );
    }

    // ─── ProtectTokenProvider ────────────────────────────────────────────────

    #[test]
    fn provider_returns_candidate_at_token_start() {
        let input = "foo https://example.com bar";
        let provider = ProtectTokenProvider::new(input);
        let url_start = input.find("https").unwrap();
        let candidates = provider.candidates_vec(&ctx(input), url_start);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].surface, "https://example.com");
        assert_eq!(candidates[0].reading, "https://example.com"); // passthrough
        assert_eq!(candidates[0].score.band, BAND_PROTECTED);
    }

    #[test]
    fn provider_returns_empty_at_non_token_position() {
        let input = "foo https://example.com bar";
        let provider = ProtectTokenProvider::new(input);
        let candidates = provider.candidates_vec(&ctx(input), 0);
        assert!(candidates.is_empty(), "pos 0 は URL の start ではない");
    }

    #[test]
    fn provider_emoji_passthrough_reading() {
        let input = "Hi😀";
        let provider = ProtectTokenProvider::new(input);
        let emoji_start = input.find('😀').unwrap();
        let candidates = provider.candidates_vec(&ctx(input), emoji_start);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].surface, "😀");
        assert_eq!(candidates[0].reading, "😀"); // passthrough、 ひらがな化しない
    }

    #[test]
    fn provider_with_no_protected_tokens_yields_empty() {
        let input = "ただのテキスト";
        let provider = ProtectTokenProvider::new(input);
        assert!(provider.tokens().is_empty());
        assert!(provider.candidates_vec(&ctx(input), 0).is_empty());
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn protected_band_higher_than_dict_exact() {
        // 保護 token は dict 完全一致より優先される必要がある
        use crate::scoring::candidate::BAND_DICT_EXACT;
        assert!(BAND_PROTECTED > BAND_DICT_EXACT);
    }

    // ─── アルファベット passthrough (C2) tests ───────────────────────────────

    #[test]
    fn alphabet_char_detects_ascii_alphanumeric() {
        assert!(is_alphabet_char('A'));
        assert!(is_alphabet_char('z'));
        assert!(is_alphabet_char('5'));
    }

    #[test]
    fn alphabet_char_detects_fullwidth() {
        assert!(is_alphabet_char('Ａ'));
        assert!(is_alphabet_char('ｚ'));
        assert!(is_alphabet_char('５'));
    }

    #[test]
    fn alphabet_char_rejects_non_alphabet() {
        assert!(!is_alphabet_char('猫'));
        assert!(!is_alphabet_char('あ'));
        assert!(!is_alphabet_char('!'));
    }

    #[test]
    fn alphabet_char_accepts_whitespace_passthrough() {
        // space / tab は alphabet 扱い (= 「猫 犬」 のような ASCII whitespace 含み input
        // で path 構築失敗を防ぐ passthrough 設計、 impl 側 doc comment 参照)
        assert!(is_alphabet_char(' '));
        assert!(is_alphabet_char('\t'));
    }

    #[test]
    fn find_alphabet_ranges_basic() {
        let ranges = find_alphabet_ranges("APIサーバー");
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0], 0..3); // "API" = 3 ASCII bytes
    }

    #[test]
    fn find_alphabet_ranges_multiple() {
        let ranges = find_alphabet_ranges("ABCのDEF");
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0], 0..3); // "ABC"
                                     // "の" は 3 bytes (UTF-8)、 「DEF」 は 6..9
        assert_eq!(ranges[1], 6..9);
    }

    #[test]
    fn find_alphabet_ranges_splits_space_next_to_digit() {
        // 数字に接する空白は range を繋がない。 「2 M3.7」 を 1 つの passthrough で
        // 覆うと数値の読みが負ける (★2026-09-15)。
        let ranges = find_alphabet_ranges("2 M3.7");
        // "2" / "M3" / "7" (「.」 は英数でないので range を切る)
        assert_eq!(ranges, vec![0..1, 2..4, 5..6]);
    }

    #[test]
    fn find_alphabet_ranges_keeps_space_between_letters() {
        // 英字どうしの間の空白は従来どおり 1 range にまとめる。
        let ranges = find_alphabet_ranges("Hello World");
        assert_eq!(ranges, vec![0..11]);
    }

    #[test]
    fn find_alphabet_ranges_empty_for_no_alphabet() {
        let ranges = find_alphabet_ranges("漢字とひらがな");
        assert!(ranges.is_empty());
    }

    #[test]
    fn find_alphabet_ranges_joins_hyphen_between_letters() {
        // Wi-Fi / Blu-ray / e-mail は 1 語 (連結記号を含めて 1 range)
        let ranges = find_alphabet_ranges("ポケットWi-Fiつけよう");
        assert_eq!(ranges.len(), 1);
        assert_eq!(&"ポケットWi-Fiつけよう"[ranges[0].clone()], "Wi-Fi");
        let ranges = find_alphabet_ranges("Blu-ray");
        assert_eq!(ranges, vec![0..7]);
        let ranges = find_alphabet_ranges("don't");
        assert_eq!(ranges, vec![0..5]);
    }

    #[test]
    fn find_alphabet_ranges_does_not_join_hyphen_next_to_digits_or_spaces() {
        // 「3-1」 は数値式 (数字提供者に任せる)、 「A - B」 は空白挟みで語ではない
        let ranges = find_alphabet_ranges("3-1");
        assert_eq!(ranges, vec![0..1, 2..3]);
        // ★2026-09-11: range の前後の空白は落とす (= 「A」 と 「B」)。
        // 空白を含めたままだと 「優勝 5回」 の 「 5」 が range になり、
        // 数字を passthrough が覆って助数詞が効かなくなる。
        let ranges = find_alphabet_ranges("A - B");
        assert_eq!(ranges, vec![0..1, 4..5]);
        // 末尾 / 先頭のハイフンは語に含めない
        let ranges = find_alphabet_ranges("abc-");
        assert_eq!(ranges, vec![0..3]);
        let ranges = find_alphabet_ranges("-abc");
        assert_eq!(ranges, vec![1..4]);
        // 数字と英字の間 (USB-3) も連結しない = 従来挙動
        let ranges = find_alphabet_ranges("USB-3");
        assert_eq!(ranges, vec![0..3, 4..5]);
    }

    #[test]
    fn alphabet_passthrough_looks_up_hyphenated_word_as_one_token() {
        let input = "ポケットWi-Fiつけよう";
        let mut lookup = HashMap::new();
        lookup.insert("wi-fi".to_string(), "ワイファイ".to_string());
        let provider = AlphabetPassthroughProvider::new(input, Arc::new(lookup));
        let pos = "ポケット".len();
        let candidates = provider.candidates_vec(&ctx(input), pos);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].surface, "Wi-Fi");
        assert_eq!(candidates[0].reading, "ワイファイ");
        assert_eq!(candidates[0].score.band, BAND_DICT_EXACT);
    }

    #[test]
    fn alphabet_passthrough_falls_back_to_connector_stripped_key() {
        // dict に "wifi" しか無くても "Wi-Fi" が hit する
        let input = "Wi-Fi";
        let mut lookup = HashMap::new();
        lookup.insert("wifi".to_string(), "ワイファイ".to_string());
        let provider = AlphabetPassthroughProvider::new(input, Arc::new(lookup));
        let candidates = provider.candidates_vec(&ctx(input), 0);
        assert_eq!(candidates[0].reading, "ワイファイ");
        assert_eq!(candidates[0].score.band, BAND_DICT_EXACT);
    }

    #[test]
    fn alphabet_passthrough_unknown_hyphenated_word_passes_through_whole() {
        let input = "Blu-ray";
        let provider = AlphabetPassthroughProvider::passthrough_only(input);
        let candidates = provider.candidates_vec(&ctx(input), 0);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].surface, "Blu-ray");
        assert_eq!(candidates[0].reading, "Blu-ray");
    }

    #[test]
    fn normalize_alphabet_lowercases_ascii() {
        assert_eq!(normalize_alphabet("API"), "api");
        assert_eq!(normalize_alphabet("Hello"), "hello");
    }

    #[test]
    fn normalize_alphabet_converts_fullwidth_to_halfwidth() {
        assert_eq!(normalize_alphabet("ＡＰＩ"), "api");
        assert_eq!(normalize_alphabet("Ｈｅｌｌｏ"), "hello");
        assert_eq!(normalize_alphabet("１２３"), "123");
    }

    #[test]
    fn normalize_alphabet_preserves_non_alphabetic() {
        // 非 alphabet 文字 (ここでは passed through but unrelated)
        assert_eq!(normalize_alphabet("a-b_c.d"), "a-b_c.d");
    }

    #[test]
    fn alphabet_passthrough_provider_returns_surface_when_no_lookup() {
        let input = "APIサーバー";
        let provider = AlphabetPassthroughProvider::passthrough_only(input);
        let candidates = provider.candidates_vec(&ctx(input), 0);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].surface, "API");
        assert_eq!(candidates[0].reading, "API"); // passthrough: reading = surface
                                                  // miss は fallback band (= BAND_KANJI)、 数字系 provider (band 950) との競合解決のため
        assert_eq!(candidates[0].score.band, BAND_KANJI);
    }

    #[test]
    fn alphabet_passthrough_provider_uses_lookup_when_hit() {
        let input = "APIサーバー";
        let mut lookup = HashMap::new();
        lookup.insert("api".to_string(), "エーピーアイ".to_string());
        let provider = AlphabetPassthroughProvider::new(input, Arc::new(lookup));
        let candidates = provider.candidates_vec(&ctx(input), 0);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].surface, "API");
        assert_eq!(candidates[0].reading, "エーピーアイ");
    }

    #[test]
    fn alphabet_with_trailing_symbol_uses_lookup_only_when_listed() {
        // C# は外来語表にあれば記号込みで引く。 表に無い X# は従来どおり英字だけ
        let input = "C#とX#";
        let mut lookup = HashMap::new();
        lookup.insert("c#".to_string(), "シーシャープ".to_string());
        let provider = AlphabetPassthroughProvider::new(input, Arc::new(lookup));
        let c = provider.candidates_vec(&ctx(input), 0);
        assert!(c
            .iter()
            .any(|x| x.surface == "C#" && x.reading == "シーシャープ"));
        let x = provider.candidates_vec(&ctx(input), "C#と".len());
        assert!(x.iter().all(|c| c.surface == "X"), "{x:?}");
    }

    #[test]
    fn alphabet_passthrough_normalizes_fullwidth_for_lookup() {
        let input = "ＡＰＩを使う";
        let mut lookup = HashMap::new();
        lookup.insert("api".to_string(), "エーピーアイ".to_string());
        let provider = AlphabetPassthroughProvider::new(input, Arc::new(lookup));
        let candidates = provider.candidates_vec(&ctx(input), 0);
        assert_eq!(candidates.len(), 1);
        // surface は full-width のまま、 normalize されるのは lookup key のみ
        assert_eq!(candidates[0].surface, "ＡＰＩ");
        assert_eq!(candidates[0].reading, "エーピーアイ");
    }

    #[test]
    fn alphabet_passthrough_returns_empty_at_non_alphabet_position() {
        let input = "APIサーバー";
        let provider = AlphabetPassthroughProvider::passthrough_only(input);
        // pos 3 は 「サ」 の start (= API の後)、 alphabet ではない
        assert!(provider.candidates_vec(&ctx(input), 3).is_empty());
    }

    #[test]
    fn alphabet_passthrough_no_alphabet_in_input() {
        let input = "ただの日本語";
        let provider = AlphabetPassthroughProvider::passthrough_only(input);
        assert!(provider.ranges().is_empty());
        assert!(provider.candidates_vec(&ctx(input), 0).is_empty());
    }

    // ─── 英字 + 数字 ([digits]) ────────────────────────────────────────────

    fn digits_lookup() -> Arc<HashMap<String, String>> {
        let mut m = HashMap::new();
        m.insert("windows".to_string(), "ウィンドウズ".to_string());
        m.insert(format!("{DIGITS_KEY_PREFIX}windows"), "english".to_string());
        m.insert("no".to_string(), "ノー".to_string());
        m.insert(
            format!("{DIGITS_KEY_PREFIX}no"),
            format!("english{DIGITS_SEP}ナンバー"),
        );
        m.insert("rtx".to_string(), "アールティーエックス".to_string());
        m.insert(format!("{DIGITS_KEY_PREFIX}rtx"), "each".to_string());
        m.insert("max".to_string(), "マックス".to_string());
        m.insert("5g".to_string(), "ファイブジー".to_string());
        Arc::new(m)
    }

    fn readings(input: &str) -> Vec<(String, String)> {
        let provider = AlphabetPassthroughProvider::new(input, digits_lookup());
        provider
            .candidates_vec(&ctx(input), 0)
            .into_iter()
            .map(|c| (c.surface.to_string(), c.reading.to_string()))
            .collect()
    }

    #[test]
    fn emoji_clusters_split_skin_tone_zwj_flag_and_keycap() {
        assert_eq!(emoji_clusters("😂😂"), ["😂", "😂"]);
        assert_eq!(emoji_clusters("👍\u{1F3FD}🙏"), ["👍\u{1F3FD}", "🙏"]);
        assert_eq!(emoji_clusters("⏰\u{FE0F}🔥"), ["⏰\u{FE0F}", "🔥"]);
        // ZWJ で繋いだ家族は 1 つ
        let family = "👨\u{200D}👩\u{200D}👧";
        assert_eq!(emoji_clusters(family), [family]);
        // 国旗は地域指示記号 2 つで 1 つ
        assert_eq!(emoji_clusters("🇯🇵🇺🇸"), ["🇯🇵", "🇺🇸"]);
        // keycap
        assert_eq!(
            emoji_clusters("1\u{FE0F}\u{20E3}2\u{FE0F}\u{20E3}"),
            ["1\u{FE0F}\u{20E3}", "2\u{FE0F}\u{20E3}"]
        );
    }

    #[test]
    fn emoji_reading_uses_table_with_fallbacks() {
        let mut t = HashMap::<String, EmojiEntry>::new();
        t.insert(
            "⏰".to_string(),
            EmojiEntry::Simple("めざましどけい|目覚まし|時計".to_string()),
        );
        t.insert("👍".to_string(), EmojiEntry::Simple("いいね".to_string()));
        t.insert("😂".to_string(), EmojiEntry::Simple(String::new()));
        t.insert("🍣".to_string(), EmojiEntry::Simple("!すし".to_string()));
        t.insert(
            "🍜".to_string(),
            EmojiEntry::Simple("ラーメン|麺".to_string()),
        );
        t.insert("🦀".to_string(), EmojiEntry::Simple("かに|蟹".to_string()));
        let wo = "を食べる";
        // VS16 / 肌色つきでも基本形で引く
        assert_eq!(
            emoji_reading("⏰\u{FE0F}", &t, "今日は", wo).as_deref(),
            Some("めざましどけい")
        );
        assert_eq!(
            emoji_reading("👍\u{1F3FD}", &t, "", wo).as_deref(),
            Some("いいね")
        );
        // 直後が助詞でも語でもなければ (文末・句読点・空白・数字・記号) 装飾 (None = 1 つも読みに変わらない)
        assert_eq!(emoji_reading("⏰", &t, "今日は", ""), None);
        assert_eq!(emoji_reading("⏰", &t, "今日は", "。"), None);
        assert_eq!(emoji_reading("⏰", &t, "今日は", " 続き"), None);
        assert_eq!(emoji_reading("⏰", &t, "今日は", "12時"), None);
        assert_eq!(emoji_reading("⏰", &t, "今日は", "✨"), None);
        // 語が続く時も読む (⚾見に行く / 🎧ながら)。 ただし行頭・空白・開き括弧の直後 (見出し) や、 直前が文末の形なら装飾
        assert_eq!(
            emoji_reading("⏰", &t, "今日は", "起きた").as_deref(),
            Some("めざましどけい")
        );
        assert_eq!(emoji_reading("⏰", &t, "", "起きた"), None);
        assert_eq!(emoji_reading("⏰", &t, "【", "起きた"), None);
        assert_eq!(
            emoji_reading("⏰", &t, "ありがとうございます", "今日は"),
            None
        );
        assert_eq!(emoji_reading("⏰", &t, "楽しかったね", "今日は"), None);
        // 助詞が続く時は行頭でも読む
        assert_eq!(
            emoji_reading("⏰", &t, "", "を止める").as_deref(),
            Some("めざましどけい")
        );
        // 先頭 ! は常に読む
        assert_eq!(emoji_reading("🍣", &t, "", "").as_deref(), Some("すし"));
        // 直前が名詞 (漢字・カタカナ・英数字・絵文字) なら、 その語に添えた装飾 (綴りに関係なく)
        assert_eq!(emoji_reading("🍜", &t, "ラーメン", wo), None);
        assert_eq!(emoji_reading("🍜", &t, "拉麺", wo), None);
        assert_eq!(emoji_reading("🍜", &t, "ramen", wo), None);
        assert_eq!(emoji_reading("🍜", &t, "3", wo), None);
        assert_eq!(emoji_reading("🍜", &t, "🍙", wo), None);
        // 直前が助詞・句読点・空白なら名詞として読む
        assert_eq!(
            emoji_reading("🍜", &t, "昼は", wo).as_deref(),
            Some("ラーメン")
        );
        assert_eq!(
            emoji_reading("🍜", &t, "昼、", wo).as_deref(),
            Some("ラーメン")
        );
        assert_eq!(
            emoji_reading("🍜", &t, "昼 ", wo).as_deref(),
            Some("ラーメン")
        );
        // 直前・直後の語が同じ読み・別表記なら装飾 (ひらがな・別表記)
        assert_eq!(emoji_reading("🍜", &t, "らーめん", wo), None);
        assert_eq!(emoji_reading("⏰", &t, "あさのめざましどけい", wo), None);
        assert_eq!(emoji_reading("🦀", &t, "今日は", "蟹座の人"), None);
        assert_eq!(emoji_reading("🦀", &t, "今日は", "カニ料理を"), None);
        // かな 1 文字の読み (き / は) は重複判定に使わない: 🌳 = き|木 が 「きょう」 の前で消えない
        t.insert("🌳".to_string(), EmojiEntry::Simple("き|木".to_string()));
        assert_eq!(
            emoji_reading("🌳", &t, "今日は", "の下").as_deref(),
            Some("き")
        );
        assert_eq!(
            emoji_reading("🌳", &t, "今日は", "きょうも").as_deref(),
            Some("き")
        );
        assert_eq!(emoji_reading("🌳", &t, "今日は", "木の下"), None);
        // 行末に同じ絵文字がもう一度ある挟み装飾
        assert_eq!(emoji_reading("🍜", &t, "昼は", "を食べた🍜"), None);
        // 空の読み = 常に読まない。 載っていない絵文字は表記のまま混ぜる
        assert_eq!(emoji_reading("😂😂", &t, "", "").as_deref(), Some(""));
        assert_eq!(emoji_reading("⏰🔥", &t, "", ""), None);
        assert_eq!(
            emoji_reading("⏰🔥😂", &t, "今日は", "を").as_deref(),
            Some("⏰🔥")
        );
        // 1 つも当たらない / 表が空なら None (表記のまま)
        assert_eq!(emoji_reading("🔥", &t, "今日は", wo), None);
        assert_eq!(
            emoji_reading("⏰", &HashMap::<String, EmojiEntry>::new(), "今日は", wo),
            None
        );
    }

    #[test]
    fn emoji_reading_follows_context_matches() {
        let data: crate::rules::EmojiData = toml::from_str(
            r#"
            [entries."🙏"]
            reading = ""
            [[entries."🙏".match]]
            alone = true
            reading = "ありがとう"
            [[entries."🙏".match]]
            prev_ends_any = ["お願い", "お願いします"]
            reading = ""

            # 既定の読みを省略 = 当たらない時は辞書に載っていないのと同じ (表記のまま)
            [entries."😭"]
            [[entries."😭".match]]
            alone = true
            reading = "うえーん"
            "#,
        )
        .unwrap();
        let t: HashMap<String, EmojiEntry> = data.entries.into_iter().collect();
        // 発言が絵文字だけなら読む
        assert_eq!(
            emoji_reading("🙏", &t, "", "").as_deref(),
            Some("ありがとう")
        );
        // 文に添えた時は既定 (読まない)。 match の reading が空でも 「読まない」 を明示する
        assert_eq!(
            emoji_reading("🙏", &t, "ありがとう", "").as_deref(),
            Some("")
        );
        assert_eq!(
            emoji_reading("🙏", &t, "よろしくお願い", "").as_deref(),
            Some("")
        );
        // 連なり (🙏🙏) は alone に当たらない → 既定の読まない
        assert_eq!(emoji_reading("🙏🙏", &t, "", "").as_deref(), Some(""));
        // 既定の読みが無い絵文字: 当たれば読む、 当たらなければ表記のまま (None)
        assert_eq!(emoji_reading("😭", &t, "", "").as_deref(), Some("うえーん"));
        assert_eq!(emoji_reading("😭", &t, "悲しい", ""), None);
    }

    #[test]
    fn emoji_run_is_extracted_as_one_token_including_joiners_and_misc_symbols() {
        // ⏰ (U+23F0) と VS16、 肌色、 国旗も絵文字の一部
        for emoji in ["⏰\u{FE0F}", "👍\u{1F3FD}", "🇯🇵", "1\u{FE0F}\u{20E3}", "⭐"] {
            let input = format!("あ{emoji}い");
            let tokens = extract_protected_tokens(&input);
            assert_eq!(tokens.len(), 1, "{emoji:?}");
            assert_eq!(tokens[0].kind, ProtectedKind::Emoji, "{emoji:?}");
            assert_eq!(&input[tokens[0].range.clone()], emoji, "{emoji:?}");
        }
        // 普通の数字は絵文字にならない
        assert!(extract_protected_tokens("123").is_empty());
    }

    #[test]
    fn kaomoji_in_parentheses_is_protected_with_empty_reading() {
        for kao in [
            "(´・ω・`)",
            "(;´Д`)",
            "(^o^)",
            "(≧▽≦)",
            "(T_T)",
            "ヽ(´ー｀)ノ",
            "＼(^o^)／",
            "(^^)/",
            "(^-^)v",
            "( ^ω^)",
            "( ̆ω ̆ )",
            "(* ́ω`*)",
            "ε=ε=(ノ≧∇≦)ノ",
            "ε=ε=ε=┌(;´Д`)┘",
        ] {
            let input = format!("あ{kao}い");
            let tokens = extract_protected_tokens(&input);
            assert_eq!(tokens.len(), 1, "{kao}");
            assert_eq!(tokens[0].kind, ProtectedKind::Kaomoji, "{kao}");
            assert_eq!(&input[tokens[0].range.clone()], kao, "{kao}");
        }
        let input = "(´・ω・`)";
        let provider = ProtectTokenProvider::new(input);
        let cands = provider.candidates_vec(&ctx(input), 0);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].reading, "");
    }

    #[test]
    fn parentheses_that_are_not_faces_are_left_alone() {
        for text in [
            "(笑)",
            "(株)",
            "(A・B)",
            "(1)",
            "(abc)",
            "(あ・い)",
            "(a_b_c)",
            "()",
        ] {
            let tokens = extract_protected_tokens(text);
            assert!(
                tokens.iter().all(|t| t.kind != ProtectedKind::Kaomoji),
                "{text}"
            );
        }
        // 中身が長すぎるものは顔ではない (15 文字)
        let long = format!("({})", "´".repeat(15));
        assert!(extract_protected_tokens(&long)
            .iter()
            .all(|t| t.kind != ProtectedKind::Kaomoji));
        // 14 文字までは顔
        let ok = format!("({})", "^".repeat(14));
        assert!(extract_protected_tokens(&ok)
            .iter()
            .any(|t| t.kind == ProtectedKind::Kaomoji));
    }

    #[test]
    fn html_entities_are_protected_as_silent_tokens() {
        for ent in [
            "&#8230;", "&#x2026;", "&amp;", "&lt;", "&gt;", "&quot;", "&nbsp;",
        ] {
            let input = format!("あ{ent}い");
            let tokens = extract_protected_tokens(&input);
            assert_eq!(tokens.len(), 1, "{ent}");
            assert_eq!(tokens[0].kind, ProtectedKind::Entity, "{ent}");
            assert_eq!(&input[tokens[0].range.clone()], ent);
        }
        // 実体参照ではない & は取らない
        for plain in ["A&B", "R&D", "&amp", "&#;", "&foo;", "a & b"] {
            assert!(
                extract_protected_tokens(plain)
                    .iter()
                    .all(|t| t.kind != ProtectedKind::Entity),
                "{plain}"
            );
        }
        // URL の中のものは URL が取る
        let url = "https://example.com/?a=1&amp;b=2";
        let tokens = extract_protected_tokens(url);
        assert!(tokens.iter().all(|t| t.kind != ProtectedKind::Entity));
    }

    #[test]
    fn bare_kaomoji_without_parentheses_is_protected() {
        for kao in [
            "^^",
            "^_^",
            "^o^",
            "^-^",
            "TT",
            "T_T",
            "T-T",
            ";_;",
            ";ω;",
            ">_<",
            ">ω<",
            "orz",
            "Orz",
            "XD",
            ":)",
            ":D",
            "(^^;",
            "(;^_^A",
            "(^-^;",
            "(^^ゞ",
            "∩^ω^∩",
            "∩´ω`∩",
            "∩・ω・∩",
            "=ω=",
            "=^ω^=",
            "＝ω＝",
            "∩^ω^",
        ] {
            let input = format!("あ{kao} い");
            let tokens = extract_protected_tokens(&input);
            assert_eq!(tokens.len(), 1, "{kao}");
            assert_eq!(tokens[0].kind, ProtectedKind::Kaomoji, "{kao}");
            assert_eq!(&input[tokens[0].range.clone()], kao, "{kao}");
        }
        // 引用符の目
        assert_eq!(extract_protected_tokens("あ( '-' )い").len(), 1);
        // 顔ではないもの: 累乗 / Tシャツ / 英単語の一部 / 英数字に挟まれた記号
        for text in [
            "2^3^4",
            "x^2^3",
            "T-Tシャツ",
            "Corzo",
            "box:D",
            "ab:)cd",
            "http://a.com/^^",
            "A∩B",
            "a=ωb",
        ] {
            assert!(
                extract_protected_tokens(text)
                    .iter()
                    .all(|t| t.kind != ProtectedKind::Kaomoji),
                "{text}"
            );
        }
    }

    #[test]
    fn url_is_not_replaced_by_kaomoji_reading() {
        let input = "https://a.com/(^o^)";
        let provider = ProtectTokenProvider::new(input);
        let cands = provider.candidates_vec(&ctx(input), 0);
        assert!(cands.iter().all(|c| !c.reading.is_empty()));
    }

    fn brand_lookup() -> Arc<HashMap<String, String>> {
        let mut m = HashMap::new();
        m.insert("hello".to_string(), "ハロー".to_string());
        m.insert("world".to_string(), "ワールド".to_string());
        m.insert("intel".to_string(), "インテル".to_string());
        m.insert("core".to_string(), "コア".to_string());
        Arc::new(m)
    }

    fn brand_readings(input: &str, starts: &[usize]) -> Vec<(String, String)> {
        let provider =
            AlphabetPassthroughProvider::new(input, brand_lookup()).split_at_model_starts(starts);
        provider
            .candidates_vec(&ctx(input), 0)
            .into_iter()
            .map(|c| (c.surface.to_string(), c.reading.to_string()))
            .collect()
    }

    #[test]
    fn multi_word_range_is_read_when_every_word_is_known() {
        assert_eq!(
            brand_readings("Hello World", &[]),
            vec![("Hello World".to_string(), "ハロー ワールド".to_string())]
        );
        // 1 語でも未登録なら英文とみなして素通し
        assert_eq!(
            brand_readings("Hello there", &[]),
            vec![("Hello there".to_string(), "Hello there".to_string())]
        );
        assert_eq!(
            brand_readings("foo bar", &[]),
            vec![("foo bar".to_string(), "foo bar".to_string())]
        );
    }

    #[test]
    fn range_before_a_model_reads_known_words_even_if_some_are_unknown() {
        // 型番の頭 (6) で range が切れ、 末尾の空白は読みにも残る
        assert_eq!(
            brand_readings("Foo Core i9", &[9]),
            vec![("Foo Core ".to_string(), "Foo コア ".to_string())]
        );
        assert_eq!(
            brand_readings("Intel i9", &[6]),
            vec![("Intel ".to_string(), "インテル ".to_string())]
        );
        // 1 語も知らなければそのまま
        assert_eq!(
            brand_readings("Foo Bar i9", &[8]),
            vec![("Foo Bar ".to_string(), "Foo Bar ".to_string())]
        );
    }

    #[test]
    fn split_at_model_starts_cuts_only_the_range_that_contains_the_start() {
        let input = "Hello i9";
        let ranges = |starts: &[usize]| {
            AlphabetPassthroughProvider::passthrough_only(input)
                .split_at_model_starts(starts)
                .ranges()
                .to_vec()
        };
        assert_eq!(ranges(&[]), vec![0..8]);
        assert_eq!(ranges(&[6]), vec![0..6]);
        // 先頭と同じ位置 / 終端と同じ位置 / 範囲外では切らない
        assert_eq!(ranges(&[0]), vec![0..8]);
        assert_eq!(ranges(&[8]), vec![0..8]);
        assert_eq!(ranges(&[20]), vec![0..8]);
        // 複数あれば最初のもの
        assert_eq!(ranges(&[7, 6]), vec![0..6]);
    }

    fn sentence_lookup() -> Arc<HashMap<String, String>> {
        let mut m = HashMap::new();
        for (k, v) in [
            ("thank", "サンク"),
            ("love", "ラブ"),
            ("game", "ゲーム"),
            ("over", "オーバー"),
            ("much", "マッチ"),
            ("no", "ノー"),
        ] {
            m.insert(k.to_string(), v.to_string());
        }
        for (k, v) in [
            ("you", "ユー"),
            ("i", "アイ"),
            ("so", "ソー"),
            ("to", "トゥ"),
            ("a", "ア"),
        ] {
            m.insert(format!("{SENTENCE_KEY_PREFIX}{k}"), v.to_string());
        }
        Arc::new(m)
    }

    fn sentence_readings(input: &str) -> Vec<(String, String)> {
        let provider = AlphabetPassthroughProvider::new(input, sentence_lookup());
        provider
            .candidates_vec(&ctx(input), 0)
            .into_iter()
            .map(|c| (c.surface.to_string(), c.reading.to_string()))
            .collect()
    }

    fn one(input: &str) -> String {
        let r = sentence_readings(input);
        assert_eq!(r.len(), 1, "{input}: {r:?}");
        r[0].1.clone()
    }

    #[test]
    fn sentence_words_are_used_only_inside_multi_word_english() {
        // 全部の語が辞書か英文の語表にあれば語ごとに読む
        assert_eq!(one("thank you so much"), "サンク ユー ソー マッチ");
        assert_eq!(one("I love you"), "アイ ラブ ユー");
        // 単独の so / to / a は引かない (従来どおり素通し)
        assert_eq!(one("so"), "so");
        assert_eq!(one("to"), "to");
        // 英文の語表に無い語が混じれば素通し
        assert_eq!(one("thank you very much"), "thank you very much");
    }

    #[test]
    fn concatenated_words_are_split_into_known_words() {
        assert_eq!(one("thankyou"), "サンクユー");
        assert_eq!(one("iloveyou"), "アイラブユー");
        assert_eq!(one("GameOver"), "ゲームオーバー");
        assert_eq!(one("gameover"), "ゲームオーバー");
        assert_eq!(one("iLoveYou"), "アイラブユー");
        assert_eq!(one("thankyousomuch"), "サンクユーソーマッチ");
    }

    #[test]
    fn concatenation_needs_every_part_known_and_a_real_word() {
        // 1 部分でも未知なら素通し
        assert_eq!(one("thankyoux"), "thankyoux");
        assert_eq!(one("gamexover"), "gamexover");
        // 英文の語だけ (辞書の語を含まない) は読まない
        assert_eq!(one("itisto"), "itisto");
        assert_eq!(one("youtoso"), "youtoso");
        // 1 語で辞書にある物は区切らず辞書どおり
        assert_eq!(one("thank"), "サンク");
    }

    #[test]
    fn short_parts_cannot_sit_next_to_each_other_or_stand_alone_mid_word() {
        // 固有名詞 Aniimo (an + i + imo …) は割らない: 短い部分どうしの隣り合わせ / 途中の 1 文字は不可
        assert_eq!(one("aniimo"), "aniimo");
        assert_eq!(one("soupto"), "soupto");
        // 先頭の i / a は 1 文字でも可 (iloveyou)。 途中の 1 文字は不可
        assert_eq!(one("iloveyou"), "アイラブユー");
        assert_eq!(one("aloveyou"), "アラブユー");
        assert_eq!(one("youiloveyou"), "youiloveyou");
        // 長い語をはさめば短い語は使える (thankyousomuch の so)
        assert_eq!(one("thankyousomuch"), "サンクユーソーマッチ");
        // 短い語が隣り合う区切り (so + to) は採用しない
        assert_eq!(one("lovesoto"), "lovesoto");
    }

    #[test]
    fn multi_word_needs_at_least_one_content_word() {
        // 機能語だけ (a a a a a / go up) は読まない
        assert_eq!(one("a a a a a"), "a a a a a");
        assert_eq!(one("up to you"), "up to you");
        // 辞書の語 (3 文字以上) が 1 つでもあれば読む
        assert_eq!(one("love you"), "ラブ ユー");
        assert_eq!(one("thank you"), "サンク ユー");
    }

    #[test]
    fn concatenation_skips_short_acronyms_digits_and_long_inputs() {
        // 5 文字以下は区切らない / 6 文字からは区切る
        assert_eq!(one("noyou"), "noyou");
        assert_eq!(one("noyous"), "noyous");
        assert_eq!(one("loveyou"), "ラブユー");
        // 全部大文字 (略語) は対象外
        assert_eq!(one("THANKYOU"), "THANKYOU");
        // 数字・記号まじりは対象外 (英字 range に数字が入る)
        assert_eq!(one("thankyou2"), "thankyou2");
        // 長すぎる入力は試さない
        let long = "thank".repeat(9);
        assert_eq!(one(&long), long);
        let max = "thank".repeat(8);
        assert_eq!(one(&max), "サンク".repeat(8));
    }

    #[test]
    fn word_break_prefers_fewest_parts() {
        let provider = AlphabetPassthroughProvider::new("x", sentence_lookup());
        let (r, parts, content) = provider.word_break("thankyou").unwrap();
        assert_eq!((r.as_str(), parts, content), ("サンクユー", 2, 1));
        // 3 字以上の辞書の語は内容語、 1〜2 字や英文の語は数えない
        let (_, _, content) = provider.word_break("iloveyou").unwrap();
        assert_eq!(content, 1);
        assert!(provider.word_break("zzzz").is_none());
        assert!(provider
            .word_break("")
            .is_none_or(|(r, p, _)| r.is_empty() && p == 0));
        // 辞書の 2 字の語 (no) は内容語として使わない: noyou は 英文の語表に no が無いので区切れない
        assert!(provider.word_break("noyou").is_none());
    }

    #[test]
    fn camel_chunks_split_after_lowercase_before_uppercase() {
        assert_eq!(camel_chunks("GameOver"), vec!["Game", "Over"]);
        assert_eq!(camel_chunks("iLoveYou"), vec!["i", "Love", "You"]);
        assert_eq!(camel_chunks("gameover"), vec!["gameover"]);
        assert_eq!(camel_chunks("ABC"), vec!["ABC"]);
        assert_eq!(camel_chunks("aB"), vec!["a", "B"]);
        assert_eq!(camel_chunks("Ab"), vec!["Ab"]);
        assert_eq!(camel_chunks(""), vec![""]);
    }

    #[test]
    fn word_reading_falls_back_to_sentence_words() {
        let provider = AlphabetPassthroughProvider::new("x", sentence_lookup());
        assert_eq!(provider.word_reading("thank"), Some("サンク"));
        assert_eq!(provider.word_reading("you"), Some("ユー"));
        assert_eq!(provider.word_reading("zzz"), None);
    }

    fn w_lookup() -> Arc<HashMap<String, String>> {
        let mut m = HashMap::new();
        m.insert("w".to_string(), "ワラ".to_string());
        m.insert("ww".to_string(), "ワラワラ".to_string());
        Arc::new(m)
    }

    fn w_reading(input: &str, pos: usize) -> String {
        let provider = AlphabetPassthroughProvider::new(input, w_lookup());
        provider
            .candidates_vec(&ctx(input), pos)
            .into_iter()
            .next()
            .map(|c| c.reading.to_string())
            .unwrap_or_default()
    }

    #[test]
    fn w_next_to_an_operator_is_the_letter() {
        assert_eq!(w_reading("w=2", 0), "ダブリュー");
        assert_eq!(w_reading("x+w", 2), "ダブリュー");
        assert_eq!(w_reading("a*w", 2), "ダブリュー");
        // 演算子が無ければ笑い
        assert_eq!(w_reading("それなw", "それな".len()), "ワラ");
    }

    #[test]
    fn single_w_before_a_particle_is_the_letter_not_laughter() {
        for p in ["は", "を", "が", "の", "も", "に", "で", "と"] {
            let input = format!("キーw{p}押す");
            assert_eq!(w_reading(&input, "キー".len()), "ダブリュー", "{input}");
        }
        assert_eq!(w_reading("W は小文字", 0), "ワラ");
        assert_eq!(w_reading("Wは小文字", 0), "ダブリュー");
        // 文頭 / 空白 / 括弧・引用符の後ろの小文字 w
        assert_eq!(w_reading("wは小文字", 0), "ダブリュー");
        assert_eq!(w_reading("これは wを押す", "これは ".len()), "ダブリュー");
        for open in ["「", "『", "(", "（", "【", "[", "\"", "“"] {
            let input = format!("{open}wを押す");
            assert_eq!(w_reading(&input, open.len()), "ダブリュー", "{input}");
        }
        // 大文字 W は何の後ろでも文字 (ガンダムWが)
        assert_eq!(w_reading("ガンダムWが", "ガンダム".len()), "ダブリュー");
    }

    #[test]
    fn w_after_japanese_text_is_laughter_even_before_a_particle() {
        // 仮名・漢字・片仮名の後ろの w は、 助詞が続いても笑い (実コメントで誤判定が出た形)
        for prefix in ["なぁ", "対応", "ウェイ", "ね", "草", "すごい"] {
            for p in ["と", "は", "を", "が", "の", "も", "に", "で"] {
                let input = format!("{prefix}w{p}続く");
                assert_eq!(w_reading(&input, prefix.len()), "ワラ", "{input}");
            }
        }
        // 小文字 w + 長音の後ろ (キーw) だけが文字名
        assert_eq!(w_reading("キーwを押す", "キー".len()), "ダブリュー");
    }

    #[test]
    fn w_at_the_end_or_repeated_stays_laughter() {
        assert_eq!(w_reading("ゆっくりw", "ゆっくり".len()), "ワラ");
        assert_eq!(w_reading("ゆっくりw。", "ゆっくり".len()), "ワラ");
        assert_eq!(w_reading("ゆっくりwwを", "ゆっくり".len()), "ワラワラ");
    }

    #[test]
    fn digits_word_glued_reads_word_and_number_as_one_candidate() {
        assert_eq!(
            readings("Windows11"),
            vec![("Windows11".to_string(), "ウィンドウズイレブン".to_string())]
        );
        assert_eq!(
            readings("RTX4090"),
            vec![(
                "RTX4090".to_string(),
                "アールティーエックスヨンゼロキュウゼロ".to_string()
            )]
        );
    }

    #[test]
    fn digits_word_not_listed_stays_passthrough() {
        // Max は [entries] にあるが [digits] には無い: Max5 を勝手に マックスファイブ にしない
        assert_eq!(
            readings("Max5"),
            vec![("Max5".to_string(), "Max5".to_string())]
        );
    }

    #[test]
    fn digits_word_with_separator_extends_over_the_number() {
        for (input, surface, reading) in [
            ("Windows 10", "Windows 10", "ウィンドウズテン"),
            ("Windows-11", "Windows-11", "ウィンドウズイレブン"),
            ("No.1", "No.1", "ナンバーワン"),
        ] {
            let got = readings(input);
            assert!(
                got.contains(&(surface.to_string(), reading.to_string())),
                "{input}: {got:?}"
            );
        }
    }

    #[test]
    fn digits_word_defers_to_decimal_and_version() {
        // 数字の直後が `.数字` なら [digits] の連結を作らない (iOS 17.2 / Ver2.0 は別に読む)
        assert!(!readings("Windows11.5")
            .iter()
            .any(|(_, r)| r == "ウィンドウズイレブン"));
        assert!(!readings("Windows 10.5")
            .iter()
            .any(|(s, _)| s == "Windows 10"));
    }

    #[test]
    fn digits_override_reading_applies_only_with_a_number() {
        // 単独の No は [entries] の ノー のまま
        assert_eq!(readings("No"), vec![("No".to_string(), "ノー".to_string())]);
    }

    #[test]
    fn letters_before_digits_gives_a_letters_only_candidate_before_japanese() {
        // SSR3体: 英字だけの passthrough も出し、 数字 + 助数詞を数値側に譲れるようにする
        let got = readings("SSR3体");
        assert!(got.contains(&("SSR3".to_string(), "SSR3".to_string())));
        assert!(got.contains(&("SSR".to_string(), "SSR".to_string())));
        // 日本語が続かない素の英数字には出さない
        let got = readings("SSR3");
        assert_eq!(got, vec![("SSR3".to_string(), "SSR3".to_string())]);
    }

    #[test]
    fn letters_before_version_uses_the_dictionary_reading() {
        // Max2.0: 数字の続きが `.数字` の時だけ 英字部分の辞書 reading を使う
        let got = readings("Max2.0");
        assert!(got.contains(&("Max".to_string(), "マックス".to_string())));
        // 助数詞向けの切り出し (日本語が続く) では辞書 reading を使わない
        let got = readings("Max5体");
        assert!(got.contains(&("Max".to_string(), "Max".to_string())));
    }

    #[test]
    fn dictionary_hit_wins_over_the_unit_skip_only_for_uppercase() {
        // 5G は SI 単位 g と紛れる: 大文字表記は辞書 (ファイブジー)、 小文字 5g は単位側に譲る
        let units: Arc<HashSet<String>> = Arc::new(["g".to_string()].into_iter().collect());
        let upper = AlphabetPassthroughProvider::with_units("5G", digits_lookup(), units.clone());
        assert_eq!(upper.candidates_vec(&ctx("5G"), 0).len(), 1);
        let lower = AlphabetPassthroughProvider::with_units("5g", digits_lookup(), units);
        assert!(lower.candidates_vec(&ctx("5g"), 0).is_empty());
    }

    #[test]
    fn letters_before_digits_only_for_letters_then_digits() {
        assert_eq!(letters_before_digits("SSR3"), Some(3));
        assert_eq!(letters_before_digits("Windows11"), Some(7));
        // 英字で始まらない / 数字だけ / 英字だけ / 数字の後に英字が続く形は対象外
        assert_eq!(letters_before_digits("123"), None);
        assert_eq!(letters_before_digits("5G"), None);
        assert_eq!(letters_before_digits("abc"), None);
        assert_eq!(letters_before_digits("H2A"), None);
    }
}
