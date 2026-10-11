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

    // URL
    for m in URL_RE.find_iter(input) {
        tokens.push(ProtectedToken {
            range: m.range(),
            kind: ProtectedKind::Url,
        });
    }

    // Email
    for m in EMAIL_RE.find_iter(input) {
        tokens.push(ProtectedToken {
            range: m.range(),
            kind: ProtectedKind::Email,
        });
    }

    // 絵文字 (連続範囲を 1 つの token に集約)
    let mut current_emoji_start: Option<usize> = None;
    let mut current_emoji_end: usize = 0;
    for (idx, c) in input.char_indices() {
        let char_end = idx + c.len_utf8();
        if is_emoji_char(c) {
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
}

impl ProtectTokenProvider {
    /// 入力 string を scan して保護 token を抽出、 provider を構築する。
    #[must_use]
    pub fn new(input: &str) -> Self {
        Self {
            tokens: extract_protected_tokens(input),
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
                out.push(RawCandidate::new(
                    surface, // reading = surface (passthrough)
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
        }
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
            };
            let (reading, band): (std::borrow::Cow<'_, str>, _) = match (hit, with_digits) {
                (Some(r), _) => (r.as_str().into(), BAND_DICT_EXACT),
                (None, Some(r)) => (r.into(), BAND_DICT_EXACT),
                (None, None) => (surface.into(), BAND_KANJI), // passthrough miss は fallback band
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
fn letters_before_digits(surface: &str) -> Option<usize> {
    let letters_len = surface.find(|c: char| !is_alphabet_letter(c))?;
    let digits = &surface[letters_len..];
    (letters_len > 0 && digits.chars().all(is_digit_char)).then_some(letters_len)
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
