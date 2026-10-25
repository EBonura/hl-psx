//! Lane tool: replay a `--cd-command-log` CSV and report data reads issued while an XA song is
//! still playing (the weapon-bank read bug), plus the order of XA starts, pauses and data reads.
//!   cdcheck cd.csv [--first-cycle N] [--last-cycle N]
use std::fs;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let text = fs::read_to_string(&args[1]).expect("cd log");
    let window = |flag: &str, default: u64| {
        args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(default)
    };
    let (first, last) = (window("--first-cycle", 0), window("--last-cycle", u64::MAX));
    let (mut mode, mut xa) = (0u8, false);
    let (mut starts, mut pauses, mut data, mut bad) = (0, 0, 0, 0);
    let (mut last_start, mut last_data) = (0u64, 0u64);
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.splitn(4, ',').collect();
        let cycle: u64 = f[0].parse().unwrap();
        let cmd = u8::from_str_radix(f[1].trim_start_matches("0x"), 16).unwrap();
        let param0 = f.get(3).and_then(|p| p.split_whitespace().next()).and_then(|p| u8::from_str_radix(p, 16).ok());
        let seen = (first..=last).contains(&cycle);
        match cmd {
            0x0e => mode = param0.unwrap_or(mode),
            0x06 | 0x1b if mode & 0x40 != 0 => {
                xa = true;
                if seen { starts += 1; last_start = cycle; println!("{cycle}: XA start"); }
            }
            0x06 | 0x1b => {
                if seen { data += 1; last_data = cycle; }
                if xa && seen { bad += 1; println!("{cycle}: DATA READ WHILE XA PLAYING"); } else if seen { println!("{cycle}: data read"); }
            }
            0x08 | 0x09 | 0x0a => {
                if xa && seen { pauses += 1; println!("{cycle}: pause/stop"); }
                xa = false;
            }
            _ => {}
        }
    }
    println!("xa starts {starts}, pauses {pauses}, data reads {data}, data reads over a playing song {bad}, song playing at the end {xa}, song started again after the last data read {}", last_start > last_data);
    if bad > 0 { std::process::exit(1); }
}
