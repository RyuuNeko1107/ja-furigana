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

mod alpha;

use alpha::*;

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
mod tests;
