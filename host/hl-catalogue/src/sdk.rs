//! Scan of the Half-Life SDK server sources (dlls/*.cpp, *.h): which sounds,
//! skill fields, animation event macros and models each C++ class names. This
//! is a lexical scan, not a compiler, but every row carries file and line so
//! the claim can be checked by hand.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct SndRef {
    pub class: String,
    /// `idle`, `alert`, `pain`, `die`, `attack`, `ae:LABEL`, `precache`, or the lower-cased function name.
    pub state: String,
    /// Lower-case path below sound/, or `!GROUP` for a sentence group.
    pub path: String,
    pub file: String,
    pub line: usize,
    /// The array the path came out of, empty for a literal.
    pub via: String,
}

#[derive(Default)]
pub struct Sdk {
    pub refs: Vec<SndRef>,
    /// entity classname -> C++ class
    pub link: BTreeMap<String, String>,
    /// C++ class -> base class
    pub base: BTreeMap<String, String>,
    /// C++ class -> first model named in a SET_MODEL/PRECACHE_MODEL
    pub model: BTreeMap<String, String>,
    /// C++ class -> gSkillData field names used
    pub skill_fields: BTreeMap<String, BTreeSet<String>>,
    /// gSkillData field -> sk_ cvar name
    pub cvar: BTreeMap<String, String>,
    /// AE macro -> numeric event id
    pub ae: BTreeMap<String, i32>,
    /// C++ class -> AE case labels its HandleAnimEvent handles, with the line of the case
    pub ae_cases: BTreeMap<String, Vec<(String, String, usize)>>,
}

fn norm_path(p: &str) -> String {
    let p = p.replace('\\', "/").to_ascii_lowercase();
    p.strip_prefix("sound/").unwrap_or(&p).to_string()
}

/// Remove `//` comments and the contents of string literals, for brace counting.
fn skeleton(line: &str) -> String {
    let b = line.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if c == b'"' {
            in_str = true;
            i += 1;
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            break;
        }
        out.push(c as char);
        i += 1;
    }
    out
}

/// Every double-quoted literal on the line, comments excluded.
fn literals(line: &str) -> Vec<String> {
    let line = match line.find("//") {
        Some(p) if !line[..p].contains('"') || line[..p].matches('"').count() % 2 == 0 => &line[..p],
        _ => line,
    };
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(a) = rest.find('"') {
        let after = &rest[a + 1..];
        let Some(z) = after.find('"') else { break };
        out.push(after[..z].to_string());
        rest = &after[z + 1..];
    }
    out
}

fn ident_before(s: &str) -> String {
    let t = s.trim_end();
    let b = t.as_bytes();
    let mut i = b.len();
    while i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_') {
        i -= 1;
    }
    t[i..].to_string()
}

fn ident_after(s: &str) -> String {
    s.trim_start()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

pub fn state_name(func: &str) -> String {
    match func {
        "IdleSound" => "idle".into(),
        "AlertSound" => "alert".into(),
        "PainSound" => "pain".into(),
        "DeathSound" => "die".into(),
        "AttackSound" => "attack".into(),
        "DoSpark" => "spark".into(),
        other => other.to_ascii_lowercase(),
    }
}

impl Sdk {
    pub fn scan(dlls: &Path) -> Result<Sdk, String> {
        let mut sdk = Sdk::default();
        let mut files: Vec<_> = fs::read_dir(dlls)
            .map_err(|e| format!("{}: {e}", dlls.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("cpp" | "h")))
            .collect();
        files.sort();
        let mut texts = Vec::new();
        for f in &files {
            let t = fs::read_to_string(f).or_else(|_| {
                fs::read(f).map(|b| String::from_utf8_lossy(&b).to_string())
            });
            texts.push((
                f.file_name().unwrap().to_string_lossy().to_string(),
                t.map_err(|e| format!("{}: {e}", f.display()))?,
            ));
        }
        for (name, text) in &texts {
            sdk.scan_global(name, text);
        }
        // arrays first (they can sit below their users), across all files
        let mut arrays: BTreeMap<(String, String), Vec<(String, usize, String)>> = BTreeMap::new();
        for (name, text) in &texts {
            collect_arrays(name, text, &mut arrays);
        }
        for (name, text) in &texts {
            sdk.scan_functions(name, text, &arrays);
        }
        sdk.alias_free();
        let mut seen = BTreeSet::new();
        sdk.refs.retain(|r| seen.insert((r.class.clone(), r.state.clone(), r.path.clone())));
        Ok(sdk)
    }

    fn scan_global(&mut self, _file: &str, text: &str) {
        for line in text.lines() {
            let t = line.trim_start();
            if let Some(rest) = t.strip_prefix("LINK_ENTITY_TO_CLASS") {
                let inner = rest.trim_start().trim_start_matches('(');
                let mut parts = inner.split(',');
                if let (Some(a), Some(b)) = (parts.next(), parts.next()) {
                    let ent = a.trim().to_ascii_lowercase();
                    let cls = b.trim().trim_end_matches(';').trim_end_matches(')').trim().to_string();
                    self.link.entry(ent).or_insert(cls);
                }
            }
            if let Some(rest) = t.strip_prefix("#define") {
                let mut it = rest.split_whitespace();
                if let (Some(n), Some(v)) = (it.next(), it.next()) {
                    if n.contains("_AE_") || n.starts_with("SCRIPT_EVENT") {
                        if let Ok(v) = v.trim_matches(|c| c == '(' || c == ')').parse::<i32>() {
                            self.ae.insert(n.to_string(), v);
                        }
                    }
                }
            }
            // class CFoo : public CBar
            if let Some(p) = t.find("class ") {
                if p == 0 {
                    let after = &t[6..];
                    let cls = ident_after(after);
                    if let Some(c) = after.find(':') {
                        let rest = after[c + 1..].trim_start();
                        if let Some(r) = rest.strip_prefix("public") {
                            let b = ident_after(r);
                            if !cls.is_empty() && !b.is_empty() && !after[..c].contains(';') {
                                self.base.entry(cls).or_insert(b);
                            }
                        }
                    }
                }
            }
            if let Some(p) = t.find("gSkillData.") {
                let _ = p;
            }
            // gSkillData.field = GetSkillCvar( "sk_x" );
            if t.starts_with("gSkillData.") && t.contains("GetSkillCvar") {
                let field = ident_after(&t["gSkillData.".len()..]);
                if let Some(l) = literals(t).first() {
                    self.cvar.entry(field).or_insert(l.clone());
                }
            }
        }
    }

    fn scan_functions(
        &mut self,
        file: &str,
        text: &str,
        arrays: &BTreeMap<(String, String), Vec<(String, usize, String)>>,
    ) {
        let mut depth = 0i32;
        let mut cur: Option<(String, String)> = None;
        let mut case: Option<String> = None;
        let mut arr_skip = false;
        for (ln, line) in text.lines().enumerate() {
            let ln1 = ln + 1;
            let sk = skeleton(line);
            if depth == 0 {
                let starts_col0 = line
                    .chars()
                    .next()
                    .map(|c| !c.is_whitespace() && !matches!(c, '/' | '#' | '{' | '}' | '*'))
                    .unwrap_or(false);
                if arr_skip {
                    if sk.contains("};") || sk.contains('}') {
                        arr_skip = false;
                    }
                    continue;
                }
                if starts_col0 && sk.contains("::") && sk.contains('(') && !sk.trim_end().ends_with(';')
                {
                    let flat = sk.replace(" :: ", "::").replace(" ::", "::").replace(":: ", "::");
                    if let Some(par) = flat.find('(') {
                        let head = &flat[..par];
                        if !head.contains('=') {
                            if let Some(cc) = head.rfind("::") {
                                let class = ident_before(&head[..cc]);
                                let func = ident_after(&head[cc + 2..]);
                                if !class.is_empty() && !func.is_empty() {
                                    cur = Some((class, func));
                                    case = None;
                                }
                            }
                        }
                    }
                } else if starts_col0
                    && sk.contains('(')
                    && !sk.contains("::")
                    && !sk.contains('=')
                    && !sk.trim_end().ends_with(';')
                    && !sk.starts_with("class ")
                {
                    let head = &sk[..sk.find('(').unwrap()];
                    let name = ident_before(head);
                    if head.trim().contains(' ') && !name.is_empty() {
                        cur = Some(("@free".to_string(), name));
                        case = None;
                    }
                } else if starts_col0 && sk.contains("[]") && sk.contains('=') {
                    arr_skip = !sk.contains("};");
                    continue;
                }
            }
            if let Some((class, func)) = cur.clone() {
                let t = line.trim();
                if depth >= 1 {
                    if let Some(rest) = t.strip_prefix("case ") {
                        let label = ident_after(rest);
                        if label.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && label.contains('_') {
                            case = Some(label.clone());
                            if func == "HandleAnimEvent" {
                                self.ae_cases
                                    .entry(class.clone())
                                    .or_default()
                                    .push((label, file.to_string(), ln1));
                            }
                        }
                    }
                    let state = if func == "HandleAnimEvent" {
                        match &case {
                            Some(c) => format!("ae:{c}"),
                            None => "handleanimevent".to_string(),
                        }
                    } else {
                        state_name(&func)
                    };
                    let precache = t.contains("PRECACHE_SOUND");
                    for lit in literals(line) {
                        let low = lit.to_ascii_lowercase();
                        if low.ends_with(".wav") {
                            self.refs.push(SndRef {
                                class: class.clone(),
                                state: if precache { "precache".into() } else { state.clone() },
                                path: norm_path(&low),
                                file: file.to_string(),
                                line: ln1,
                                via: String::new(),
                            });
                        } else if looks_like_sentence_group(&lit)
                            && (t.contains("Sentence") || t.contains("SENTENCEG") || t.contains("m_szGrp"))
                        {
                            self.refs.push(SndRef {
                                class: class.clone(),
                                state: state.clone(),
                                path: format!("!{lit}"),
                                file: file.to_string(),
                                line: ln1,
                                via: String::new(),
                            });
                        } else if lit.starts_with("models/") && lit.ends_with(".mdl") {
                            self.model.entry(class.clone()).or_insert(lit.clone());
                        }
                    }
                    // arrays used by this function body
                    for ((acls, aname), entries) in arrays {
                        if acls == &class && t.contains(aname.as_str()) && !t.starts_with("//") {
                            // a function that merely defines the array is skipped by depth 0 above
                            let astate = match aname.as_str() {
                                "pIdleSounds" => "idle".to_string(),
                                "pAlertSounds" => "alert".to_string(),
                                "pPainSounds" => "pain".to_string(),
                                "pDieSounds" => "die".to_string(),
                                "pAttackSounds" => "attack".to_string(),
                                _ => state.clone(),
                            };
                            let astate = if func == "Precache" { "precache".to_string() } else { astate };
                            for (p, aln, afile) in entries {
                                self.refs.push(SndRef {
                                    class: class.clone(),
                                    state: astate.clone(),
                                    path: p.clone(),
                                    file: afile.clone(),
                                    line: *aln,
                                    via: aname.clone(),
                                });
                            }
                        }
                    }
                    let mut from = 0;
                    while let Some(p) = line[from..].find("gSkillData.") {
                        let field = ident_after(&line[from + p + "gSkillData.".len()..]);
                        if !field.is_empty() && !line[..from + p].contains("//") {
                            self.skill_fields.entry(class.clone()).or_default().insert(field);
                        }
                        from += p + 11;
                    }
                }
            }
            depth += sk.matches('{').count() as i32;
            depth -= sk.matches('}').count() as i32;
            if depth <= 0 {
                depth = 0;
                if cur.is_some() && !sk.trim().is_empty() && sk.contains('}') {
                    cur = None;
                    case = None;
                }
            }
        }
    }
}

fn looks_like_sentence_group(s: &str) -> bool {
    let n = s.len();
    (3..=24).contains(&n)
        && s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && s.contains('_')
}

fn collect_arrays(
    file: &str,
    text: &str,
    out: &mut BTreeMap<(String, String), Vec<(String, usize, String)>>,
) {
    let mut cur: Option<(String, String)> = None;
    for (ln, line) in text.lines().enumerate() {
        let starts_col0 = line
            .chars()
            .next()
            .map(|c| !c.is_whitespace() && !matches!(c, '/' | '#' | '{' | '}'))
            .unwrap_or(false);
        let sk = skeleton(line);
        if starts_col0 && sk.contains("[]") && sk.contains('=') && sk.contains("::") {
            let flat = sk.replace(" :: ", "::").replace(" ::", "::").replace(":: ", "::");
            if let Some(cc) = flat.find("::") {
                let class = ident_before(&flat[..cc]);
                let name = ident_after(&flat[cc + 2..]);
                cur = Some((class, name));
                out.entry(cur.clone().unwrap()).or_default();
            }
        }
        if let Some(key) = cur.clone() {
            for lit in literals(line) {
                if lit.to_ascii_lowercase().ends_with(".wav") {
                    out.get_mut(&key)
                        .unwrap()
                        .push((norm_path(&lit), ln + 1, file.to_string()));
                }
            }
            if sk.contains('}') {
                cur = None;
            }
        }
    }
}

impl Sdk {
    /// The C++ class and its bases up to (and excluding) CBaseMonster.
    pub fn chain(&self, class: &str) -> Vec<String> {
        let mut out = vec![class.to_string()];
        if class == "CNihilanth" {
            out.push("CNihilanthHVR".to_string());
        }
        let mut c = class.to_string();
        while let Some(b) = self.base.get(&c) {
            if b == "CBaseMonster" || b == "CBaseEntity" || b == "CBaseDelay" || out.contains(b) {
                break;
            }
            out.push(b.clone());
            c = b.clone();
        }
        out
    }
}

/// Free functions of the SDK that several entity classes share: the C++ classes
/// that call them.
const FREE_CALLERS: &[(&str, &[&str])] = &[
    ("spark", &["CEnvSpark", "CBaseButton"]),
];

impl Sdk {
    /// Copy the rows of the shared free functions onto their callers.
    fn alias_free(&mut self) {
        let mut extra = Vec::new();
        for r in self.refs.iter().filter(|r| r.class == "@free") {
            for (func, callers) in FREE_CALLERS {
                if r.state == *func {
                    for c in *callers {
                        let mut n = r.clone();
                        n.class = (*c).to_string();
                        extra.push(n);
                    }
                }
            }
        }
        self.refs.retain(|r| r.class != "@free");
        self.refs.extend(extra);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_text(src: &str) -> Sdk {
        let dir = std::env::temp_dir().join(format!("hl-catalogue-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("m.cpp"), src).unwrap();
        let sdk = Sdk::scan(&dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        sdk
    }

    #[test]
    fn arrays_literals_cases_and_free_functions_attribute_to_the_right_class() {
        let sdk = scan_text(
            "LINK_ENTITY_TO_CLASS( monster_x, CX );\n\
             const char *CX::pPainSounds[] = \n{\n\t\"x/pain1.wav\",\n\t\"x/pain2.wav\",\n};\n\
             void CX :: PainSound ( void )\n{\n\tEMIT_SOUND_ARRAY_DYN( CHAN_VOICE, pPainSounds );\n}\n\
             void CX::HandleAnimEvent( MonsterEvent_t *e )\n{\n\tswitch( e->event )\n\t{\n\tcase X_AE_HIT:\n\t\tEMIT_SOUND( ENT(pev), CHAN_WEAPON, \"X/hit.wav\", 1, ATTN_NORM );\n\t\tbreak;\n\t}\n}\n\
             void DoSpark(entvars_t *pev)\n{\n\tEMIT_SOUND(ENT(pev), CHAN_VOICE, \"buttons/spark1.wav\", 1, ATTN_NORM);\n}\n",
        );
        assert_eq!(sdk.link["monster_x"], "CX");
        let pain: Vec<_> = sdk.refs.iter().filter(|r| r.class == "CX" && r.state == "pain").map(|r| r.path.as_str()).collect();
        assert_eq!(pain, ["x/pain1.wav", "x/pain2.wav"]);
        assert!(sdk.refs.iter().any(|r| r.class == "CX" && r.state == "ae:X_AE_HIT" && r.path == "x/hit.wav"));
        assert!(!sdk.refs.iter().any(|r| r.class == "@free"));
    }
}
