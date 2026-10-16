//! 採用した読みの理由 (`why`) と、 採用しなかった読み (`alternatives`) を辞書から引き直す。
//!
//! `Pipeline::analyze` (debug / inspection 経路) だけが呼ぶ。 path の選択には一切触れない
//! (production の `tokens()` は変わらないので、 速度にも出力にも影響しない)。
//!
//! 辞書の見出し ([`crate::scoring::format::Entry`]) と `[[kanji]]` block は、 文脈の条件 (`[[match]]`) ごとに
//! 読みを持つ。 採用された token について、
//!
//! - 当たった条件を `why` に (`match: next_eq=から`)、 どの条件にも当たらなければ `default`
//! - 採用しなかった読み (外れた条件の読み / 既定の読み / `alt`) を `alternatives` に、 外れた条件を `sense` に
//!
//! 入れる。 本番の signal で 「条件が外れた側の読み」 が使われた頻度を数えれば、 迷いの多い語が分かる。

use crate::dict::Dict;
use crate::kana::hira_to_kata;
use crate::scoring::analyze::{AlternativeReading, Token};
use crate::scoring::bracket::strip_intonation_markers;
use crate::scoring::dict_bridge::DictBridgeProvider;
use crate::scoring::format::{Alternative, MatchBlock};

/// 外れた条件の読みの重み (採用した読みは 100、 `alt` は辞書の指定値)。
const UNMATCHED_WEIGHT: u8 = 30;
/// 条件に当たらず既定になった側の読みの重み。
const DEFAULT_WEIGHT: u8 = 60;

fn norm(r: &str) -> String {
    hira_to_kata(&strip_intonation_markers(r))
}

/// `tokens` の各 token に `why` と 採用しなかった読みを足す。 辞書に見出しが無い token は触らない。
pub(crate) fn explain(tokens: &mut [Token], input: &str, dict: &Dict) {
    for t in tokens.iter_mut() {
        if t.why.is_some() {
            continue;
        }
        // 見出し (熟語) を優先し、 無ければ 1 字の [[kanji]] block。
        let (default, matches, alts): (&str, &[MatchBlock], &[Alternative]) = if let Some(e) =
            dict.lookup_rich(&t.surface)
        {
            (e.default_reading(), e.matches(), e.alternatives())
        } else if let (Some(c), true) = (t.surface.chars().next(), t.surface.chars().count() == 1) {
            match dict.kanji_starting_with(c).next() {
                Some(b) => (b.default.as_str(), b.matches.as_slice(), b.alt.as_slice()),
                None => continue,
            }
        } else {
            continue;
        };
        let ctx = DictBridgeProvider::build_match_context(input, t.range.start, t.range.end);
        // 第一 hit の block を採用 (resolve_readings と同じ規則)。
        let chosen = matches
            .iter()
            .find(|m| m.condition.context_hits(&ctx).is_some());
        let (chosen_reading, why) = match chosen {
            Some(m) => {
                let d = m.condition.describe();
                (
                    m.reading.as_str(),
                    if d.is_empty() {
                        "default".to_string()
                    } else {
                        format!("match: {d}")
                    },
                )
            }
            None => (default, "default".to_string()),
        };
        let winning = norm(&t.reading);
        // 後段 (post-pass / 形態素 layer) が別の読みにした token は、 辞書の理由を付けない。
        if norm(chosen_reading) != winning {
            continue;
        }
        t.why = Some(why);
        // 採用した読みと同じもの (ひらがな/カタカナの表記違い) は代替ではない。
        t.alternatives.retain(|a| norm(&a.reading) != winning);
        t.ambiguous = !t.alternatives.is_empty();

        let mut extra: Vec<AlternativeReading> = Vec::new();
        let mut push = |reading: &str, sense: Option<String>, weight: u8| {
            let r = norm(reading);
            if r == winning
                || extra.iter().any(|a| a.reading == r)
                || t.alternatives.iter().any(|a| a.reading == r)
            {
                return;
            }
            extra.push(AlternativeReading {
                reading: r,
                sense,
                weight,
            });
        };
        if chosen.is_some() {
            push(
                default,
                Some("既定 (条件に当たらない時)".to_string()),
                DEFAULT_WEIGHT,
            );
        }
        for m in matches {
            let d = m.condition.describe();
            push(&m.reading, Some(format!("条件: {d}")), UNMATCHED_WEIGHT);
        }
        for a in alts {
            let d = a.condition.describe();
            let sense = a
                .sense
                .clone()
                .or_else(|| (!d.is_empty()).then(|| format!("条件: {d}")));
            push(&a.reading, sense, a.weight);
        }
        if !extra.is_empty() {
            t.alternatives.extend(extra);
            t.alternatives.sort_by_key(|a| std::cmp::Reverse(a.weight));
            t.ambiguous = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Range;

    fn token(surface: &str, reading: &str, range: Range<usize>) -> Token {
        Token {
            surface: surface.to_string(),
            reading: reading.to_string(),
            range,
            accent_phrases: Vec::new(),
            ambiguous: false,
            alternatives: Vec::new(),
            why: None,
            is_name: false,
        }
    }

    fn dict() -> Dict {
        Dict::from_toml_str(
            r#"
[entries."上手"]
reading = "ジョウズ"
[[entries."上手".match]]
next_eq = "から"
reading = "カミテ"
"#,
            "test",
        )
        .expect("toml")
    }

    #[test]
    fn matched_condition_is_reported_with_the_other_reading() {
        let d = dict();
        let input = "上手から来る";
        let mut t = vec![token("上手", "カミテ", 0..6)];
        explain(&mut t, input, &d);
        assert_eq!(t[0].why.as_deref(), Some("match: next_eq=から"));
        assert_eq!(t[0].alternatives.len(), 1);
        assert_eq!(t[0].alternatives[0].reading, "ジョウズ");
        assert!(t[0].ambiguous);
    }

    #[test]
    fn default_is_reported_with_the_unmatched_reading_and_its_condition() {
        let d = dict();
        let input = "上手だ";
        let mut t = vec![token("上手", "ジョウズ", 0..6)];
        explain(&mut t, input, &d);
        assert_eq!(t[0].why.as_deref(), Some("default"));
        assert_eq!(t[0].alternatives[0].reading, "カミテ");
        assert_eq!(
            t[0].alternatives[0].sense.as_deref(),
            Some("条件: next_eq=から")
        );
    }

    #[test]
    fn same_reading_in_other_kana_is_not_an_alternative() {
        let d = dict();
        let mut t = vec![token("上手", "じょうず", 0..6)];
        t[0].alternatives.push(AlternativeReading {
            reading: "ジョウズ".to_string(),
            sense: None,
            weight: 100,
        });
        explain(&mut t, "上手だ", &d);
        assert!(t[0].alternatives.iter().all(|a| a.reading != "ジョウズ"));
    }

    #[test]
    fn token_not_in_dict_is_untouched() {
        let d = dict();
        let mut t = vec![token("天気", "テンキ", 0..6)];
        explain(&mut t, "天気", &d);
        assert!(t[0].why.is_none());
        assert!(t[0].alternatives.is_empty());
    }

    #[test]
    fn reading_changed_by_later_stage_gets_no_dict_reason() {
        let d = dict();
        let mut t = vec![token("上手", "ウワテ", 0..6)];
        explain(&mut t, "上手だ", &d);
        assert!(t[0].why.is_none());
    }
}
