//! 人名の手がかり (敬称・肩書き) と、 手がかりの強さの判定 ([`PersonCues::cue_score`])
//!
//! 表は辞書の `person_name_cues.toml` が正 ([`crate::rules::PersonNameCuesData`])。 lib には既定値を
//! 持たない。 [`PersonCues`] は、 その表から **構築時に 1 度だけ** 作る判定用の形 (前判定の字の集合を含む)
//! で、 解析のたびには作らない。

use crate::fxmap::FxSet;
use crate::kana::is_kanji_char;
use crate::rules::PersonNameCuesData;
use crate::scoring::analyze::Token;

/// 結合した名前として許す最大文字数 (姓 1〜3 + 名 1〜4)。 漢字の偶然の連続での誤爆を避ける。
pub(super) const MAX_RUN_CHARS: usize = 7;
/// 手がかりの前に遡る token 数の上限。
pub(super) const MAX_RUN_TOKENS: usize = 5;
/// 組み直す前の読みを `alternatives` に残すときの重み。
pub(super) const ALT_WEIGHT: u8 = 50;

/// 人名の手がかりの判定用の表。 空 (既定) なら手がかりが 1 つも無く、 人名の専用経路は何もしない。
#[derive(Debug, Default, Clone)]
pub struct PersonCues {
    /// 敬称 (読み方の揺れがない)
    pub(super) honorifics: Vec<String>,
    /// 肩書き・職名。 人名の直後に来る語 (単独の語としても読みが決まっているもの)
    titles: Vec<String>,
    /// 名の表にあるが、 実コメントでは一般語の誤認ばかりだった語 (新人さん / 初代大統領 / 勝利投手)
    pub(super) not_given_names: Vec<String>,
    /// 肩書きの直後に隣り合ってこの字で始まる token が続く時は、 別の複合語 (医師会 / 記者会見)
    compound_heads: Vec<char>,
    /// 手がかり語 (敬称と肩書き) の先頭の字
    cue_first: FxSet<char>,
    /// 肩書き語の先頭の字 (選 / 監 / 先 / 社 …)。 肩書きの候補になる token を絞る前判定に使う
    title_first: FxSet<char>,
    /// 敬称の最後の字 (ん / 氏)。 「原さん」 のように敬称で終わる token の前判定に使う
    honorific_last: FxSet<char>,
}

impl PersonCues {
    /// 辞書の表から判定用の形を作る。 構築時に 1 度だけ呼ぶ。
    #[must_use]
    pub fn from_data(data: &PersonNameCuesData) -> Self {
        let c = &data.cues;
        let first = |s: &String| s.chars().next();
        Self {
            honorifics: c.honorifics.clone(),
            titles: c.titles.clone(),
            not_given_names: c.not_given_names.clone(),
            compound_heads: c
                .compound_heads
                .iter()
                .filter_map(|s| s.chars().next())
                .collect(),
            cue_first: c
                .honorifics
                .iter()
                .chain(&c.titles)
                .filter_map(first)
                .collect(),
            title_first: c.titles.iter().filter_map(first).collect(),
            honorific_last: c
                .honorifics
                .iter()
                .filter_map(|s| s.chars().next_back())
                .collect(),
        }
    }

    /// token の表記が手がかり語なら、 その点数 (1〜) を返す。 手がかりでなければ 0。
    ///
    /// 手がかり語そのもの、 または手がかり語 + ひらがな (「選手たち」 「さんが」) だけを認める。
    /// 「選手権」 のように後ろに漢字が続く語は手がかりにしない。 敬称は 2 点、 肩書きは 1 点。
    pub(super) fn cue_score(&self, surface: &str) -> u8 {
        for (list, score) in [(&self.honorifics, 2u8), (&self.titles, 1u8)] {
            for cue in list {
                if let Some(rest) = surface.strip_prefix(cue.as_str()) {
                    if rest.chars().all(crate::kana::is_hiragana_char) {
                        return score;
                    }
                }
            }
        }
        0
    }

    /// 肩書き (医師 / 記者 / 弁護士 …) の直後に、 隣り合って 会・団・連・協・組・学・院・所 のような字で
    /// 始まる token が続く時は、 医師会 / 記者会見 / 弁護士会 / 選手会 / 議員連盟 のような別の複合語で、
    /// 肩書きではない (日野市医師会 の 市 を名 (いち) にしないため)。 字の一覧は `compound_heads`。
    pub(super) fn title_continues_into_compound(&self, tokens: &[Token], c: usize) -> bool {
        let cue = &tokens[c];
        if !cue.surface.chars().next().is_some_and(is_kanji_char) {
            return false;
        }
        tokens.get(c + 1).is_some_and(|next| {
            next.range.start == cue.range.end
                && next
                    .surface
                    .chars()
                    .next()
                    .is_some_and(|ch| self.compound_heads.contains(&ch))
        })
    }

    /// token が手がかり語 (敬称 / 肩書き) か、 それを含む形 (原さん / 利さ|ん の ん) でありうるか。
    /// 手がかりが 1 つも無い行 (ほとんどの行) を、 本処理に入れずに素通りさせる前判定。
    /// 本処理の判定 ([`Self::cue_score`] / 敬称で終わる token / 敬称をまたぐ token) より必ず広い。
    /// 大半の token は 先頭の字の表引きと 末尾の字の表引きだけで終わるようにしてある
    pub(super) fn token_may_be_cue(&self, surface: &str) -> bool {
        let Some(first) = surface.chars().next() else {
            return false;
        };
        // 敬称の後半だけの token (利さ|ん / ちゃ|ん / ち|ゃん)
        if self.honorifics.iter().any(|h| h.ends_with(surface)) {
            return true;
        }
        // 手がかり語で始まる (さん / ちゃん / くん / 氏 / 肩書き)
        if self.cue_first.contains(&first)
            && (self.title_first.contains(&first)
                || self
                    .honorifics
                    .iter()
                    .any(|h| surface.starts_with(h.as_str())))
        {
            return true;
        }
        // 敬称で終わる (原さん)
        surface
            .chars()
            .next_back()
            .is_some_and(|last| self.honorific_last.contains(&last))
            && self
                .honorifics
                .iter()
                .any(|h| surface.ends_with(h.as_str()))
    }

    /// 手がかり語になりうる token が 1 つでもあるか。
    pub(super) fn may_have_cue(&self, tokens: &[Token]) -> bool {
        tokens.iter().any(|t| self.token_may_be_cue(&t.surface))
    }
}
