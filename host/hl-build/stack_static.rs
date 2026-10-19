//! Static worst-case stack depth of the shipped game, from its ELF.
//!
//! The emulator watermark (stack_gate.rs) only sees the paths a scenario walks.
//! The deepest stack the game can reach is set by its recursive functions: the
//! BSP clip-hull traces recurse once per plane they straddle, 136 B a level,
//! down trees 84 levels deep on the campaign's worst map. This reads
//! `llvm-objdump -d` of the link (every `addiu $sp, $sp, -N` is a frame, every
//! `jal`/`j` an edge), collapses cycles with Tarjan's algorithm, and bounds each
//! cycle by its frame times the depth of the data it walks.
//!
//! Indirect calls (`jalr`) are not edges; the build has three, none recursive.

use std::collections::{BTreeMap, HashMap};

/// Depths the recursion bounds use, measured from the retail BSPs the campaign
/// ships (`clipnodes` of hulls 1..3 and `nodes` of hull 0, over the 103 maps).
/// Raise them when a map is recooked deeper.
#[derive(Clone, Copy, Debug)]
pub struct Depths {
    /// Deepest clip tree (c2a4a, hull 1: 84 levels), plus the root frame.
    pub clip_frames: u32,
    /// Deepest hull-0 node tree (c2a4a: 63 levels), plus two.
    pub hull0_frames: u32,
}

pub const CAMPAIGN_DEPTHS: Depths = Depths {
    clip_frames: 85,
    hull0_frames: 65,
};

/// Frames stacked by the logic chain per hop (`logic_use_entity` and its
/// targets) and the hop limit the code enforces (`depth > 8`).
const LOGIC_FRAMES_PER_HOP: usize = 4;
const LOGIC_HOPS: u32 = 9;
/// `SOFT_SPLIT_MAX_DEPTH` (6) plus the root.
const SOFT_CELL_FRAMES: u32 = 7;
/// Most levels `emit_residue_children` is asked for (call sites pass 1 to 3).
const RESIDUE_FRAMES: u32 = 4;

#[derive(Debug)]
pub struct Analysis {
    /// Bytes of stack below `main`'s entry, worst case.
    pub worst: u32,
    /// What sets it.
    pub limiter: String,
    /// One line per recursive function group: its bound.
    pub recursion: Vec<(u32, String)>,
    /// The deepest plain call chains, by the function they end in.
    pub chains: Vec<(u32, String)>,
}

struct Func {
    name: String,
    frame: u32,
    calls: Vec<u32>,
}

fn parse(text: &str) -> BTreeMap<u32, Func> {
    let mut funcs: BTreeMap<u32, Func> = BTreeMap::new();
    let mut current = None;
    for line in text.lines() {
        if line.ends_with(">:") && !line.starts_with(' ') {
            if let Some((addr, rest)) = line.split_once(' ') {
                if let Ok(addr) = u32::from_str_radix(addr, 16) {
                    funcs.insert(
                        addr,
                        Func {
                            name: rest
                                .trim_start_matches('<')
                                .trim_end_matches(">:")
                                .to_string(),
                            frame: 0,
                            calls: Vec::new(),
                        },
                    );
                    current = Some(addr);
                    continue;
                }
            }
        }
        let Some(current) = current else { continue };
        let Some((addr, rest)) = line.trim_start().split_once(':') else {
            continue;
        };
        if u32::from_str_radix(addr, 16).is_err() {
            continue;
        }
        let instruction = rest.trim();
        let func = funcs.get_mut(&current).expect("current function exists");
        let mut words = instruction.split_whitespace();
        match words.next() {
            Some("addiu") => {
                let operands = words.collect::<Vec<_>>().join(" ");
                if let Some(amount) = operands.strip_prefix("$sp, $sp, -") {
                    let amount = amount.split_whitespace().next().unwrap_or("0");
                    let value = match amount.strip_prefix("0x") {
                        Some(hex) => u32::from_str_radix(hex, 16).unwrap_or(0),
                        None => amount.parse().unwrap_or(0),
                    };
                    func.frame = func.frame.max(value);
                }
            }
            Some(kind @ ("jal" | "j")) => {
                if let Some(target) = words
                    .next()
                    .and_then(|operand| operand.strip_prefix("0x"))
                    .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                {
                    // objdump prints the 26-bit jump target without the segment.
                    let target = if target < 0x8000_0000 {
                        target | 0x8000_0000
                    } else {
                        target
                    };
                    func.calls
                        .push(if kind == "j" { target | 1 } else { target });
                }
            }
            _ => {}
        }
    }
    funcs
}

/// Worst-case stack below `root`'s entry. `zero_frame` names functions whose
/// frame is not counted (none, now that the menu's 16 KiB font packing runs
/// where the arena is idle; kept for what-if runs).
pub fn analyse(text: &str, root: &str, depths: Depths, zero_frame: &[&str]) -> Option<Analysis> {
    let funcs = parse(text);
    let starts: Vec<u32> = funcs.keys().copied().collect();
    let index: HashMap<u32, usize> = starts.iter().enumerate().map(|(i, a)| (*a, i)).collect();
    let owner = |addr: u32| match starts.binary_search(&addr) {
        Ok(i) => starts[i],
        Err(i) => starts[i.saturating_sub(1)],
    };
    let n = starts.len();
    let mut frame: Vec<u32> = starts.iter().map(|a| funcs[a].frame).collect();
    for (i, a) in starts.iter().enumerate() {
        if zero_frame.iter().any(|z| funcs[a].name.contains(z)) {
            frame[i] = 0;
        }
    }
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, a) in starts.iter().enumerate() {
        for &call in &funcs[a].calls {
            let tail = call & 1 == 1;
            let target = call & !1;
            let o = owner(target);
            if target != o || (tail && o == *a) {
                continue;
            }
            adj[i].push(index[&o]);
        }
        adj[i].sort_unstable();
        adj[i].dedup();
    }

    // Tarjan, iterative.
    let mut order_index = vec![usize::MAX; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack = Vec::new();
    let mut comp = vec![usize::MAX; n];
    let mut ncomp = 0;
    let mut counter = 0;
    for start in 0..n {
        if order_index[start] != usize::MAX {
            continue;
        }
        let mut work = vec![(start, 0usize)];
        order_index[start] = counter;
        low[start] = counter;
        counter += 1;
        stack.push(start);
        on_stack[start] = true;
        while let Some(&(v, edge)) = work.last() {
            if edge < adj[v].len() {
                work.last_mut().expect("work is non-empty").1 += 1;
                let w = adj[v][edge];
                if order_index[w] == usize::MAX {
                    order_index[w] = counter;
                    low[w] = counter;
                    counter += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    work.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(order_index[w]);
                }
            } else {
                work.pop();
                if let Some(&(parent, _)) = work.last() {
                    low[parent] = low[parent].min(low[v]);
                }
                if low[v] == order_index[v] {
                    loop {
                        let w = stack.pop().expect("component member");
                        on_stack[w] = false;
                        comp[w] = ncomp;
                        if w == v {
                            break;
                        }
                    }
                    ncomp += 1;
                }
            }
        }
    }
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); ncomp];
    for v in 0..n {
        members[comp[v]].push(v);
    }
    let component_frame: Vec<u32> = members
        .iter()
        .map(|m| m.iter().map(|&v| frame[v]).max().unwrap_or(0))
        .collect();
    let mut cadj: Vec<Vec<usize>> = vec![Vec::new(); ncomp];
    for v in 0..n {
        for &w in &adj[v] {
            if comp[v] != comp[w] {
                cadj[comp[v]].push(comp[w]);
            }
        }
    }
    let root_addr = funcs
        .iter()
        .find(|(_, f)| f.name == root)
        .map(|(a, _)| *a)?;
    let root_comp = comp[index[&root_addr]];

    // Reverse topological order of everything reachable from the root.
    fn visit(c: usize, cadj: &[Vec<usize>], seen: &mut [bool], post: &mut Vec<usize>) {
        if seen[c] {
            return;
        }
        seen[c] = true;
        for &d in &cadj[c] {
            visit(d, cadj, seen, post);
        }
        post.push(c);
    }
    let mut seen = vec![false; ncomp];
    let mut post = Vec::new();
    visit(root_comp, &cadj, &mut seen, &mut post);
    let topo: Vec<usize> = post.iter().rev().copied().collect();

    let mut entry = vec![i64::MIN; ncomp];
    entry[root_comp] = 0;
    for &c in &topo {
        if entry[c] == i64::MIN {
            continue;
        }
        for &d in &cadj[c] {
            entry[d] = entry[d].max(entry[c] + i64::from(component_frame[c]));
        }
    }
    let mut below = vec![0u32; ncomp];
    for &c in post.iter() {
        below[c] = cadj[c]
            .iter()
            .map(|&d| component_frame[d] + below[d])
            .max()
            .unwrap_or(0);
    }

    let mut worst = (0u32, String::new());
    let mut recursion = Vec::new();
    let mut chains = Vec::new();
    for c in 0..ncomp {
        if entry[c] == i64::MIN {
            continue;
        }
        let names: Vec<&str> = members[c]
            .iter()
            .map(|&v| funcs[&starts[v]].name.as_str())
            .collect();
        let cyclic = members[c].len() > 1 || adj[members[c][0]].contains(&members[c][0]);
        let has = |suffix: &str| names.iter().any(|name| name.ends_with(suffix));
        let (recur, label) = if !cyclic {
            (component_frame[c], "call chain")
        } else if has("4phys7recurse")
            || has("4phys18recurse_node_trace")
            || has("4phys13recurse_clear")
        {
            (
                component_frame[c] * depths.clip_frames,
                "clip-hull trace recursion",
            )
        } else if has("4phys20recurse_visual_clear")
            || has("4phys24visual_sphere_solid_from")
            || has("4phys14sound_contents")
            || has("20box_touches_pvs_node")
        {
            (
                component_frame[c] * depths.hull0_frames,
                "hull-0 node recursion",
            )
        } else if has("14emit_soft_cell") {
            (component_frame[c] * SOFT_CELL_FRAMES, "soft-cell split")
        } else if has("21emit_residue_children") {
            (component_frame[c] * RESIDUE_FRAMES, "residue split")
        } else {
            let mut frames: Vec<u32> = members[c].iter().map(|&v| frame[v]).collect();
            frames.sort_unstable_by(|a, b| b.cmp(a));
            (
                frames.iter().take(LOGIC_FRAMES_PER_HOP).sum::<u32>() * LOGIC_HOPS,
                "logic chain",
            )
        };
        let total = entry[c] as u32 + recur + below[c];
        if cyclic {
            recursion.push((
                total,
                format!(
                    "{label}: {} (above {} + recursion {recur} + below {})",
                    short(names[0]),
                    entry[c],
                    below[c]
                ),
            ));
        } else if below[c] == 0 {
            chains.push((
                total,
                format!("{} ({} B frame)", short(names[0]), component_frame[c]),
            ));
        }
        if total > worst.0 {
            worst = (total, format!("{label}: {}", short(names[0])));
        }
    }
    recursion.sort_by(|a, b| b.0.cmp(&a.0));
    chains.sort_by(|a, b| b.0.cmp(&a.0));
    Some(Analysis {
        worst: worst.0,
        limiter: worst.1,
        recursion,
        chains,
    })
}

/// The trailing identifier of a v0-mangled Rust symbol, for messages.
fn short(name: &str) -> String {
    let tail: String = name
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let digits = tail.trim_start_matches(|c: char| c.is_ascii_digit());
    digits.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // main (frame 16) calls f (frame 24); f calls itself once and g (frame 8).
    const DIS: &str = "\
80010000 <main>:
80010000: \taddiu\t$sp, $sp, -0x10 <x>
80010004: \tjal\t0x10010 <f>
80010008: \tnop
8001000c: \tjr\t$ra
80010010 <f>:
80010010: \taddiu\t$sp, $sp, -24 <x>
80010014: \tjal\t0x10010 <f>
80010018: \tnop
8001001c: \tjal\t0x10030 <g>
80010020: \tnop
80010024: \tjr\t$ra
80010030 <g>:
80010030: \taddiu\t$sp, $sp, -0x8 <x>
80010034: \tjr\t$ra
";

    #[test]
    fn frames_edges_and_the_logic_bound() {
        // `f` is a recursive group with no known depth: logic-chain rule,
        // top frames 24 * 9 = 216 on top of main's 16 and g's 8 below.
        let analysis = analyse(DIS, "main", CAMPAIGN_DEPTHS, &[]).unwrap();
        assert_eq!(analysis.worst, 16 + 24 * 9 + 8);
        assert_eq!(analysis.recursion.len(), 1);
    }

    #[test]
    fn a_missing_root_is_reported() {
        assert!(analyse(DIS, "nope", CAMPAIGN_DEPTHS, &[]).is_none());
    }

    #[test]
    fn zeroed_frames_drop_out() {
        let analysis = analyse(DIS, "main", CAMPAIGN_DEPTHS, &["main"]).unwrap();
        assert_eq!(analysis.worst, 24 * 9 + 8);
    }
}
