/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! Read-only hints, NOT reverse-engineered field names or write-safety proofs.
//! A nearby word or a value-change pattern can belong to unrelated game data.

use super::{Mem, SearchResult, VType};
use crate::mem::ConstVoidPtr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Category {
    Money,
    Ammo,
    Health,
    Score,
    Timer,
    #[default]
    Unknown,
}

impl Category {
    pub const ALL: [Self; 6] = [Self::Money, Self::Ammo, Self::Health, Self::Score, Self::Timer, Self::Unknown];

    pub const fn index(self) -> usize {
        self as usize
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Money => "Money?",
            Self::Ammo => "Ammo?",
            Self::Health => "Health?",
            Self::Score => "Score?",
            Self::Timer => "Timer?",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResultFilter {
    #[default]
    All,
    Category(Category),
}

impl ResultFilter {
    pub fn matches(self, category: Category) -> bool {
        self == Self::All || self == Self::Category(category)
    }

    pub fn next(self) -> Self {
        match self {
            Self::All => Self::Category(Category::ALL[0]),
            Self::Category(c) => Category::ALL.get(c.index() + 1)
                .copied().map_or(Self::All, Self::Category),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Category(c) => c.label(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Evidence {
    #[default]
    Pending,
    None,
    NearbyText,
    UnitDrops,
    SmoothDrops,
    TextAndChanges,
    ConflictingText,
    ScalarField,
    ReloadCycles,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Analysis {
    pub category: Category,
    evidence: Evidence,
    text_mask: u8,
    inspected: bool,
    unit_drops: u8,
    smooth_drops: u8,
    field_mask: u8,
    reloads: u8,
    peak: u16,
    last_rate: f32,
    quiet_seconds: f32,
}

impl Analysis {
    /// Categorical confidence only, never a made-up probability.
    pub fn description(self) -> &'static str {
        match self.evidence {
            Evidence::Pending => "Pending scan; not verified",
            Evidence::None => "No useful evidence",
            Evidence::NearbyText => "Nearby word; low confidence",
            Evidence::UnitDrops => "Unit drops; purpose ambiguous",
            Evidence::SmoothDrops => "Float countdown; low confidence",
            Evidence::TextAndChanges => "Word + changes; medium confidence",
            Evidence::ConflictingText => "Conflicting words; unknown",
            Evidence::ScalarField => "Exact typed ObjC field; stronger hint",
            Evidence::ReloadCycles => "Repeated depletion/refill; low confidence",
        }
    }

    /// The trainer's own edits must not teach the classifier a fake pattern.
    pub fn reset_history(&mut self) {
        self.unit_drops = 0;
        self.smooth_drops = 0;
        self.reloads = 0;
        self.peak = 0;
        self.last_rate = 0.0;
        self.quiet_seconds = 0.0;
        self.update_hint();
    }

    pub fn observe(&mut self, t: VType, before: u64, after: u64) {
        self.observe_timed(t, before, after, 0.25);
    }

    pub fn observe_timed(&mut self, t: VType, before: u64, after: u64, seconds: f32) {
        self.quiet_seconds = (self.quiet_seconds + seconds.max(0.0)).min(60.0);
        if before == after { return; }
        let elapsed = self.quiet_seconds.max(0.001);
        self.quiet_seconds = 0.0;
        let a = number(t, before);
        let b = number(t, after);
        let unit_drop = t != VType::F32 && (1.0..=300.0).contains(&a) && b == a - 1.0;
        if unit_drop {
            if self.unit_drops == 0 {
                if self.peak != a as u16 { self.reloads = 0; }
                self.peak = a as u16;
            }
            self.unit_drops = self.unit_drops.saturating_add(1);
        } else {
            if self.unit_drops >= 3 && b == self.peak as f64 && b > a {
                self.reloads = self.reloads.saturating_add(1);
            } else {
                self.reloads = 0;
            }
            self.unit_drops = 0;
        }
        let rate = ((a - b) / elapsed as f64) as f32;
        if t == VType::F32 && a.is_finite() && b.is_finite()
            && b >= 0.0 && a <= 86400.0 && a > b && a - b <= 2.0
            && (a.fract() != 0.0 || b.fract() != 0.0)
        {
            self.smooth_drops = if self.last_rate > 0.0
                && (0.7..=1.3).contains(&(rate / self.last_rate))
            { self.smooth_drops.saturating_add(1) } else { 1 };
            self.last_rate = rate;
        } else {
            self.smooth_drops = 0;
            self.last_rate = 0.0;
        }
        self.update_hint();
    }

    fn update_hint(&mut self) {
        self.category = Category::Unknown;
        self.evidence = if self.inspected { Evidence::None } else { Evidence::Pending };
        if self.field_mask.count_ones() == 1 {
            self.category = Category::ALL[self.field_mask.trailing_zeros() as usize];
            self.evidence = Evidence::ScalarField;
            return;
        }
        if self.field_mask.count_ones() > 1 || self.text_mask.count_ones() > 1 {
            self.evidence = Evidence::ConflictingText;
            return;
        }
        if let Some(category) = Category::ALL[..5].iter().copied()
            .find(|c| self.text_mask & (1 << c.index()) != 0)
        {
            self.category = category;
            self.evidence = if (category == Category::Ammo && self.reloads >= 2)
                || (category == Category::Timer && self.smooth_drops >= 6)
            { Evidence::TextAndChanges } else { Evidence::NearbyText };
        } else if self.reloads >= 2 {
            self.category = Category::Ammo;
            self.evidence = Evidence::ReloadCycles;
        } else if self.smooth_drops >= 6 {
            self.category = Category::Timer;
            self.evidence = Evidence::SmoothDrops;
        } else if self.unit_drops >= 3 {
            self.evidence = Evidence::UnitDrops;
        }
    }
}

pub(super) fn number(t: VType, bits: u64) -> f64 {
    match t {
        VType::Auto => f64::NAN,
        VType::U8 => bits as u8 as f64,
        VType::I8 => bits as i8 as f64,
        VType::U16 => bits as u16 as f64,
        VType::I16 => bits as i16 as f64,
        VType::U32 => bits as u32 as f64,
        VType::I32 => bits as i32 as f64,
        VType::F32 => f32::from_bits(bits as u32) as f64,
        VType::F64 => f64::from_bits(bits),
        VType::U64 => bits as f64,
        VType::I64 => bits as i64 as f64,
    }
}

fn word_category(word: &[u8]) -> Option<Category> {
    let groups: &[(Category, &[&[u8]])] = &[
        (Category::Money, &[b"money", b"coins", b"coin", b"gold", b"cash", b"currency", b"gems", b"credits", b"moneycount", b"coincount", b"currentcoins"]),
        (Category::Ammo, &[b"ammo", b"ammunition", b"ammocount", b"currentammo", b"bullets", b"bulletcount", b"magazine"]),
        (Category::Health, &[b"health", b"healthpoints", b"hitpoints", b"maxhealth", b"currenthealth"]),
        (Category::Score, &[b"score", b"highscore", b"points"]),
        (Category::Timer, &[b"timer", b"countdown", b"cooldown", b"remainingtime"]),
    ];
    groups.iter().find_map(|&(category, words)| {
        words.iter().any(|w| word.eq_ignore_ascii_case(w)).then_some(category)
    })
}

/// Only complete tokens: don't label "golden" as money or "healthcare" as HP.
/// The first/last tokens of a clipped window are deliberately not trusted.
fn ascii_word_mask(bytes: &[u8]) -> u8 {
    let mut mask = 0;
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_alphabetic() { i += 1; continue; }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_alphanumeric() { i += 1; }
        if start > 0 && i < bytes.len() && !bytes[start - 1].is_ascii_alphanumeric() {
            if let Some(c) = word_category(&bytes[start..i]) { mask |= 1 << c.index(); }
        }
    }
    mask
}

/// ASCII and ASCII-compatible UTF-16LE names, without allocating strings.
fn word_mask(bytes: &[u8]) -> u8 {
    let mut mask = ascii_word_mask(bytes);
    for offset in 0..2.min(bytes.len()) {
        let mut decoded = [0u8; 64];
        let mut len = 0;
        for pair in bytes[offset..].chunks_exact(2).take(decoded.len()) {
            decoded[len] = if pair[1] == 0 && pair[0].is_ascii() { pair[0] } else { 0 };
            len += 1;
        }
        mask |= ascii_word_mask(&decoded[..len]);
    }
    mask
}

/// Field identifiers can use camelCase, underscores, or acronym suffixes.
fn field_mask(name: &str) -> u8 {
    if let Some(category) = word_category(name.trim_matches('_').as_bytes()) {
        return 1 << category.index();
    }
    let mut normalized = Vec::with_capacity(130);
    normalized.push(0);
    let bytes = name.as_bytes();
    if bytes.len() > 128 { return 0; }
    for (i, &byte) in bytes.iter().enumerate() {
        if i > 0 && byte.is_ascii_uppercase() && bytes[i - 1].is_ascii_lowercase() {
            normalized.push(b'_');
        }
        normalized.push(byte);
    }
    normalized.push(0);
    let mut mask = ascii_word_mask(&normalized);
    for word in normalized.split(|b| !b.is_ascii_alphanumeric()) {
        if word.eq_ignore_ascii_case(b"hp") { mask |= 1 << Category::Health.index(); }
    }
    mask
}

pub(super) fn encoding(t: VType) -> u8 {
    match t {
        VType::I8 => b'c', VType::U8 => b'C', VType::I16 => b's', VType::U16 => b'S',
        VType::I32 => b'i', VType::U32 => b'I', VType::F32 => b'f', VType::Auto => 0,
        // Objective-C type encodings: `d` double, `q` long long, `Q` unsigned
        // long long. They are used to match a value against nearby property
        // metadata, so the letters have to be the real ones.
        VType::F64 => b'd', VType::I64 => b'q', VType::U64 => b'Q',
    }
}

/// Incremental bounded inspection: at most 1024 results and 128 neighbouring
/// bytes each per tick, staying inside the same currently-live allocation.
/// No arbitrary pointer chasing, guest execution or write probes.
pub(super) fn analyze_batch(mem: &Mem, results: &mut [SearchResult], cursor: &mut usize) {
    analyze_batch_with_objects(mem, results, cursor, None);
}

pub(super) fn analyze_batch_with_objects(
    mem: &Mem, results: &mut [SearchResult], cursor: &mut usize,
    objc: Option<&crate::objc::ObjC>,
) {
    if results.is_empty() { *cursor = 0; return; }
    let mut allocations = mem.live_allocations();
    allocations.sort_unstable_by_key(|&(base, _)| base);
    *cursor %= results.len();
    for _ in 0..results.len().min(1024) {
        let result = &mut results[*cursor];
        let addr = result.addr;
        let index = allocations.partition_point(|&(base, _)| base <= addr);
        let allocation = index.checked_sub(1).and_then(|i| allocations.get(i)).copied();
        let mask = allocation.and_then(|(base, size)| {
            let end = base as u64 + size as u64;
            if addr < mem.null_segment_size() || addr as u64 + result.vtype.size() as u64 > end {
                return None;
            }
            let lo = (addr as u64).saturating_sub(64).max(base as u64);
            let hi = (addr as u64 + result.vtype.size() as u64 + 64).min(end);
            let mut mask = 0;
            // Don't interpret the searched number's own bytes as a field name.
            for (start, stop) in [(lo, addr as u64), (addr as u64 + result.vtype.size() as u64, hi)] {
                if start == stop { continue; }
                let bytes = mem.get_bytes_fallible(ConstVoidPtr::from_bits(start as u32), (stop - start) as u32)?;
                mask |= word_mask(bytes);
            }
            Some(mask)
        });
        result.analysis.field_mask = allocation.and_then(|(base, size)| {
            objc?.diagnostic_scalar_field(mem, base, size, addr, result.vtype.size(), encoding(result.vtype))
        }).map_or(0, field_mask);
        if let Some(mask) = mask {
            result.analysis.inspected = true;
            result.analysis.text_mask = mask;
            result.analysis.update_hint();
        } else {
            result.analysis = Analysis { inspected: true, ..Analysis::default() };
            result.analysis.update_hint();
        }
        *cursor = (*cursor + 1) % results.len();
    }
}

#[cfg(test)]
mod tests;
