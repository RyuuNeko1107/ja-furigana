//! path 確定後に token 列を補正する **post-pass** の seam。
//!
//! Smart Engine が Viterbi で path を確定した後、 [`crate::scoring::analyze::Token`] 列を
//! 走査して読みを補正する処理を 1 つの trait に揃える。 matcher が前後 1 token しか
//! 見られない制約を補う層で、 現在は連濁 ([`RendakuPass`]) ・腹+空く文脈補正
//! ([`HaraSukuPass`]) ・人名+敬称 token 衝突補正 ([`NameBoundaryPass`]) の
//! 3 adapter がぶら下がる。
//!
//! ## 適用順
//!
//! [`apply_all`] 内の配列順に適用する。 順序が意味を持つ post-pass を足すときは
//! 配列の位置で表現する (= 適用順が data として明示される)。
//!
//! [`RendakuPass`]: crate::scoring::odoriji::RendakuPass
//! [`HaraSukuPass`]: crate::scoring::contextual::HaraSukuPass
//! [`NameBoundaryPass`]: crate::scoring::names::NameBoundaryPass

use crate::analyzer::Analyzer;
use crate::dict::Dict;
use crate::scoring::analyze::Token;
use crate::scoring::contextual::HaraSukuPass;
use crate::scoring::names::NameBoundaryPass;
use crate::scoring::odoriji::RendakuPass;

/// path 確定後の token 列を in-place で補正する post-pass。
///
/// 各 adapter は冪等・順序依存を自覚した上で `tokens` を直接書き換える。
/// interface は `apply` 1 本で、 これが test surface になる (token 列を組んで呼ぶだけ)。
pub trait ReadingPostPass {
    /// 確定 token 列を補正する。
    ///
    /// `Vec` を取るのは [`NameBoundaryPass`] の bare suffix 形 merge が token 数を
    /// 減らすため。 読みだけ書き換える pass は slice として扱えばよい。
    fn apply(&self, tokens: &mut Vec<Token>);
}

/// `tokens` に全 post-pass を適用順に回す。 [`crate::scoring::pipeline::Pipeline`] が呼ぶ。
///
/// 適用順: 連濁 → 腹+空く → 人名+敬称境界。 連濁は隣接 token の reading を参照する
/// ため、 reading を書き換える他 pass より前に置く。 人名境界 pass は dict / 形態素
/// 辞書を読み source に使うため参照を取る (= const 配列でなく本関数で束ねる)。
pub fn apply_all(tokens: &mut Vec<Token>, dict: &Dict, analyzer: &Analyzer) {
    let name_boundary = NameBoundaryPass::new(dict, analyzer);
    let passes: [&dyn ReadingPostPass; 3] = [&RendakuPass, &HaraSukuPass, &name_boundary];
    for pass in passes {
        pass.apply(tokens);
    }
}
