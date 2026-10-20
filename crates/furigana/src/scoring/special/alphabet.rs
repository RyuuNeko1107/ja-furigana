//! 英字の lookup と passthrough ([`AlphabetPassthroughProvider`]) と、 その正規化

use crate::char_class::{self, is_digit_char};
use crate::scoring::candidate::{
    CandidateProvider, RawCandidate, Score, ScoringContext, BAND_DICT_EXACT, BAND_KANJI,
};
use crate::scoring::digit_reading::DigitStyle;
use std::collections::HashMap;
use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;

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
    pub(super) fn word_reading(&self, norm: &str) -> Option<&str> {
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
    pub(super) fn word_break(&self, s: &str) -> Option<(String, usize, usize)> {
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
        // ranges は start の昇順で重ならない (find_alphabet_ranges が走査順に積む)。 二分探索で pos の先頭へ飛ぶ
        let first = self.ranges.partition_point(|r| r.start < pos);
        for range in self.ranges[first..].iter().take_while(|r| r.start <= pos) {
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
pub(super) fn camel_chunks(s: &str) -> Vec<&str> {
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

pub(super) fn letters_before_digits(surface: &str) -> Option<usize> {
    let letters_len = surface.find(|c: char| !is_alphabet_letter(c))?;
    let digits = &surface[letters_len..];
    (letters_len > 0 && digits.chars().all(is_digit_char)).then_some(letters_len)
}
