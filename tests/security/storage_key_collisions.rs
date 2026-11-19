//! Storage-key collision tests (Issue #609).
//!
//! All modules share one contract storage namespace, and a `#[contracttype]`
//! enum variant encodes to `Vec[Symbol(name), ..payload]`. These tests scan the
//! crate sources and fail if two key enums declare the same variant name with
//! the same payload signature, or if a variant name exceeds the 32-char
//! `Symbol` limit. They are the Rust-side twin of `scripts/check_storage_keys.py`.

// Test code: pedantic style lints (too_many_lines, semicolon_if_nothing_returned, ...)
// add noise without catching bugs here.
#![allow(clippy::pedantic)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

struct KeyVariant {
    file: String,
    enum_name: String,
    name: String,
    sig: String,
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

fn strip_comments(s: &str) -> String {
    s.lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

fn ident_end(s: &str) -> usize {
    s.find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(s.len())
}

/// Collect variants of every `#[contracttype] pub enum *Key(s)`.
fn key_variants() -> Vec<KeyVariant> {
    let mut files = Vec::new();
    rs_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let text = fs::read_to_string(&f).unwrap();
        let mut rest = text.as_str();
        while let Some(pos) = rest.find("#[contracttype]") {
            let line_start = rest[..pos].rfind('\n').map_or(0, |i| i + 1);
            let in_prose = !rest[line_start..pos].trim().is_empty();
            rest = &rest[pos + "#[contracttype]".len()..];
            if in_prose {
                continue; // mentioned inside a doc comment, not an attribute
            }
            // Next item header up to `{` or `;` – only enums matter.
            let header_end = rest.find(['{', ';']).unwrap_or(rest.len());
            let header = &rest[..header_end];
            let Some(epos) = header.find("pub enum ") else {
                continue;
            };
            let after = &header[epos + "pub enum ".len()..];
            let enum_name = after[..ident_end(after)].to_string();
            if !(enum_name.ends_with("Key") || enum_name.ends_with("Keys")) {
                continue;
            }
            let body_start = header_end + 1;
            let mut depth = 1;
            let mut end = body_start;
            for (i, c) in rest[body_start..].char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = body_start + i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let body = strip_comments(&rest[body_start..end]);
            let (mut depth, mut cur, mut vs) = (0i32, String::new(), Vec::new());
            for c in body.chars() {
                match c {
                    '(' | '<' => depth += 1,
                    ')' | '>' => depth -= 1,
                    _ => {}
                }
                if c == ',' && depth == 0 {
                    vs.push(std::mem::take(&mut cur));
                } else {
                    cur.push(c);
                }
            }
            vs.push(cur);
            for v in vs {
                let mut v = v.trim().to_string();
                while v.starts_with("#[") {
                    v = v[v.find(']').unwrap() + 1..].trim().to_string();
                }
                if v.is_empty() {
                    continue;
                }
                let n = ident_end(&v);
                out.push(KeyVariant {
                    file: f.file_name().unwrap().to_string_lossy().into_owned(),
                    enum_name: enum_name.clone(),
                    name: v[..n].to_string(),
                    sig: v[n..].split_whitespace().collect(),
                });
            }
        }
    }
    out
}

#[test]
fn scanner_finds_key_enums() {
    let v = key_variants();
    assert!(v.len() > 300, "scanner found only {} key variants", v.len());
}

#[test]
fn no_two_modules_share_a_storage_key() {
    let mut groups: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for v in key_variants() {
        groups
            .entry((v.name.clone(), v.sig.clone()))
            .or_default()
            .push(format!("{}::{}", v.file, v.enum_name));
    }
    let collisions: Vec<String> = groups
        .into_iter()
        .filter(|(_, w)| w.len() > 1)
        .map(|((n, s), w)| format!("{n}{s} in {}", w.join(", ")))
        .collect();
    assert!(
        collisions.is_empty(),
        "storage key collisions:\n{}",
        collisions.join("\n")
    );
}

#[test]
fn key_variant_names_fit_in_a_symbol() {
    let too_long: Vec<String> = key_variants()
        .into_iter()
        .filter(|v| v.name.len() > 32)
        .map(|v| format!("{}::{}::{}", v.file, v.enum_name, v.name))
        .collect();
    assert!(
        too_long.is_empty(),
        "variant names over 32 chars: {too_long:?}"
    );
}

#[test]
fn generic_bare_names_are_module_prefixed() {
    const GENERIC: [&str; 11] = [
        "Admin",
        "Config",
        "Counter",
        "State",
        "Paused",
        "Version",
        "Metrics",
        "Registry",
        "Session",
        "Leaderboard",
        "Initialized",
    ];
    let bare: Vec<String> = key_variants()
        .into_iter()
        .filter(|v| GENERIC.contains(&v.name.as_str()))
        .map(|v| format!("{}::{}::{}", v.file, v.enum_name, v.name))
        .collect();
    assert!(
        bare.is_empty(),
        "bare generic key names (prefix with the module): {bare:?}"
    );
}

/// Runtime proof: two formerly colliding admin keys now address different slots.
#[test]
fn formerly_colliding_admin_keys_do_not_overwrite_each_other() {
    use soroban_sdk::{testutils::Address as _, Address, Env};
    use stellar_nebula_nomad::{access_control::AccessControlKey, staking};

    let env = Env::default();
    let contract = env.register(stellar_nebula_nomad::NebulaNomadContract, ());
    let a = Address::generate(&env);
    let b = Address::generate(&env);
    env.as_contract(&contract, || {
        env.storage()
            .instance()
            .set(&AccessControlKey::AccessControlAdmin, &a);
        env.storage()
            .instance()
            .set(&staking::DataKey::StakingAdmin, &b);
        let got_a: Address = env
            .storage()
            .instance()
            .get(&AccessControlKey::AccessControlAdmin)
            .unwrap();
        let got_b: Address = env
            .storage()
            .instance()
            .get(&staking::DataKey::StakingAdmin)
            .unwrap();
        assert_eq!(got_a, a);
        assert_eq!(got_b, b);
    });
}
