//! 数式で使う文字の表と、 文字の分類 (ギリシャ文字・上付き / 下付き・演算子・式の端になれる文字)

/// 式として読む最大の長さ (文字数)。 これを超える英数字の連なりは式にしない
pub(super) const MAX_SPAN: usize = 160;

/// 英字の関数名 (長い順。 前方一致で分ける)
pub(super) const FUNCS: [(&str, &str); 12] = [
    ("arcsin", "アークサイン"),
    ("arccos", "アークコサイン"),
    ("arctan", "アークタンジェント"),
    ("sin", "サイン"),
    ("cos", "コサイン"),
    ("tan", "タンジェント"),
    ("log", "ログ"),
    ("exp", "エクスポネンシャル"),
    ("lim", "リミット"),
    ("max", "マックス"),
    ("min", "ミニマム"),
    ("ln", "エルエヌ"),
];

/// 数の直後に付くと単位 (kg / km / ms / m / t / W …) に読まれる英字。 変数 (2x) とは区別する
pub(super) const UNIT_RUNS: [&str; 35] = [
    "kg", "km", "cm", "mm", "mg", "ml", "ms", "hz", "gb", "mb", "kb", "tb", "pt", "px", "pc", "kw",
    "mw", "db", "ft", "lb", "oz", "ha", "cc", "m", "g", "t", "s", "h", "l", "L", "W", "V", "K",
    "D", "G",
];

/// 変数の積に見えるが英単語の 2 字 (これらが `=` の隣にあっても式にしない)
pub(super) const WORDS: [&str; 25] = [
    "id", "is", "to", "in", "on", "or", "if", "do", "no", "ok", "so", "as", "be", "by", "go", "he",
    "me", "my", "of", "up", "us", "we", "it", "an", "ww",
];

pub(super) fn greek(c: char) -> Option<&'static str> {
    Some(match c {
        'α' => "アルファ",
        'β' => "ベータ",
        'γ' => "ガンマ",
        'δ' | 'Δ' => "デルタ",
        'ε' => "イプシロン",
        'θ' | 'Θ' => "シータ",
        'λ' | 'Λ' => "ラムダ",
        'μ' => "ミュー",
        'π' | 'Π' => "パイ",
        'ρ' => "ロー",
        'σ' | 'Σ' => "シグマ",
        'τ' => "タウ",
        'φ' | 'Φ' => "ファイ",
        'ω' | 'Ω' => "オメガ",
        _ => return None,
    })
}

/// 全角の ASCII (！〜～) を半角に、 数学のマイナス (− U+2212) を `-` に
pub(super) fn fold(c: char) -> char {
    let c = match c {
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        '\u{2212}' => '-',
        _ => c,
    };
    // [0,1] / E[X] の角括弧は丸括弧と同じに読む
    match c {
        '[' => '(',
        ']' => ')',
        _ => c,
    }
}

pub(super) fn sup_digit(c: char) -> Option<char> {
    "⁰¹²³⁴⁵⁶⁷⁸⁹"
        .chars()
        .position(|k| k == c)
        .and_then(|i| char::from_digit(u32::try_from(i).ok()?, 10))
}

pub(super) fn sub_digit(c: char) -> Option<char> {
    "₀₁₂₃₄₅₆₇₈₉"
        .chars()
        .position(|k| k == c)
        .and_then(|i| char::from_digit(u32::try_from(i).ok()?, 10))
}

pub(super) fn is_op(c: char) -> bool {
    matches!(
        c,
        '+' | '-'
            | '*'
            | '/'
            | '='
            | '<'
            | '>'
            | '×'
            | '÷'
            | '±'
            | '≦'
            | '≧'
            | '≠'
            | '≒'
            | '≈'
            | '≤'
            | '≥'
            | '∈'
            | '∉'
            | '∪'
            | '∩'
            | '⊥'
            | '∥'
            | '≡'
            | '→'
            | ':'
            | '⊂'
            | '⊆'
            | '¬'
            | '∧'
            | '∨'
            | '⇔'
            | '⇒'
            | '∀'
            | '∃'
    )
}

/// 式の中に現れうる字 (空白は別に扱う)
pub(super) fn is_span_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || is_op(c)
        || matches!(
            c,
            '(' | ')'
                | '^'
                | ','
                | '.'
                | '√'
                | '°'
                | '△'
                | '∠'
                | '∫'
                | '∞'
                | '\''
                | '′'
                | '…'
                | '⋯'
        )
        || sup_digit(c).is_some()
        || sub_digit(c).is_some()
        || c == '⁻'
        || greek(c).is_some()
}

/// 式を始められる字
/// 式の根拠になりうる字か (前判定用。 `is_math` が要求する 等号・不等号・関係記号・累乗・√・∫・絶対値・引き算のどれか)
pub(super) fn is_trigger_char(c: char) -> bool {
    matches!(
        c,
        '=' | '<'
            | '>'
            | '≦'
            | '≧'
            | '≠'
            | '≒'
            | '≈'
            | '≤'
            | '≥'
            | '∈'
            | '∉'
            | '∪'
            | '∩'
            | '⊥'
            | '∥'
            | '≡'
            | '⊂'
            | '⊆'
            | '∧'
            | '∨'
            | '^'
            | '√'
            | '²'
            | '³'
            | '∫'
            | '|'
            | '-'
            | '\u{207B}'
            | '\u{2070}'
            | '\u{00B9}'
            | '\u{2074}'..='\u{2079}'
    )
}

pub(super) fn is_start_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '(' | '√' | '-' | '+' | '²' | '³' | '^' | '△' | '∠' | '∫' | '|' | '¬' | '∀' | '∃'
        )
        || greek(c).is_some()
}
