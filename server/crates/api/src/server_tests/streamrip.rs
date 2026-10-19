//! streamrip (encore_requests::sources::streamrip) on a server's settings, with a stand-in for its `rip`
//! program: its config written, checked and reset, and its failures in plain words.
use crate::state::AppState;
use encore_http::Ctx as _;
use encore_requests::sources::streamrip::*;
use std::path::{Path, PathBuf};

fn state(name: &str) -> (AppState, PathBuf) {
    let dir = std::env::temp_dir().join(format!("encore-rip-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = crate::test_support::state_in(&dir);
    (st, dir)
}

/// A stand-in for streamrip's `rip`, in `dir`: `config path` writes the 2.x default file when there is
/// none and complains the way the fork does about a file marked `broken = true`; `url` does what the
/// `mode` file beside it says (prompt for the ARL, fail to load the config, or download one song).
/// Every call, with what its stdin is, lands in `calls.log`.
fn fake_rip(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).unwrap();
    let body = r#"here=$(dirname "$0")
echo "call: $* stdin=$(readlink /proc/$$/fd/0 2>/dev/null || echo unknown)" >> "$here/calls.log"
cfg=""; folder=""; sub=""; url=""
while [ $# -gt 0 ]; do
  case "$1" in
--config-path) cfg=$2; shift;;
--folder) folder=$2; shift;;
--quality) shift;;
config) sub=config;;
path) [ "$sub" = config ] && sub=path;;
reset) [ "$sub" = config ] && sub=reset;;
url) sub=url; url=$2; shift;;
  esac
  shift
done
if [ ! -f "$cfg" ]; then
  echo "No file found at $cfg, creating default config."
  cat > "$cfg" <<'EOF'
[downloads]
folder = "/x"

[qobuz]
quality = 3
use_auth_token = false
email_or_userid = ""
password_or_token = ""

[tidal]
quality = 3
user_id = ""
country_code = ""
access_token = ""
refresh_token = ""
token_expiry = ""

[deezer]
quality = 2
arl = ""

[database]
downloads_enabled = true

[misc]
version = "2.0.6"
EOF
fi
if grep -q '^broken = true' "$cfg"; then
  echo "Error loading config from $cfg: "
  echo "QobuzConfig.__init__() got an unexpected keyword argument 'user_id'"
  echo "Try running rip config reset"
  exit 0
fi
case "$sub" in
  path) echo "$cfg";;
  url)
mode=$(cat "$here/mode" 2>/dev/null)
case "$mode" in
  prompt)
    echo "If you're not sure how to find the ARL cookie, see the instructions at https://github.com/nathom/streamrip/wiki/Finding-Your-Deezer-ARL-Cookie"
    echo "Enter your ARL: Traceback (most recent call last):"
    echo "  File \"rip/prompter.py\", line 80, in prompt_creds"
    echo "EOFError: EOF when reading a line"
    exit 1;;
  config_error)
    echo "Error loading config from $cfg: "
    echo "Need to update config from 2.0.5 to 2.0.6"
    echo "Try running rip config reset"
    exit 0;;
  *) mkdir -p "$folder/Album"; printf 'fLaC' > "$folder/Album/01 Song.flac";;
esac;;
esac
"#;
    let p = dir.join("rip");
    let tmp = dir.join(".rip.tmp");
    std::fs::write(&tmp, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&tmp, &p).unwrap();
    p
}

#[tokio::test]
async fn a_missing_or_broken_config_is_written_fresh_with_the_sign_ins_carried_over() {
    let (st, dir) = state("config");
    let rip = fake_rip(&dir.join("fake"));
    st.cfg().set_many(&serde_json::json!({ "streamrip_enabled": true, "streamrip_bin": rip.to_string_lossy(), "streamrip_qobuz_user": "qu", "streamrip_qobuz_token": "qt" }));
    assert_eq!(binary(&st).as_deref(), Some(rip.to_string_lossy().as_ref()));
    // no file yet: streamrip writes its defaults, Encore its credentials (2.x key names, downloads db off)
    let c = check_config(&st, false).await.unwrap();
    assert!(c.regenerated && c.backup.is_none() && c.problem.is_none(), "{c:?}");
    let text = std::fs::read_to_string(&c.path).unwrap();
    assert_eq!(toml_value(&text, "qobuz", "email_or_userid"), "qu");
    assert_eq!(toml_value(&text, "qobuz", "password_or_token"), "qt");
    assert_eq!(toml_value(&text, "qobuz", "use_auth_token"), "true");
    assert!(!section_body(&text, "qobuz").contains("user_id ="), "{text}");
    assert_eq!(toml_value(&text, "database", "downloads_enabled"), "false");
    assert_eq!(toml_value(&text, "misc", "version"), "2.0.6");
    // a sound file is left alone, and Diagnostics says so
    let c = check_config(&st, false).await.unwrap();
    assert!(!c.regenerated && c.problem.is_none(), "{c:?}");
    let check = config_loads_check(&st).await;
    assert_eq!(check["ok"], true, "{check}");
    assert_eq!(check["name"], "streamrip config loads");
    // the file breaks (keys from another streamrip, say) while holding a Tidal sign-in, a hand-typed ARL
    // and the 1.x Qobuz login
    let mut broken = text.clone();
    for (k, v) in [("access_token", "\"tok-default-abcdef\""), ("hires_access_token", "\"tok-hires-abcdef\""), ("hires_client", "true")] {
        broken = set_toml(&broken, "tidal", k, v);
    }
    broken = set_toml(&broken, "deezer", "arl", "\"old-arl-cookie\"");
    broken = set_toml(&broken, "qobuz", "email_or_userid", "\"\"");
    broken = set_toml(&broken, "qobuz", "password_or_token", "\"\"");
    broken = set_toml(&broken, "qobuz", "user_id", "\"polluted\"");
    broken = set_toml(&broken, "qobuz", "auth_token", "\"polluted-token\"");
    broken = set_toml(&broken, "misc", "broken", "true");
    std::fs::write(&c.path, &broken).unwrap();
    st.cfg().set_many(&serde_json::json!({ "streamrip_qobuz_user": "", "streamrip_qobuz_token": "" }));
    let check = config_loads_check(&st).await;
    assert_eq!(check["ok"], false, "{check}");
    let detail = check["detail"].as_str().unwrap();
    assert!(detail.contains("unexpected keyword argument 'user_id'") && detail.contains("Reset streamrip config"), "{detail}");
    let c = check_config(&st, false).await.unwrap();
    assert!(c.regenerated && c.problem.is_none(), "{c:?}");
    let backup = c.backup.clone().expect("the old file is kept");
    assert!(backup.file_name().unwrap().to_string_lossy().starts_with("config.toml.bak-"), "{backup:?}");
    assert!(std::fs::read_to_string(&backup).unwrap().contains("broken = true"));
    let fresh = std::fs::read_to_string(&c.path).unwrap();
    assert!(!fresh.contains("broken"), "{fresh}");
    let t = parse_tidal_toml(&fresh);
    assert_eq!(t["access_token"], "tok-default-abcdef");
    assert_eq!(t["hires_access_token"], "tok-hires-abcdef");
    assert_eq!(t["hires_client"], "true");
    assert_eq!(toml_value(&fresh, "deezer", "arl"), "old-arl-cookie");
    assert_eq!(toml_value(&fresh, "qobuz", "email_or_userid"), "polluted", "the 1.x login moves under the 2.x key names");
    assert_eq!(toml_value(&fresh, "qobuz", "password_or_token"), "polluted-token");
    assert_eq!(toml_value(&fresh, "qobuz", "use_auth_token"), "true");
    assert!(!fresh.lines().any(|l| l.starts_with("user_id = \"polluted") || l.starts_with("auth_token =")), "{fresh}");
    assert!(c.message().contains("carried over"), "{}", c.message());
    // "Reset streamrip config" on a sound file: a fresh one all the same, sign-ins kept
    let c = reset_config(&st).await.unwrap();
    assert!(c.regenerated && c.backup.is_some() && c.problem.is_none(), "{c:?}");
    assert_eq!(parse_tidal_toml(&std::fs::read_to_string(&c.path).unwrap())["access_token"], "tok-default-abcdef");
    assert_eq!(c.json()["ok"], true);
    // a run whose output is the fork's config complaint: plain words, the whole output for the log
    std::fs::write(dir.join("fake/mode"), "config_error").unwrap();
    let e = download(&st, "https://tidal.com/browse/track/1", &dir.join("out")).await.expect_err("no files");
    let m = e.to_string();
    assert!(m.starts_with(CONFIG_BROKEN_MSG), "{m}");
    assert!(m.contains("Need to update config from 2.0.5 to 2.0.6"), "{m}");
    assert!(run_log(&e).unwrap().contains("Try running rip config reset"));
    let calls = std::fs::read_to_string(dir.join("fake/calls.log")).unwrap();
    assert!(calls.lines().last().unwrap().contains("url https://tidal.com/browse/track/1"), "{calls}");
    assert!(calls.lines().all(|l| l.contains("stdin=/dev/null")), "stdin is closed on every run:\n{calls}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_deezer_arl_prompt_becomes_a_plain_message() {
    let (st, dir) = state("arl");
    let rip = fake_rip(&dir.join("fake"));
    st.cfg().set_many(&serde_json::json!({ "streamrip_enabled": true, "streamrip_bin": rip.to_string_lossy() }));
    let out = dir.join("out");
    // no ARL anywhere: a Deezer link is refused before streamrip runs
    let e = download(&st, "https://www.deezer.com/track/1", &out).await.expect_err("no ARL");
    assert_eq!(e.to_string(), ARL_MSG);
    assert!(run_log(&e).is_none());
    let calls = std::fs::read_to_string(dir.join("fake/calls.log")).unwrap();
    assert!(!calls.contains(" url "), "{calls}");
    // an ARL streamrip still asks about (its prompt reads EOF): the same plain message, the output kept
    st.cfg().set_many(&serde_json::json!({ "streamrip_deezer_arl": "a-cookie" }));
    std::fs::write(dir.join("fake/mode"), "prompt").unwrap();
    let e = download(&st, "https://www.deezer.com/track/1", &out).await.expect_err("prompted");
    assert_eq!(e.to_string(), ARL_MSG);
    assert!(run_log(&e).unwrap().contains("EOFError: EOF when reading a line"));
    let calls = std::fs::read_to_string(dir.join("fake/calls.log")).unwrap();
    assert!(calls.contains("url https://www.deezer.com/track/1"), "{calls}");
    assert_eq!(toml_value(&std::fs::read_to_string(config_path(&st)).unwrap(), "deezer", "arl"), "a-cookie", "the ARL from Settings is in the file before the run");
    // and a run that works
    std::fs::write(dir.join("fake/mode"), "ok").unwrap();
    let files = download(&st, "https://www.deezer.com/track/1", &out).await.unwrap();
    assert_eq!(files.len(), 1);
    assert!(files[0].ends_with("Album/01 Song.flac"), "{files:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
