//! Verdict aggregation (蓝图 §七 硬门槛 + 软加权). For milestone 1 the gate is the
//! hard-门槛 layer: any High/Hard finding fails the run; findings are sorted by
//! severity then turn so the red-board reads worst-first.

use crate::model::{Transcript, Verdict};
use crate::probes;

pub fn evaluate(t: &Transcript) -> Verdict {
    let mut findings = probes::run_all(t);
    findings.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| a.turn_ids.first().cmp(&b.turn_ids.first()))
    });
    Verdict { findings }
}
