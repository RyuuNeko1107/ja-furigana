//! 人名の手がかり (敬称・肩書き) の表と、 手がかりの強さの判定 ([`cue_score`])

use crate::kana::is_kanji_char;
use crate::scoring::analyze::Token;

/// 敬称 (読み方の揺れがない)。
// 様 / 君 / 殿 は王子様・神様・諸君のような一般語と衝突するので手がかりにしない (実コメント A/B で確認)。
pub(super) const HONORIFICS: &[&str] = &["さん", "ちゃん", "くん", "氏"];

/// 名の表にあるが、 実コメントでは一般語の誤認ばかりだった語 (新人さん / 初代大統領 / 勝利投手)
pub(super) const COMMON_WORD_GIVEN: &[&str] = &["新人", "初代", "勝利"];

/// 肩書き・職名。 人名の直後に来る語 (単独の語としても読みが決まっているもの)。
const TITLES: &[&str] = &[
    "選手",
    "監督",
    "先生",
    "社長",
    "議員",
    "首相",
    "大臣",
    "容疑者",
    "教授",
    "部長",
    "課長",
    "会長",
    "代表",
    "知事",
    "市長",
    "町長",
    "村長",
    "総理",
    "隊長",
    "校長",
    "医師",
    "投手",
    "騎手",
    "棋士",
    "名人",
    "記者",
    "弁護士",
    "被告",
    "被害者",
    "師匠",
    "先輩",
    "後輩",
    "コーチ",
    "キャプテン",
    "主将",
    "団長",
    "署長",
    "長官",
    "大統領",
    "元帥",
    "将軍",
    "監察官",
    "社員",
    "店長",
    "支配人",
    "理事長",
    "委員長",
    "幹事長",
    "党首",
    "総裁",
];

/// 結合した名前として許す最大文字数 (姓 1〜3 + 名 1〜4)。 漢字の偶然の連続での誤爆を避ける。
pub(super) const MAX_RUN_CHARS: usize = 7;
/// 手がかりの前に遡る token 数の上限。
pub(super) const MAX_RUN_TOKENS: usize = 5;
/// 組み直す前の読みを `alternatives` に残すときの重み。
pub(super) const ALT_WEIGHT: u8 = 50;

/// token の表記が手がかり語なら、 その点数 (1〜) を返す。 手がかりでなければ 0。
///
/// 手がかり語そのもの、 または手がかり語 + ひらがな (「選手たち」 「さんが」) だけを認める。
/// 「選手権」 のように後ろに漢字が続く語は手がかりにしない。
pub(super) fn cue_score(surface: &str) -> u8 {
    for (list, score) in [(HONORIFICS, 2u8), (TITLES, 1u8)] {
        for cue in list {
            if let Some(rest) = surface.strip_prefix(cue) {
                if rest.chars().all(crate::kana::is_hiragana_char) {
                    return score;
                }
            }
        }
    }
    0
}

/// 肩書き (医師 / 記者 / 弁護士 …) の直後に、 隣り合って 会・団・連・協・組・学・院・所 で始まる token が続く時は、
/// 医師会 / 記者会見 / 弁護士会 / 選手会 / 議員連盟 のような別の複合語で、 肩書きではない (日野市医師会 の 市 を名にしない)。
pub(super) fn title_continues_into_compound(tokens: &[Token], c: usize) -> bool {
    let cue = &tokens[c];
    if !cue.surface.chars().next().is_some_and(is_kanji_char) {
        return false;
    }
    tokens.get(c + 1).is_some_and(|next| {
        next.range.start == cue.range.end
            && next.surface.chars().next().is_some_and(|ch| {
                matches!(ch, '会' | '団' | '連' | '協' | '組' | '学' | '院' | '所')
            })
    })
}

/// 手がかり語 (敬称と肩書き) の先頭の字
static CUE_FIRST_CHARS: once_cell::sync::Lazy<crate::fxmap::FxSet<char>> =
    once_cell::sync::Lazy::new(|| {
        HONORIFICS
            .iter()
            .chain(TITLES.iter())
            .filter_map(|c| c.chars().next())
            .collect()
    });

/// 肩書き語の先頭の字 (選 / 監 / 先 / 社 …)。 肩書きの候補になる token を絞る前判定に使う。
static TITLE_FIRST_CHARS: once_cell::sync::Lazy<crate::fxmap::FxSet<char>> =
    once_cell::sync::Lazy::new(|| TITLES.iter().filter_map(|c| c.chars().next()).collect());

/// token が手がかり語 (敬称 / 肩書き) か、 それを含む形 (原さん / 利さ|ん の ん) でありうるか。
/// 手がかりが 1 つも無い行 (ほとんどの行) を、 本処理に入れずに素通りさせる前判定。
/// 本処理の判定 ([`cue_score`] / 敬称で終わる token / 敬称をまたぐ token) より必ず広い。
/// 大半の token は 先頭の字の表引きと 末尾の字の比較だけで終わるようにしてある
pub(super) fn token_may_be_cue(surface: &str) -> bool {
    let Some(first) = surface.chars().next() else {
        return false;
    };
    // 敬称の後半だけの token (利さ|ん / ちゃ|ん / ち|ゃん)
    if surface == "ん" || surface == "ゃん" {
        return true;
    }
    // 手がかり語で始まる (さん / ちゃん / くん / 氏 / 肩書き)
    if CUE_FIRST_CHARS.contains(&first)
        && (TITLE_FIRST_CHARS.contains(&first) || HONORIFICS.iter().any(|h| surface.starts_with(h)))
    {
        return true;
    }
    // 敬称で終わる (原さん)
    surface.ends_with(['ん', '氏']) && HONORIFICS.iter().any(|h| surface.ends_with(h))
}

/// 手がかり語になりうる token が 1 つでもあるか。
pub(super) fn may_have_cue(tokens: &[Token]) -> bool {
    tokens.iter().any(|t| token_may_be_cue(&t.surface))
}
