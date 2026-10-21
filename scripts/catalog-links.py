#!/usr/bin/env python3
"""Resolve every download of the plugin catalog (server/crates/plugins/catalog/*.toml) the way Encore's
installer finds it, and check every sum the manifests name. Run by .github/workflows/catalog-links.yml
(by hand); it changes nothing.

For each catalog plugin:
- [update] github + [[binary]]: the newest release whose tag fits `tag` (drafts left out, pre-releases only with
  `prerelease`, the 50 newest releases as Encore reads them) that has the asset of every arch; each asset's
  download address answers over https; its sum is there: `sums = "github"` – GitHub lists a sha256 digest,
  a file name – that file is in the release and names the asset, an https address – it names the asset.
  A `pin` is resolved too (the version Install puts in).
- [python]: `from = "pypi"` – PyPI's current version and its files with their sha256; a package given as an
  https address (streamrip's archive zip) – the address answers for the pinned tag and for the newest one.
- [[download]] (the Translator's models): the address answers; `sums = "huggingface"` – Hugging Face lists the
  file's sha256 (its LFS id).

--download also fetches each release asset and compares its sha256 with the sum (--models: the model files too,
several GB each). Exit status 1 when anything failed. Python 3.11+ (tomllib), standard library only.
Set GITHUB_TOKEN for GitHub's API (60 requests an hour without one).
"""
import hashlib
import json
import os
import re
import sys
import tomllib
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

API = "https://api.github.com"
ARCHES = ["x86_64", "aarch64"]
UA = "encore-catalog-links"
failures = []
warnings = []
lines = []


def say(ok, plugin, what):
    line = f"{'ok  ' if ok else 'FAIL'} {plugin}: {what}"
    print(line, flush=True)
    lines.append(("ok" if ok else "**failed**", plugin, what))
    if not ok:
        failures.append(line)


def warn(plugin, what):
    """Something Encore copes with (it installs and writes a line in the plugin's log) but worth knowing."""
    print(f"WARN {plugin}: {what}", flush=True)
    lines.append(("warning", plugin, what))
    warnings.append(what)


class HttpsOnly(urllib.request.HTTPRedirectHandler):
    """Follows redirects the way Encore does: https only, at most 10."""
    max_redirections = 10

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if not newurl.startswith("https://"):
            raise urllib.error.URLError(f"redirected to a non-https address: {newurl}")
        new = super().redirect_request(req, fp, code, msg, headers, newurl)
        if new is not None and urllib.parse.urlsplit(newurl).hostname != "api.github.com":
            new.remove_header("Authorization")
        return new


OPENER = urllib.request.build_opener(HttpsOnly)


def request(url, headers=None, method="GET"):
    if not url.startswith("https://"):
        raise urllib.error.URLError(f"not an https address: {url}")
    h = {"User-Agent": UA}
    h.update(headers or {})
    return OPENER.open(urllib.request.Request(url, headers=h, method=method), timeout=60)


def get_json(url):
    headers = {}
    if url.startswith(API):
        headers = {"Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28"}
        token = os.environ.get("GITHUB_TOKEN", "").strip()
        if token:
            headers["Authorization"] = f"Bearer {token}"
    with request(url, headers) as r:
        return json.load(r)


def get_text(url, limit=1 << 20):
    with request(url) as r:
        return r.read(limit).decode("utf-8", "replace")


def resolves(url):
    """The address answers (the first byte of it): (True, size or "") or (False, why)."""
    try:
        with request(url, {"Range": "bytes=0-0"}) as r:
            size = ""
            cr = r.headers.get("Content-Range", "")
            if "/" in cr:
                size = cr.rsplit("/", 1)[1]
            elif r.headers.get("Content-Length"):
                size = r.headers["Content-Length"]
            r.read(1)
            return True, size
    except (urllib.error.URLError, OSError) as e:
        return False, str(e)


def sha256_of_url(url):
    h = hashlib.sha256()
    with request(url) as r:
        while chunk := r.read(1 << 20):
            h.update(chunk)
    return h.hexdigest()


def version_parts(v):
    """The numeric dot parts, as Encore compares versions ("v2.4.10" → [2, 4, 10], "6800" → [6800])."""
    m = re.search(r"\d+(?:\.\d+)+", v)
    if m:
        s = m.group(0)
    else:
        s = v.strip().lstrip("vV")
        if not s.isdigit():
            return []
    return [int(re.match(r"\d*", p).group(0) or 0) for p in s.split(".")]


def newer(a, b):
    x, y = version_parts(a), version_parts(b)
    n = max(len(x), len(y))
    return (x + [0] * (n - len(x))) > (y + [0] * (n - len(y)))


def is_version(s):
    return bool(s) and len(s) <= 40 and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9.+_-]*", s) is not None and re.search(r"\d", s) is not None


def version_in_tag(update, tag):
    pre, _, post = update.get("tag", "v{version}").partition("{version}")
    if not tag.startswith(pre) or not tag.endswith(post):
        return None
    v = tag[len(pre):len(tag) - len(post)] if post else tag[len(pre):]
    return v if is_version(v) else None


def fill(template, version, tag):
    return template.replace("{version}", version).replace("{tag}", tag)


def asset_names(binary):
    a = binary.get("asset")
    return [a] if isinstance(a, str) else list(a or [])


def sum_for(text, name):
    """The sha256 a sums file lists for `name` ("<hex>  name" or "<hex> *name")."""
    for line in text.splitlines():
        parts = line.strip().split()
        if len(parts) >= 2 and re.fullmatch(r"[0-9a-fA-F]{64}", parts[0]) and parts[-1].lstrip("*").split("/")[-1] == name:
            return parts[0].lower()
    return None


def release_sum(update, release, name):
    sums = update.get("sums")
    if not sums:
        return None, "the manifest names no sum"
    if sums == "github":
        a = next((a for a in release["assets"] if a["name"] == name), None)
        d = (a or {}).get("digest") or ""
        return (d[7:].lower(), "GitHub's digest") if d.startswith("sha256:") else (None, f"GitHub lists no sha256 for {name} (Encore installs it with a line in the plugin's log)")
    url = sums
    if not sums.startswith("https://"):
        a = next((a for a in release["assets"] if a["name"] == sums), None)
        if not a:
            return None, f"the {release['tag_name']} release has no {sums}"
        url = a["browser_download_url"]
    try:
        s = sum_for(get_text(url), name)
    except (urllib.error.URLError, OSError) as e:
        return None, f"{sums}: {e}"
    return (s, sums) if s else (None, f"{sums} does not name {name}")


def newest_release(repo, update, fits):
    releases = get_json(f"{API}/repos/{repo}/releases?per_page=50")
    best = None
    for r in releases:
        if r.get("draft") or (r.get("prerelease") and not update.get("prerelease")):
            continue
        v = version_in_tag(update, r.get("tag_name", ""))
        if v is None or not fits(r, v):
            continue
        if best is None or newer(v, best[1]):
            best = (r, v)
    return best


def newest_tag(repo, update):
    tags = get_json(f"{API}/repos/{repo}/tags?per_page=100")
    best = None
    for t in tags:
        v = version_in_tag(update, t.get("name", ""))
        if v is not None and (best is None or newer(v, best[1])):
            best = (t["name"], v)
    return best


def check_binaries(pid, m, download):
    update, binaries = m.get("update", {}), m.get("binary", [])
    repo = update.get("github")
    arches = [a for a in ARCHES if any(b.get("arch") == a for b in binaries)]
    for a in ARCHES:
        if a not in arches:
            say(False, pid, f"no [[binary]] for {a}")
    if not repo:
        for b in binaries:  # a fixed address with its own sha256
            if b.get("url"):
                ok, why = resolves(b["url"])
                say(ok, pid, f"{b.get('arch')}: {b['url']} {'answers' if ok else why}")
                if ok and download and b.get("sha256"):
                    got = sha256_of_url(b["url"])
                    say(got == b["sha256"].lower(), pid, f"{b.get('arch')}: sha256 {got}")
        return

    def has_all(r, v):
        names = {x["name"] for x in r.get("assets", [])}
        return all(any(fill(n, v, r["tag_name"]) in names for n in asset_names(b)) for b in binaries if b.get("arch") in arches)

    found = newest_release(repo, update, has_all)
    if not found:
        say(False, pid, f"no release of {repo} fits tag {update.get('tag', 'v{version}')!r} with an asset for every arch")
        return
    releases = [found]
    if update.get("pin"):
        try:
            r = get_json(f"{API}/repos/{repo}/releases/tags/{urllib.parse.quote(update['pin'], safe='')}")
            releases.append((r, version_in_tag(update, update["pin"]) or update["pin"].lstrip("vV")))
        except (urllib.error.URLError, OSError) as e:
            say(False, pid, f"the pinned release {update['pin']}: {e}")
    for r, v in releases:
        say(True, pid, f"{repo} {r['tag_name']} (version {v})")
        for b in binaries:
            names = [fill(n, v, r["tag_name"]) for n in asset_names(b)]
            a = next((x for n in names for x in r["assets"] if x["name"] == n), None)
            if not a:
                say(False, pid, f"{b['arch']}: none of {names} in {r['tag_name']}")
                continue
            ok, why = resolves(a["browser_download_url"])
            say(ok, pid, f"{b['arch']}: {a['name']} {('answers, ' + why + ' bytes') if ok else why}")
            s, src = release_sum(update, r, a["name"])
            if s:
                say(True, pid, f"{b['arch']}: {a['name']} sha256 {s} ({src})")
            elif update.get("sums") == "github" and "lists no sha256" in src:
                warn(pid, f"{b['arch']}: {src}")
            else:
                say(False, pid, f"{b['arch']}: {src}")
            if ok and s and download:
                got = sha256_of_url(a["browser_download_url"])
                say(got == s, pid, f"{b['arch']}: downloaded {a['name']}, sha256 {'matches' if got == s else 'is ' + got}")


def check_python(pid, m, download):
    update, py = m.get("update", {}), m.get("python", {})
    packages = py.get("packages", [])
    if update.get("from") == "pypi" or update.get("pypi"):
        project = update.get("pypi") or pid
        try:
            info = get_json(f"https://pypi.org/pypi/{urllib.parse.quote(project)}/json")
        except (urllib.error.URLError, OSError) as e:
            say(False, pid, f"PyPI {project}: {e}")
            return
        v = info["info"]["version"]
        files = info.get("urls", [])
        with_sum = [f for f in files if f.get("digests", {}).get("sha256")]
        say(bool(files) and len(with_sum) == len(files), pid, f"PyPI {project} {v}: {len(files)} files, {len(with_sum)} with sha256")
        for f in files[:3]:
            ok, why = resolves(f["url"])
            say(ok, pid, f"{f['filename']} {'answers' if ok else why}")
            if ok and download:
                got = sha256_of_url(f["url"])
                say(got == f["digests"]["sha256"], pid, f"downloaded {f['filename']}, sha256 {'matches' if got == f['digests']['sha256'] else 'is ' + got}")
        for p in packages:
            say("{tag}" not in p, pid, f"package {fill(p, v, v)} (from PyPI)")
        return
    repo = update.get("github")
    if not repo:
        say(False, pid, "[python] without [update] pypi or github")
        return
    targets = []
    if update.get("pin"):
        targets.append((update["pin"], version_in_tag(update, update["pin"]) or update["pin"].lstrip("vV"), "pinned"))
    try:
        found = newest_release(repo, update, lambda r, v: True)
        if found:
            targets.append((found[0]["tag_name"], found[1], "newest release"))
        elif update.get("tags"):
            t = newest_tag(repo, update)
            if t:
                targets.append((t[0], t[1], "newest tag"))
    except (urllib.error.URLError, OSError) as e:
        say(False, pid, f"{repo} releases: {e}")
    if not targets:
        say(False, pid, f"no release or tag of {repo} fits {update.get('tag', 'v{version}')!r}")
    for tag, v, why in targets:
        for p in packages:
            spec = fill(p, v, tag)
            url = spec.split(" @ ", 1)[1].strip() if " @ " in spec else None
            if not url:
                say(True, pid, f"{why} {tag}: package {spec} (from the package index)")
                continue
            ok, how = resolves(url)
            say(ok, pid, f"{why} {tag}: {url} {'answers' if ok else how}")
            if ok and download:
                say(True, pid, f"{why} {tag}: sha256 {sha256_of_url(url)} (a venv package; Encore holds no sum for it)")


def check_downloads(pid, m, download, models):
    for d in m.get("download", []):
        url = d.get("url", "")
        ok, why = resolves(url)
        say(ok, pid, f"{d.get('path')}: {url} {('answers, ' + why + ' bytes') if ok else why}")
        sums, want = d.get("sums"), d.get("sha256")
        why = "the file is not in the repository's listing"
        if sums == "huggingface":
            parts = url.split("huggingface.co/", 1)[1].split("/", 4) if "huggingface.co/" in url else []
            if len(parts) == 5:
                owner, repo, _, rev, file = parts
                try:
                    tree = get_json(f"https://huggingface.co/api/models/{owner}/{repo}/tree/{rev}")
                    want = next((f.get("lfs", {}).get("oid") for f in tree if f.get("path") == file), None)
                except (urllib.error.URLError, OSError) as e:
                    want = None
                    why = str(e)
            if want:
                say(True, pid, f"{d.get('path')}: Hugging Face sha256 {want}")
            else:
                warn(pid, f"{d.get('path')}: Hugging Face lists no sha256 ({why}); Encore checks the file by its magic {d.get('magic')!r} only")
        elif sums and sums.startswith("https://"):
            try:
                want = sum_for(get_text(sums), url.rsplit("/", 1)[-1])
            except (urllib.error.URLError, OSError):
                want = None
            say(bool(want), pid, f"{d.get('path')}: sha256 {want} from {sums}" if want else f"{d.get('path')}: {sums} names no sha256 for it")
        elif not want:
            say(bool(d.get("magic")), pid, f"{d.get('path')}: no sum, checked by its magic {d.get('magic')!r} only")
        if ok and want and download and models:
            got = sha256_of_url(url)
            say(got == want.lower(), pid, f"{d.get('path')}: downloaded, sha256 {'matches' if got == want.lower() else 'is ' + got}")


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    download, models = "--download" in sys.argv, "--models" in sys.argv
    folder = Path(args[0] if args else Path(__file__).resolve().parent.parent / "server/crates/plugins/catalog")
    files = sorted(folder.glob("*.toml"))
    if not files:
        sys.exit(f"no catalog manifests in {folder}")
    for f in files:
        m = tomllib.loads(f.read_text())
        pid = m.get("plugin", {}).get("id", f.stem)
        has = [k for k in ("binary", "python", "download") if m.get(k)]
        if not has:
            say(True, pid, "downloads nothing (settings and Encore's own code only)")
            continue
        try:
            if m.get("binary"):
                check_binaries(pid, m, download)
            if m.get("python"):
                check_python(pid, m, download)
            if m.get("download"):
                check_downloads(pid, m, download, models)
        except (urllib.error.URLError, OSError, KeyError, ValueError) as e:
            say(False, pid, f"stopped: {e}")
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a") as out:
            out.write(f"### Plugin catalog downloads\n\n{len(files)} manifests, {len(failures)} failed checks, {len(warnings)} warnings\n\n| | Plugin | Check |\n|---|---|---|\n")
            for state, pid, what in lines:
                out.write(f"| {state} | {pid} | {what.replace('|', '/')} |\n")
    print(f"\n{len(files)} manifests, {len(lines)} checks, {len(failures)} failed, {len(warnings)} warnings")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
