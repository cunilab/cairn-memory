use super::model::{Memory, Rank, Status, TrustLevel};
use crate::process::Origin;

pub const PROVEN_HELPFUL: u32 = 2;
pub const PROVEN_SAVES: u32 = 3;

impl Memory {
    /// Display trust from the current record and usage counters.
    pub fn trust(&self) -> (TrustLevel, String) {
        use TrustLevel::{Doubted, New, Normal, Proven};

        let usage = &self.usage;
        if self.status == Status::Stale || usage.wrong > 0 {
            let reason =
                if self.status == Status::Stale && self.stale_cause.as_deref() == Some("missing") {
                    "file missing".to_owned()
                } else if usage.wrong > 0 {
                    let plural = if usage.wrong == 1 { "" } else { "s" };
                    format!("{} wrong vote{plural}", usage.wrong)
                } else {
                    "stale".to_owned()
                };
            return (Doubted, format!("doubted · {reason}"));
        }
        if self.rank == Rank::Candidate {
            let reason = match self.origin {
                Origin::System => "system note",
                Origin::Agent => "awaiting confirmation",
            };
            return (New, format!("new · {reason}"));
        }
        let (level, mut line) = if usage.helpful >= PROVEN_HELPFUL {
            (Proven, format!("proven · {} helpful", usage.helpful))
        } else if self.proof_count >= PROVEN_SAVES {
            (Proven, format!("proven · saved {}×", self.proof_count))
        } else {
            (Normal, "normal".to_owned())
        };
        if usage.fetched > 0 {
            line = format!("{line} · used {}×", usage.fetched);
        }
        (level, line)
    }
}

#[cfg(test)]
mod tests;
