//! 英字の読み (綴り読み・ローマ字読み) と、 英字の連なりの切り出し (記号列に出せない英字語の扱い)。

use super::*;

/// 英字の名前 (A〜Z) で 1 文字ずつ読む。
pub(super) fn spell_word(word: &str) -> String {
    word.chars()
        .filter(char::is_ascii_alphabetic)
        .map(|c| {
            crate::scoring::model_number::LETTER_NAMES
                [usize::from(c.to_ascii_uppercase() as u8 - b'A')]
        })
        .collect()
}

/// ローマ字として読んだ簡易のカタカナ (実験的)。
pub(super) fn romaji_word(word: &str) -> String {
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
pub(super) fn alpha_reading(word: &str, mode: UnknownAlpha) -> Option<String> {
    let r = match mode {
        UnknownAlpha::Drop => return None,
        UnknownAlpha::Spell => spell_word(word),
        UnknownAlpha::Romaji => romaji_word(word),
    };
    (!r.is_empty()).then_some(r)
}

/// 文字列の中の 英字の連なり (ASCII の英字だけ。 数字は含めない) と、 それ以外の文字を順に返す。
pub(super) fn alpha_split(text: &str) -> Vec<(bool, String)> {
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
