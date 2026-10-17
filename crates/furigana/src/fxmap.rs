//! 速い決定的ハッシュ (FxHash 系) の `HashMap`。
//!
//! 辞書引きは形態素解析の 1 位置ごとに走る hot path で、 標準の `HashMap` が使う SipHash
//! (HashDoS 対策で重い) が時間の大半を占めていた。 辞書の key は信頼できる辞書ファイル由来で、
//! 入力が決められるのは 「1 字の char」 を引く側だけ (取り得る異なる key は辞書の字種数で頭打ち) なので、
//! 軽いハッシュでよい。 seed を持たないので、 実行ごとに反復順が変わることもない。

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

#[derive(Default, Clone, Copy)]
pub(crate) struct FxHasher {
    hash: u64,
}

impl FxHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let (chunks, rest) = bytes.as_chunks::<8>();
        for c in chunks {
            self.add(u64::from_le_bytes(*c));
        }
        if !rest.is_empty() {
            let mut buf = [0u8; 8];
            buf[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_le_bytes(buf));
        }
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add(u64::from(i));
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        // hashbrown は上位 7 bit を制御 byte に、 下位 bit を bucket 位置に使う。 乗算の結果は下位 bit が
        // 弱いので、 上位を折り返して混ぜる。
        self.hash ^ (self.hash >> 32)
    }
}

pub(crate) type FxMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;
pub(crate) type FxSet<K> = std::collections::HashSet<K, BuildHasherDefault<FxHasher>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn behaves_like_a_map_and_is_deterministic() {
        let mut m: FxMap<String, u32> = FxMap::default();
        for (i, w) in ["猫", "猫舌", "犬", "犬小屋", "あ", "ありがとう"]
            .iter()
            .enumerate()
        {
            m.insert((*w).to_string(), i as u32);
        }
        assert_eq!(m.get("猫舌"), Some(&1));
        assert_eq!(m.get("ありがとう"), Some(&5));
        assert_eq!(m.get("鳥"), None);
        let order = |m: &FxMap<String, u32>| m.keys().cloned().collect::<Vec<_>>();
        let mut m2: FxMap<String, u32> = FxMap::default();
        for (i, w) in ["猫", "猫舌", "犬", "犬小屋", "あ", "ありがとう"]
            .iter()
            .enumerate()
        {
            m2.insert((*w).to_string(), i as u32);
        }
        assert_eq!(order(&m), order(&m2), "seed が無いので反復順も同じ");
    }

    #[test]
    fn char_keys_spread_over_low_bits() {
        let mut m: FxMap<char, usize> = FxMap::default();
        for (i, c) in ('あ'..='ん').enumerate() {
            m.insert(c, i);
        }
        assert_eq!(m.len(), 'ん' as usize - 'あ' as usize + 1);
        assert_eq!(m.get(&'か'), Some(&('か' as usize - 'あ' as usize)));
    }
}
