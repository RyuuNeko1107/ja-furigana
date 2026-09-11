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
    Candidate, CandidateProvider, Score, ScoringContext, BAND_DICT_EXACT, BAND_KANJI,
    BAND_PROTECTED,
};
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
    fn candidates_at(&self, ctx: &ScoringContext, pos: usize) -> Vec<Candidate> {
        let mut out = Vec::new();
        for token in &self.tokens {
            if token.range.start == pos {
                let surface = &ctx.input[token.range.clone()];
                let char_count = surface.chars().count();
                let length = u8::try_from(char_count).unwrap_or(u8::MAX);
                out.push(Candidate::new(
                    surface.to_string(),
                    surface.to_string(), // reading = surface (passthrough)
                    token.range.clone(),
                    Score::new(BAND_PROTECTED, length, 0),
                ));
            }
        }
        out
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
        if is_alphabet_char(c) || joins_word {
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
    ranges
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
    fn candidates_at(&self, ctx: &ScoringContext, pos: usize) -> Vec<Candidate> {
        let mut out = Vec::new();
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

            // 「3km」 のような 数字 + 単位 は NumberCandidateProvider の責務。
            if self.is_number_with_unit(surface) {
                continue;
            }

            let char_count = surface.chars().count();
            let length = u8::try_from(char_count).unwrap_or(u8::MAX);

            let normalized = normalize_alphabet(surface);
            // 完全一致 → 連結記号除去 (Wi-Fi → wifi) の順で lookup。
            let hit = self.lookup.get(&normalized).or_else(|| {
                strip_alphabet_connectors(&normalized).and_then(|k| self.lookup.get(&k))
            });
            let (reading, band) = match hit {
                Some(r) => (r.clone(), BAND_DICT_EXACT),
                None => (surface.to_string(), BAND_KANJI), // passthrough miss は fallback band
            };

            out.push(Candidate::new(
                surface.to_string(),
                reading,
                range.clone(),
                Score::new(band, length, 0),
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scoring::boundary::BoundaryAnalysis;

    fn ctx(input: &str) -> ScoringContext<'_> {
        let boundary = Box::leak(Box::new(BoundaryAnalysis::empty()));
        ScoringContext { input, boundary }
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
        let candidates = provider.candidates_at(&ctx(input), url_start);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].surface, "https://example.com");
        assert_eq!(candidates[0].reading, "https://example.com"); // passthrough
        assert_eq!(candidates[0].score.band, BAND_PROTECTED);
    }

    #[test]
    fn provider_returns_empty_at_non_token_position() {
        let input = "foo https://example.com bar";
        let provider = ProtectTokenProvider::new(input);
        let candidates = provider.candidates_at(&ctx(input), 0);
        assert!(candidates.is_empty(), "pos 0 は URL の start ではない");
    }

    #[test]
    fn provider_emoji_passthrough_reading() {
        let input = "Hi😀";
        let provider = ProtectTokenProvider::new(input);
        let emoji_start = input.find('😀').unwrap();
        let candidates = provider.candidates_at(&ctx(input), emoji_start);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].surface, "😀");
        assert_eq!(candidates[0].reading, "😀"); // passthrough、 ひらがな化しない
    }

    #[test]
    fn provider_with_no_protected_tokens_yields_empty() {
        let input = "ただのテキスト";
        let provider = ProtectTokenProvider::new(input);
        assert!(provider.tokens().is_empty());
        assert!(provider.candidates_at(&ctx(input), 0).is_empty());
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
        let ranges = find_alphabet_ranges("A - B");
        assert_eq!(ranges, vec![0..2, 3..5]);
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
        let candidates = provider.candidates_at(&ctx(input), pos);
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
        let candidates = provider.candidates_at(&ctx(input), 0);
        assert_eq!(candidates[0].reading, "ワイファイ");
        assert_eq!(candidates[0].score.band, BAND_DICT_EXACT);
    }

    #[test]
    fn alphabet_passthrough_unknown_hyphenated_word_passes_through_whole() {
        let input = "Blu-ray";
        let provider = AlphabetPassthroughProvider::passthrough_only(input);
        let candidates = provider.candidates_at(&ctx(input), 0);
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
        let candidates = provider.candidates_at(&ctx(input), 0);
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
        let candidates = provider.candidates_at(&ctx(input), 0);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].surface, "API");
        assert_eq!(candidates[0].reading, "エーピーアイ");
    }

    #[test]
    fn alphabet_passthrough_normalizes_fullwidth_for_lookup() {
        let input = "ＡＰＩを使う";
        let mut lookup = HashMap::new();
        lookup.insert("api".to_string(), "エーピーアイ".to_string());
        let provider = AlphabetPassthroughProvider::new(input, Arc::new(lookup));
        let candidates = provider.candidates_at(&ctx(input), 0);
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
        assert!(provider.candidates_at(&ctx(input), 3).is_empty());
    }

    #[test]
    fn alphabet_passthrough_no_alphabet_in_input() {
        let input = "ただの日本語";
        let provider = AlphabetPassthroughProvider::passthrough_only(input);
        assert!(provider.ranges().is_empty());
        assert!(provider.candidates_at(&ctx(input), 0).is_empty());
    }
}
