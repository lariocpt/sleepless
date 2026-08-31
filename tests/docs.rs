//! The gate that stops the README, the website and the code from drifting apart.
//!
//! Every check here exists because one of them was already wrong: the site said
//! sleepless had "no state directory to get out of sync" while the README
//! documented one in detail, its block font claimed to be "lifted verbatim from
//! src/art.rs" and was missing every digit, and it quoted a pulse cadence that
//! belonged to the tray. None of that could be caught by anything, because a
//! documented feature that does not exist compiles perfectly.
//!
//! The rule is that the code is the source of truth and the prose is checked
//! against it, never the other way round.

use clap::CommandFactory;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use sleepless::run::Action;
use sleepless::{art, cli::Args, config, run};
use std::collections::BTreeSet;
use std::path::Path;

fn repo_file(name: &str) -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display()))
}
fn readme() -> String {
    repo_file("README.md")
}
fn site() -> String {
    repo_file("docs/index.html")
}

/// Long flags the program really has. `--help`/`--version` are clap's own and are
/// deliberately not part of the documented surface.
fn flags(include_hidden: bool) -> BTreeSet<String> {
    Args::command()
        .get_arguments()
        .filter(|a| !matches!(a.get_id().as_str(), "help" | "version"))
        .filter(|a| include_hidden || !a.is_hide_set())
        .filter_map(|a| a.get_long())
        .map(|l| format!("--{l}"))
        .collect()
}

/// The `--flag` tokens on a command line that actually invokes sleepless. Scoped
/// that way because both documents are full of cargo, clippy and systemd flags,
/// and a blanket scan would call every one of them ours.
fn flags_in_sleepless_commands(lines: impl Iterator<Item = String>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in lines {
        let mut words = line.split_whitespace().peekable();
        let Some(first) = words.peek().copied() else {
            continue;
        };
        // A bare `<code>--splash</code>` style reference names one flag.
        if first.starts_with("--") {
            out.insert(first.trim_end_matches(['.', ',', ')']).to_string());
            continue;
        }
        if first != "sleepless" && !line.starts_with("cargo run -- ") {
            continue;
        }
        for w in words {
            // Everything after a `#` is the comment beside the command, and the
            // prose in there is allowed to name other programs' flags.
            if w.starts_with('#') {
                break;
            }
            if w.starts_with("--") && w.len() > 2 {
                out.insert(w.trim_end_matches(['.', ',', ')']).to_string());
            }
        }
    }
    out
}

fn readme_command_lines() -> Vec<String> {
    readme()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| l.starts_with("sleepless ") || l.starts_with("cargo run -- "))
        .collect()
}

/// The text inside every `<code>` element, plus every line of the site that runs
/// `sleepless`. HTML entities the site uses inside commands are folded back first.
fn site_command_lines() -> Vec<String> {
    let html = site();
    let mut out: Vec<String> = Vec::new();
    let mut rest = html.as_str();
    while let Some(i) = rest.find("<code>") {
        rest = &rest[i + "<code>".len()..];
        let Some(j) = rest.find("</code>") else { break };
        out.push(rest[..j].to_string());
        rest = &rest[j..];
    }
    // The install panels are `<div class="cmd">$cargo install ...</div>`: not <code>,
    // and not a line beginning with `sleepless`, so neither rule above sees them.
    let mut rest = html.as_str();
    while let Some(i) = rest.find(r#"<div class="cmd">"#) {
        rest = &rest[i..];
        let Some(j) = rest.find("</div>") else { break };
        let cmd = strip_tags(&rest[..j]);
        out.push(cmd.trim().trim_start_matches('$').trim().to_string());
        rest = &rest[j..];
    }
    for line in html.lines() {
        let t = strip_tags(line);
        let t = t.trim();
        if t.starts_with("sleepless ") {
            out.push(t.to_string());
        }
    }
    out
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

#[test]
fn readme_documents_every_flag_the_program_has() {
    let readme = readme();
    let mut missing: Vec<String> = flags(false)
        .into_iter()
        .filter(|f| !readme.contains(f.as_str()))
        .collect();
    // --smoke is hidden from `--help` but documented under Development, because it
    // is what CI asserts against and contributors need it.
    if !readme.contains("--smoke") {
        missing.push("--smoke".into());
    }
    assert!(
        missing.is_empty(),
        "README does not document: {missing:?} -- a flag nobody can find is a flag nobody has"
    );
}

#[test]
fn neither_document_invents_a_flag() {
    let real = flags(true);
    for (what, found) in [
        (
            "README.md",
            flags_in_sleepless_commands(readme_command_lines().into_iter()),
        ),
        (
            "docs/index.html",
            flags_in_sleepless_commands(site_command_lines().into_iter()),
        ),
    ] {
        let invented: Vec<_> = found.difference(&real).collect();
        assert!(
            invented.is_empty(),
            "{what} documents flags the program does not have: {invented:?}"
        );
        assert!(
            !found.is_empty(),
            "{what}: the scan found no flags at all, so it proves nothing"
        );
    }
}

#[test]
fn readme_config_keys_are_exactly_the_keys_the_program_reads() {
    // The README's config.toml example is the specification users copy from, so it
    // has to parse and it has to be complete in both directions.
    let readme = readme();
    let block = readme
        .split("```toml")
        .nth(1)
        .and_then(|b| b.split("```").next())
        .expect("README has a ```toml config example");
    let table: toml::Table = block
        .parse()
        .expect("the README's config example must parse");
    let _: config::FileConfig =
        toml::from_str(block).expect("the README's config example must deserialize");

    for k in table.keys() {
        assert!(
            config::KNOWN_TOP.contains(&k.as_str()),
            "README documents config key {k:?}, which the program ignores"
        );
    }
    for k in config::KNOWN_TOP {
        assert!(
            table.contains_key(*k) || block.contains(&format!("[[{k}]]")),
            "config key {k:?} exists but the README never mentions it"
        );
    }

    let splashes = table["splashes"].as_array().expect("[[splashes]] entries");
    let documented: BTreeSet<&str> = splashes
        .iter()
        .filter_map(|d| d.as_table())
        .flat_map(|t| t.keys().map(String::as_str))
        .collect();
    for k in &documented {
        assert!(
            config::KNOWN_SPLASH.contains(k),
            "README documents splash key {k:?}, which the program ignores"
        );
    }
    for k in config::KNOWN_SPLASH {
        assert!(
            documented.contains(k),
            "splash key {k:?} exists but the README's example never shows it"
        );
    }
}

#[test]
fn readme_lists_the_built_in_splashes_that_exist() {
    let readme = readme();
    let built_in = config::builtin_splashes();
    for s in &built_in {
        assert!(
            readme.contains(&format!("\"{}\"", s.name)),
            "built-in splash {:?} is not named in the README",
            s.name
        );
    }
    assert!(
        readme.contains(&format!("{} are built in", spell(built_in.len()))),
        "the README's built-in count is out of date: there are {} now",
        built_in.len()
    );
}

fn spell(n: usize) -> &'static str {
    match n {
        2 => "Two",
        3 => "Three",
        4 => "Four",
        5 => "Five",
        _ => panic!("teach spell() about {n}"),
    }
}

#[test]
fn readme_block_font_charset_matches_the_font() {
    // "A-Z 0-9 '-!?." -- an unsupported character renders as a blank rather than
    // failing, so documenting one that does not exist looks exactly like
    // documenting one that does.
    let readme = readme();
    let documented = readme
        .lines()
        .find(|l| l.contains("rendered with the block font"))
        .expect("README documents the block font's charset");
    let punctuation: String = art::SUPPORTED
        .chars()
        .filter(|c| !c.is_ascii_alphanumeric())
        .collect();
    assert!(
        documented.contains("A-Z")
            && documented.contains("0-9")
            && documented.contains(&punctuation),
        "README says {documented:?} but the font supports {:?}",
        art::SUPPORTED
    );
    for c in art::SUPPORTED.chars() {
        assert!(art::has_glyph(c), "{c:?} is advertised but has no glyph");
    }
}

#[test]
fn every_key_the_readme_documents_does_something() {
    let readme = readme();
    let documented: &[(&str, KeyCode, KeyModifiers, Action)] = &[
        (
            "**m**",
            KeyCode::Char('m'),
            KeyModifiers::NONE,
            Action::ToggleMode,
        ),
        (
            "**l**",
            KeyCode::Char('l'),
            KeyModifiers::NONE,
            Action::ToggleLid,
        ),
        (
            "**←/→**",
            KeyCode::Left,
            KeyModifiers::NONE,
            Action::PrevSplash,
        ),
        (
            "**←/→**",
            KeyCode::Right,
            KeyModifiers::NONE,
            Action::NextSplash,
        ),
        (
            "**q**",
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            Action::Quit,
        ),
        ("Esc", KeyCode::Esc, KeyModifiers::NONE, Action::Quit),
        (
            "Ctrl-C",
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            Action::Quit,
        ),
    ];
    for (doc, code, mods, want) in documented {
        assert!(
            readme.contains(doc),
            "README no longer documents the {doc} key"
        );
        assert_eq!(
            run::action_for(*code, *mods),
            *want,
            "the README documents {doc} but the program does something else with it"
        );
    }
}

#[test]
fn the_site_does_not_deny_the_state_directory() {
    // The site used to argue the product's whole case with "there is no --stop flag,
    // no PID file and no state directory to get out of sync". There is one, and the
    // README documents it in detail.
    let site = site();
    for lie in [
        "no state directory",
        "no state dir",
        "nothing is written to disk",
    ] {
        assert!(
            !site.to_lowercase().contains(lie),
            "docs/index.html claims {lie:?}; state.toml exists"
        );
    }
    assert!(
        site.contains("state.toml"),
        "the site should say where the remembered settings live, since it discusses what is on disk"
    );
    assert!(
        config::state_path().is_some_and(|p| p.ends_with("state.toml")),
        "and there really is one"
    );
}

#[test]
fn the_sites_pulse_matches_the_programs() {
    // The site quoted 750 ms next to the splash. That is the tray cadence; the
    // splash runs at half of it.
    let site = site();
    let interval = site
        .split("setInterval(")
        .nth(1)
        .and_then(|s| s.rsplit_once("}, "))
        .and_then(|(_, tail)| tail.split(')').next())
        .and_then(|n| n.trim().parse::<u64>().ok())
        .expect("the site animates the splash with setInterval");
    assert_eq!(
        interval,
        run::SPLASH_PULSE_MS,
        "the site pulses the splash at {interval} ms; the program uses {} ms ({} ms is the tray)",
        run::SPLASH_PULSE_MS,
        run::TRAY_PULSE_MS
    );
    assert!(
        site.contains(&format!("{} ms", run::SPLASH_PULSE_MS))
            || site.contains(&format!("{}ms", run::SPLASH_PULSE_MS)),
        "the site should quote the cadence it actually uses"
    );
}

#[test]
fn the_sites_block_font_is_the_programs_block_font() {
    // "lifted verbatim from src/art.rs" was a copy-paste of the letters only: every
    // digit and every punctuation glyph was missing, and nothing noticed.
    let site = site();
    let block = site
        .split("const GLYPHS = {")
        .nth(1)
        .and_then(|s| s.split("};").next())
        .expect("the site defines a GLYPHS table");

    let mut seen = BTreeSet::new();
    for entry in block.split(',') {
        let Some((key, _)) = entry.split_once(':') else {
            continue;
        };
        let key = key.trim().trim_matches(['"', '\n', ' ']);
        let mut chars = key.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            continue;
        };
        seen.insert(c);
    }
    let want: BTreeSet<char> = art::SUPPORTED.chars().collect();
    let missing: Vec<_> = want.difference(&seen).collect();
    let extra: Vec<_> = seen.difference(&want).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "the site's font is not the program's: missing {missing:?}, extra {extra:?} \
         -- run tools/gen-site-font.py"
    );

    // And the rows themselves, not just the key set.
    for c in art::SUPPORTED.chars() {
        for row in art::glyph(c) {
            assert!(
                block.contains(&format!("\"{row}\"")),
                "the site's glyph for {c:?} does not contain the row {row:?} \
                 -- run tools/gen-site-font.py"
            );
        }
    }
}

#[test]
fn the_readme_names_the_checksum_files_that_actually_exist() {
    // The README told people to fetch `sleepless-<target>-<tag>.sha256`. The release
    // publishes `sleepless-<target>-<tag>.tar.gz.sha256` -- the archive's own name plus
    // a suffix, which is what `sha256sum -c` reads out of it. The documented command
    // 404'd on the checksum and every other line of it worked, so it looked fine.
    let readme = readme();
    let mut checked = 0;
    let mut fenced = false;
    for line in readme.lines() {
        if line.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if !fenced {
            continue;
        }
        for tok in line.split(|c: char| c.is_whitespace() || c == '"' || c == '\'') {
            if !tok.contains(".sha256") {
                continue;
            }
            checked += 1;
            // The rule is that the checksum file is the ARCHIVE's name plus
            // `.sha256`, so a reference is either a literal ending in
            // `.tar.gz.sha256`/`.zip.sha256`, or the snippet's `$archive` variable --
            // which is bound to the archive name two lines above it.
            let names_the_archive = tok.ends_with(".tar.gz.sha256")
                || tok.ends_with(".zip.sha256")
                || tok.ends_with("archive.sha256")
                || tok.ends_with("archive}.sha256");
            assert!(
                names_the_archive,
                "README asks for {tok:?}; the release publishes <archive>.sha256, so \
                 that URL is a 404"
            );
        }
    }
    assert!(
        checked > 0,
        "the README no longer shows how to verify a download"
    );
}

#[test]
fn the_install_methods_are_the_same_on_both_pages() {
    // "Offered" means a command the page tells you to run, not a mention. The README's
    // prose discusses the AUR package -- written, ready, and unpublishable while AUR
    // registrations are closed -- and a check that could not tell an offer from an
    // explanation would demand the website advertise something nobody can install.
    // tools/check-channels.sh scopes itself to the same block for the same reason.
    let readme = readme();
    let offers = readme
        .split("## Install")
        .nth(1)
        .and_then(|s| s.split("```sh").nth(1))
        .and_then(|s| s.split("```").next())
        .expect("README has an Install block");
    let site = site();
    let site_offers: String = site_command_lines().join("\n");

    for method in [
        "cargo install sleepless",
        "brew install lariocpt/sleepless/sleepless",
        "cargo install --path .",
        "sleepless-bin",
    ] {
        assert_eq!(
            offers.contains(method),
            site_offers.contains(method),
            "{method:?} is offered on one page and not the other"
        );
    }
    // The releases page is a link rather than a command, so it is checked as prose.
    let releases = "https://github.com/lariocpt/sleepless/releases";
    assert!(readme.contains(releases), "README omits the releases page");
    assert!(site.contains(releases), "the site omits the releases page");
}
