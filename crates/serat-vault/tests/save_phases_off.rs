//! Task #207, NFR-PRF-03: with nothing having called `arm()` (this test binary never does),
//! the save-phase marks record nothing, so the product path reads no clock for them.

use serat_vault::save_phases::{self, SavePhase};

#[test]
fn unarmed_begin_mark_and_finish_record_nothing() {
    assert!(!save_phases::armed());
    save_phases::begin();
    save_phases::mark(SavePhase::Write);
    assert!(
        save_phases::finish().is_none(),
        "an unarmed process collects nothing"
    );
}
