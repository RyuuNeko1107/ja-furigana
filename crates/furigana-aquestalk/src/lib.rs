//! AquesTalk adapter — [`AccentResult`] を **本家 AquesTalk 音声記号列** に変換する。
//!
//! ADR-0001: engine 固有フォーマットは lib 本体でなく adapter crate が持つ。
//! 姉妹 crate の `ja-furigana-voicevox` は VOICEVOX の kana 記法 (AquesTalk-"風") 用で、
//! 本 crate は AquesTalk2 / AquesTalk10 (棒読みちゃん の内部エンジン等) に
//! そのまま渡せる記号列を出す。
//!
//! ## 記法 (VOICEVOX kana 記法との差分)
//!
//! | | AquesTalk (本 crate) | VOICEVOX kana |
//! |---|---|---|
//! | 疑問文 | 半角 `?` を **その疑問文の末尾** に (文ごと) | 全角 `？` を発話末に |
//! | pause | `、` (短) / `。` (長、 `。！` 由来) | `、` のみ |
//! | 無声化 | `_` を無声化する mora の**直前**に置く | 非対応 |
//! | 長さ | エンジン側に上限あり ([`MAX_LEN`] / [`split_for_aquestalk`]) | 実質無制限 |
//!
//! 共通点: アクセント句は `/` 区切り、 各句にアクセント核 `'` をちょうど 1 個
//! (平板型は句末に置く)、 カタカナ + `ー` のみで表記。
//!
//! ## 使い方 (ライブラリ組み込み)
//!
//! [`Converter`] が「テキスト → 音声記号列」の 1 stop entry。 解析器 ([`Furigana`])
//! の構築は呼び出し側が握るので、 辞書 dir や feature の指定はアプリ側の流儀で書ける。
//!
//! ```no_run
//! use ja_furigana_aquestalk::{Converter, Options};
//!
//! // accent 推定を有効にした解析器を組み立てて包む (dir 指定はアプリ側の自由)
//! let conv = Converter::new(
//!     ja_furigana_aquestalk::furigana::Furigana::builder()
//!         .rules_dir("dict/rules")
//!         .core_dict_dir("dict/core")
//!         .estimate_accent(true)
//!         .build()?,
//! );
//!
//! let symbols = conv.convert("今日は雨が降ります。");
//! // AquesTalk の合成 API (AquesTalk_Synthe 等) にそのまま渡せる
//!
//! // エンジン側の長さ上限に合わせて逐次合成したい場合
//! for chunk in conv.convert_chunks("長い文章…", ja_furigana_aquestalk::MAX_LEN) {
//!     // synthesize(&chunk);
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! 既に [`AccentResult`] を持っているなら [`to_aquestalk`] / [`to_aquestalk_with`] を直接
//! 呼べばよい (変換部は解析器に依存しない純粋な文字列処理)。
//!
//! 依存 crate の version ずれを避けるため、 本 crate は `furigana` 本体を
//! [`furigana`](crate::furigana) として re-export している。

/// 本 crate がリンクしている `ja-furigana` 本体の re-export。
///
/// アプリ側が別途 `ja-furigana` を依存に足すと version がずれて型が食い違いうるので、
/// 組み込み時はこちらを使うのが安全。
pub use furigana;

use furigana::{AccentResult, AccentToken, Furigana};

/// AquesTalk エンジンが 1 回の合成で受け取れる音声記号列のおおよその上限 (文字数)。
///
/// AquesTalk1/2 系の目安。 これを超える入力は [`split_for_aquestalk`] で
/// アクセント句境界ごとに分割して逐次合成すること。
pub const MAX_LEN: usize = 255;

/// 変換オプション。
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// 無声化記号 `_` を自動付与する (default: true)。
    ///
    /// 「無声子音 + イ/ウ」 の mora が 無声子音 mora の直前にある場合、 および
    /// 文末の 「デス」「マス」 の 「ス」 を無声化する。 アクセント核 mora と
    /// 連続無声化は避ける (安全側)。
    pub devoice: bool,
    /// 記号で終わっていない入力の末尾に `。` (長 pause) を補う (default: true)。
    ///
    /// 入力自体が `。` / `、` / `？` で終わっている場合は、 この設定に関係なく
    /// その記号 (`。` / `、` / `?`) がそのまま発話末に残る。
    pub trailing_period: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            devoice: true,
            trailing_period: true,
        }
    }
}

/// ひらがな → カタカナ (U+3041..U+3096 を +0x60 シフト)。
fn hira_to_kata(s: &str) -> String {
    s.chars()
        .map(|c| {
            if ('ぁ'..='ゖ').contains(&c) {
                char::from_u32(c as u32 + 0x60).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

fn is_katakana_or_prolonged(c: char) -> bool {
    matches!(c, 'ァ'..='ヶ' | 'ー')
}

fn is_small_kana(c: char) -> bool {
    matches!(c, 'ャ' | 'ュ' | 'ョ' | 'ァ' | 'ィ' | 'ゥ' | 'ェ' | 'ォ')
}

/// カタカナ文字列を mora 単位に分割 (拗音 / 小書き母音は直前と合算)。
fn mora_split(reading: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in reading.chars() {
        if is_small_kana(c) {
            if let Some(last) = out.last_mut() {
                last.push(c);
                continue;
            }
        }
        out.push(c.to_string());
    }
    out
}

/// mora の頭子音が無声子音 (カ / サ / タ / ハ / パ 行) か。
fn has_voiceless_onset(mora: &str) -> bool {
    matches!(
        mora.chars().next(),
        Some(
            'カ' | 'キ'
                | 'ク'
                | 'ケ'
                | 'コ'
                | 'サ'
                | 'シ'
                | 'ス'
                | 'セ'
                | 'ソ'
                | 'タ'
                | 'チ'
                | 'ツ'
                | 'テ'
                | 'ト'
                | 'ハ'
                | 'ヒ'
                | 'フ'
                | 'ヘ'
                | 'ホ'
                | 'パ'
                | 'ピ'
                | 'プ'
                | 'ペ'
                | 'ポ'
                | 'ッ'
        )
    )
}

/// 無声化しうる mora か (= 無声子音 + 狭母音 イ / ウ)。
fn is_devoiceable(mora: &str) -> bool {
    matches!(
        mora,
        "キ" | "ク"
            | "シ"
            | "ス"
            | "チ"
            | "ツ"
            | "ヒ"
            | "フ"
            | "ピ"
            | "プ"
            | "キュ"
            | "シュ"
            | "チュ"
            | "ヒュ"
            | "ピュ"
    )
}

/// 組み立て中の 1 accent phrase。
struct Phrase {
    morae: Vec<String>,
    /// `'` を置く mora 位置 (1-based)。 `None` = 平板/不明 (= 末尾 `'`)
    nucleus: Option<usize>,
}

impl Phrase {
    /// 核位置 (1-based)。 平板/不明や範囲外は句末に落とす。
    fn nucleus_pos(&self) -> usize {
        match self.nucleus {
            // AquesTalk も 0 型を直接は表現できないため 平板/不明は句末 `'`
            Some(p) if p >= 1 && p <= self.morae.len() => p,
            _ => self.morae.len(),
        }
    }

    /// `devoice` = 無声化記号 `_` の自動付与、 `utterance_end` = 発話末の句か。
    fn render(&self, devoice: bool, utterance_end: bool) -> String {
        let pos = self.nucleus_pos();
        let mut out = String::new();
        let mut prev_devoiced = false;
        for (i, m) in self.morae.iter().enumerate() {
            let is_nucleus = i + 1 == pos;
            if devoice && !prev_devoiced && !is_nucleus && self.should_devoice(i, utterance_end) {
                out.push('_');
                prev_devoiced = true;
            } else {
                prev_devoiced = false;
            }
            out.push_str(m);
            if is_nucleus {
                out.push('\'');
            }
        }
        out
    }

    fn should_devoice(&self, i: usize, utterance_end: bool) -> bool {
        let mora = self.morae[i].as_str();
        if !is_devoiceable(mora) {
            return false;
        }
        match self.morae.get(i + 1) {
            // 無声子音に挟まれた狭母音
            Some(next) => has_voiceless_onset(next),
            // 発話末の 「デス」「マス」 の ス
            None => {
                utterance_end
                    && mora == "ス"
                    && matches!(
                        self.morae.get(i.wrapping_sub(1)).map(String::as_str),
                        Some("デ" | "マ")
                    )
            }
        }
    }
}

/// アクセント句の区切り記号。 **直前の句の後ろ** に置かれる。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Sep {
    /// `/` — pause なし phrase 境界 (中黒 「・」 もこれ: 語中なので間を空けない)
    Slash,
    /// `、` — 短 pause (読点 / 空白由来)
    Comma,
    /// `。` — 長 pause (句点 / 感嘆符由来)
    Period,
    /// `?` — 疑問文の終わり (= 直前の文だけ尻上がり)
    Question,
}

impl Sep {
    fn symbol(self) -> char {
        match self {
            Sep::Slash => '/',
            Sep::Comma => '、',
            Sep::Period => '。',
            Sep::Question => '?',
        }
    }
}

#[derive(Default)]
struct Builder {
    rendered: Vec<(Sep, Phrase)>,
    open: Option<Phrase>,
    /// 直前に確定した句の後ろに置く区切り記号 (次の句の頭 or 発話末で render)。
    pending: Option<Sep>,
    opts: Options,
}

impl Builder {
    /// 組み立て中 phrase を確定 (空なら捨てる)。
    fn flush(&mut self) {
        if let Some(p) = self.open.take() {
            if !p.morae.is_empty() {
                let sep = self.pending.take().unwrap_or(Sep::Slash);
                self.rendered.push((sep, p));
            }
        }
    }

    /// 記号 token を受けて pending 区切りを更新する (強い方を優先)。
    fn add_separator(&mut self, sep: Sep) {
        self.flush();
        self.pending = Some(self.pending.map_or(sep, |cur| cur.max(sep)));
    }

    fn finish(mut self) -> String {
        self.flush();
        let last = self.rendered.len().saturating_sub(1);
        let mut out = String::new();
        for (i, (sep, phrase)) in self.rendered.iter().enumerate() {
            if i > 0 {
                out.push(sep.symbol());
            }
            out.push_str(&phrase.render(self.opts.devoice, i == last));
        }
        if out.is_empty() {
            return out;
        }
        match self.pending {
            // 入力末尾の記号は そのまま発話末の記号として残す
            // (`?` = 疑問文、 `。`/`、` = 文末 pause)
            Some(sep) => out.push(sep.symbol()),
            // 記号で終わっていない入力に `。` を補う (option)
            None if self.opts.trailing_period => out.push('。'),
            None => {}
        }
        out
    }
}

/// 記号 token の種別。 `None` = 記号ではない。
fn punctuation_sep(surface: &str) -> Option<Sep> {
    if surface.is_empty() {
        return None;
    }
    let mut sep = None;
    for c in surface.chars() {
        let s = match c {
            '？' | '?' => Sep::Question,
            '。' | '．' | '！' | '!' => Sep::Period,
            '、' | '，' | ',' | '…' | '‥' => Sep::Comma,
            // 中黒は 「ジョン・スミス」 のような語中区切り = pause を入れない
            '・' => Sep::Slash,
            c if c.is_whitespace() => Sep::Comma,
            _ => return None,
        };
        // 強い方 (= 長い pause / 疑問) を優先
        sep = Some(sep.map_or(s, |cur: Sep| cur.max(s)));
    }
    sep
}

fn token_phrases(token: &AccentToken) -> Vec<Phrase> {
    token
        .accent_phrases
        .iter()
        .filter_map(|ap| {
            // 記号列に出せない文字は落とす (parser の未定義文字エラー回避)
            let reading: String = hira_to_kata(&ap.reading)
                .chars()
                .filter(|c| is_katakana_or_prolonged(*c))
                .collect();
            let morae = mora_split(&reading);
            if morae.is_empty() {
                return None;
            }
            let nucleus = match ap.accent {
                Some(a) if a >= 1 => Some(a as usize),
                _ => None, // 平板 (0) / 不明 → 句末 '
            };
            Some(Phrase { morae, nucleus })
        })
        .collect()
}

/// [`AccentResult`] を AquesTalk 音声記号列に変換する ([`Options::default`])。
///
/// 各 phrase に `'` がちょうど 1 個・空 phrase なし・句頭 `'` なし、 という
/// AquesTalk parser のエラー条件を構造的に満たす。 読める token が 1 つも
/// なければ空文字列を返す (caller 側で skip すること)。
///
/// 長い入力はエンジン側の上限 ([`MAX_LEN`]) を超えうるので、
/// 必要に応じて [`split_for_aquestalk`] で分割すること。
#[must_use]
pub fn to_aquestalk(result: &AccentResult) -> String {
    to_aquestalk_with(result, Options::default())
}

/// [`to_aquestalk`] の option 指定版。
#[must_use]
pub fn to_aquestalk_with(result: &AccentResult, opts: Options) -> String {
    let mut b = Builder {
        opts,
        ..Builder::default()
    };

    for token in &result.tokens {
        if let Some(sep) = punctuation_sep(&token.surface) {
            b.add_separator(sep);
            continue;
        }

        // 助詞の発音変換: 音声記号列は音声表記なので は→ワ / へ→エ / を→オ。
        // 単独 token の場合のみ (語中の ハ/ヘ/ヲ は無関係)。 accent_phrases が
        // 付いていても 1 モーラの助詞なので、 変換した読みを優先して直前句へ連結する。
        let particle = match token.surface.as_str() {
            "は" => Some("ワ"),
            "へ" => Some("エ"),
            "を" => Some("オ"),
            _ => None,
        };
        let reading = match particle {
            Some(r) => r.to_string(),
            // 読める文字だけ残す (絵文字 / 記号混じりの token でも語の実体は落とさない)
            None => hira_to_kata(&token.reading)
                .chars()
                .filter(|c| is_katakana_or_prolonged(*c))
                .collect(),
        };
        if reading.is_empty() {
            // 読めない token (絵文字 / 記号 / URL / 英字 passthrough 等) は
            // 落として phrase 境界だけ切る
            b.flush();
            continue;
        }

        if particle.is_none() && !token.accent_phrases.is_empty() {
            // dict bracket / 推定由来の phrase 列をそのまま採用。
            // 末尾 phrase は open にして後続の助詞連結を受ける。
            b.flush();
            let mut phrases = token_phrases(token);
            if let Some(last) = phrases.pop() {
                for p in phrases {
                    b.open = Some(p);
                    b.flush();
                }
                b.open = Some(last);
            }
            continue;
        }

        let is_hiragana_function_word = !token.surface.is_empty()
            && token
                .surface
                .chars()
                .all(|c| matches!(c, 'ぁ'..='ゖ' | 'ー'));

        if is_hiragana_function_word {
            if let Some(p) = b.open.as_mut() {
                // 直前 phrase へ連結。 核確定済みなら位置維持 (ア'メガ)、
                // 平板/不明は None のまま = ' が句末へ動く (サカナガ')
                p.morae.extend(mora_split(&reading));
                continue;
            }
        }

        // accent 不明の content token → 自分の phrase (平板 fallback)
        b.flush();
        b.open = Some(Phrase {
            morae: mora_split(&reading),
            nucleus: None,
        });
    }

    b.finish()
}

/// テキスト → AquesTalk 音声記号列 の変換器 (ライブラリ組み込み用 facade)。
///
/// [`Furigana`] を 1 つ抱えて使い回す。 構築は重い (辞書 load) が、 変換自体は
/// `&self` で thread-safe に呼べるので、 アプリ側では 1 個を共有すればよい。
///
/// **accent 推定** (`FuriganaBuilder::estimate_accent(true)`) を有効にした解析器を
/// 渡すこと。 無効だと dict に bracket のある語しか accent が付かず、 残りは
/// 平板 fallback になる。
pub struct Converter {
    furigana: Furigana,
    options: Options,
}

impl Converter {
    /// 既存の解析器を包む ([`Options::default`])。
    #[must_use]
    pub fn new(furigana: Furigana) -> Self {
        Self {
            furigana,
            options: Options::default(),
        }
    }

    /// option 指定版。
    #[must_use]
    pub fn with_options(furigana: Furigana, options: Options) -> Self {
        Self { furigana, options }
    }

    /// 変換 option を差し替える。
    pub fn set_options(&mut self, options: Options) {
        self.options = options;
    }

    /// 現在の変換 option。
    #[must_use]
    pub fn options(&self) -> Options {
        self.options
    }

    /// 内側の解析器 (furigana / ruby など別用途に使いたい場合)。
    #[must_use]
    pub fn furigana(&self) -> &Furigana {
        &self.furigana
    }

    /// 解析器を取り出す (変換器を畳む)。
    #[must_use]
    pub fn into_furigana(self) -> Furigana {
        self.furigana
    }

    /// テキストを音声記号列へ変換する。
    ///
    /// 読める token が 1 つも無ければ空文字列を返す (合成を skip すること)。
    #[must_use]
    pub fn convert(&self, text: &str) -> String {
        to_aquestalk_with(&self.furigana.to_accent(text), self.options)
    }

    /// テキストを `max_len` 文字以下の音声記号列の列へ変換する (逐次合成用)。
    ///
    /// 分割はアクセント句境界でのみ行う。 上限の目安は [`MAX_LEN`]。
    #[must_use]
    pub fn convert_chunks(&self, text: &str, max_len: usize) -> Vec<String> {
        split_for_aquestalk(&self.convert(text), max_len)
    }
}

/// 音声記号列を `max_len` 文字以下の塊へ分割する (逐次合成用)。
///
/// 分割は **アクセント句境界** (`。` / `、` / `/`) でのみ行い、 pause の強い
/// 境界を優先する。 各塊は単体で合成できる (核 `'` を必ず含む) が、
/// 1 句だけで `max_len` を超える場合はその句をそのまま返す (分割不能)。
/// `max_len` は文字数 (char) で数え、 `0` は 「これ以上分けられない最小単位」
/// = 1 アクセント句ずつ の意味になる。
///
/// pause 記号 (`。` / `、` / `?`) は直前の句に属するものとして塊に残るので、
/// 分割しても文末や読点の間 (無音) が消えない。
#[must_use]
pub fn split_for_aquestalk(symbols: &str, max_len: usize) -> Vec<String> {
    if symbols.is_empty() {
        return Vec::new();
    }
    if max_len > 0 && symbols.chars().count() <= max_len {
        return vec![symbols.to_string()];
    }

    // (句, 直後の境界記号) へ分解。 pause 記号 (`。` / `、` / `?`) は **直前の句に属する**
    // ので句へ付けたまま運ぶ。 `/` は単なる句境界なので分割点では捨ててよい。
    let mut units: Vec<(String, Option<char>)> = Vec::new();
    let mut cur = String::new();
    for c in symbols.chars() {
        if matches!(c, '。' | '、' | '/' | '?') {
            if !cur.is_empty() {
                units.push((std::mem::take(&mut cur), Some(c)));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        units.push((cur, None));
    }

    let mut out: Vec<String> = Vec::new();
    let mut chunk = String::new();
    for (unit, sep) in units {
        // pause 記号は句に付けて運ぶ (分割しても文末/読点の間が消えない)。
        // `/` は句境界としてのみ意味を持つので chunk 内でだけ復元する。
        let tail = match sep {
            Some('/') | None => String::new(),
            Some(s) => s.to_string(),
        };
        // 直前の句が pause 記号で終わっているなら `/` は不要 (記号の二重付けを避ける)
        let needs_slash = !chunk.is_empty() && !chunk.ends_with(['。', '、', '?']);
        let add = unit.chars().count() + tail.chars().count() + usize::from(needs_slash);
        if !chunk.is_empty() && chunk.chars().count() + add > max_len {
            out.push(std::mem::take(&mut chunk));
        } else if needs_slash {
            chunk.push('/');
        }
        chunk.push_str(&unit);
        chunk.push_str(&tail);
    }
    if !chunk.is_empty() {
        out.push(chunk);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use furigana::Furigana;

    fn talk(f: &Furigana, input: &str) -> String {
        to_aquestalk(&f.to_accent(input))
    }

    /// 無声化のみ有効 (文末 `。` なし)。
    fn talk_devoice(f: &Furigana, input: &str) -> String {
        to_aquestalk_with(
            &f.to_accent(input),
            Options {
                devoice: true,
                trailing_period: false,
            },
        )
    }

    fn talk_plain(f: &Furigana, input: &str) -> String {
        to_aquestalk_with(
            &f.to_accent(input),
            Options {
                devoice: false,
                trailing_period: false,
            },
        )
    }

    #[test]
    fn atamadaka_with_particle_attached() {
        // 雨 = ア]メ (dict bracket) + が → ア'メガ
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        f.add_reading("降る", "フル");
        assert_eq!(talk_plain(&f, "雨が降る"), "ア'メガ/フル'");
    }

    #[test]
    fn heiban_particle_moves_apostrophe_to_end() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("魚", "サ[カナ");
        assert_eq!(talk_plain(&f, "魚が"), "サカナガ'");
    }

    #[test]
    fn odaka_keeps_nucleus_before_particle() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("花", "ハ[ナ]");
        assert_eq!(talk_plain(&f, "花が"), "ハナ'ガ");
    }

    #[test]
    fn trailing_period_appended_by_default() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        assert_eq!(talk(&f, "雨"), "ア'メ。");
    }

    #[test]
    fn comma_is_short_pause_and_period_is_long() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        f.add_reading("雪", "ユ]キ");
        f.add_reading("風", "カ[ゼ]");
        assert_eq!(talk_plain(&f, "雨、雪。風"), "ア'メ、ユ'キ。カゼ'");
    }

    #[test]
    fn question_uses_halfwidth_and_suppresses_period() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        assert_eq!(talk(&f, "雨？"), "ア'メ?");
    }

    #[test]
    fn devoicing_between_voiceless_consonants() {
        let mut f = Furigana::minimal().unwrap();
        // キ (無声子音+イ) が タ の直前 → _キ
        f.add_reading("北", "キ[タ]");
        assert_eq!(talk_devoice(&f, "北"), "_キタ'");
    }

    #[test]
    fn devoicing_skips_nucleus_mora() {
        let mut f = Furigana::minimal().unwrap();
        // 核が キ に乗るので無声化しない
        f.add_reading("菊", "キ]ク");
        assert_eq!(talk_devoice(&f, "菊"), "キ'ク");
    }

    #[test]
    fn devoicing_of_sentence_final_desu() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        f.add_reading("です", "デス");
        assert_eq!(talk(&f, "雨です"), "ア'メデ_ス。");
    }

    #[test]
    fn devoicing_can_be_disabled() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("北", "キ[タ]");
        assert_eq!(
            to_aquestalk_with(
                &f.to_accent("北"),
                Options {
                    devoice: false,
                    trailing_period: true
                }
            ),
            "キタ'。"
        );
    }

    #[test]
    fn multi_phrase_entry_splits() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("都立", "[トウキョウ][ト]リツ");
        assert_eq!(talk_plain(&f, "都立"), "トウキョウ'/ト'リツ");
    }

    #[test]
    fn unreadable_token_dropped() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        assert_eq!(talk_plain(&f, "雨🌧"), "ア'メ");
    }

    #[test]
    fn empty_input_yields_empty_string() {
        let f = Furigana::minimal().unwrap();
        assert_eq!(talk(&f, ""), "");
    }

    #[test]
    fn particle_ha_he_wo_phonetic() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        f.add_reading("見る", "ミル");
        assert_eq!(talk_plain(&f, "雨は"), "ア'メワ");
        assert_eq!(talk_plain(&f, "雨を見る"), "ア'メオ/ミル'");
    }

    #[test]
    fn split_keeps_phrases_intact_and_under_limit() {
        let symbols = "アメ'ガ/フル'。ユキ'モ/フル'。カゼ'モ/フク'。";
        let chunks = split_for_aquestalk(symbols, 20);
        assert!(chunks.len() > 1, "{chunks:?}");
        for c in &chunks {
            assert!(c.chars().count() <= 20, "{c:?}");
            assert!(!c.starts_with(['。', '、', '/']), "{c:?}");
            assert!(c.contains('\''), "{c:?}");
        }
        // 記号を除いた本文が保存されている
        let strip = |s: &str| s.replace(['。', '、', '/'], "");
        assert_eq!(
            strip(&chunks.join("")),
            strip(symbols),
            "content lost: {chunks:?}"
        );
    }

    #[test]
    fn question_is_per_sentence_not_global() {
        // 文中の `？` は その文だけ疑問符、 後続の平叙文は `。` で終わる
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        f.add_reading("雪", "ユ]キ");
        assert_eq!(talk(&f, "雨？雪。"), "ア'メ?ユ'キ。");
        assert_eq!(talk(&f, "雨。雪？"), "ア'メ。ユ'キ?");
    }

    #[test]
    fn trailing_input_punctuation_survives_without_trailing_period() {
        // trailing_period=false でも 入力由来の文末 pause は消さない
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        assert_eq!(talk_plain(&f, "雨。"), "ア'メ。");
        assert_eq!(talk_plain(&f, "雨？"), "ア'メ?");
        // 記号で終わらない入力にだけ `。` 補完の有無が効く
        assert_eq!(talk_plain(&f, "雨"), "ア'メ");
        assert_eq!(talk(&f, "雨"), "ア'メ。");
    }

    #[test]
    fn nakaguro_does_not_insert_pause() {
        // 「ジョン・スミス」 は 1 語なので中黒で間を空けない (句境界のみ)
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("甲", "コ]ウ");
        f.add_reading("乙", "オ]ツ");
        assert_eq!(talk_plain(&f, "甲・乙"), "コ'ウ/オ'ツ");
    }

    #[test]
    fn particle_phonetic_wins_over_accent_phrases() {
        // 助詞に accent_phrases が付いていても は→ワ を優先し直前句へ連結する
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        f.add_reading("は", "[ハ]");
        assert_eq!(talk_plain(&f, "雨は"), "ア'メワ");
    }

    #[test]
    fn converter_wraps_analyzer_and_options() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        f.add_reading("降る", "フル");

        let mut conv = Converter::new(f);
        assert_eq!(conv.convert("雨が降る"), "ア'メガ/フル'。");

        conv.set_options(Options {
            devoice: false,
            trailing_period: false,
        });
        assert_eq!(conv.convert("雨が降る"), "ア'メガ/フル'");
        assert!(!conv.options().devoice);

        // 内側の解析器はそのまま他用途にも使える
        assert_eq!(conv.furigana().to_hiragana("雨"), "あめ");
    }

    #[test]
    fn converter_chunks_respect_limit() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        f.add_reading("雪", "ユ]キ");
        f.add_reading("風", "カ[ゼ]");
        let conv = Converter::new(f);
        let chunks = conv.convert_chunks("雨、雪、風", 6);
        assert!(chunks.len() > 1, "{chunks:?}");
        for c in &chunks {
            assert!(c.chars().count() <= 6, "{c:?}");
        }
    }

    #[test]
    fn split_preserves_pause_symbols() {
        // 分割しても文末 `。` / 読点 `、` / 疑問 `?` が消えない
        let symbols = "アメ'ガ/フル'。ユキ'モ/フル'、カゼ'モ?";
        let chunks = split_for_aquestalk(symbols, 12);
        assert!(chunks.len() > 1, "{chunks:?}");
        assert!(chunks.last().unwrap().ends_with('?'), "{chunks:?}");
        let pauses = |s: &str| s.matches(['。', '、', '?']).count();
        assert_eq!(
            chunks.iter().map(|c| pauses(c)).sum::<usize>(),
            pauses(symbols),
            "pause 記号が落ちた: {chunks:?}"
        );
    }

    #[test]
    fn split_with_zero_max_len_yields_one_phrase_each() {
        // 0 = 「これ以上分けられない最小単位」 = 1 句ずつ (丸ごと 1 塊で返さない)
        let chunks = split_for_aquestalk("アメ'ガ/フル'。ユキ'モ", 0);
        assert_eq!(chunks, vec!["アメ'ガ", "フル'。", "ユキ'モ"]);
    }

    #[test]
    fn split_returns_input_when_short() {
        assert_eq!(split_for_aquestalk("ア'メ。", 255), vec!["ア'メ。"]);
        assert!(split_for_aquestalk("", 255).is_empty());
    }

    #[test]
    fn output_is_valid_aquestalk_symbols() {
        // AquesTalk parser のエラー条件 (核なし / 核 2 個 / 空句 / 未定義文字) を
        // 構造的に満たすことの property check
        let f = Furigana::builder().estimate_accent(true).build().unwrap();
        for input in [
            "今日は雨が降るかもしれない。",
            "カーテンとエレベーターを買った",
            "田中さんが来た！",
            "峠道、注意？",
            "私は学生です。",
            "元気？今日は雨が降ります。",
            "ジョン・スミスさんが来た",
        ] {
            let out = talk(&f, input);
            assert!(
                out.chars().all(|c| is_katakana_or_prolonged(c)
                    || matches!(c, '\'' | '_' | '/' | '、' | '。' | '?')),
                "undefined char in {out:?}"
            );
            for phrase in out
                .trim_end_matches(['。', '、', '?'])
                .split(['/', '、', '。', '?'])
            {
                assert!(!phrase.is_empty(), "empty phrase in {out:?}");
                assert_eq!(
                    phrase.matches('\'').count(),
                    1,
                    "phrase {phrase:?} in {out:?}"
                );
                assert!(!phrase.starts_with('\''), "leading ' in {out:?}");
                // `_` は必ず mora の直前 (末尾や記号の前には来ない)
                assert!(!phrase.ends_with('_'), "dangling _ in {out:?}");
            }
        }
    }
}
