//! 表の key を 64 ビットのハッシュに畳む (決定的。 実行ごと / 環境ごとに値が変わらない)。
//!
//! アクセント表 (約 27 万件) と 連結表 (内容語の例外 約 8 万件) は 「表記 + 読み (+ 品詞)」 を key にした
//! 引くだけの表なので、 文字列そのものを持たず ハッシュだけを持つ (メモリは 1 件 約 16 byte、 文字列 key の 1/3 以下)。
//! 衝突の確率は 件数 N に対して N^2 / 2^65 (30 万件で 約 2.4e-9) なので、 引き違いは実用上起きない。
//! 実行時に使う hasher (`DefaultHasher` / `RandomState`) は実行ごとに seed が変わるので使わない。

/// 区切り (表記 / 読み / 品詞の境目)。 key の部品どうしが連結で紛れないようにする
const SEP: u8 = 1;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

#[inline]
fn feed(mut h: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

/// FNV-1a の最終混ぜ (splitmix64 の finalizer)。 下位 bit の偏りを散らす
#[inline]
fn finish(mut h: u64) -> u64 {
    h ^= h >> 30;
    h = h.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^ (h >> 31)
}

/// 部品を区切り付きで畳んだハッシュ。 `hash_parts(&["表記", "読み"])`
#[inline]
pub(crate) fn hash_parts(parts: &[&str]) -> u64 {
    let mut h = FNV_OFFSET;
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h = feed(h, &[SEP]);
        }
        h = feed(h, p.as_bytes());
    }
    finish(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_across_runs_and_sensitive_to_parts() {
        // 値を固定: 実行ごとに変わらないこと (表の key は この値で並べる)
        assert_eq!(
            hash_parts(&["天気", "テンキ"]),
            hash_parts(&["天気", "テンキ"])
        );
        assert_ne!(
            hash_parts(&["天気", "テンキ"]),
            hash_parts(&["天気テ", "ンキ"])
        );
        assert_ne!(
            hash_parts(&["a", "b", "c", "d"]),
            hash_parts(&["a", "b", "cd"])
        );
        assert_ne!(hash_parts(&["雨", "アメ"]), hash_parts(&["雨", "アマ"]));
    }

    #[test]
    fn known_value_is_pinned() {
        // 将来 hash の実装を変えたら 表の並びが変わるので、 気づけるよう固定しておく (Python の参照実装で算出)
        assert_eq!(hash_parts(&["天気", "テンキ"]), 0x23db8663ca7ff750_u64);
        assert_eq!(hash_parts(&["雨", "アメ"]), 0x9cac2e9921070955_u64);
    }
}
