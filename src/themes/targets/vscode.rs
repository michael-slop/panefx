//! VS Code: the theme laid over whatever colour theme is chosen, through
//! `workbench.colorCustomizations` in the user's `settings.json`.
//!
//! That file is JSONC -- comments and trailing commas allowed -- so it is NOT
//! parsed and re-serialised (that would strip every comment). One top-level key
//! is spliced in place by a scanner that understands strings and comments, and
//! if the file does not look like an object at all the target refuses rather
//! than guesses. VS Code watches the file, so the change is live.
//!
//! House puts the key's previous text back exactly, or removes it.

use super::{Outcome, Target};
use crate::themes::catalog::Theme;
use crate::themes::derive::mix;
use crate::themes::fsutil::{write_atomic, Env};
use std::path::PathBuf;

pub struct VsCode;

pub const KEY: &str = "workbench.colorCustomizations";

/// Byte ranges of one top-level member: (member start, value start, value end).
/// Member start is the opening quote of the key.
fn find_member(text: &str, key: &str) -> Option<(usize, usize, usize)> {
    let b = text.as_bytes();
    let mut i = 0;
    let mut depth = 0i32;
    let skip_ws = |mut j: usize| {
        loop {
            while j < b.len() && (b[j] as char).is_whitespace() {
                j += 1;
            }
            if b[j..].starts_with(b"//") {
                while j < b.len() && b[j] != b'\n' {
                    j += 1;
                }
            } else if b[j..].starts_with(b"/*") {
                j = text[j + 2..].find("*/").map(|e| j + 2 + e + 2).unwrap_or(b.len());
            } else {
                return j;
            }
        }
    };
    let string_end = |j: usize| {
        // b[j] == '"'; returns index just past the closing quote.
        let mut k = j + 1;
        while k < b.len() {
            match b[k] {
                b'\\' => k += 2,
                b'"' => return k + 1,
                _ => k += 1,
            }
        }
        b.len()
    };
    let value_end = |j: usize| {
        let mut k = j;
        let mut d = 0i32;
        while k < b.len() {
            match b[k] {
                b'"' => {
                    k = string_end(k);
                    if d == 0 {
                        return k;
                    }
                    continue;
                }
                b'/' if b[k..].starts_with(b"//") || b[k..].starts_with(b"/*") => {
                    k = skip_ws(k);
                    continue;
                }
                b'{' | b'[' => d += 1,
                b'}' | b']' => {
                    if d == 0 {
                        return k;
                    }
                    d -= 1;
                    if d == 0 {
                        return k + 1;
                    }
                }
                b',' if d == 0 => return k,
                _ => {}
            }
            k += 1;
        }
        k
    };
    while i < b.len() {
        i = skip_ws(i);
        if i >= b.len() {
            break;
        }
        match b[i] {
            b'{' | b'[' => {
                depth += 1;
                i += 1;
            }
            b'}' | b']' => {
                depth -= 1;
                i += 1;
            }
            b'"' => {
                let end = string_end(i);
                if depth == 1 && &text[i + 1..end - 1] == key {
                    let colon = skip_ws(end);
                    if b.get(colon) == Some(&b':') {
                        let vs = skip_ws(colon + 1);
                        let ve = value_end(vs);
                        return Some((i, vs, ve));
                    }
                }
                i = end;
            }
            _ => i += 1,
        }
    }
    None
}

/// Set (`Some`) or remove (`None`) one top-level member. `None` back means the
/// text is not a JSON object this can edit safely.
pub fn splice(text: &str, key: &str, value: Option<&str>) -> Option<String> {
    let open = text.find('{')?;
    let close = text.rfind('}')?;
    if close < open {
        return None;
    }
    match (find_member(text, key), value) {
        (Some((_, vs, ve)), Some(v)) => Some(format!("{}{v}{}", &text[..vs], &text[ve..])),
        (Some((ms, _, ve)), None) => {
            // Take the member and its comma: the one after it, else the one before.
            let after = &text[ve..];
            let trimmed = after.trim_start();
            if let Some(rest) = trimmed.strip_prefix(',') {
                let line_start = text[..ms].rfind('\n').map(|i| i + 1).unwrap_or(ms);
                let rest = rest.strip_prefix(['\r', '\n']).unwrap_or(rest);
                let rest = rest.strip_prefix('\n').unwrap_or(rest);
                let start = if text[line_start..ms].trim().is_empty() { line_start } else { ms };
                Some(format!("{}{}", &text[..start], rest))
            } else {
                let before = text[..ms].trim_end();
                let before = before.strip_suffix(',').unwrap_or(before);
                Some(format!("{before}{after}"))
            }
        }
        (None, Some(v)) => {
            // Shaped so removing it again gives the original bytes back: after
            // a trailing comma the new member carries its own, otherwise it
            // borrows one from the member before it.
            // The whitespace before the closing brace is kept exactly (`ws`), and
            // the file's own line ending is used, so removal can hand back the
            // same bytes -- a CRLF file must not come back with an LF in it.
            let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
            let body = text[open + 1..close].trim();
            let head = text[..close].trim_end();
            let ws = &text[head.len()..close];
            let ws = if ws.is_empty() { nl } else { ws };
            let tail = &text[close..];
            Some(if body.is_empty() {
                format!("{head}{nl}    \"{key}\": {v}{ws}{tail}")
            } else if body.ends_with(',') {
                format!("{head}{nl}    \"{key}\": {v},{ws}{tail}")
            } else {
                format!("{head},{nl}    \"{key}\": {v}{ws}{tail}")
            })
        }
        (None, None) => Some(text.to_string()),
    }
}

/// The colour customisations for a theme, as JSON text indented for the file.
pub fn customisations(t: &Theme) -> Option<String> {
    let bg = t.rgb("background")?;
    let fg = t.rgb("foreground")?;
    let acc = t.rgb("accent")?;
    let raised = mix(bg, fg, 0.05).to_hex();
    let line = mix(bg, fg, 0.15).to_hex();
    let h = |k: &str| t.rgb(k).map(|c| c.to_hex()).unwrap_or_else(|| fg.to_hex());
    let mut pairs: Vec<(String, String)> = vec![
        ("editor.background", bg.to_hex()),
        ("editor.foreground", fg.to_hex()),
        ("editorCursor.foreground", h("cursor")),
        ("editor.selectionBackground", format!("{}66", acc.to_hex())),
        ("editor.lineHighlightBackground", raised.clone()),
        ("editorLineNumber.foreground", h("color8")),
        ("editorLineNumber.activeForeground", fg.to_hex()),
        ("activityBar.background", raised.clone()),
        ("activityBar.foreground", fg.to_hex()),
        ("activityBarBadge.background", acc.to_hex()),
        ("sideBar.background", raised.clone()),
        ("sideBar.foreground", fg.to_hex()),
        ("sideBarSectionHeader.background", bg.to_hex()),
        ("titleBar.activeBackground", raised.clone()),
        ("titleBar.activeForeground", fg.to_hex()),
        ("statusBar.background", raised.clone()),
        ("statusBar.foreground", fg.to_hex()),
        ("panel.background", bg.to_hex()),
        ("panel.border", line.clone()),
        ("tab.activeBackground", bg.to_hex()),
        ("tab.inactiveBackground", raised.clone()),
        ("tab.activeBorderTop", acc.to_hex()),
        ("editorGroupHeader.tabsBackground", raised.clone()),
        ("focusBorder", acc.to_hex()),
        ("button.background", acc.to_hex()),
        ("button.foreground", bg.to_hex()),
        ("list.activeSelectionBackground", mix(bg, acc, 0.3).to_hex()),
        ("list.hoverBackground", mix(bg, fg, 0.08).to_hex()),
        ("input.background", raised),
        ("dropdown.background", bg.to_hex()),
        ("terminal.background", bg.to_hex()),
        ("terminal.foreground", fg.to_hex()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let ansi = [
        "Black", "Red", "Green", "Yellow", "Blue", "Magenta", "Cyan", "White",
    ];
    for (i, n) in ansi.iter().enumerate() {
        pairs.push((format!("terminal.ansi{n}"), h(&format!("color{i}"))));
        pairs.push((format!("terminal.ansiBright{n}"), h(&format!("color{}", i + 8))));
    }
    let mut s = format!("{{\n        \"// panefx\": \"{} -- written by panefx; the house theme removes this block\",\n", t.name);
    let n = pairs.len();
    for (i, (k, v)) in pairs.into_iter().enumerate() {
        s.push_str(&format!("        \"{k}\": \"{v}\"{}\n", if i + 1 < n { "," } else { "" }));
    }
    s.push_str("    }");
    Some(s)
}

fn settings(env: &Env) -> PathBuf {
    env.appdata.join("Code").join("User").join("settings.json")
}

fn edit(env: &Env, value: Option<&str>) -> Outcome {
    let path = settings(env);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Outcome::Skipped("not installed".into());
    };
    // The block is written with `\n`; a CRLF file gets CRLF throughout.
    let value = value.map(|v| {
        let v = v.replace("\r\n", "\n");
        if text.contains("\r\n") {
            v.replace('\n', "\r\n")
        } else {
            v
        }
    });
    match splice(&text, KEY, value.as_deref()) {
        Some(next) if next == text => Outcome::Live("already set".into()),
        Some(next) => match write_atomic(&path, next.as_bytes()) {
            Ok(()) => Outcome::Live("settings.json updated; VS Code reloads it".into()),
            Err(e) => Outcome::Failed(e.to_string()),
        },
        None => Outcome::Failed("settings.json is not an object panefx can edit safely; left alone".into()),
    }
}

impl Target for VsCode {
    fn id(&self) -> &'static str {
        "vscode"
    }
    fn label(&self) -> &'static str {
        "VS Code"
    }
    fn capture(&self, env: &Env) -> Option<serde_json::Value> {
        let text = std::fs::read_to_string(settings(env)).ok()?;
        let prev = find_member(&text, KEY).map(|(_, vs, ve)| text[vs..ve].trim_end().to_string());
        Some(serde_json::json!({ "value": prev }))
    }
    fn apply(&self, env: &Env, theme: &Theme) -> Outcome {
        match customisations(theme) {
            Some(v) => edit(env, Some(&v)),
            None => Outcome::Failed(format!("{} lacks a background, foreground or accent", theme.id)),
        }
    }
    fn restore(&self, env: &Env, saved: Option<&serde_json::Value>) -> Outcome {
        let prev = saved.and_then(|s| s.get("value")).and_then(|v| v.as_str());
        edit(env, prev)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::themes::catalog::{load, Dirs};

    const JSONC: &str = "// my settings\n{\n    \"editor.fontSize\": 14, // big\n    /* a { tricky } \"comment\" */\n    \"files.exclude\": { \"**/.git\": true },\n}\n";

    #[test]
    fn a_new_key_is_added_and_every_comment_survives() {
        let out = splice(JSONC, KEY, Some("{ \"a\": \"#000000\" }")).unwrap();
        assert!(out.contains("// my settings") && out.contains("// big") && out.contains("tricky"));
        assert!(out.contains("\"workbench.colorCustomizations\": { \"a\": \"#000000\" }"), "{out}");
        assert_eq!(find_member(&out, KEY).map(|(_, vs, ve)| &out[vs..ve]), Some("{ \"a\": \"#000000\" }"));
    }

    #[test]
    fn apply_then_remove_is_the_original_file() {
        let added = splice(JSONC, KEY, Some("{ \"a\": \"#000000\" }")).unwrap();
        let removed = splice(&added, KEY, None).unwrap();
        assert_eq!(removed, JSONC);
    }

    /// pHub's real file, CRLF throughout. The first live test gave it back
    /// with its last line ending turned into a bare LF.
    #[test]
    fn a_crlf_file_comes_back_byte_for_byte() {
        let orig = "{\r\n    \"a\": true,\r\n    \"editor.minimap.enabled\": false\r\n}";
        let added = splice(orig, KEY, Some("{ \"x\": \"#000000\" }")).unwrap();
        assert!(!added.replace("\r\n", "").contains('\n'), "no bare LF: {added:?}");
        assert_eq!(splice(&added, KEY, None).unwrap(), orig);
    }

    #[test]
    fn an_existing_value_is_replaced_in_place_and_restored_exactly() {
        let orig = "{\n    \"workbench.colorCustomizations\": { \"x\": \"#123456\" },\n    \"a\": 1\n}\n";
        let (_, vs, ve) = find_member(orig, KEY).unwrap();
        let prev = &orig[vs..ve];
        let themed = splice(orig, KEY, Some("{ \"y\": 1 }")).unwrap();
        assert!(themed.contains("{ \"y\": 1 },\n    \"a\": 1"), "{themed}");
        assert_eq!(splice(&themed, KEY, Some(prev)).unwrap(), orig);
    }

    #[test]
    fn a_key_inside_a_nested_object_is_not_the_top_level_one() {
        let t = "{ \"inner\": { \"workbench.colorCustomizations\": 1 } }";
        assert!(find_member(t, KEY).is_none());
    }

    #[test]
    fn not_an_object_is_refused() {
        assert!(splice("[1, 2]", KEY, Some("{}")).is_none());
    }

    #[test]
    fn every_theme_produces_valid_json() {
        for t in load(&Dirs::bundled_only()) {
            let v = customisations(&t).unwrap();
            serde_json::from_str::<serde_json::Value>(&v).unwrap_or_else(|e| panic!("{}: {e}", t.id));
        }
    }
}
