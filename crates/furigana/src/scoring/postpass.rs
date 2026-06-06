//! path 確定後に token 列を補正する **post-pass** の seam。
//!
//! Smart Engine が Viterbi で path を確定した後、 [`crate::scoring::analyze::Token`] 列を
//! 走査して読みを補正する処理を 1 つの trait に揃える。 matcher が前後 1 token しか
//! 見られない制約を補う層で、 現在は連濁 ([`RendakuPass`]) と腹+空く文脈補正
//! ([`HaraSukuPass`]) の 2 adapter がぶら下がる。
//!
//! ## 適用順
//!
//! [`POST_PASSES`] の配列順に適用する。 順序が意味を持つ post-pass を足すときは
//! 配列の位置で表現する (= 適用順が data として明示される)。
//!
//! [`RendakuPass`]: crate::scoring::odoriji::RendakuPass
//! [`HaraSukuPass`]: crate::scoring::contextual::HaraSukuPass

use crate::scoring::analyze::Token;
use crate::scoring::contextual::HaraSukuPass;
use crate::scoring::odoriji::RendakuPass;

/// path 確定後の token 列を in-place で補正する post-pass。
///
/// 各 adapter は冪等・順序依存を自覚した上で `tokens` を直接書き換える。
/// interface は `apply` 1 本で、 これが test surface になる (token 列を組んで呼ぶだけ)。
pub trait ReadingPostPass {
    /// 確定 token 列を補正する。
    fn apply(&self, tokens: &mut [Token]);
}

/// 適用順に並べた post-pass 一覧。 [`crate::Furigana::analyze`] がこの順で回す。
///
/// 連濁 → 腹+空く の順。 連濁は隣接 token の reading を参照するため、
/// reading を書き換える他 pass より前に置く。
pub const POST_PASSES: &[&dyn ReadingPostPass] = &[&RendakuPass, &HaraSukuPass];

/// `tokens` に [`POST_PASSES`] を順に適用する。
pub fn apply_all(tokens: &mut [Token]) {
    for pass in POST_PASSES {
        pass.apply(tokens);
    }
}
