use super::*;

const MD: &str = "## Encore 0.36.0

**Clean up your library by type, see who requested what.** If you skipped
0.35.2, its fixes are in [0.35.2](https://example.org/0.35.2.md).

### Library
- **Delete albums by type.** Library → Albums has a **Type** filter: Studio albums,
  Live, Compilations.
- How the type is known: `RELEASETYPE`, then the title.

### What runs at once
| Source | At once |
|---|---|
| streamrip | 1 on Deezer |

### Before you update
- The database goes from schema 30 to 31.
";

#[test]
fn notes_become_blocks() {
    let r = parse("0.36.0", MD);
    assert_eq!(r.title, "Encore 0.36.0");
    assert_eq!(r.summary, "Clean up your library by type, see who requested what. If you skipped 0.35.2, its fixes are in 0.35.2.");
    assert_eq!(r.highlights, vec!["Delete albums by type".to_string()]);
    assert_eq!(r.blocks[1], Block::H { text: "Library".into() });
    assert_eq!(r.blocks[2], Block::Li { lead: "Delete albums by type.".into(), text: "Library → Albums has a Type filter: Studio albums, Live, Compilations.".into() });
    assert_eq!(r.blocks[3], Block::Li { lead: String::new(), text: "How the type is known: RELEASETYPE, then the title.".into() });
    assert_eq!(r.blocks[5], Block::Row { cells: vec!["Source".into(), "At once".into()], head: true });
    assert_eq!(r.blocks[6], Block::Row { cells: vec!["streamrip".into(), "1 on Deezer".into()], head: false });
    assert_eq!(r.blocks.len(), 9);
}

#[test]
fn a_heading_with_a_name_keeps_the_name() {
    assert_eq!(parse("1.0.0", "## Encore 1.0.0 – Encore 2 (Rust) is here\n\nText.").title, "Encore 2 (Rust) is here");
}

#[test]
fn without_bold_leads_the_first_bullets_are_the_highlights() {
    let r = parse("0.1.0", "## Encore 0.1.0\n\n### Fixed\n- One thing.\n- Another thing.\n");
    assert_eq!(r.highlights, vec!["One thing.".to_string(), "Another thing.".to_string()]);
}

#[test]
fn plain_keeps_the_words() {
    assert_eq!(plain("**Pause for everyone** is in the *player* `menu` – see [docs](http://x/y)"), "Pause for everyone is in the player menu – see docs");
    assert_eq!(plain("snake_case stays, _italic_ goes"), "snake_case stays, italic goes");
}

fn rel(v: &str) -> Release {
    parse(v, &format!("## Encore {v}\n"))
}

#[test]
fn only_releases_after_the_seen_one_and_not_past_this_server() {
    let all = vec![rel("0.37.0"), rel("0.36.1"), rel("0.36.0"), rel("0.35.2")];
    let v = |l: Vec<&Release>| l.iter().map(|r| r.version.clone()).collect::<Vec<_>>();
    // a hotfix build of 0.36.1 includes 0.36.1, never the notes written ahead for 0.37.0
    assert_eq!(v(unseen(&all, "0.36.1.17", Some("0.35.2"))), vec!["0.36.1", "0.36.0"]);
    // another hotfix of a release already seen: nothing new
    assert_eq!(v(unseen(&all, "0.36.1.18", Some("0.36.1.17"))), Vec::<String>::new());
    // never seen: the newest only
    assert_eq!(v(unseen(&all, "0.36.1", None)), vec!["0.36.1"]);
    assert_eq!(v(unseen(&all, "0.36.1", Some(""))), vec!["0.36.1"]);
}

#[test]
fn the_built_in_notes_parse() {
    // docs/releases is in the repository; every file gives a title and at least one block
    for r in releases() {
        assert!(!r.title.is_empty() && !r.blocks.is_empty(), "{}", r.version);
    }
    assert_eq!(base("0.36.0.17"), "0.36.0");
}
