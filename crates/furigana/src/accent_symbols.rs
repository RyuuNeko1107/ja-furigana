//! TTS engine 向け「音声記号列」 adapter の共有コア。
//!
//! AquesTalk 系の記号列 (VOICEVOX の kana 記法もこの系統) は、 どの engine でも
//!
//! - カタカナ + 長音符で読みを書く
//! - アクセント句に分ける
//! - 句ごとにアクセント核の位置を 1 つ持つ
//! - 句読点で間 (pause) を入れる
//!
//! という骨格が共通で、 違うのは **記号の綴り方** (`'` の置き方 / pause 記号 /
//! 疑問符 / 無声化記号の有無) だけ。 その共通部分 —— [`AccentResult`] を
//! 「モーラ列 + 核位置 + 区切りの強さ」 へ落とす所 —— をここに置き、
//! engine 固有の render は adapter crate 側 (ADR-0001) に任せる。
//!
//! ```
//! use furigana::accent_symbols::{to_mora_phrases, PhraseBreak};
//! # use furigana::Furigana;
//! # let mut f = Furigana::minimal().unwrap();
//! # f.add_reading("雨", "ア]メ");
//! let symbols = to_mora_phrases(&f.to_accent("雨が"));
//! assert_eq!(symbols.phrases[0].morae, ["ア", "メ", "ガ"]);
//! assert_eq!(symbols.phrases[0].nucleus, Some(1));
//! assert_eq!(symbols.trailing, PhraseBreak::None);
//! ```

use crate::api::{AccentResult, AccentToken};
use crate::kana::hira_to_kata;

/// アクセント句の区切りの強さ。 engine 固有の記号への写像は adapter が持つ。
///
/// 順序は 「弱い < 強い」 で、 記号 token が連続したときは強い方を採る。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum PhraseBreak {
    /// pause なしの句境界 (中黒 「・」 もこれ: 語中なので間を空けない)
    #[default]
    None,
    /// 短い pause (読点 / 空白由来)
    Short,
    /// 長い pause (句点 / 感嘆符由来)
    Long,
    /// 疑問文の終わり (= その文だけ尻上がり)
    Question,
}

/// 1 アクセント句 (モーラ列 + 核位置)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoraPhrase {
    /// モーラ列 (拗音 / 小書き母音は直前と合算済み、 カタカナ + `ー` のみ)
    pub morae: Vec<String>,
    /// アクセント核のモーラ位置 (1-based)。 `None` = 平板 (0 型) または accent 不明。
    ///
    /// AquesTalk 系記法は 0 型を直接表現できないので、 adapter は通常 `None` を
    /// 句末の核として render する。
    pub nucleus: Option<usize>,
    /// **この句の前** に置く区切り (先頭句は常に [`PhraseBreak::None`])
    pub break_before: PhraseBreak,
}

impl MoraPhrase {
    /// 核のモーラ位置 (1-based)。 平板 / 不明 / 範囲外は句末に落とす。
    ///
    /// 空の句には核を置けないので `0` を返す (adapter 側で空句は作らないこと)。
    #[must_use]
    pub fn nucleus_pos(&self) -> usize {
        match self.nucleus {
            Some(p) if p >= 1 && p <= self.morae.len() => p,
            _ => self.morae.len(),
        }
    }
}

/// [`to_mora_phrases`] の戻り値。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MoraPhrases {
    /// アクセント句列 (空句は含まない)。 読める token が無ければ空。
    pub phrases: Vec<MoraPhrase>,
    /// 発話末に残った区切り (= 入力が記号で終わっていた場合)。
    pub trailing: PhraseBreak,
}

/// カタカナ文字列をモーラ単位に分割する (拗音 / 小書き母音は直前と合算)。
#[must_use]
pub fn mora_split(reading: &str) -> Vec<String> {
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

/// 記号列に書ける文字 (カタカナ + 長音符) か。
#[must_use]
pub fn is_symbol_kana(c: char) -> bool {
    matches!(c, 'ァ'..='ヶ' | 'ー')
}

fn is_small_kana(c: char) -> bool {
    matches!(c, 'ャ' | 'ュ' | 'ョ' | 'ァ' | 'ィ' | 'ゥ' | 'ェ' | 'ォ')
}

/// 記号 token の区切り種別。 `None` = 記号 token ではない。
fn token_break(surface: &str) -> Option<PhraseBreak> {
    if surface.is_empty() {
        return None;
    }
    let mut brk: Option<PhraseBreak> = None;
    for c in surface.chars() {
        let b = match c {
            '？' | '?' => PhraseBreak::Question,
            '。' | '．' | '！' | '!' => PhraseBreak::Long,
            '、' | '，' | ',' | '…' | '‥' => PhraseBreak::Short,
            // 中黒は 「ジョン・スミス」 のような語中区切り = pause を入れない
            '・' => PhraseBreak::None,
            c if c.is_whitespace() => PhraseBreak::Short,
            _ => return None,
        };
        brk = Some(brk.map_or(b, |cur| cur.max(b)));
    }
    brk
}

/// 読みの無い token の並び (句読点・空白・記号) から、 句の切れ目の強さを決める (強い方を採る。 該当する記号が無ければ `None`)
fn gap_break(gap: &str) -> PhraseBreak {
    gap.chars()
        .filter_map(|c| token_break(c.encode_utf8(&mut [0; 4])))
        .max()
        .unwrap_or(PhraseBreak::None)
}

/// 連結規則で組み立てたアクセント句 ([`crate::Furigana::to_accent_phrases`]) を 記号列の骨格へ落とす。
///
/// `trailing_gap` = 最後の句の後ろに残った読みの無い token (入力末の `。` や `？` など)。
/// 句の核は OpenJTalk と同じ連結規則で決まっているので、 [`to_mora_phrases`] にある ます / です の個別規則は要らない。
/// 句と句の間 (`gap_before`) に英字のまま残った語があれば、 `opts` に従って読むか落とす。
#[must_use]
pub(crate) fn chained_to_mora_phrases(
    phrases: &[crate::chain_phrases::ChainedPhrase],
    trailing_gap: &str,
    opts: SymbolOptions,
) -> SymbolOutput {
    let mut out = MoraPhrases::default();
    let mut unknown = Vec::new();
    let mut pending = PhraseBreak::None;
    let mut gap = |text: &str, out: &mut MoraPhrases, pending: &mut PhraseBreak| {
        for (is_alpha, seg) in alpha_split(text) {
            if !is_alpha {
                *pending = (*pending).max(gap_break(&seg));
                continue;
            }
            let reading = alpha_reading(&seg, opts.unknown_alpha);
            unknown.push(seg);
            let Some(r) = reading else { continue };
            let morae = mora_split(&r);
            if morae.is_empty() {
                continue;
            }
            let brk = if out.phrases.is_empty() {
                PhraseBreak::None
            } else {
                std::mem::take(pending)
            };
            *pending = PhraseBreak::None;
            out.phrases.push(MoraPhrase {
                morae,
                nucleus: None,
                break_before: brk,
            });
        }
    };
    for p in phrases {
        gap(&p.gap_before, &mut out, &mut pending);
        let morae = mora_split(&symbol_reading(&p.sound));
        if morae.is_empty() {
            continue;
        }
        let brk = if out.phrases.is_empty() {
            PhraseBreak::None
        } else {
            std::mem::take(&mut pending)
        };
        pending = PhraseBreak::None;
        out.phrases.push(MoraPhrase {
            nucleus: (p.accent > 0).then_some(p.accent),
            morae,
            break_before: brk,
        });
    }
    gap(trailing_gap, &mut out, &mut pending);
    out.trailing = pending;
    SymbolOutput {
        phrases: out,
        unknown,
    }
}

// ─── 辞書に無い英字語 ───────────────────────────────────────────────────────

/// 辞書に無い英字語 (`AviUtl` など、 読みが英字のまま残った語) の扱い。
///
/// AquesTalk 系の記号列はカタカナしか出せないので、 英字のままの語は 記号列からは落ちる (従来の挙動 = [`Drop`](Self::Drop))。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum UnknownAlpha {
    /// 落とす (従来どおり)。 読めなかった語は [`SymbolOutput::unknown`] で分かる
    #[default]
    Drop,
    /// 1 文字ずつ英字の名前で読む (`AviUtl` → エービーアイユーティーエル)
    Spell,
    /// ローマ字として読む簡易の読み (`AviUtl` → アビウトル)。 **実験的**: 呼び方が割れる語では外れる
    Romaji,
}

/// 記号列の組み立ての option。
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct SymbolOptions {
    /// 辞書に無い英字語の扱い
    pub unknown_alpha: UnknownAlpha,
}

/// [`SymbolOptions`] 付きの組み立ての結果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SymbolOutput {
    pub phrases: MoraPhrases,
    /// 読めずに英字のまま残った語 (出現順。 [`UnknownAlpha`] の設定に関わらず並ぶ)
    pub unknown: Vec<String>,
}

/// 英字の名前 (A〜Z) で 1 文字ずつ読む。
fn spell_word(word: &str) -> String {
    word.chars()
        .filter(char::is_ascii_alphabetic)
        .map(|c| {
            crate::scoring::model_number::LETTER_NAMES
                [usize::from(c.to_ascii_uppercase() as u8 - b'A')]
        })
        .collect()
}

/// ローマ字として読んだ簡易のカタカナ (実験的)。
fn romaji_word(word: &str) -> String {
    const ROWS: [(&str, [&str; 5]); 20] = [
        ("k", ["カ", "キ", "ク", "ケ", "コ"]),
        ("c", ["カ", "キ", "ク", "ケ", "コ"]),
        ("s", ["サ", "シ", "ス", "セ", "ソ"]),
        ("t", ["タ", "ティ", "トゥ", "テ", "ト"]),
        ("n", ["ナ", "ニ", "ヌ", "ネ", "ノ"]),
        ("h", ["ハ", "ヒ", "フ", "ヘ", "ホ"]),
        ("m", ["マ", "ミ", "ム", "メ", "モ"]),
        ("r", ["ラ", "リ", "ル", "レ", "ロ"]),
        ("g", ["ガ", "ギ", "グ", "ゲ", "ゴ"]),
        ("z", ["ザ", "ジ", "ズ", "ゼ", "ゾ"]),
        ("d", ["ダ", "ディ", "ドゥ", "デ", "ド"]),
        ("b", ["バ", "ビ", "ブ", "ベ", "ボ"]),
        ("p", ["パ", "ピ", "プ", "ペ", "ポ"]),
        ("j", ["ジャ", "ジ", "ジュ", "ジェ", "ジョ"]),
        ("f", ["ファ", "フィ", "フ", "フェ", "フォ"]),
        ("v", ["バ", "ビ", "ブ", "ベ", "ボ"]),
        ("sh", ["シャ", "シ", "シュ", "シェ", "ショ"]),
        ("ch", ["チャ", "チ", "チュ", "チェ", "チョ"]),
        ("ts", ["ツァ", "ツィ", "ツ", "ツェ", "ツォ"]),
        ("w", ["ワ", "ウィ", "ウ", "ウェ", "ウォ"]),
    ];
    const SMALL: [(&str, &str); 3] = [("a", "ャ"), ("u", "ュ"), ("o", "ョ")];
    let vowel_idx = |c: char| "aiueo".find(c);
    let w: Vec<char> = word
        .chars()
        .filter(char::is_ascii_alphabetic)
        .map(|c| c.to_ascii_lowercase())
        .map(|c| match c {
            'l' => 'r',
            'q' => 'k',
            'x' => 's',
            other => other,
        })
        .collect();
    let mut out = String::new();
    let mut i = 0;
    while i < w.len() {
        let c = w[i];
        if let Some(v) = vowel_idx(c) {
            out.push_str(["ア", "イ", "ウ", "エ", "オ"][v]);
            i += 1;
            continue;
        }
        let next = w.get(i + 1).copied();
        // 促音: 同じ子音の連続 (n は除く)
        if c != 'n' && next == Some(c) {
            out.push('ッ');
            i += 1;
            continue;
        }
        // 子音の綴り (sh ch ts を優先)
        let two: String = w[i..(i + 2).min(w.len())].iter().collect();
        let key: String = if matches!(two.as_str(), "sh" | "ch" | "ts") {
            two
        } else {
            c.to_string()
        };
        let after = i + key.chars().count();
        let nv = w.get(after).copied();
        // m は b p の前で ン (shimbun)。 n は母音・y の前以外で ン
        if key == "m" && matches!(nv, Some('b' | 'p')) {
            out.push('ン');
            i = after;
            continue;
        }
        if key == "n" && !nv.is_some_and(|x| vowel_idx(x).is_some() || x == 'y') {
            out.push('ン');
            i = after;
            continue;
        }
        // 子音 + y + (a u o) = 拗音 (キャ など)。 y 単独は ヤ ユ ヨ
        if key != "y" && nv == Some('y') {
            if let Some(&(_, small)) = w
                .get(after + 1)
                .and_then(|&x| SMALL.iter().find(|(s, _)| s.starts_with(x)))
            {
                if let Some((_, row)) = ROWS.iter().find(|(k, _)| *k == key) {
                    let base = row[1];
                    // ジ チ シ はそのまま小書きを足さない (ジャ ではなく 行の値を使う)
                    if matches!(key.as_str(), "j" | "sh" | "ch") {
                        out.push_str(
                            row[SMALL
                                .iter()
                                .position(|(_, s)| *s == small)
                                .map_or(0, |p| [0, 2, 4][p])],
                        );
                    } else {
                        out.push_str(base);
                        out.push_str(small);
                    }
                    i = after + 2;
                    continue;
                }
            }
        }
        if key == "y" {
            let ya = nv.and_then(|x| match x {
                'a' => Some("ヤ"),
                'u' => Some("ユ"),
                'o' => Some("ヨ"),
                _ => None,
            });
            if let Some(k) = ya {
                out.push_str(k);
                i = after + 1;
            } else {
                out.push('イ');
                i = after;
            }
            continue;
        }
        let Some((_, row)) = ROWS.iter().find(|(k, _)| *k == key) else {
            i = after;
            continue;
        };
        if let Some(v) = nv.and_then(vowel_idx) {
            out.push_str(row[v]);
            i = after + 1;
        } else {
            // 母音が続かない子音は 母音を補う (t d は オ、 ch sh は イ、 他は ウ)
            out.push_str(match key.as_str() {
                "t" | "d" => row[4],
                "ch" | "sh" => row[1],
                _ => row[2],
            });
            i = after;
        }
    }
    out
}

/// 英字語の読み (`Drop` なら `None`)。
fn alpha_reading(word: &str, mode: UnknownAlpha) -> Option<String> {
    let r = match mode {
        UnknownAlpha::Drop => return None,
        UnknownAlpha::Spell => spell_word(word),
        UnknownAlpha::Romaji => romaji_word(word),
    };
    (!r.is_empty()).then_some(r)
}

/// 文字列の中の 英字の連なり (ASCII の英字だけ。 数字は含めない) と、 それ以外の文字を順に返す。
fn alpha_split(text: &str) -> Vec<(bool, String)> {
    let mut out: Vec<(bool, String)> = Vec::new();
    for c in text.chars() {
        let is_alpha = c.is_ascii_alphabetic();
        match out.last_mut() {
            Some((a, s)) if *a == is_alpha => s.push(c),
            _ => out.push((is_alpha, c.to_string())),
        }
    }
    out
}

/// 記号列に出せない文字を落としたカタカナ読み。
fn symbol_reading(reading: &str) -> String {
    hira_to_kata(reading)
        .chars()
        .filter(|c| is_symbol_kana(*c))
        .collect()
}

fn token_phrases(token: &AccentToken) -> Vec<MoraPhrase> {
    token
        .accent_phrases
        .iter()
        .filter_map(|ap| {
            let morae = mora_split(&symbol_reading(&ap.reading));
            if morae.is_empty() {
                return None;
            }
            Some(MoraPhrase {
                morae,
                nucleus: match ap.accent {
                    Some(a) if a >= 1 => Some(a as usize),
                    _ => None, // 平板 (0) / 不明
                },
                break_before: PhraseBreak::None,
            })
        })
        .collect()
}

/// 組み立て中の状態。
#[derive(Default)]
struct Builder {
    out: MoraPhrases,
    open: Option<MoraPhrase>,
    /// 直前に確定した句の後ろに置く区切り (次の句の `break_before` になる)
    pending: Option<PhraseBreak>,
}

impl Builder {
    fn flush(&mut self) {
        if let Some(mut p) = self.open.take() {
            if !p.morae.is_empty() {
                p.break_before = self.pending.take().unwrap_or(PhraseBreak::None);
                self.out.phrases.push(p);
            }
        }
    }

    fn add_break(&mut self, brk: PhraseBreak) {
        self.flush();
        self.pending = Some(self.pending.map_or(brk, |cur| cur.max(brk)));
    }

    fn open_phrase(&mut self, p: MoraPhrase) {
        self.flush();
        self.open = Some(p);
    }

    fn finish(mut self) -> MoraPhrases {
        self.flush();
        self.out.trailing = self.pending.unwrap_or(PhraseBreak::None);
        self.out
    }
}

/// 丁寧の ます 系 token の核が、 連結先の句末から何モーラ目に来るか (ます / まし = マ、 ませ / ましょ = セ / ショ)
fn masu_nucleus_offset(surface: &str) -> Option<usize> {
    match surface {
        "ます" | "まし" => Some(1),
        "ませ" | "ましょ" => Some(2),
        _ => None,
    }
}

/// 断定の です 系 token の核が、 連結先の句末から何モーラ目に来るか (です / でし = デ、 でしょ = ショ)
fn desu_nucleus_offset(surface: &str) -> Option<usize> {
    match surface {
        "です" | "でし" => Some(1),
        "でしょ" => Some(2),
        _ => None,
    }
}

/// モーラがイ段 / エ段で終わるか (動詞の連用形の末尾)
fn ends_with_ie_row(mora: &str) -> bool {
    mora.chars()
        .next_back()
        .is_some_and(|c| "イキギシジチヂニヒビピミリエケゲセゼテデネヘベペメレ".contains(c))
}

/// [`AccentResult`] を 「モーラ列 + 核位置 + 区切り」 の列へ落とす。
///
/// 変換規則:
///
/// - accent 情報を持つ token は [`AccentPhrase`](crate::AccentPhrase) ごとに 1 句
/// - accent 不明の内容語は平板 fallback で 1 句
/// - ひらがな surface の token (助詞 / 助動詞 / 送り仮名) は直前の句に連結する
///   (`雨が` → 1 句 `アメガ`、 核位置は維持)
/// - 助詞 `は` / `へ` / `を` は音声表記へ (ワ / エ / オ)
/// - 読めない token (絵文字 / 記号 / URL / 英字 passthrough 等) は落として句境界だけ切る
/// - 句読点は [`PhraseBreak`] へ (文単位。 入力末尾の記号は
///   [`MoraPhrases::trailing`] に残る)
///
/// 空の句は作らないので、 adapter は 「各句に核を 1 つ」 を無条件に満たせる。
#[must_use]
pub fn to_mora_phrases(result: &AccentResult) -> MoraPhrases {
    to_mora_phrases_with(result, SymbolOptions::default()).phrases
}

/// [`to_mora_phrases`] の option 付き版。 辞書に無い英字語の扱い ([`UnknownAlpha`]) を選べ、 読めなかった語を返す。
#[must_use]
pub fn to_mora_phrases_with(result: &AccentResult, opts: SymbolOptions) -> SymbolOutput {
    let mut b = Builder::default();
    let mut unknown: Vec<String> = Vec::new();

    for token in &result.tokens {
        if let Some(brk) = token_break(&token.surface) {
            b.add_break(brk);
            continue;
        }

        // 助詞の発音変換: 記号列は音声表記なので は→ワ / へ→エ / を→オ。
        // 単独 token の場合のみ (語中の ハ/ヘ/ヲ は無関係)。 accent_phrases が
        // 付いていても 1 モーラの助詞なので、 変換した読みを優先する。
        let particle = match token.surface.as_str() {
            "は" => Some("ワ"),
            "へ" => Some("エ"),
            "を" => Some("オ"),
            _ => None,
        };
        let reading = match particle {
            Some(r) => r.to_string(),
            None => symbol_reading(&token.reading),
        };
        if reading.is_empty() {
            // 読みが英字のまま残った語 (辞書に無い英単語) は 記号列に出せない
            for (is_alpha, seg) in alpha_split(&token.reading) {
                if !is_alpha {
                    continue;
                }
                let r = alpha_reading(&seg, opts.unknown_alpha);
                unknown.push(seg);
                if let Some(r) = r {
                    b.open_phrase(MoraPhrase {
                        morae: mora_split(&r),
                        nucleus: None,
                        break_before: PhraseBreak::None,
                    });
                }
            }
            b.flush();
            continue;
        }

        if particle.is_none() && !token.accent_phrases.is_empty() {
            // dict bracket / 推定由来の句列をそのまま採用。
            // 末尾句は open にして後続の助詞連結を受ける。
            let mut phrases = token_phrases(token);
            if let Some(last) = phrases.pop() {
                for p in phrases {
                    b.open_phrase(p);
                }
                b.open_phrase(last);
            } else {
                b.flush();
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
                // 直前句へ連結。 核確定済みなら位置維持 (アメ + ガ で核は 1 のまま)、
                // 平板 / 不明は `None` のままなので核が句末へ動く
                let before = p.morae.len();
                // 丁寧の ます は動詞の型によらず ます 側に核 (東京式、 2026-09-28):
                // 食べ + ます = タベマ'ス / 行き + まし + た = イキマ'シタ / 食べ + ませ + ん = タベマセ'ン /
                // 行き + ましょ + う = イキマショ'ー。 直前がイ段 / エ段 (動詞の連用形) の時だけ
                // (「こっちが + まし」 のような形容動詞 まし を避ける)
                if let Some(offset) = masu_nucleus_offset(&token.surface) {
                    if p.morae.last().is_some_and(|m| ends_with_ie_row(m)) {
                        p.nucleus = Some(before + offset);
                    }
                }
                // 平板 (核の無い) 句に です が付くと です 側に核: 学生です = ガクセイデ'ス / 学生でしょう = ガクセイデショ'ー。
                // 起伏の句 (雨です = ア'メデス) は元の核のまま (2026-09-28)
                if p.nucleus.is_none() {
                    if let Some(offset) = desu_nucleus_offset(&token.surface) {
                        p.nucleus = Some(before + offset);
                    }
                }
                p.morae.extend(mora_split(&reading));
                continue;
            }
        }

        // accent 不明の内容語 → 自分の句 (平板 fallback)
        b.open_phrase(MoraPhrase {
            morae: mora_split(&reading),
            nucleus: None,
            break_before: PhraseBreak::None,
        });
    }

    SymbolOutput {
        phrases: b.finish(),
        unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Furigana;

    fn phrases(f: &Furigana, input: &str) -> MoraPhrases {
        to_mora_phrases(&f.to_accent(input))
    }

    fn shape(p: &MoraPhrases) -> Vec<(String, Option<usize>, PhraseBreak)> {
        p.phrases
            .iter()
            .map(|ph| (ph.morae.concat(), ph.nucleus, ph.break_before))
            .collect()
    }

    #[test]
    fn spell_word_names_each_letter() {
        assert_eq!(spell_word("AviUtl"), "エーブイアイユーティーエル");
        assert_eq!(spell_word("abc"), "エービーシー");
        assert_eq!(spell_word("XYZ"), "エックスワイゼット");
        // 英字以外は読まない
        assert_eq!(spell_word("A-1"), "エー");
        assert_eq!(spell_word(""), "");
    }

    #[test]
    fn romaji_word_reads_simple_syllables() {
        let cases = [
            ("AviUtl", "アビウトル"),
            ("kawaii", "カワイイ"),
            ("sakura", "サクラ"),
            ("shimbun", "シンブン"),
            ("kitte", "キッテ"),
            ("kyou", "キョウ"),
            ("chotto", "チョット"),
            ("jyu", "ジュ"),
            ("tsunami", "ツナミ"),
            ("fuji", "フジ"),
            ("yama", "ヤマ"),
            ("hon", "ホン"),
            ("tea", "テア"),
            ("ra", "ラ"),
            ("lamp", "ランプ"),
            ("cat", "カト"),
            ("nyan", "ニャン"),
            ("yuki", "ユキ"),
            ("yoru", "ヨル"),
            ("ash", "アシ"),
            ("bunch", "ブンチ"),
            ("kyu", "キュ"),
            ("rakuda", "ラクダ"),
        ];
        for (w, want) in cases {
            assert_eq!(romaji_word(w), want, "{w}");
        }
        assert_eq!(romaji_word(""), "");
    }

    #[test]
    fn alpha_reading_follows_the_mode() {
        assert_eq!(alpha_reading("Ab", UnknownAlpha::Drop), None);
        assert_eq!(
            alpha_reading("Ab", UnknownAlpha::Spell).as_deref(),
            Some("エービー")
        );
        assert_eq!(
            alpha_reading("sa", UnknownAlpha::Romaji).as_deref(),
            Some("サ")
        );
    }

    #[test]
    fn alpha_split_separates_latin_runs() {
        assert_eq!(
            alpha_split("、AviUtl。ab"),
            vec![
                (false, "、".to_string()),
                (true, "AviUtl".to_string()),
                (false, "。".to_string()),
                (true, "ab".to_string()),
            ]
        );
        assert!(alpha_split("").is_empty());
        // 数字は英字語に含めない
        assert_eq!(
            alpha_split("a1b"),
            vec![
                (true, "a".to_string()),
                (false, "1".to_string()),
                (true, "b".to_string())
            ]
        );
    }

    #[test]
    fn chained_unknown_alpha_modes() {
        use crate::chain_phrases::ChainedPhrase;
        let ph = |sound: &str, accent: usize, gap: &str| ChainedPhrase {
            surface: sound.to_string(),
            reading: sound.to_string(),
            mora: sound.chars().count(),
            accent,
            estimated: false,
            sound: sound.to_string(),
            gap_before: gap.to_string(),
        };
        let spell = SymbolOptions {
            unknown_alpha: UnknownAlpha::Spell,
            ..SymbolOptions::default()
        };
        // 先頭の英字語: 区切りなし。 後ろの句は 記号の分だけ区切りが付く
        let out = chained_to_mora_phrases(&[ph("デス", 1, "AB、")], "", spell);
        assert_eq!(out.unknown, vec!["AB".to_string()]);
        assert_eq!(out.phrases.phrases.len(), 2);
        assert_eq!(out.phrases.phrases[0].morae.concat(), "エービー");
        assert_eq!(out.phrases.phrases[0].nucleus, None);
        assert_eq!(out.phrases.phrases[0].break_before, PhraseBreak::None);
        assert_eq!(out.phrases.phrases[1].break_before, PhraseBreak::Short);
        // 2 句目以降の英字語: 直前の記号が区切りになる
        let out = chained_to_mora_phrases(&[ph("ア", 0, ""), ph("イ", 0, "。C")], "", spell);
        assert_eq!(out.phrases.phrases.len(), 3);
        assert_eq!(out.phrases.phrases[1].morae.concat(), "シー");
        assert_eq!(out.phrases.phrases[1].break_before, PhraseBreak::Long);
        assert_eq!(out.phrases.phrases[2].break_before, PhraseBreak::None);
        // 末尾の英字語と記号
        let out = chained_to_mora_phrases(&[ph("ア", 0, "")], "、D？", spell);
        assert_eq!(out.phrases.phrases.len(), 2);
        assert_eq!(out.phrases.phrases[1].morae.concat(), "ディー");
        assert_eq!(out.phrases.phrases[1].break_before, PhraseBreak::Short);
        assert_eq!(out.phrases.trailing, PhraseBreak::Question);
        // Drop: 語は落ちるが 一覧には残る
        let out = chained_to_mora_phrases(&[ph("デス", 1, "AB、")], "", SymbolOptions::default());
        assert_eq!(out.unknown, vec!["AB".to_string()]);
        assert_eq!(out.phrases.phrases.len(), 1);
        // Romaji
        let romaji = SymbolOptions {
            unknown_alpha: UnknownAlpha::Romaji,
            ..SymbolOptions::default()
        };
        let out = chained_to_mora_phrases(&[ph("デス", 1, "kawaii")], "", romaji);
        assert_eq!(out.phrases.phrases[0].morae.concat(), "カワイイ");
    }

    #[test]
    fn chained_phrases_flat_has_no_nucleus_and_gap_sets_break() {
        use crate::chain_phrases::ChainedPhrase;
        let ph = |sound: &str, accent: usize, gap: &str| ChainedPhrase {
            surface: sound.to_string(),
            reading: sound.to_string(),
            mora: sound.chars().count(),
            accent,
            estimated: false,
            sound: sound.to_string(),
            gap_before: gap.to_string(),
        };
        let m = chained_to_mora_phrases(
            &[ph("アメ", 0, ""), ph("カゼ", 1, "、"), ph("ユキ", 2, "。?")],
            "！",
            SymbolOptions::default(),
        )
        .phrases;
        assert_eq!(m.phrases.len(), 3);
        // 平板 (0) は 核なし、 起伏はその位置
        assert_eq!(m.phrases[0].nucleus, None);
        assert_eq!(m.phrases[1].nucleus, Some(1));
        assert_eq!(m.phrases[2].nucleus, Some(2));
        // 先頭句は常に区切りなし / 記号の並びは強い方
        assert_eq!(m.phrases[0].break_before, PhraseBreak::None);
        assert_eq!(m.phrases[1].break_before, PhraseBreak::Short);
        assert_eq!(m.phrases[2].break_before, PhraseBreak::Question);
        assert_eq!(m.trailing, PhraseBreak::Long);
        // 先頭の句の前に記号があっても 区切りは付けない
        let m =
            chained_to_mora_phrases(&[ph("アメ", 1, "、")], "", SymbolOptions::default()).phrases;
        assert_eq!(m.phrases[0].break_before, PhraseBreak::None);
        assert_eq!(m.trailing, PhraseBreak::None);
        // 記号列に出せない文字だけの句は作らない
        let m = chained_to_mora_phrases(&[ph("😀", 0, "")], "", SymbolOptions::default()).phrases;
        assert!(m.phrases.is_empty());
    }

    #[test]
    fn mora_split_merges_small_kana() {
        assert_eq!(mora_split("キョウ"), ["キョ", "ウ"]);
        assert_eq!(mora_split("カーテン"), ["カ", "ー", "テ", "ン"]);
        // 先頭に小書きが来ても panic しない (直前が無いので単独モーラ)
        assert_eq!(mora_split("ャ"), ["ャ"]);
        assert!(mora_split("").is_empty());
    }

    #[test]
    fn particle_attaches_to_previous_phrase_keeping_nucleus() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        let p = phrases(&f, "雨が");
        assert_eq!(
            shape(&p),
            [("アメガ".to_string(), Some(1), PhraseBreak::None)]
        );
    }

    #[test]
    fn masu_puts_nucleus_on_masu() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("食べ", "タ]ベ");
        f.add_reading("行き", "イ[キ");
        let nuc = |s: &str| {
            let p = phrases(&f, s);
            (p.phrases[0].morae.concat(), p.phrases[0].nucleus_pos())
        };
        assert_eq!(nuc("食べます"), ("タベマス".to_string(), 3), "タベマ'ス");
        assert_eq!(nuc("行きました").1, 3, "イキマ'シタ");
        assert_eq!(nuc("食べません").1, 4, "タベマセ'ン");
    }

    #[test]
    fn desu_puts_nucleus_on_de_only_after_heiban() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("学生", "ガ[クセイ");
        f.add_reading("雨", "ア]メ");
        let nuc = |s: &str| phrases(&f, s).phrases[0].nucleus_pos();
        assert_eq!(nuc("学生です"), 5, "ガクセイデ'ス");
        assert_eq!(nuc("雨です"), 1, "起伏は元の核 (ア'メデス)");
    }

    #[test]
    fn heiban_phrase_has_no_nucleus() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("魚", "サ[カナ");
        let p = phrases(&f, "魚が");
        assert_eq!(p.phrases[0].nucleus, None);
        // 平板は句末を核位置として扱う
        assert_eq!(p.phrases[0].nucleus_pos(), 4);
    }

    #[test]
    fn ha_he_wo_become_phonetic_even_with_accent_phrases() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        f.add_reading("は", "[ハ]");
        f.add_reading("見る", "ミル");
        assert_eq!(shape(&phrases(&f, "雨は"))[0].0, "アメワ");
        assert_eq!(shape(&phrases(&f, "雨を見る"))[0].0, "アメオ");
    }

    #[test]
    fn punctuation_becomes_break_before_next_phrase() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        f.add_reading("雪", "ユ]キ");
        f.add_reading("風", "カ[ゼ]");
        let p = phrases(&f, "雨、雪。風");
        assert_eq!(
            shape(&p).iter().map(|(_, _, b)| *b).collect::<Vec<_>>(),
            [PhraseBreak::None, PhraseBreak::Short, PhraseBreak::Long]
        );
        assert_eq!(p.trailing, PhraseBreak::None);
    }

    #[test]
    fn question_is_per_sentence_and_trailing_is_kept() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        f.add_reading("雪", "ユ]キ");
        // 文中の `？` は 次の句の break、 文末の `。` は trailing
        let p = phrases(&f, "雨？雪。");
        assert_eq!(p.phrases[1].break_before, PhraseBreak::Question);
        assert_eq!(p.trailing, PhraseBreak::Long);
    }

    #[test]
    fn nakaguro_does_not_pause() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("甲", "コ]ウ");
        f.add_reading("乙", "オ]ツ");
        assert_eq!(
            phrases(&f, "甲・乙").phrases[1].break_before,
            PhraseBreak::None
        );
    }

    #[test]
    fn unreadable_token_is_dropped_but_breaks_phrase() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        let p = phrases(&f, "雨🌧");
        assert_eq!(p.phrases.len(), 1);
        assert_eq!(p.phrases[0].morae.concat(), "アメ");
    }

    #[test]
    fn empty_input_yields_no_phrases() {
        let f = Furigana::minimal().unwrap();
        let p = phrases(&f, "");
        assert!(p.phrases.is_empty());
        assert_eq!(p.trailing, PhraseBreak::None);
    }

    #[test]
    fn multi_phrase_entry_splits_into_phrases() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("都立", "[トウキョウ][ト]リツ");
        let p = phrases(&f, "都立");
        assert_eq!(
            shape(&p),
            [
                // `]` 無しの句は句末核として parse される (4 モーラ目 = ウ)
                ("トウキョウ".to_string(), Some(4), PhraseBreak::None),
                ("トリツ".to_string(), Some(1), PhraseBreak::None),
            ]
        );
    }

    #[test]
    fn no_phrase_is_empty() {
        // adapter が 「各句に核 1 つ」 を無条件に満たせることの前提
        let f = Furigana::builder().estimate_accent(true).build().unwrap();
        for input in ["今日は雨が降るかもしれない。", "田中さんが来た！", "…?"]
        {
            for p in to_mora_phrases(&f.to_accent(input)).phrases {
                assert!(!p.morae.is_empty(), "empty phrase for {input:?}");
                assert!(p.nucleus_pos() >= 1, "no nucleus for {input:?}");
                assert!(
                    p.morae.iter().all(|m| m.chars().all(is_symbol_kana)),
                    "non-kana mora in {input:?}: {:?}",
                    p.morae
                );
            }
        }
    }

    #[test]
    fn unknown_words_come_from_alpha_runs_only() {
        use crate::api::{AccentResult, AccentToken};
        let result = AccentResult {
            schema_version: "1".to_string(),
            tokens: vec![AccentToken {
                surface: "AB.C".to_string(),
                reading: "AB.C".to_string(),
                accent_phrases: Vec::new(),
                ambiguous: false,
                alternatives: Vec::new(),
            }],
        };
        let spell = SymbolOptions {
            unknown_alpha: UnknownAlpha::Spell,
            ..SymbolOptions::default()
        };
        let out = to_mora_phrases_with(&result, spell);
        assert_eq!(out.unknown, vec!["AB".to_string(), "C".to_string()]);
        assert_eq!(out.phrases.phrases.len(), 2);
        assert_eq!(out.phrases.phrases[0].morae.concat(), "エービー");
        assert_eq!(out.phrases.phrases[1].morae.concat(), "シー");
        let out = to_mora_phrases_with(&result, SymbolOptions::default());
        assert_eq!(out.unknown.len(), 2);
        assert!(out.phrases.phrases.is_empty());
    }
}
