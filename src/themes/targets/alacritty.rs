//! Alacritty: the theme rendered into a file of our own, imported last.
//!
//! panefx already writes `alacritty.toml` (the opacity line, `term_opacity.rs`),
//! so the palette does NOT go in there. It goes in `themes\panefx-theme.toml`,
//! and `alacritty.toml` gets one edit, once: that file added as the LAST
//! import, so it overrides whatever palette the imports before it set. For the
//! house theme the file is emptied, and the user's own palette shows through.
//! `live_config_reload` picks the change up; `alacritty.toml` is touched too, in
//! case a build does not watch imported files.
//!
//! A config color.mesh set up (it imported `themes\color.mesh.toml`) has that
//! entry replaced in place, so there is never a second override fighting ours.

use super::{Outcome, Target};
use crate::themes::catalog::Theme;
use crate::themes::fsutil::{touch, write_atomic, Env};
use std::path::{Path, PathBuf};

pub struct Alacritty;

pub const FILE_NAME: &str = "panefx-theme.toml";
const LEGACY_NAME: &str = "color.mesh.toml";

/// Omarchy's `default/themed/alacritty.toml.tpl`, verbatim, so a theme looks the
/// same in Alacritty on the laptop and here.
const TEMPLATE: &str = r#"[colors.primary]
background = "{{ background }}"
foreground = "{{ foreground }}"

[colors.cursor]
text = "{{ background }}"
cursor = "{{ cursor }}"

[colors.vi_mode_cursor]
text = "{{ background }}"
cursor = "{{ cursor }}"

[colors.search.matches]
foreground = "{{ background }}"
background = "{{ color3 }}"

[colors.search.focused_match]
foreground = "{{ background }}"
background = "{{ color1 }}"

[colors.footer_bar]
foreground = "{{ background }}"
background = "{{ foreground }}"

[colors.selection]
text = "{{ selection_foreground }}"
background = "{{ selection_background }}"

[colors.normal]
black = "{{ color0 }}"
red = "{{ color1 }}"
green = "{{ color2 }}"
yellow = "{{ color3 }}"
blue = "{{ color4 }}"
magenta = "{{ color5 }}"
cyan = "{{ color6 }}"
white = "{{ color7 }}"

[colors.bright]
black = "{{ color8 }}"
red = "{{ color9 }}"
green = "{{ color10 }}"
yellow = "{{ color11 }}"
blue = "{{ color12 }}"
magenta = "{{ color13 }}"
cyan = "{{ color14 }}"
white = "{{ color15 }}"
"#;

/// Fill every `{{ key }}` from the palette. A missing key is an error naming
/// it -- Omarchy's own renderer is a `sed` and would leave the placeholder in
/// the file, which Alacritty then refuses to load.
pub fn render(theme: &Theme) -> Result<String, String> {
    let mut out = String::new();
    let mut missing = Vec::new();
    let mut rest = TEMPLATE;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find("}}") else { break };
        let key = rest[start + 2..start + end].trim();
        match theme.colors.get(key) {
            Some(v) => out.push_str(v),
            None => missing.push(key.to_string()),
        }
        rest = &rest[start + end + 2..];
    }
    out.push_str(rest);
    if !missing.is_empty() {
        missing.dedup();
        return Err(format!("{} lacks {}", theme.id, missing.join(", ")));
    }
    Ok(format!(
        "# WRITTEN BY panefx -- overwritten on every theme change. Imported last by alacritty.toml.\n# {}\n\n{out}",
        theme.name
    ))
}

const HOUSE_BODY: &str = "# WRITTEN BY panefx -- overwritten on every theme change. Imported last by alacritty.toml.\n# House theme: empty on purpose, so your own palette applies.\n";

fn toml_quote(p: &Path) -> String {
    format!("\"{}\"", p.to_string_lossy().replace('\\', "\\\\"))
}

/// Make `ours` the last `import` of an alacritty.toml. Returns the new text and
/// whether it changed. Handles a one-line `import = [...]` (keeping whatever
/// follows the `]`, such as pHub's comment), a multi-line one, and none at all
/// (added under `[general]`, where Alacritty 0.14+ reads it).
pub fn ensure_import(conf: &str, ours: &Path) -> (String, bool) {
    let nl = if conf.contains("\r\n") { "\r\n" } else { "\n" };
    let q = toml_quote(ours);
    let file_name = ours.file_name().map(|f| f.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut lines: Vec<String> = conf.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect();
    let trailing_newline = conf.ends_with('\n');
    if trailing_newline {
        lines.pop();
    }
    let join = |lines: &[String]| {
        let mut s = lines.join(nl);
        if trailing_newline {
            s.push_str(nl);
        }
        s
    };

    let is_import = |l: &str| {
        let t = l.trim_start();
        t.starts_with("import") && t["import".len()..].trim_start().starts_with('=')
    };
    if let Some(i) = lines.iter().position(|l| is_import(l)) {
        // The span of the array: this line, or down to the line closing it.
        let end = (i..lines.len()).find(|&j| lines[j].contains(']')).unwrap_or(i);
        let span = lines[i..=end].join("\n").to_lowercase();
        if span.contains(&file_name) {
            return (conf.to_string(), false);
        }
        // Replace color.mesh's override in place: same slot, our file.
        for l in &mut lines[i..=end] {
            if let Some(a) = l.to_lowercase().find(LEGACY_NAME) {
                let open = l[..a].rfind('"');
                let close = l[a..].find('"').map(|c| a + c);
                if let (Some(o), Some(c)) = (open, close) {
                    l.replace_range(o..=c, &q);
                    return (join(&lines), true);
                }
            }
        }
        if end == i {
            let l = &lines[i];
            let (open, close) = (l.find('[').unwrap_or(l.len()), l.find(']').unwrap_or(l.len()));
            let inner = l[open + 1..close].trim().trim_end_matches(',').trim();
            let inner = if inner.is_empty() { q } else { format!("{inner}, {q}") };
            lines[i] = format!("{}[{inner}]{}", &l[..open], &l[close + 1..]);
        } else {
            // Multi-line: a new entry before the closing bracket, and a comma on
            // the entry above it if it lacked one.
            let indent: String = lines[end - 1].chars().take_while(|c| c.is_whitespace()).collect();
            if let Some(prev) = (i + 1..end).rev().find(|&j| !lines[j].trim().is_empty()) {
                let t = lines[prev].trim_end().to_string();
                if !t.ends_with(',') && !t.ends_with('[') {
                    lines[prev] = format!("{t},");
                }
            }
            let indent = if indent.is_empty() { "  ".to_string() } else { indent };
            lines.insert(end, format!("{indent}{q},"));
        }
        return (join(&lines), true);
    }
    if let Some(g) = lines.iter().position(|l| l.trim() == "[general]") {
        lines.insert(g + 1, format!("import = [{q}]"));
    } else {
        lines.push(String::new());
        lines.push("[general]".into());
        lines.push(format!("import = [{q}]"));
    }
    (join(&lines), true)
}

impl Alacritty {
    fn conf_path(env: &Env) -> PathBuf {
        if env.live {
            if let Some(p) = crate::term_opacity::alacritty_config_path() {
                return p;
            }
        }
        env.appdata.join("alacritty").join("alacritty.toml")
    }

    fn write(&self, env: &Env, body: &str) -> Result<(), String> {
        let conf = Self::conf_path(env);
        let dir = conf.parent().map(Path::to_path_buf).unwrap_or_default();
        let ours = dir.join("themes").join(FILE_NAME);
        let text = match std::fs::read_to_string(&conf) {
            Ok(t) => t,
            // A folder but no file: Alacritty is set up with defaults. Give it a
            // config that only imports ours, so the theme reaches it too.
            Err(_) if dir.is_dir() => String::new(),
            Err(_) => return Err("not installed".into()),
        };
        write_atomic(&ours, body.as_bytes()).map_err(|e| e.to_string())?;
        let (next, changed) = ensure_import(&text, &ours);
        if changed {
            write_atomic(&conf, next.as_bytes()).map_err(|e| e.to_string())?;
            // color.mesh's file is no longer imported by anything.
            let _ = std::fs::remove_file(dir.join("themes").join(LEGACY_NAME));
        } else {
            touch(&conf).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

impl Target for Alacritty {
    fn id(&self) -> &'static str {
        "alacritty"
    }
    fn label(&self) -> &'static str {
        "Alacritty"
    }
    fn capture(&self, _env: &Env) -> Option<serde_json::Value> {
        // Nothing: house empties our file, and the user's palette shows through.
        None
    }
    fn apply(&self, env: &Env, theme: &Theme) -> Outcome {
        let body = match render(theme) {
            Ok(b) => b,
            Err(e) => return Outcome::Failed(e),
        };
        match self.write(env, &body) {
            Ok(()) => Outcome::Live("palette file written; live reload picks it up".into()),
            Err(e) if e == "not installed" => Outcome::Skipped("no Alacritty config".into()),
            Err(e) => Outcome::Failed(e),
        }
    }
    fn restore(&self, env: &Env, _saved: Option<&serde_json::Value>) -> Outcome {
        match self.write(env, HOUSE_BODY) {
            Ok(()) => Outcome::Live("override emptied; your own palette shows".into()),
            Err(e) if e == "not installed" => Outcome::Skipped("no Alacritty config".into()),
            Err(e) => Outcome::Failed(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::themes::catalog::{load, Dirs};

    fn ours() -> PathBuf {
        PathBuf::from(r"C:\Users\x\AppData\Roaming\alacritty\themes\panefx-theme.toml")
    }

    #[test]
    fn every_theme_renders_with_no_placeholder_left() {
        for t in load(&Dirs::bundled_only()) {
            let out = render(&t).unwrap_or_else(|e| panic!("{e}"));
            assert!(!out.contains("{{"), "{} left a placeholder", t.id);
        }
    }

    #[test]
    fn the_import_goes_last_and_keeps_the_trailing_comment() {
        let conf = "[general]\r\nimport = [\"C:\\\\x\\\\necronomicon.toml\"]   # the house palette\r\nlive_config_reload = true\r\n";
        let (out, changed) = ensure_import(conf, &ours());
        assert!(changed);
        assert!(out.contains(
            "import = [\"C:\\\\x\\\\necronomicon.toml\", \"C:\\\\Users\\\\x\\\\AppData\\\\Roaming\\\\alacritty\\\\themes\\\\panefx-theme.toml\"]   # the house palette\r\n"
        ), "{out}");
        assert!(out.ends_with("live_config_reload = true\r\n"), "line endings kept");
        let (again, changed) = ensure_import(&out, &ours());
        assert!(!changed, "idempotent");
        assert_eq!(again, out);
    }

    #[test]
    fn color_meshs_override_is_replaced_in_place() {
        let conf = "import = [\"a.toml\", \"C:\\\\y\\\\themes\\\\color.mesh.toml\"]\n";
        let (out, _) = ensure_import(conf, &ours());
        assert!(!out.contains("color.mesh.toml"), "{out}");
        assert_eq!(out.matches("panefx-theme.toml").count(), 1);
        assert!(out.starts_with("import = [\"a.toml\", "));
    }

    #[test]
    fn a_multi_line_import_gets_a_new_last_entry() {
        let conf = "import = [\n  \"a.toml\"\n]\nx = 1\n";
        let (out, _) = ensure_import(conf, &ours());
        assert_eq!(
            out,
            "import = [\n  \"a.toml\",\n  \"C:\\\\Users\\\\x\\\\AppData\\\\Roaming\\\\alacritty\\\\themes\\\\panefx-theme.toml\",\n]\nx = 1\n"
        );
    }

    #[test]
    fn no_import_at_all_goes_under_general() {
        let (out, _) = ensure_import("[window]\nopacity = 0.6\n", &ours());
        assert!(out.ends_with("[general]\nimport = [\"C:\\\\Users\\\\x\\\\AppData\\\\Roaming\\\\alacritty\\\\themes\\\\panefx-theme.toml\"]\n"), "{out}");
        let (out, _) = ensure_import("[general]\nlive_config_reload = true\n", &ours());
        assert!(out.starts_with("[general]\nimport = ["), "{out}");
    }

    #[test]
    fn apply_then_house_leaves_only_the_one_import() {
        let root = std::env::temp_dir().join("panefx-test-alacritty");
        let _ = std::fs::remove_dir_all(&root);
        let env = Env::under(&root);
        let conf = env.appdata.join("alacritty").join("alacritty.toml");
        std::fs::create_dir_all(conf.parent().unwrap()).unwrap();
        std::fs::write(&conf, "import = []\nopacity = 0.72\n").unwrap();
        let t = load(&Dirs::bundled_only()).into_iter().find(|t| t.id == "nord").unwrap();
        assert!(matches!(Alacritty.apply(&env, &t), Outcome::Live(_)));
        let file = conf.parent().unwrap().join("themes").join(FILE_NAME);
        assert!(std::fs::read_to_string(&file).unwrap().contains("[colors.normal]"));
        assert!(matches!(Alacritty.restore(&env, None), Outcome::Live(_)));
        assert!(!std::fs::read_to_string(&file).unwrap().contains("[colors"), "house empties it");
        assert!(std::fs::read_to_string(&conf).unwrap().ends_with("opacity = 0.72\n"), "opacity untouched");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_alacritty_is_skipped_not_failed() {
        let root = std::env::temp_dir().join("panefx-test-alacritty-none");
        let _ = std::fs::remove_dir_all(&root);
        let t = load(&Dirs::bundled_only()).into_iter().nth(1).unwrap();
        assert!(matches!(Alacritty.apply(&Env::under(&root), &t), Outcome::Skipped(_)));
    }
}
