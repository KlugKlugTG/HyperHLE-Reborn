/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use super::*;
use crate::mem::{MutVoidPtr, PAGE_SIZE};

pub(super) fn memory(size: u32) -> (Mem, u32) {
    let mut mem = Mem::new();
    mem.set_null_segment_size(PAGE_SIZE);
    let ptr = mem.alloc(size);
    mem.bytes_at_mut(ptr.cast(), size).fill(0xAA);
    (mem, ptr.to_bits())
}

pub(super) fn result(mem: &mut Mem, addr: u32, vtype: VType, value: &str) -> SearchResult {
    let bits = vtype.parse(value).unwrap();
    assert!(vtype.write_at(mem, addr, bits));
    SearchResult { addr, vtype, bits, changed: false, analysis: Analysis::default() }
}

fn snapshot(mem: &Mem, addr: u32, size: u32) -> Vec<u8> {
    mem.get_bytes_fallible(ConstVoidPtr::from_bits(addr), size).unwrap().to_vec()
}

#[test]
fn values_must_fit_their_integer_type() {
    for (t, min, max, below, above) in [
        (VType::U8, "0", "255", "-1", "256"),
        (VType::I8, "-128", "127", "-129", "128"),
        (VType::U16, "0", "65535", "-1", "65536"),
        (VType::I16, "-32768", "32767", "-32769", "32768"),
        (VType::U32, "0", "4294967295", "-1", "4294967296"),
        (VType::I32, "-2147483648", "2147483647", "-2147483649", "2147483648"),
    ] {
        assert!(t.parse(min).is_some(), "{t:?}");
        assert!(t.parse(max).is_some(), "{t:?}");
        assert_eq!(t.parse(below), None, "{t:?}");
        assert_eq!(t.parse(above), None, "{t:?}");
    }
    assert_eq!(VType::U8.parse("600"), None);
    assert_eq!(VType::I8.parse("0xff"), Some(255));
    assert_eq!(VType::I8.parse("0x100"), None);
    assert_eq!(VType::I16.parse("-1"), Some(65535));
}

#[test]
fn float_input_is_numeric_not_an_integer_bit_pattern() {
    assert_eq!(VType::F32.parse("600"), Some(600.0f32.to_bits() as u64));
    assert_eq!(VType::F32.parse("12.5"), Some(12.5f32.to_bits() as u64));
    assert_eq!(VType::F32.parse("-1.5"), Some((-1.5f32).to_bits() as u64));
    for invalid in ["NaN", "inf", "1e100", ""] {
        assert_eq!(VType::F32.parse(invalid), None);
    }
}

#[test]
fn sixty_four_bit_types_parse_format_and_scan() {
    // Money stored as a double or as a 64-bit integer is invisible to a
    // 32-bit-only scanner; these are the types that fix that.
    assert_eq!(VType::F64.parse("9873"), Some(9873.0f64.to_bits()));
    assert_eq!(VType::F64.parse("12.5"), Some(12.5f64.to_bits()));
    assert_eq!(VType::U64.parse("18446744073709551615"), Some(u64::MAX));
    assert_eq!(VType::I64.parse("-9223372036854775808"), Some(i64::MIN as u64));
    for invalid in ["NaN", "inf", "", "1e400"] {
        assert_eq!(VType::F64.parse(invalid), None, "{invalid:?}");
    }
    assert_eq!(VType::U64.parse("-1"), None);
    assert_eq!(VType::U64.parse("18446744073709551616"), None);

    assert_eq!((VType::F64.size(), VType::U64.size(), VType::I64.size()), (8, 8, 8));
    assert_eq!(VType::F64.format(9873.0f64.to_bits()), "9873.0000");
    assert_eq!(VType::U64.format(u64::MAX), "18446744073709551615");
    assert_eq!(VType::I64.format((-5i64) as u64), "-5");

    let (mut mem, base) = memory(64);
    let double = result(&mut mem, base, VType::F64, "9873");
    let long = result(&mut mem, base + 16, VType::U64, "9873");
    for (t, expected) in [(VType::F64, double.addr), (VType::U64, long.addr)] {
        let hits = search_all(&mem, t, "9873", None);
        assert!(hits.iter().any(|r| r.addr == expected), "{t:?}");
    }
    // AUTO has to consider the 64-bit types too, otherwise a value stored as
    // `double` is only findable when the user guesses the exact type first.
    // 9873.0 has a bit pattern no narrower type can produce, so AUTO must
    // report it as the F64 it is.
    let hits = search_all(&mem, VType::Auto, "9873", None);
    assert!(hits.iter().any(|r| r.addr == double.addr && r.vtype == VType::F64));
    // The same value in eight bytes is also an exact match for the narrower
    // types at that address (the upper bytes are zero). AUTO keeps the first
    // one in `auto_types()` order, which is a 32-bit integer: a narrow write
    // cannot clobber the neighbouring bytes. The address is what matters here.
    assert!(hits.iter().any(|r| r.addr == long.addr));
    assert!(!hits.iter().any(|r| r.addr == long.addr && r.vtype.size() > 4));
    // ... and a refine from AUTO must keep working on them.
    let refined = search_all(&mem, VType::Auto, "9873", Some(&hits));
    assert!(refined.iter().any(|r| r.addr == double.addr));
    assert!(refined.iter().any(|r| r.addr == long.addr));
    // A value that does not fit in 32 bits has no narrower interpretation at
    // all, so AUTO must report its 64-bit type.
    let big = result(&mut mem, base + 32, VType::U64, "5000000000");
    let auto = search_all(&mem, VType::Auto, "5000000000", None);
    assert!(auto.iter().any(|r| r.addr == big.addr && r.vtype == VType::U64));
    assert!(!auto.iter().any(|r| r.addr == big.addr && r.vtype != VType::U64));
}

#[test]
fn sixty_four_bit_values_can_be_written_and_frozen() {
    let (mut mem, base) = memory(64);
    let hit = result(&mut mem, base, VType::F64, "135");
    assert_eq!(VType::F64.read_at(&mem, base), Some(135.0f64.to_bits()));
    assert!(VType::F64.write_at(&mut mem, hit.addr, 999999.0f64.to_bits()));
    assert_eq!(VType::F64.read_at(&mem, base), Some(999999.0f64.to_bits()));
    // A 64-bit value must not spill past the end of guest memory.
    assert!(!VType::U64.write_at(&mut mem, u32::MAX - 3, 1));
}

#[test]
fn auto_search_does_not_find_truncated_values() {
    let (mut mem, base) = memory(64);
    let integer = result(&mut mem, base, VType::I32, "600");
    let float = result(&mut mem, base + 8, VType::F32, "600");
    let byte = result(&mut mem, base + 16, VType::U8, "88");
    let hits = search_all(&mem, VType::Auto, "600", None);
    assert!(hits.iter().any(|r| r.addr == integer.addr && r.vtype == VType::I32));
    assert!(hits.iter().any(|r| r.addr == float.addr && r.vtype == VType::F32));
    assert!(!hits.iter().any(|r| matches!(r.vtype, VType::U8 | VType::I8)));
    let refined = search_all(&mem, VType::Auto, "600", Some(&[integer, float, byte]));
    assert_eq!(refined.len(), 2);
    assert_eq!(refined[0].addr, integer.addr);
    assert_eq!(refined[1].addr, float.addr);
}

#[test]
fn empty_refine_never_restarts_a_search() {
    let (mut mem, base) = memory(64);
    result(&mut mem, base, VType::I32, "600");
    for t in [VType::I32, VType::Auto] {
        assert!(search_all(&mem, t, "600", Some(&[])).is_empty());
        assert!(!search_all(&mem, t, "600", None).is_empty());
    }
}

#[test]
fn preview_skips_overflow_without_writing_anything() {
    let (mut mem, base) = memory(64);
    let mut hits = [
        result(&mut mem, base, VType::I32, "100"),
        result(&mut mem, base + 8, VType::U8, "100"),
    ];
    let before = snapshot(&mem, base, 64);
    let plan = plan_bulk(&mem, &hits, VType::Auto, "999999", true, ResultFilter::All).unwrap();
    assert_eq!((plan.writes.len(), plan.skipped), (1, 1));
    assert_eq!(snapshot(&mem, base, 64), before);
    assert_eq!(hits[0].bits, 100);
    assert_eq!(apply_bulk(&mut mem, &mut hits, &plan), Ok(1));
    assert_eq!(VType::I32.read_at(&mem, base), Some(999999));
    assert_eq!(snapshot(&mem, base + 4, 60), before[4..]);
}

#[test]
fn changing_ui_type_cannot_widen_a_bulk_write() {
    let (mut mem, base) = memory(64);
    let hits = [result(&mut mem, base, VType::U8, "100")];
    let before = snapshot(&mem, base, 64);
    assert!(plan_bulk(&mem, &hits, VType::I32, "999999", true, ResultFilter::All).is_err());
    assert_eq!(snapshot(&mem, base, 64), before);
}

#[test]
fn overlapping_results_are_all_skipped_not_arbitrarily_selected() {
    let (mut mem, base) = memory(64);
    let wide = result(&mut mem, base, VType::I32, "600");
    let narrow = SearchResult { vtype: VType::U16, ..wide };
    let good = result(&mut mem, base + 8, VType::I32, "600");
    let before = snapshot(&mem, base, 64);
    for hits in [[wide, narrow, good], [narrow, wide, good], [wide, wide, good]] {
        let plan = plan_bulk(&mem, &hits, VType::Auto, "1000", true, ResultFilter::All).unwrap();
        assert_eq!((plan.writes.len(), plan.skipped), (1, 2));
        assert_eq!(snapshot(&mem, base, 64), before);
    }
    let mut hits = [wide, narrow, good];
    let plan = plan_bulk(&mem, &hits, VType::Auto, "1000", true, ResultFilter::All).unwrap();
    assert_eq!(apply_bulk(&mut mem, &mut hits, &plan), Ok(1));
    assert_eq!(VType::I32.read_at(&mem, base), Some(600));
    assert_eq!(VType::I32.read_at(&mem, base + 8), Some(1000));
}

#[test]
fn more_than_32_matches_are_previewed_and_written_without_truncation() {
    const COUNT: usize = 1325;
    let size = (COUNT * 4) as u32;
    let (mut mem, base) = memory(size);
    let mut hits: Vec<_> = (0..COUNT)
        .map(|i| result(&mut mem, base + i as u32 * 4, VType::I32, "135"))
        .collect();
    let before = snapshot(&mem, base, size);
    let plan = plan_bulk(&mem, &hits, VType::Auto, "999", true, ResultFilter::All).unwrap();
    assert_eq!((plan.writes.len(), plan.skipped), (COUNT, 0));
    assert_eq!(snapshot(&mem, base, size), before);
    assert_eq!(apply_bulk(&mut mem, &mut hits, &plan), Ok(COUNT));
    assert!(hits.iter().all(|hit| VType::I32.read_at(&mem, hit.addr) == Some(999)));
}

#[test]
fn changed_or_freed_memory_after_preview_prevents_every_write() {
    let (mut mem, base) = memory(64);
    let other = mem.alloc(64).to_bits();
    let mut hits = [
        result(&mut mem, base, VType::I32, "600"),
        result(&mut mem, other, VType::I32, "600"),
    ];
    let plan = plan_bulk(&mem, &hits, VType::Auto, "1000", true, ResultFilter::All).unwrap();
    assert!(VType::I32.write_at(&mut mem, other, 601));
    assert!(apply_bulk(&mut mem, &mut hits, &plan).is_err());
    assert_eq!(VType::I32.read_at(&mem, base), Some(600));
    assert!(VType::I32.write_at(&mut mem, other, 600));
    mem.free(MutVoidPtr::from_bits(other));
    assert!(apply_bulk(&mut mem, &mut hits, &plan).is_err());
    assert_eq!(VType::I32.read_at(&mem, base), Some(600));
}

#[test]
fn ineligible_hits_are_filtered_and_counted() {
    let (mut mem, base) = memory(64);
    let good = result(&mut mem, base, VType::I32, "600");
    let unaligned = result(&mut mem, base + 9, VType::I32, "600");
    let outside = SearchResult { addr: base + 64, ..good };
    let stale = result(&mut mem, base + 16, VType::I32, "600");
    assert!(VType::I32.write_at(&mut mem, stale.addr, 601));
    let unchanged = result(&mut mem, base + 24, VType::I32, "1000");
    let mut hits = [good, unaligned, outside, stale, unchanged];
    let before = snapshot(&mem, base, 64);
    let plan = plan_bulk(&mem, &hits, VType::Auto, "1000", true, ResultFilter::All).unwrap();
    assert_eq!((plan.writes.len(), plan.skipped), (1, 4));
    assert_eq!(snapshot(&mem, base, 64), before);
    assert_eq!(apply_bulk(&mut mem, &mut hits, &plan), Ok(1));
    assert_eq!(snapshot(&mem, base + 4, 60), before[4..]);
}

#[test]
fn valid_batch_preserves_types_neighbours_and_displayed_values() {
    let (mut mem, base) = memory(64);
    let mut hits = [
        result(&mut mem, base, VType::I32, "600"),
        result(&mut mem, base + 8, VType::U16, "600"),
        result(&mut mem, base + 16, VType::F32, "600"),
    ];
    let plan = plan_bulk(&mem, &hits, VType::Auto, "1000", true, ResultFilter::All).unwrap();
    assert_eq!(apply_bulk(&mut mem, &mut hits, &plan), Ok(3));
    for hit in hits {
        let expected = hit.vtype.parse("1000").unwrap();
        assert_eq!(hit.bits, expected);
        assert_eq!(hit.vtype.read_at(&mem, hit.addr), Some(expected));
        assert_eq!(VType::U8.read_at(&mem, hit.addr + hit.vtype.size()), Some(0xAA));
    }
}

#[test]
fn invalid_memory_accesses_fail_without_a_panic_or_sink_write() {
    let (mut mem, _) = memory(64);
    for addr in [0, PAGE_SIZE - 1, u32::MAX - 1] {
        assert_eq!(VType::I32.read_at(&mem, addr), None);
        assert!(!VType::I32.write_at(&mut mem, addr, 123));
    }
    assert!(plan_bulk(&mem, &[], VType::Auto, "1000", true, ResultFilter::All).is_err());
}

fn bulk_command(text: &str, confirm: bool) -> TrainerCmd {
    TrainerCmd::SetAll { vtype: VType::I32, text: text.to_string(), confirm, safe_mode: true, filter: ResultFilter::All }
}

#[test]
fn set_all_requires_an_explicit_confirmation_of_the_preview() {
    let (mut mem, base) = memory(64);
    let mut trainer = Trainer::new(true);
    trainer.state.results = vec![result(&mut mem, base, VType::I32, "135")];
    trainer.handle_command(&mut mem, bulk_command("999", false));
    assert!(trainer.state.pending_bulk.is_some());
    assert_eq!(VType::I32.read_at(&mem, base), Some(135));
    // Two rapid clicks queued before the UI shows CONFIRM are only previews.
    trainer.handle_command(&mut mem, bulk_command("999", false));
    assert_eq!(VType::I32.read_at(&mem, base), Some(135));
    trainer.handle_command(&mut mem, bulk_command("999", true));
    assert_eq!(VType::I32.read_at(&mem, base), Some(999));
    assert!(trainer.state.pending_bulk.is_none());
}

#[test]
fn changed_values_or_input_require_a_new_confirmation() {
    let (mut mem, base) = memory(64);
    let mut trainer = Trainer::new(true);
    trainer.state.results = vec![
        result(&mut mem, base, VType::I32, "135"),
        result(&mut mem, base + 8, VType::I32, "135"),
    ];
    trainer.handle_command(&mut mem, bulk_command("999", false));
    assert!(VType::I32.write_at(&mut mem, base + 8, 136));
    trainer.handle_command(&mut mem, bulk_command("999", true));
    assert_eq!(VType::I32.read_at(&mem, base), Some(135));
    let plan = &trainer.state.pending_bulk.as_ref().unwrap().plan;
    assert_eq!((plan.writes.len(), plan.skipped), (1, 1));
    trainer.handle_command(&mut mem, bulk_command("1000", true));
    assert_eq!(VType::I32.read_at(&mem, base), Some(135));
    trainer.handle_command(&mut mem, bulk_command("1000", true));
    assert_eq!(VType::I32.read_at(&mem, base), Some(1000));
    assert_eq!(VType::I32.read_at(&mem, base + 8), Some(136));
}

#[test]
fn cancelled_or_expired_confirmation_cannot_write() {
    let (mut mem, base) = memory(64);
    let mut trainer = Trainer::new(true);
    trainer.state.results = vec![result(&mut mem, base, VType::I32, "135")];
    trainer.handle_command(&mut mem, bulk_command("999", false));
    trainer.handle_command(&mut mem, TrainerCmd::CancelBulk);
    assert!(trainer.state.pending_bulk.is_none());
    trainer.handle_command(&mut mem, bulk_command("999", true));
    assert_eq!(VType::I32.read_at(&mem, base), Some(135));
    trainer.state.pending_bulk.as_mut().unwrap().created_at =
        Instant::now() - BULK_CONFIRM_TIMEOUT;
    trainer.handle_command(&mut mem, bulk_command("999", true));
    assert_eq!(VType::I32.read_at(&mem, base), Some(135));
    assert!(trainer.state.pending_bulk.is_some());
}


#[test]
fn safe_mode_filters_unaligned_hits_while_normal_mode_includes_them() {
    let (mut mem, base) = memory(64);
    let mut hits = [
        result(&mut mem, base, VType::I32, "135"),
        result(&mut mem, base + 9, VType::I32, "135"),
    ];
    let safe = plan_bulk(&mem, &hits, VType::I32, "999", true, ResultFilter::All).unwrap();
    assert_eq!((safe.writes.len(), safe.skipped), (1, 1));
    let normal = plan_bulk(&mem, &hits, VType::I32, "999", false, ResultFilter::All).unwrap();
    assert_eq!((normal.writes.len(), normal.skipped), (2, 0));
    assert_eq!(VType::I32.read_at(&mem, base + 9), Some(135));
    assert_eq!(apply_bulk(&mut mem, &mut hits, &normal), Ok(2));
    assert_eq!(VType::I32.read_at(&mem, base + 9), Some(999));
}

#[test]
fn mode_change_cannot_confirm_another_modes_preview() {
    let (mut mem, base) = memory(64);
    let mut trainer = Trainer::new(true);
    trainer.state.results = vec![result(&mut mem, base, VType::I32, "135")];
    trainer.handle_command(&mut mem, bulk_command("999", false));
    let normal_confirm = TrainerCmd::SetAll {
        vtype: VType::I32, text: "999".to_string(), confirm: true, safe_mode: false, filter: ResultFilter::All,
    };
    trainer.handle_command(&mut mem, normal_confirm.clone());
    assert_eq!(VType::I32.read_at(&mem, base), Some(135));
    trainer.handle_command(&mut mem, normal_confirm);
    assert_eq!(VType::I32.read_at(&mem, base), Some(999));
}

#[test]
fn normal_mode_still_rejects_invalid_types_and_expired_addresses() {
    let (mut mem, base) = memory(64);
    let good = result(&mut mem, base, VType::I32, "135");
    let narrow = result(&mut mem, base + 8, VType::U8, "135");
    let invalid = SearchResult { addr: base + 64, ..good };
    let before = snapshot(&mem, base, 64);
    assert!(plan_bulk(&mem, &[good, narrow], VType::Auto, "999", false, ResultFilter::All).is_err());
    assert!(plan_bulk(&mem, &[good, invalid], VType::I32, "999", false, ResultFilter::All).is_err());
    assert_eq!(snapshot(&mem, base, 64), before);
}

#[test]
fn normal_mode_reports_actual_contents_after_overlapping_writes() {
    let (mut mem, base) = memory(64);
    let wide = result(&mut mem, base, VType::I32, "135");
    let narrow = SearchResult {
        addr: base + 2, vtype: VType::U16, bits: 0, changed: false, analysis: Analysis::default(),
    };
    let mut hits = [wide, narrow];
    assert!(plan_bulk(&mem, &hits, VType::Auto, "999", true, ResultFilter::All).is_err());
    let plan = plan_bulk(&mem, &hits, VType::Auto, "999", false, ResultFilter::All).unwrap();
    assert_eq!(apply_bulk(&mut mem, &mut hits, &plan), Ok(2));
    assert_eq!(hits[0].bits, (999 << 16) | 999);
    assert_eq!(hits[1].bits, 999);
    for hit in hits {
        assert_eq!(hit.vtype.read_at(&mem, hit.addr), Some(hit.bits));
    }
}

#[test]
fn category_bulk_only_targets_matches_in_the_selected_group() {
    use classify::Category;
    let (mut mem, base) = memory(128);
    let mut hits = [
        result(&mut mem, base, VType::I32, "135"),
        result(&mut mem, base + 16, VType::I32, "135"),
        result(&mut mem, base + 32, VType::I32, "135"),
    ];
    hits[0].analysis.category = Category::Money;
    hits[1].analysis.category = Category::Ammo;
    let filter = ResultFilter::Category(Category::Money);
    let before = snapshot(&mem, base, 128);
    let plan = plan_bulk(&mem, &hits, VType::I32, "999", true, filter).unwrap();
    assert_eq!((plan.writes.len(), plan.skipped), (1, 0));
    assert_eq!(snapshot(&mem, base, 128), before);
    assert_eq!(apply_bulk(&mut mem, &mut hits, &plan), Ok(1));
    assert_eq!(VType::I32.read_at(&mem, base), Some(999));
    assert_eq!(snapshot(&mem, base + 4, 124), before[4..]);
}

#[test]
fn changed_category_rejects_an_entire_pending_batch() {
    use classify::Category;
    let (mut mem, base) = memory(64);
    let mut hits = [
        result(&mut mem, base, VType::I32, "135"),
        result(&mut mem, base + 16, VType::I32, "135"),
    ];
    for hit in &mut hits { hit.analysis.category = Category::Money; }
    let plan = plan_bulk(&mem, &hits, VType::I32, "999", true,
        ResultFilter::Category(Category::Money)).unwrap();
    let before = snapshot(&mem, base, 64);
    hits[1].analysis.category = Category::Unknown;
    assert!(apply_bulk(&mut mem, &mut hits, &plan).is_err());
    assert_eq!(snapshot(&mem, base, 64), before);
}

#[test]
fn switching_group_cannot_confirm_another_groups_preview() {
    use classify::Category;
    let (mut mem, base) = memory(64);
    let mut trainer = Trainer::new(true);
    let mut hit = result(&mut mem, base, VType::I32, "135");
    hit.analysis.category = Category::Money;
    trainer.state.results = vec![hit];
    trainer.handle_command(&mut mem, bulk_command("999", false));
    // Same writes but a different filter must still require a new preview.
    let command = TrainerCmd::SetAll {
        vtype: VType::I32, text: "999".to_string(), confirm: true,
        safe_mode: true, filter: ResultFilter::Category(Category::Money),
    };
    trainer.handle_command(&mut mem, command.clone());
    assert_eq!(VType::I32.read_at(&mem, base), Some(135));
    trainer.handle_command(&mut mem, command);
    assert_eq!(VType::I32.read_at(&mem, base), Some(999));
}

#[test]
fn trainer_writes_reset_observations_for_overlapping_aliases() {
    use classify::Category;
    let (mut mem, base) = memory(64);
    let mut hits = [result(&mut mem, base + 1, VType::U8, "27")];
    for (old, new) in [(30, 29), (29, 28), (28, 27), (27, 30), (30, 29), (29, 28), (28, 27), (27, 30)] {
        hits[0].analysis.observe(VType::U8, old, new);
    }
    assert_eq!(hits[0].analysis.category, Category::Ammo);
    VType::I32.write_at(&mut mem, base, 0);
    record_trainer_write(&mem, &mut hits, base, 4);
    assert_eq!(hits[0].bits, 0);
    assert_eq!(hits[0].analysis.category, Category::Unknown);
}

#[test]
fn frozen_aliases_do_not_teach_ammo_patterns() {
    use classify::Category;
    let (mut mem, base) = memory(64);
    let mut trainer = Trainer::new(true);
    trainer.state.results = vec![result(&mut mem, base + 1, VType::U8, "30")];
    trainer.state.frozen.push(Patch { addr: base, vtype: VType::I32, bits: 0, freeze: true });
    for value in [29, 28, 27] {
        VType::U8.write_at(&mut mem, base + 1, value);
        trainer.refresh_live_values(&mut mem, None, 0.25);
    }
    assert_eq!(trainer.state.results[0].analysis.category, Category::Unknown);
}

#[test]
fn activity_reports_before_after_but_not_trainer_writes_or_frozen_aliases() {
    let (mut mem, base) = memory(64);
    let mut trainer = Trainer::new(true);
    trainer.state.results = vec![result(&mut mem, base, VType::I32, "30")];
    VType::I32.write_at(&mut mem, base, 29);
    trainer.refresh_live_values(&mut mem, None, 0.25);
    let change = trainer.state.activity.entries[0];
    assert_eq!((change.addr, change.before, change.after), (base, 30, 29));
    trainer.state.activity = Activity::default();
    trainer.handle_command(&mut mem, TrainerCmd::ApplyHack { addr: base, vtype: VType::I32, bits: 28 });
    trainer.refresh_live_values(&mut mem, None, 0.25);
    assert!(trainer.state.activity.entries.is_empty());
    trainer.state.frozen.push(Patch { addr: base, vtype: VType::I32, bits: 28, freeze: true });
    VType::I32.write_at(&mut mem, base, 27);
    trainer.refresh_live_values(&mut mem, None, 0.25);
    assert!(trainer.state.activity.entries.is_empty());
}

#[test]
fn mark_comparison_is_read_only_and_rebases_after_filtering() {
    let (mut mem, base) = memory(64);
    let mut trainer = Trainer::new(true);
    trainer.state.results = vec![result(&mut mem, base, VType::I32, "30"),
        result(&mut mem, base + 8, VType::I32, "30")];
    trainer.handle_command(&mut mem, TrainerCmd::Mark);
    VType::I32.write_at(&mut mem, base, 29);
    trainer.refresh_live_values(&mut mem, None, 0.25);
    let before = snapshot(&mem, base, 64);
    trainer.handle_command(&mut mem, TrainerCmd::Compare(watch::WatchFilter::Decreased));
    assert_eq!(trainer.state.results.len(), 1);
    assert_eq!(trainer.state.results[0].addr, base);
    assert_eq!(snapshot(&mem, base, 64), before);
    trainer.handle_command(&mut mem, TrainerCmd::Compare(watch::WatchFilter::Changed));
    assert!(trainer.state.results.is_empty());
}

#[test]
fn watch_write_uses_explicit_concrete_target_even_after_value_changes() {
    let (mut mem, base) = memory(64);
    let mut trainer = Trainer::new(true);
    trainer.state.results = vec![result(&mut mem, base, VType::U8, "30"),
        result(&mut mem, base + 8, VType::I32, "30")];
    trainer.state.snapshot = Some(Snapshot::capture(&mem, &trainer.state.results));
    VType::U8.write_at(&mut mem, base, 28);
    let before = snapshot(&mem, base, 64);
    assert_eq!(trainer.set_watch_value(&mut mem, base, VType::U8, "99"), Ok(99));
    assert_eq!(snapshot(&mem, base + 1, 63), before[1..]);
    assert_eq!(trainer.state.results[0].bits, 99);
    assert!(trainer.state.snapshot.is_none());
    trainer.refresh_live_values(&mut mem, None, 0.25);
    assert!(trainer.state.activity.entries.is_empty(), "own writes are not game events");
}

#[test]
fn watch_write_rejects_overflow_type_changes_frozen_aliases_and_expiry() {
    let (mut mem, base) = memory(64);
    let mut trainer = Trainer::new(true);
    trainer.state.results = vec![result(&mut mem, base + 1, VType::U8, "30")];
    let before = snapshot(&mem, base, 64);
    assert!(trainer.set_watch_value(&mut mem, base + 1, VType::U8, "999").is_err());
    assert!(trainer.set_watch_value(&mut mem, base + 1, VType::I32, "99").is_err());
    assert!(trainer.set_watch_value(&mut mem, base + 1, VType::Auto, "99").is_err());
    assert!(trainer.set_watch_value(&mut mem, base + 8, VType::U8, "99").is_err());
    trainer.state.frozen.push(Patch { addr: base, vtype: VType::I32, bits: 0, freeze: true });
    assert!(trainer.set_watch_value(&mut mem, base + 1, VType::U8, "99").is_err());
    assert_eq!(snapshot(&mem, base, 64), before);
    trainer.state.frozen.clear();
    mem.free(MutVoidPtr::from_bits(base));
    assert!(trainer.set_watch_value(&mut mem, base + 1, VType::U8, "99").is_err());
}
