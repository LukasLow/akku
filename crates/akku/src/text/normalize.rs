//! Unicode canonical normalization (`NFC` and `NFD`, UAX #15).
//!
//! Supports full algorithmic Hangul syllable decomposition/composition, canonical ordering,
//! and the complete canonical decomposition / primary composite / composition exclusion
//! data of Unicode 16.0.0 (generated tables in `tables.rs`; conformant with
//! `NormalizationTest.txt` for NFC/NFD). Compatibility forms (`NFKC`/`NFKD`) are
//! intentionally out of scope for Wave 2.

use std::borrow::Cow;

use crate::text::tables::{
    canonical_combining_class, canonical_composition, canonical_decomposition,
};

const S_BASE: u32 = 0xAC00;
const L_BASE: u32 = 0x1100;
const V_BASE: u32 = 0x1161;
const T_BASE: u32 = 0x11A7;
const L_COUNT: u32 = 19;
const V_COUNT: u32 = 21;
const T_COUNT: u32 = 28;
const N_COUNT: u32 = V_COUNT * T_COUNT; // 588
const S_COUNT: u32 = L_COUNT * N_COUNT; // 11172

/// Unicode canonical normalization form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NormalizationForm {
    /// Canonical Decomposition, followed by Canonical Composition.
    Nfc,
    /// Canonical Decomposition.
    Nfd,
}

/// Normalize `input` according to `form`, borrowing when already normalized (e.g. ASCII).
pub fn normalize<'a>(input: &'a str, form: NormalizationForm) -> Cow<'a, str> {
    if input.is_ascii() {
        return Cow::Borrowed(input);
    }
    match form {
        NormalizationForm::Nfd => {
            if is_nfd(input) {
                Cow::Borrowed(input)
            } else {
                Cow::Owned(nfd(input))
            }
        }
        NormalizationForm::Nfc => {
            if is_nfc(input) {
                Cow::Borrowed(input)
            } else {
                Cow::Owned(nfc(input))
            }
        }
    }
}

/// Return the NFD (Canonical Decomposition) form of `input`.
pub fn nfd(input: &str) -> String {
    if input.is_ascii() {
        return input.to_string();
    }
    let chars = decompose_ordered(input);
    chars.into_iter().collect()
}

/// Return the NFC (Canonical Decomposition + Canonical Composition) form of `input`.
pub fn nfc(input: &str) -> String {
    if input.is_ascii() {
        return input.to_string();
    }
    let decomposed = decompose_ordered(input);
    let mut out: Vec<char> = Vec::with_capacity(decomposed.len());
    let mut starter_idx: Option<usize> = None;
    let mut last_ccc: u8 = 0;

    for ch in decomposed {
        let cc = canonical_combining_class(ch);
        if let Some(si) = starter_idx {
            if last_ccc == 0 || last_ccc < cc {
                if let Some(comp) = try_compose(out[si], ch) {
                    out[si] = comp;
                    continue;
                }
            }
        }
        if cc == 0 {
            starter_idx = Some(out.len());
            last_ccc = 0;
        } else {
            last_ccc = cc;
        }
        out.push(ch);
    }

    out.into_iter().collect()
}

/// Check whether `input` is already in NFD.
pub fn is_nfd(input: &str) -> bool {
    if input.is_ascii() {
        return true;
    }
    let mut prev_ccc = 0u8;
    for ch in input.chars() {
        let cp = ch as u32;
        if (S_BASE..S_BASE + S_COUNT).contains(&cp) || canonical_decomposition(ch).is_some() {
            return false;
        }
        let ccc = canonical_combining_class(ch);
        if ccc != 0 && prev_ccc > ccc {
            return false;
        }
        prev_ccc = ccc;
    }
    true
}

/// Check whether `input` is already in NFC.
pub fn is_nfc(input: &str) -> bool {
    if input.is_ascii() {
        return true;
    }
    nfc(input) == input
}

fn decompose_ordered(input: &str) -> Vec<char> {
    let mut out = Vec::with_capacity(input.len());
    let mut work: Vec<char> = Vec::with_capacity(4);

    for ch in input.chars() {
        work.clear();
        work.push(ch);
        while let Some(cur) = work.pop() {
            let cp = cur as u32;
            if (S_BASE..S_BASE + S_COUNT).contains(&cp) {
                let s_idx = cp - S_BASE;
                let l = char::from_u32(L_BASE + s_idx / N_COUNT).unwrap();
                let v = char::from_u32(V_BASE + (s_idx % N_COUNT) / T_COUNT).unwrap();
                let t_idx = s_idx % T_COUNT;
                out.push(l);
                out.push(v);
                if t_idx > 0 {
                    let t = char::from_u32(T_BASE + t_idx).unwrap();
                    out.push(t);
                }
            } else if let Some((a, b)) = canonical_decomposition(cur) {
                if let Some(second) = b {
                    work.push(second);
                }
                work.push(a);
            } else {
                out.push(cur);
            }
        }
    }
    canonical_reorder(&mut out);
    out
}

/// Canonical Ordering Algorithm (UAX #15 / Unicode §3.11): stably sort every maximal run
/// of non-starters (`ccc != 0`) by combining class. Starters (`ccc == 0`) act as barriers.
///
/// Runs in `O(n log k)` for runs of length `k` (previously an `O(k^2)` insertion sort, which
/// allowed a DoS with long runs of reverse-ordered combining marks).
fn canonical_reorder(chars: &mut [char]) {
    let len = chars.len();
    let mut i = 0;
    while i < len {
        if canonical_combining_class(chars[i]) == 0 {
            i += 1;
            continue;
        }
        let start = i;
        let mut prev = 0u8;
        let mut sorted = true;
        while i < len {
            let ccc = canonical_combining_class(chars[i]);
            if ccc == 0 {
                break;
            }
            if ccc < prev {
                sorted = false;
            }
            prev = ccc;
            i += 1;
        }
        if !sorted {
            let run = &mut chars[start..i];
            let mut keyed: Vec<(u8, char)> = run
                .iter()
                .map(|&c| (canonical_combining_class(c), c))
                .collect();
            // `sort_by_key` is a stable sort: equal classes keep their original order.
            keyed.sort_by_key(|&(ccc, _)| ccc);
            for (slot, (_, c)) in run.iter_mut().zip(keyed) {
                *slot = c;
            }
        }
    }
}

#[inline]
fn try_compose(starter: char, ch: char) -> Option<char> {
    let a = starter as u32;
    let b = ch as u32;

    // Hangul L + V -> LV
    if (L_BASE..L_BASE + L_COUNT).contains(&a) && (V_BASE..V_BASE + V_COUNT).contains(&b) {
        let l_idx = a - L_BASE;
        let v_idx = b - V_BASE;
        return char::from_u32(S_BASE + (l_idx * V_COUNT + v_idx) * T_COUNT);
    }

    // Hangul LV + T -> LVT
    if (S_BASE..S_BASE + S_COUNT).contains(&a)
        && (a - S_BASE) % T_COUNT == 0
        && (T_BASE + 1..T_BASE + T_COUNT).contains(&b)
    {
        let t_idx = b - T_BASE;
        return char::from_u32(a + t_idx);
    }

    canonical_composition(starter, ch)
}
