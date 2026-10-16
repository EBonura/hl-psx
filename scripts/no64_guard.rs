//! Fails when guest Rust source names a 64/128-bit integer or a float.
//!
//! The PS1 has no 64-bit ALU and no FPU, so guest code uses 32-bit fixed point
//! (see psx_math::int32 for the exact wide-product helpers). Build and run:
//!
//!   rustc -O --edition 2021 scripts/no64_guard.rs -o target/no64_guard
//!   target/no64_guard [--floats] [--exempt FILE]... DIR...
//!
//! Every `.rs` file under each DIR is scanned, skipping `tests.rs` files and
//! `#[cfg(test)]` items (host oracles may use i64 and f64). A reviewed
//! exception carries `// psx-numeric-allow-next-line: reason` on the line
//! above (or `psx-numeric-allow-line` on the same line); `--exempt` skips a
//! whole file whose purpose is wide arithmetic. Float literals are only
//! judged with `--floats`: a `const fn fx(0.5)` folds at compile time, and the
//! link-map symbol gate (guest_symbol_gate.sh) proves no soft-float routine
//! survives; the float TYPES are always judged. Same convention and scanner
//! as the editor's `psoxide-dev runtime-numeric-guard`, which scans only the
//! editor repository's own crates.
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

fn relative_to_root(path: &Path) -> String {
    path.display().to_string()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            if path.file_name().and_then(|n| n.to_str()) != Some("target") {
                collect(&path, out)?;
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs")
            && path.file_name().and_then(|n| n.to_str()) != Some("tests.rs")
        {
            out.push(path);
        }
    }
    Ok(())
}

fn main() {
    let mut floats = false;
    let mut exempt: Vec<PathBuf> = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--floats" {
            floats = true;
        } else if arg == "--exempt" {
            exempt.push(PathBuf::from(args.next().expect("--exempt needs a file")));
        } else {
            dirs.push(PathBuf::from(arg));
        }
    }
    if dirs.is_empty() {
        eprintln!("usage: no64_guard [--floats] [--exempt FILE]... DIR...");
        std::process::exit(2);
    }
    let mut files = Vec::new();
    for dir in &dirs {
        if let Err(error) = collect(dir, &mut files) {
            eprintln!("{error}");
            std::process::exit(2);
        }
    }
    files.retain(|f| !exempt.iter().any(|e| e == f));
    let mut violations = Vec::new();
    let mut allowed_hits = 0usize;
    for path in &files {
        let src = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let code = blank_test_items(&blank_comments_and_literals(&src));
        let allowed = allowed_lines(&src);
        scan_forbidden_types(path, &src, &code, &allowed, &mut allowed_hits, &mut violations);
        if floats {
            scan_float_literals(path, &src, &code, &allowed, &mut allowed_hits, &mut violations);
        }
    }
    if violations.is_empty() {
        println!("no64 guard: ok ({} files, {} allowed lines)", files.len(), allowed_hits);
        return;
    }
    eprintln!("no64 guard failed: guest code names a wide integer or a float.");
    eprintln!("Use 32-bit fixed point (psx_math::int32), or mark a reviewed exception with");
    eprintln!("`// psx-numeric-allow-next-line: reason`.\n");
    for violation in &violations {
        eprintln!("{violation}");
    }
    eprintln!("\n{} violation(s)", violations.len());
    std::process::exit(1);
}

fn allowed_lines(src: &str) -> HashSet<usize> {
    let mut allowed = HashSet::new();
    for (idx, line) in src.lines().enumerate() {
        let number = idx + 1;
        if line.contains("psx-numeric-allow-line") {
            allowed.insert(number);
        }
        if line.contains("psx-numeric-allow-next-line") {
            allowed.insert(number + 1);
        }
    }
    allowed
}

fn blank_comments_and_literals(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out: Vec<u8> = bytes.to_vec();
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] == b'r' {
            if let Some(end) = raw_string_end(bytes, idx) {
                blank_range(&mut out, idx, end);
                idx = end;
                continue;
            }
        }
        if bytes[idx..].starts_with(b"//") {
            let end = bytes[idx..]
                .iter()
                .position(|&b| b == b'\n')
                .map(|p| idx + p)
                .unwrap_or(bytes.len());
            blank_range(&mut out, idx, end);
            idx = end;
            continue;
        }
        if bytes[idx..].starts_with(b"/*") {
            let mut depth = 1usize;
            let mut end = idx + 2;
            while end < bytes.len() && depth > 0 {
                if bytes[end..].starts_with(b"/*") {
                    depth += 1;
                    end += 2;
                } else if bytes[end..].starts_with(b"*/") {
                    depth -= 1;
                    end += 2;
                } else {
                    end += 1;
                }
            }
            blank_range(&mut out, idx, end);
            idx = end;
            continue;
        }
        if bytes[idx] == b'"' {
            let mut end = idx + 1;
            let mut escaped = false;
            while end < bytes.len() {
                let current = bytes[end];
                if escaped {
                    escaped = false;
                } else if current == b'\\' {
                    escaped = true;
                } else if current == b'"' {
                    end += 1;
                    break;
                }
                end += 1;
            }
            blank_range(&mut out, idx, end);
            idx = end;
            continue;
        }
        if bytes[idx] == b'\'' {
            if let Some(end) = char_literal_end(bytes, idx) {
                blank_range(&mut out, idx, end);
                idx = end;
                continue;
            }
        }
        idx += 1;
    }
    String::from_utf8(out).expect("source bytes stay utf8")
}

fn raw_string_end(src: &[u8], start: usize) -> Option<usize> {
    if src.get(start) != Some(&b'r') {
        return None;
    }
    let mut idx = start + 1;
    while src.get(idx) == Some(&b'#') {
        idx += 1;
    }
    if src.get(idx) != Some(&b'"') {
        return None;
    }
    let hashes = idx - start - 1;
    let mut marker = vec![b'"'];
    marker.extend(std::iter::repeat_n(b'#', hashes));
    find_bytes(&src[idx + 1..], &marker).map(|pos| idx + 1 + pos + marker.len())
}

fn char_literal_end(src: &[u8], start: usize) -> Option<usize> {
    if start + 1 >= src.len() {
        return None;
    }
    let next = src[start + 1];
    if next.is_ascii_alphabetic() || next == b'_' {
        return None;
    }
    let mut idx = start + 1;
    if src[idx] == b'\\' {
        idx += 2;
    } else {
        idx += 1;
    }
    (src.get(idx) == Some(&b'\'')).then_some(idx + 1)
}

fn blank_range(out: &mut [u8], start: usize, end: usize) {
    let end = end.min(out.len());
    for byte in &mut out[start..end] {
        if *byte != b'\n' {
            *byte = b' ';
        }
    }
}

/// Blank `#[cfg(test)]` items so the guard only judges code that ships to
/// the console. Host tests legitimately use f64: the reference oracle for
/// a fixed-point port is the float closed form it was ported from, and
/// flagging those made the guard mostly false positives.
///
/// Only unconditionally test-only forms are blanked: `#[cfg(test)]` and
/// `#[cfg(all(test, ...))]`. `#[cfg(any(target_arch = "mips", test))]`
/// also compiles into the guest, so it stays under the guard.
///
/// Runs on comment/literal-blanked source, so braces inside strings and
/// comments cannot break the brace match. Blanks with spaces to keep byte
/// offsets, and therefore reported line numbers, intact.
fn blank_test_items(code: &str) -> String {
    let bytes = code.as_bytes();
    let mut out = bytes.to_vec();
    let mut idx = 0usize;
    while let Some(pos) = code[idx..].find("#[cfg(") {
        let start = idx + pos;
        let rest = &code[start + "#[cfg(".len()..];
        if !(rest.starts_with("test)]") || rest.starts_with("all(test,")) {
            idx = start + "#[cfg(".len();
            continue;
        }
        // Walk to the end of the attached item: a brace-balanced block, or
        // a `;` for brace-less items like `#[cfg(test)] use super::*;`.
        let mut cursor = start;
        let mut depth = 0usize;
        let mut nested = 0usize; // ( and [ : the `;` in `[i32; 3]` does not end the item
        let mut end = bytes.len();
        while cursor < bytes.len() {
            match bytes[cursor] {
                b'(' | b'[' => nested += 1,
                b')' | b']' => nested = nested.saturating_sub(1),
                b'{' => depth += 1,
                b'}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        end = cursor + 1;
                        break;
                    }
                }
                b';' if depth == 0 && nested == 0 => {
                    end = cursor + 1;
                    break;
                }
                _ => {}
            }
            cursor += 1;
        }
        blank_range(&mut out, start, end);
        idx = end;
    }
    String::from_utf8(out).expect("blanking only substitutes ASCII spaces")
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn line_col(src: &str, offset: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut col = 1usize;
    for (idx, ch) in src.char_indices() {
        if idx >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

fn report_context(src: &str, line: usize) -> String {
    src.lines()
        .nth(line.saturating_sub(1))
        .unwrap_or("")
        .trim()
        .to_string()
}

fn scan_forbidden_types(
    path: &Path,
    src: &str,
    code: &str,
    allowed: &HashSet<usize>,
    allowed_hits: &mut usize,
    violations: &mut Vec<String>,
) {
    let forbidden = ["f32", "f64", "u64", "i64", "u128", "i128"];
    for token in forbidden {
        let mut start = 0usize;
        while let Some(pos) = code[start..].find(token) {
            let offset = start + pos;
            let before = offset
                .checked_sub(1)
                .and_then(|i| code.as_bytes().get(i).copied());
            let after = code.as_bytes().get(offset + token.len()).copied();
            // `0u64` and `1_i64`: the type is a literal suffix, so what precedes it is a
            // run of identifier bytes that starts with a digit.
            let suffix_of_literal = before.is_some_and(is_ident_byte) && {
                let mut start = offset;
                while start > 0 && is_ident_byte(code.as_bytes()[start - 1]) {
                    start -= 1;
                }
                code.as_bytes()[start].is_ascii_digit()
            };
            if (before.is_none_or(|b| !is_ident_byte(b)) || suffix_of_literal)
                && after.is_none_or(|b| !is_ident_byte(b))
            {
                let (line, col) = line_col(code, offset);
                if allowed.contains(&line) {
                    *allowed_hits += 1;
                } else {
                    let kind = if token.starts_with('f') {
                        "float type"
                    } else {
                        "wide integer type"
                    };
                    violations.push(format!(
                        "{}:{line}:{col}: forbidden {kind} `{token}`\n    {}",
                        relative_to_root(path),
                        report_context(src, line)
                    ));
                }
            }
            start = offset + token.len();
        }
    }
}

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn scan_float_literals(
    path: &Path,
    src: &str,
    code: &str,
    allowed: &HashSet<usize>,
    allowed_hits: &mut usize,
    violations: &mut Vec<String>,
) {
    let bytes = code.as_bytes();
    let mut idx = 0usize;
    while idx < bytes.len() {
        if !bytes[idx].is_ascii_digit()
            || idx > 0 && (is_ident_byte(bytes[idx - 1]) || bytes[idx - 1] == b'.')
        {
            idx += 1;
            continue;
        }
        let start = idx;
        while idx < bytes.len() && bytes[idx].is_ascii_digit() {
            idx += 1;
        }
        let mut has_float = false;
        if idx < bytes.len()
            && bytes[idx] == b'.'
            && idx + 1 < bytes.len()
            && bytes[idx + 1].is_ascii_digit()
        {
            has_float = true;
            idx += 1;
            while idx < bytes.len() && bytes[idx].is_ascii_digit() {
                idx += 1;
            }
        }
        if idx < bytes.len() && matches!(bytes[idx], b'e' | b'E') {
            has_float = true;
            idx += 1;
            if idx < bytes.len() && matches!(bytes[idx], b'+' | b'-') {
                idx += 1;
            }
            while idx < bytes.len() && bytes[idx].is_ascii_digit() {
                idx += 1;
            }
        }
        if has_float {
            if idx < bytes.len() && bytes[idx] == b'_' {
                idx += 1;
            }
            if idx + 2 <= bytes.len() && &bytes[idx..idx + 1] == b"f" {
                idx += 1;
                while idx < bytes.len() && bytes[idx].is_ascii_digit() {
                    idx += 1;
                }
            }
            let literal = &code[start..idx];
            let (line, col) = line_col(code, start);
            if allowed.contains(&line) {
                *allowed_hits += 1;
            } else {
                violations.push(format!(
                    "{}:{line}:{col}: forbidden float literal `{literal}`\n    {}",
                    relative_to_root(path),
                    report_context(src, line)
                ));
            }
        }
        idx = idx.max(start + 1);
    }
}

