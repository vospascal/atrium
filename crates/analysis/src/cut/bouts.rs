//! Step 8, second half: bouts. A run of takes of one type, one after another
//! in the recording, is a bout; the preset's `bout` is the shortest and the
//! longest run.

use super::record::{Bouts, Run};

/// The runs in `types` (each take's type name, in take order).
pub(super) fn runs(types: &[String]) -> Bouts {
    let mut runs: Vec<Run> = Vec::new();
    for (index, type_name) in types.iter().enumerate() {
        match runs.last_mut() {
            Some(run) if run.type_name == *type_name => run.takes += 1,
            _ => runs.push(Run {
                type_name: type_name.clone(),
                first_take: index + 1,
                takes: 1,
            }),
        }
    }
    Bouts {
        shortest: runs.iter().map(|run| run.takes).min(),
        longest: runs.iter().map(|run| run.takes).max(),
        runs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_of_one_type() {
        let types: Vec<String> = "AAABBAA".chars().map(String::from).collect();
        let bouts = runs(&types);
        let lengths: Vec<usize> = bouts.runs.iter().map(|run| run.takes).collect();
        assert_eq!(lengths, [3, 2, 2]);
        assert_eq!((bouts.shortest, bouts.longest), (Some(2), Some(3)));
        assert_eq!(bouts.runs[1].first_take, 4);
    }
}
