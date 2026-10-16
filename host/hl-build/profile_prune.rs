//! Keep only the hot functions of a sample profile.
//!
//! The game is built with `-profile-sample-accurate`, so a function the profile
//! does not name is treated as cold and optimised for size. A profile that names
//! every function the replays touched makes LLVM inline and unroll hot callees
//! into dozens of callers: on 2026-10-15 the unpruned profile grew `.text` by
//! 47 KB over the stale one and `play()` from 56 KB to 128 KB, for +19..+34%
//! frame rate. The samples are very concentrated (the top 33 of 561 functions
//! hold 80% of them), so the shipped profile keeps the functions that cover
//! most of the samples and leaves the rest cold. Functions that only wait
//! (CD, vblank, controller, SPU upload) or only run while loading are dropped
//! first: they are hot in the histogram but not in the frame.

/// What the pruning kept.
#[derive(Debug, PartialEq, Eq)]
pub struct Pruned {
    pub kept: usize,
    pub total: usize,
    /// Share of all samples the kept functions hold, in permille.
    pub covered_permille: u64,
}

/// Top-level function blocks of an AutoFDO text profile: the header line
/// `name:total:head` and every indented line under it.
fn blocks(text: &str) -> Vec<(String, u64, Vec<&str>)> {
    let mut blocks: Vec<(String, u64, Vec<&str>)> = Vec::new();
    for line in text.lines() {
        if !line.starts_with(' ') && !line.starts_with('#') && !line.is_empty() {
            let mut fields = line.rsplitn(3, ':');
            let _head = fields.next();
            let total = fields.next().and_then(|t| t.parse().ok()).unwrap_or(0);
            let name = fields.next().unwrap_or("").to_string();
            blocks.push((name, total, vec![line]));
        } else if let Some(block) = blocks.last_mut() {
            block.2.push(line);
        }
    }
    blocks
}

/// The profile with only its hottest functions, in their original order:
/// functions are taken by descending samples, skipping any whose name contains
/// one of `cold`, until they hold `coverage_permille` of the samples of the
/// functions that are not `cold` (the drive and the loader are not part of the
/// frame, so they are not part of the share either).
pub fn prune(text: &str, coverage_permille: u64, cold: &[&str]) -> (String, Pruned) {
    let blocks = blocks(text);
    let is_cold = |name: &str| cold.iter().any(|fragment| name.contains(fragment));
    let all: u64 = blocks
        .iter()
        .filter(|block| !is_cold(&block.0))
        .map(|block| block.1)
        .sum();
    let mut order: Vec<usize> = (0..blocks.len()).collect();
    order.sort_by(|&a, &b| blocks[b].1.cmp(&blocks[a].1).then(a.cmp(&b)));
    let mut keep = vec![false; blocks.len()];
    let mut covered = 0u64;
    for index in order {
        if covered * 1000 >= all * coverage_permille {
            break;
        }
        if is_cold(&blocks[index].0) {
            continue;
        }
        keep[index] = true;
        covered += blocks[index].1;
    }
    let mut out = String::new();
    let mut kept = 0;
    for (index, block) in blocks.iter().enumerate() {
        if keep[index] {
            kept += 1;
            for line in &block.2 {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    let covered_permille = if all == 0 { 0 } else { covered * 1000 / all };
    (
        out,
        Pruned {
            kept,
            total: blocks.len(),
            covered_permille,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROFILE: &str = "\
hot:600:0
 1: 600
cd_read:300:0
 1: 300
warm:90:0
 1: other:50
  2: 50
 3: 40
cold:10:0
 1: 10
";

    #[test]
    fn keeps_the_hottest_until_covered_and_skips_cold_names() {
        let (_, pruned) = prune(PROFILE, 700, &["cd_read"]);
        // Without cd_read the samples are 700: hot (600) is 857 permille of
        // them, so 700 is reached by hot alone; with 990 warm and cold join.
        assert_eq!(pruned.kept, 1);
        let (text, pruned) = prune(PROFILE, 990, &["cd_read"]);
        assert_eq!(pruned.kept, 3);
        assert!(
            text.contains("hot:600:0") && text.contains("warm:90:0") && text.contains("cold:10:0")
        );
        assert!(!text.contains("cd_read"));
        // Nested lines travel with their function and the order is kept.
        assert!(text.find("hot").unwrap() < text.find("warm").unwrap());
        assert!(text.contains("  2: 50"));
    }

    #[test]
    fn stops_once_the_coverage_is_reached() {
        let (_, pruned) = prune(PROFILE, 600, &[]);
        assert_eq!(pruned.kept, 1);
        assert_eq!(pruned.covered_permille, 600);
        assert_eq!(pruned.total, 4);
    }

    #[test]
    fn an_empty_profile_prunes_to_nothing() {
        let (text, pruned) = prune("", 800, &[]);
        assert!(text.is_empty());
        assert_eq!(pruned.kept, 0);
    }
}
