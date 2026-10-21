//! 送り仮名・記号・連濁の判定 helper (LinderaFallbackProvider が使う)。

use super::*;

/// 漢字 1 字以上 + 送り仮名 (ひらがな) の形か (疲れ / 使い / 書き)。 1 字漢字の接尾
/// (代 / 度 / 部) は元から濁音の語があるので対象外にする。
pub(super) fn is_okurigana_form(surface: &str) -> bool {
    let mut chars = surface.chars();
    chars.next().is_some_and(is_real_cjk_ideograph)
        && surface.chars().next_back().is_some_and(crate::kana::is_hiragana_char)
        // 出し は元の読みが濁音 (出す = だす)
        && surface != "出し"
}

/// [`is_okurigana_form`] のうち、 「に + 移動動詞」 の前で清音に戻してよいもの。
/// 通り / 越し / 帰り / 攻め / 沿い / 伝い は名詞の後ろで濁るのが普通なので除く
/// (時間通りに行く / 年越しに / 仕事帰りに寄る)
pub(super) fn is_okurigana_form_voiced_after_noun_excluded(surface: &str) -> bool {
    is_okurigana_form(surface)
        && !matches!(surface, "通り" | "越し" | "帰り" | "攻め" | "沿い" | "伝い")
}

/// 記号だけの token を途中で切ってよい字か (英数・かな・漢字・空白・制御文字以外)
pub(super) fn is_split_symbol(c: char) -> bool {
    !c.is_alphanumeric() && !c.is_whitespace() && !c.is_control()
}

/// 閉じ括弧・閉じ引用符 (この直後は直前の語句の続きになりやすい)
pub(super) fn is_closing_symbol(c: char) -> bool {
    matches!(
        c,
        ')' | '）' | '」' | '』' | '】' | ']' | '］' | '〉' | '》' | '〕' | '"' | '”' | '’'
    )
}

/// 連濁の境界になる記号 (句読点 / 括弧 / 空白 / 絵文字の区切り)。 `%` や数字の後ろは
/// 20%引き = ビキ のように濁るので含めない
pub(super) fn is_rendaku_boundary_symbol(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '。' | '、'
                | '，'
                | '．'
                | '・'
                | '…'
                | '！'
                | '？'
                | '!'
                | '?'
                | '「'
                | '」'
                | '『'
                | '』'
                | '（'
                | '）'
                | '('
                | ')'
                | '【'
                | '】'
                | '['
                | ']'
                | '~'
                | '〜'
                | ':'
                | '：'
                | ';'
                | '；'
                | '_'
                | '*'
                | '゚'
                | '.'
                | ','
        )
}

/// カタカナ読みの先頭の濁音を清音に戻す (ヅカレ → ツカレ)。 濁音で始まらなければ `None`。
pub(super) fn devoice_first_kana(reading: &str) -> Option<String> {
    let mut chars = reading.chars();
    let plain = match chars.next()? {
        'ガ' => 'カ',
        'ギ' => 'キ',
        'グ' => 'ク',
        'ゲ' => 'ケ',
        'ゴ' => 'コ',
        'ザ' => 'サ',
        'ジ' => 'シ',
        'ズ' => 'ス',
        'ゼ' => 'セ',
        'ゾ' => 'ソ',
        'ダ' => 'タ',
        'ヂ' => 'チ',
        'ヅ' => 'ツ',
        'デ' => 'テ',
        'ド' => 'ト',
        'バ' => 'ハ',
        'ビ' => 'ヒ',
        'ブ' => 'フ',
        'ベ' => 'ヘ',
        'ボ' => 'ホ',
        _ => return None,
    };
    Some(std::iter::once(plain).chain(chars).collect())
}
