// Copyright 2015 The Servo Project Developers. See the
// COPYRIGHT file at the top-level directory of this distribution.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! 3.3.4 - 3.3.6. Resolve implicit levels and types.

use std::cmp::max;
use matches::matches;

use super::char_data::BidiClass::{self, *};
use super::prepare::{IsolatingRunSequence, LevelRun, not_removed_by_x9, removed_by_x9};
use super::level::Level;

/// 3.3.4 Resolving Weak Types
///
/// <http://www.unicode.org/reports/tr9/#Resolving_Weak_Types>
#[cfg_attr(feature = "flame_it", flamer::flame)]
pub fn resolve_weak(sequence: &IsolatingRunSequence, processing_classes: &mut [BidiClass]) {
    // FIXME (#8): This function applies steps W1-W6 in a single pass.  This can produce
    // incorrect results in cases where a "later" rule changes the value of `prev_class` seen
    // by an "earlier" rule.  We should either split this into separate passes, or preserve
    // extra state so each rule can see the correct previous class.

    // FIXME: Also, this could be the cause of increased failure for using longer-UTF-8 chars in
    // conformance tests, like BidiTest:69635 (AL ET EN)

    let mut prev_class = sequence.sos;
    let mut last_strong_is_al = false;
    let mut et_run_indices = Vec::new(); // for W5

    // Like sequence.runs.iter().flat_map(Clone::clone), but make indices itself clonable.
    fn id(x: LevelRun) -> LevelRun {
        x
    }
    let mut indices = sequence.runs.iter().cloned().flat_map(
        id as fn(LevelRun) -> LevelRun,
    );

    while let Some(i) = indices.next() {
        match processing_classes[i] {
            // <http://www.unicode.org/reports/tr9/#W1>
            NSM => {
                processing_classes[i] = match prev_class {
                    RLI | LRI | FSI | PDI => ON,
                    _ => prev_class,
                };
            }
            EN => {
                if last_strong_is_al {
                    // W2. If previous strong char was AL, change EN to AN.
                    processing_classes[i] = AN;
                } else {
                    // W5. If a run of ETs is adjacent to an EN, change the ETs to EN.
                    for j in &et_run_indices {
                        processing_classes[*j] = EN;
                    }
                    et_run_indices.clear();
                }
            }
            // <http://www.unicode.org/reports/tr9/#W3>
            AL => processing_classes[i] = R,

            // <http://www.unicode.org/reports/tr9/#W4>
            ES | CS => {
                let next_class = indices
                    .clone()
                    .map(|j| processing_classes[j])
                    .find(not_removed_by_x9)
                    .unwrap_or(sequence.eos);
                processing_classes[i] = match (prev_class, processing_classes[i], next_class) {
                    (EN, ES, EN) | (EN, CS, EN) => EN,
                    (AN, CS, AN) => AN,
                    (_, _, _) => ON,
                }
            }
            // <http://www.unicode.org/reports/tr9/#W5>
            ET => {
                match prev_class {
                    EN => processing_classes[i] = EN,
                    _ => et_run_indices.push(i), // In case this is followed by an EN.
                }
            }
            class => {
                if removed_by_x9(class) {
                    continue;
                }
            }
        }

        prev_class = processing_classes[i];
        match prev_class {
            L | R => {
                last_strong_is_al = false;
            }
            AL => {
                last_strong_is_al = true;
            }
            _ => {}
        }
        if prev_class != ET {
            // W6. If we didn't find an adjacent EN, turn any ETs into ON instead.
            for j in &et_run_indices {
                processing_classes[*j] = ON;
            }
            et_run_indices.clear();
        }
    }

    // W7. If the previous strong char was L, change EN to L.
    let mut last_strong_is_l = sequence.sos == L;
    for run in &sequence.runs {
        for i in run.clone() {
            match processing_classes[i] {
                EN if last_strong_is_l => {
                    processing_classes[i] = L;
                }
                L => {
                    last_strong_is_l = true;
                }
                R | AL => {
                    last_strong_is_l = false;
                }
                _ => {}
            }
        }
    }
}

/// 3.3.5 Resolving Neutral Types
///
/// <http://www.unicode.org/reports/tr9/#Resolving_Neutral_Types>
#[cfg_attr(feature = "flame_it", flamer::flame)]
pub fn resolve_neutral(
    sequence: &IsolatingRunSequence,
    levels: &[Level],
    processing_classes: &mut [BidiClass],
) {
    let e: BidiClass = levels[sequence.runs[0].start].bidi_class();
    let mut indices = sequence.runs.iter().flat_map(Clone::clone);
    let mut prev_class = sequence.sos;

    while let Some(mut i) = indices.next() {
        // N0. Process bracket pairs.
        // TODO

        // Process sequences of NI characters.
        let mut ni_run = Vec::new();
        if is_NI(processing_classes[i]) {
            // Consume a run of consecutive NI characters.
            ni_run.push(i);
            let mut next_class;
            loop {
                match indices.next() {
                    Some(j) => {
                        i = j;
                        if removed_by_x9(processing_classes[i]) {
                            continue;
                        }
                        next_class = processing_classes[j];
                        if is_NI(next_class) {
                            ni_run.push(i);
                        } else {
                            break;
                        }
                    }
                    None => {
                        next_class = sequence.eos;
                        break;
                    }
                };
            }

            // N1-N2.
            //
            // <http://www.unicode.org/reports/tr9/#N1>
            // <http://www.unicode.org/reports/tr9/#N2>
            let new_class = match (prev_class, next_class) {
                (L, L) => L,
                (R, R) | (R, AN) | (R, EN) | (AN, R) | (AN, AN) | (AN, EN) | (EN, R) |
                (EN, AN) | (EN, EN) => R,
                (_, _) => e,
            };
            for j in &ni_run {
                processing_classes[*j] = new_class;
            }
            ni_run.clear();
        }
        prev_class = processing_classes[i];
    }
}

/// 3.3.6 Resolving Implicit Levels
///
/// Returns the maximum embedding level in the paragraph.
///
/// <http://www.unicode.org/reports/tr9/#Resolving_Implicit_Levels>
#[cfg_attr(feature = "flame_it", flamer::flame)]
pub fn resolve_levels(original_classes: &[BidiClass], levels: &mut [Level]) -> Level {
    let mut max_level = Level::ltr();

    assert_eq!(original_classes.len(), levels.len());
    for i in 0..levels.len() {
        match (levels[i].is_rtl(), original_classes[i]) {
            (false, AN) | (false, EN) => levels[i].raise(2).expect("Level number error"),
            (false, R) | (true, L) | (true, EN) | (true, AN) => {
                levels[i].raise(1).expect("Level number error")
            }
            (_, _) => {}
        }
        max_level = max(max_level, levels[i]);
    }

    max_level
}

/// Neutral or Isolate formatting character (B, S, WS, ON, FSI, LRI, RLI, PDI)
///
/// <http://www.unicode.org/reports/tr9/#NI>
#[allow(non_snake_case)]
fn is_NI(class: BidiClass) -> bool {
    matches!(class, B | S | WS | ON | FSI | LRI | RLI | PDI)
}
