use super::*;
use crate::memory::{Kind, Scope};
use crate::testing::memory;

#[test]
fn trust_explains_each_level_in_precedence_order() {
    let mut item = memory("trust", Kind::Note, Scope::Personal);
    let check = |item: &Memory, level, text: &str| {
        assert_eq!(item.trust(), (level, text.to_owned()));
    };
    check(&item, TrustLevel::Normal, "normal");
    item.proof_count = 2;
    item.usage.helpful = 1;
    item.usage.fetched = 3;
    check(&item, TrustLevel::Normal, "normal · used 3×");
    item.proof_count = 3;
    check(&item, TrustLevel::Proven, "proven · saved 3× · used 3×");
    item.proof_count = 1;
    item.usage.helpful = 4;
    item.usage.fetched = 12;
    check(&item, TrustLevel::Proven, "proven · 4 helpful · used 12×");
    item.rank = Rank::Candidate;
    check(&item, TrustLevel::New, "new · awaiting confirmation");
    item.origin = Origin::System;
    check(&item, TrustLevel::New, "new · system note");
    item.usage.wrong = 1;
    check(&item, TrustLevel::Doubted, "doubted · 1 wrong vote");
    item.status = Status::Stale;
    item.stale_cause = Some("feedback".into());
    item.usage.wrong = 2;
    check(&item, TrustLevel::Doubted, "doubted · 2 wrong votes");
    item.stale_cause = Some("missing".into());
    item.usage.wrong = 0;
    check(&item, TrustLevel::Doubted, "doubted · file missing");
}
