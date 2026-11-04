"""Builds recommendations.html (then printed to PDF) explaining Encore's taste engine.

Every number in the document is computed here from the same formulas as
server/crates/library/src/taste.rs, so the examples match the real code.
"""
import math

D = 86400.0
ACC = "#6d4aff"   # plays / positive
ACC2 = "#e0517a"  # skips / negative
ACC3 = "#1aa39a"  # co-listening
INK = "#1d1b26"
MUTED = "#6b6880"
GRID = "#e4e2ee"


def fade(ago, hl):
    """Same as taste.rs `fade`: halves every `hl` seconds."""
    return 0.5 ** (max(ago, 0) / hl)


def cool(h):
    """The just-played penalty for a song heard `h` hours ago (Profile::score)."""
    return 0.8 * math.exp(-h / 6) + 0.25 * math.exp(-h / 72)


# ---------------------------------------------------------------- charts
def line_chart(series, xmax, ymax, xticks, yticks, xlabel, w=640, h=230, fmt_x=str, fmt_y=lambda v: f"{v:g}"):
    """A small SVG line chart. series: [(label, colour, fn, xcut)]."""
    l, r, t, b = 46, 14, 12, 38
    pw, ph = w - l - r, h - t - b
    X = lambda x: l + pw * x / xmax
    Y = lambda y: t + ph * (1 - y / ymax)
    out = [f'<svg viewBox="0 0 {w} {h}" class="chart" role="img">']
    for y in yticks:
        out.append(f'<line x1="{l}" x2="{w-r}" y1="{Y(y):.1f}" y2="{Y(y):.1f}" stroke="{GRID}"/>')
        out.append(f'<text x="{l-6}" y="{Y(y)+4:.1f}" text-anchor="end" class="tick">{fmt_y(y)}</text>')
    for x in xticks:
        out.append(f'<line x1="{X(x):.1f}" x2="{X(x):.1f}" y1="{t}" y2="{t+ph}" stroke="{GRID}"/>')
        out.append(f'<text x="{X(x):.1f}" y="{t+ph+15}" text-anchor="middle" class="tick">{fmt_x(x)}</text>')
    out.append(f'<text x="{l+pw/2}" y="{h-4}" text-anchor="middle" class="axis">{xlabel}</text>')
    for i, (label, col, fn, xcut) in enumerate(series):
        pts = []
        steps = 300
        for k in range(steps + 1):
            x = xcut * k / steps
            pts.append(f"{X(x):.1f},{Y(fn(x)):.1f}")
        out.append(f'<polyline points="{" ".join(pts)}" fill="none" stroke="{col}" stroke-width="2.5"/>')
        if xcut < xmax:
            out.append(f'<line x1="{X(xcut):.1f}" x2="{X(xcut):.1f}" y1="{Y(fn(xcut)):.1f}" y2="{Y(0):.1f}" stroke="{col}" stroke-dasharray="3 3"/>')
        out.append(f'<rect x="{w-r-190}" y="{t+8+i*18}" width="12" height="4" fill="{col}"/>'
                   f'<text x="{w-r-172}" y="{t+13+i*18}" class="leg">{label}</text>')
    out.append("</svg>")
    return "".join(out)


fade_chart = line_chart(
    [("Play (half-life 120 days)", ACC, lambda d: fade(d * D, 120 * D), 365),
     ("Skip (half-life 21 days, gone at 90)", ACC2, lambda d: fade(d * D, 21 * D), 90)],
    365, 1.0, [0, 30, 60, 90, 120, 180, 240, 300, 365], [0, .25, .5, .75, 1],
    "days since the play or skip", fmt_y=lambda v: f"{int(v*100)}%")

cool_chart = line_chart(
    [("Score taken off a just-played song", ACC2, cool, 168)],
    168, 1.1, [0, 6, 12, 24, 48, 72, 96, 120, 144, 168], [0, .25, .5, .75, 1.0],
    "time since you last heard the song", h=190, fmt_x=lambda x: f"{x}h" if x < 24 else f"{x//24}d" if x % 24 == 0 else f"{x}h")

# ---------------------------------------------------------------- the worked example (verified against taste.rs)
songs = {
    'A': ("“Paranoid Android”", 'Radiohead', 'alternative rock', 1990),
    'B': ("“Idioteque”", 'Radiohead', 'alternative rock', 2000),
    'C': ("“Teardrop”", 'Massive Attack', 'trip hop', 1990),
    'D': ("“Glory Box”", 'Portishead', 'trip hop', 1990),
    'E': ("“Blank Space”", 'Taylor Swift', 'pop', 2010),
    'F': ("“So What”", 'Miles Davis', 'jazz', 1950),
    'G': ("“Hysteria”", 'Muse', 'alternative rock', 2000),
    'H': ("“Starlight”", 'Muse', 'alternative rock', 2000),
}
now = 0.0
plays = [('A', -3 * k * D) for k in range(1, 20)]
plays += [('D', -2 * 3600)] + [('D', -x * D) for x in (150, 160, 170, 180, 190)]
likes = ['A', 'D']
playlisted = ['A', 'B', 'D']
skips = [('E', -2 * D), ('E', -5 * D)]
dislikes = ['H']
artist, genre, dec, song = {}, {}, {}, {}


def add(s, w):
    _, a, g, d = songs[s]
    song[s] = song.get(s, 0) + w
    artist[a] = artist.get(a, 0) + w
    genre[g] = genre.get(g, 0) + w
    dec[d] = dec.get(d, 0) + w * 0.5


last, skipped = {}, {}
for s, at in plays:
    add(s, fade(now - at, 120 * D))
    last[s] = max(last.get(s, -1e18), at)
for s in likes:
    add(s, 4)
for s in playlisted:
    add(s, 0.75)
for s, at in skips:
    w = fade(now - at, 21 * D)
    add(s, -0.6 * w)
    skipped[s] = skipped.get(s, 0) + w
for s in dislikes:
    add(s, -3)


def squash(raw):
    top = sorted([v for v in raw.values() if v > 0], reverse=True)[:10]
    sc = max(sum(top) / len(top), 1) if top else 1
    return {k: math.tanh(v / sc * 1.5) for k, v in raw.items()}, sc


A, sa = squash(artist)
G, sg = squash(genre)
Dd, sd = squash(dec)
S = {k: math.tanh(v / 3) for k, v in song.items()}
sims = {('Radiohead', 'Massive Attack'): .40, ('Radiohead', 'Muse'): .30, ('Portishead', 'Massive Attack'): .55, ('Portishead', 'Radiohead'): .20}
near = {}
for a, aff in A.items():
    if aff <= 0:
        continue
    for (x, y), s in sims.items():
        for p, q in ((x, y), (y, x)):
            if p == a:
                near[q] = near.get(q, 0) + aff * s
mx = max(near.values())
N = {k: v / mx for k, v in near.items()}

rows = []
for s, (title, a, g, d) in songs.items():
    if s in dislikes:
        rows.append(dict(s=s, title=title, artist=a, blocked=True))
        continue
    parts = [.30 * A.get(a, 0), .25 * G.get(g, 0), .08 * Dd.get(d, 0), .17 * N.get(a, 0), .20 * S.get(s, 0)]
    pen_cool = cool((now - last[s]) / 3600) if s in last else 0
    pen_skip = .25 * skipped.get(s, 0)
    score = sum(parts) - pen_cool - pen_skip
    rows.append(dict(s=s, title=title, artist=a, parts=parts, cool=pen_cool, skip=pen_skip, score=score,
                     w=math.exp(score / .3), raw=(A.get(a, 0), G.get(g, 0), Dd.get(d, 0), N.get(a, 0), S.get(s, 0))))
tw = sum(r['w'] for r in rows if not r.get('blocked'))
for r in rows:
    if not r.get('blocked'):
        r['share'] = r['w'] / tw


def f2(x):
    return f"{x:+.2f}".replace("+0.00", "0.00").replace("-0.00", "0.00")


def f3(x):
    return f"{x:+.3f}".replace("+0.000", "0").replace("-0.000", "0")


# score breakdown chart: one row per song, stacked positive parts, penalties drawn to the left
PART_COL = ["#6d4aff", "#9b7bff", "#c7b6ff", ACC3, "#f2a93b"]
PART_NAME = ["Artist", "Genre", "Era", "Co-listening", "The song itself"]


def score_chart():
    w, rowh, l = 660, 30, 190
    zero = l + 170
    unit = 230  # px per 1.0 score
    h = 30 + rowh * len(rows) + 30
    out = [f'<svg viewBox="0 0 {w} {h}" class="chart" role="img">']
    for v in (-1.0, -0.5, 0, 0.5, 1.0):
        x = zero + v * unit * 0.7
        out.append(f'<line x1="{x:.1f}" x2="{x:.1f}" y1="18" y2="{h-26}" stroke="{GRID if v else "#b9b6c8"}"/>'
                   f'<text x="{x:.1f}" y="{h-12}" text-anchor="middle" class="tick">{v:+.1f}</text>')
    sc = unit * 0.7
    for i, r in enumerate(rows):
        y = 24 + i * rowh
        out.append(f'<text x="{l-8}" y="{y+14}" text-anchor="end" class="lab"><tspan font-weight="600">{r["s"]}</tspan> {r["artist"]}</text>')
        if r.get('blocked'):
            out.append(f'<rect x="{zero-60}" y="{y+2}" width="120" height="18" rx="9" fill="#2b2838"/>'
                       f'<text x="{zero}" y="{y+15}" text-anchor="middle" class="lab" style="fill:#fff">blocked (disliked)</text>')
            continue
        # positive parts stack right from zero, negative parts stack left
        px, nx = zero, zero
        for k, v in enumerate(r['parts']):
            if v >= 0:
                out.append(f'<rect x="{px:.1f}" y="{y+3}" width="{v*sc:.1f}" height="16" fill="{PART_COL[k]}"/>')
                px += v * sc
            else:
                nx -= -v * sc
                out.append(f'<rect x="{nx:.1f}" y="{y+3}" width="{-v*sc:.1f}" height="16" fill="{PART_COL[k]}" opacity=".55"/>')
        pen = r['cool'] + r['skip']
        if pen:
            nx -= pen * sc
            out.append(f'<rect x="{nx:.1f}" y="{y+3}" width="{pen*sc:.1f}" height="16" fill="{ACC2}" opacity=".85"/>')
        # the final score as a marker
        xs = zero + r['score'] * sc
        out.append(f'<path d="M{xs:.1f} {y} l5 -6 h-10 z" fill="{INK}"/><text x="{w-6}" y="{y+15}" text-anchor="end" class="lab" font-weight="700">{r["score"]:+.2f}</text>')
    # legend
    lx = 10
    for k, nme in enumerate(PART_NAME + ["Penalty (just played / skipped)"]):
        col = PART_COL[k] if k < 5 else ACC2
        out.append(f'<rect x="{lx}" y="2" width="10" height="10" fill="{col}"/><text x="{lx+14}" y="11" class="leg">{nme}</text>')
        lx += 14 + len(nme) * 6.3 + 14
    out.append("</svg>")
    return "".join(out)


def share_chart():
    rs = [r for r in rows if not r.get('blocked')]
    w, rowh, l = 660, 24, 190
    unit = 400
    h = 10 + rowh * len(rs) + 6
    out = [f'<svg viewBox="0 0 {w} {h}" class="chart" role="img">']
    for i, r in enumerate(rs):
        y = 6 + i * rowh
        out.append(f'<text x="{l-8}" y="{y+13}" text-anchor="end" class="lab"><tspan font-weight="600">{r["s"]}</tspan> {r["artist"]}</text>')
        out.append(f'<rect x="{l}" y="{y+2}" width="{max(r["share"]*unit,1.5):.1f}" height="15" rx="3" fill="{ACC}"/>')
        out.append(f'<text x="{l+r["share"]*unit+8:.1f}" y="{y+14}" class="lab">{r["share"]*100:.1f}%  <tspan fill="{MUTED}">(weight {r["w"]:.2f})</tspan></text>')
    out.append("</svg>")
    return "".join(out)


# ---------------------------------------------------------------- "More like this" example (seed = song A)
def similarity(a, b):
    _, aa, ag, ad = songs[a]
    _, ba, bg, bd = songs[b]
    art = 1.0 if aa == ba else sims.get((aa, ba), sims.get((ba, aa), 0.0))
    gen = 1.0 if ag == bg else 0.0  # one genre each in the example, so Jaccard is 1 or 0
    era = max(0.0, 1 - abs(ad - bd) / 30)
    return art, gen, era, .45 * art + .40 * gen + .15 * era


radio_rows = []
for s in "BCDEFG":
    art, gen, era, sim = similarity('A', s)
    r = next(x for x in rows if x['s'] == s)
    radio_rows.append((s, songs[s], art, gen, era, sim, r['score'], sim + .25 * r['score'] if sim > .05 else None))

# ---------------------------------------------------------------- tables
signal_rows = "".join(f"<tr><td>{s}</td><td class='num'>{w}</td><td>{f}</td><td>{n}</td></tr>" for s, w, f, n in [
    ("Play", "+0.8 to +1.2", "Halves every 120 days", "Counts once you've heard half the song or 4 minutes, whichever comes first (very short songs: to the end). How much of it you heard sets the weight: +0.8 if you moved on right after it counted, +1.2 if you heard it to the end (the last tenth counts as the end). +1.0 when the app didn't say."),
    ("Song in Liked Songs", "+4.0", "Never fades", "The strongest “yes”. Liking a song also takes back a dislike of it."),
    ("Share link you made", "+5.0", "Halves every 180 days", "A little more than a like: sending a song to someone says it out loud. An album link counts like one shared song, spread over the album's songs. Playlist links don't count (the playlist already does)."),
    ("Song on one of your own playlists", "+0.75", "Never fades", "Each song once, however many playlists it is on. Playlists shared with you don't count."),
    ("Skip", "−0.6 to −0.12", "Halves every 21 days, ignored after 90", "Pressing Next after 2 seconds but before the play counted. A mild “not now”: full weight in the first seconds, falling to a fifth just before the play would have counted."),
    ("Dislike (a song)", "−3.0", "Until you take it back", "The song is never played by any recommendation again, and leaves Liked Songs."),
    ("Don't play this artist", "blocks", "Until you take it back", "Every song where they are the main artist is never played. Songs they only feature on just sink."),
])

fade_rows = "".join(
    f"<tr><td>{d} days</td><td class='num'>{fade(d*D,120*D)*100:.0f}%</td><td class='num'>{'%.0f%%' % (fade(d*D,21*D)*100) if d <= 90 else 'ignored'}</td></tr>"
    for d in (0, 7, 21, 30, 60, 90, 120, 240, 365))

cool_rows = "".join(f"<td class='num'><span style='color:#6b6880'>{lab}</span><br><b>−{cool(h):.2f}</b></td>" for lab, h in [
    ("just now", 0), ("1 hour", 1), ("3 hours", 3), ("6 hours", 6), ("12 hours", 12), ("1 day", 24), ("2 days", 48), ("3 days", 72), ("1 week", 168)])

temp_rows = "".join(f"<tr><td class='num'>{s:+.1f}</td><td class='num'>{math.exp(s/.3):.2f}×</td></tr>" for s in (-0.5, -0.2, 0, 0.2, 0.4, 0.6, 0.9))

profile_rows = "".join(
    f"<tr><td>{name}</td><td class='num'>{artist[name]:+.2f}</td><td class='num'>{A[name]:+.2f}</td></tr>"
    for name in ['Radiohead', 'Portishead', 'Taylor Swift', 'Muse'])
genre_prof_rows = "".join(
    f"<tr><td>{name}</td><td class='num'>{genre[name]:+.2f}</td><td class='num'>{G[name]:+.2f}</td></tr>"
    for name in ['alternative rock', 'trip hop', 'pop'])
dec_rows = "".join(f"<tr><td>{d}s</td><td class='num'>{dec[d]:+.2f}</td><td class='num'>{Dd[d]:+.2f}</td></tr>" for d in (1990, 2000, 2010))

score_rows = ""
for r in rows:
    if r.get('blocked'):
        score_rows += f"<tr class='blk'><td><b>{r['s']}</b> {r['artist']} {r['title']}</td><td colspan='8'>Disliked: never played, not even scored</td></tr>"
        continue
    a, g, e, n, o = r['parts']
    score_rows += (f"<tr><td><b>{r['s']}</b> {r['artist']} {r['title']}</td><td class='num'>{f3(a)}</td><td class='num'>{f3(g)}</td>"
                   f"<td class='num'>{f3(e)}</td><td class='num'>{f3(n)}</td><td class='num'>{f3(o)}</td>"
                   f"<td class='num neg'>{('−%.3f' % (r['cool']+r['skip'])) if r['cool']+r['skip'] else '0'}</td>"
                   f"<td class='num'><b>{r['score']:+.2f}</b></td><td class='num'>{r['share']*100:.1f}%</td></tr>")

radio_tbl = ""
for s, (title, a, g, d), art, gen, era, sim, score, cand in radio_rows:
    radio_tbl += (f"<tr{' class=blk' if cand is None else ''}><td><b>{s}</b> {a} {title}</td><td class='num'>{art:.2f}</td><td class='num'>{gen:.0f}</td>"
                  f"<td class='num'>{era:.2f}</td><td class='num'>{sim:.2f}</td>"
                  f"<td class='num'>{(f'{cand:.2f}') if cand is not None else 'left out (≤ 0.05)'}</td></tr>")

e_skip = skipped['E']

html = open("template.html").read()
for k, v in {
    "FADE_CHART": fade_chart, "COOL_CHART": cool_chart, "SCORE_CHART": score_chart(), "SHARE_CHART": share_chart(),
    "SIGNAL_ROWS": signal_rows, "FADE_ROWS": fade_rows, "COOL_ROWS": cool_rows, "TEMP_ROWS": temp_rows,
    "PROFILE_ROWS": profile_rows, "GENRE_PROF_ROWS": genre_prof_rows, "DEC_ROWS": dec_rows,
    "SCORE_ROWS": score_rows, "RADIO_ROWS": radio_tbl,
    "SCALE_A": f"{sa:.2f}", "E_SKIP": f"{e_skip:.2f}", "E_SKIP_PEN": f"{.25*e_skip:.2f}",
    "D_COOL": f"{cool(2):.2f}", "A_COOL": f"{cool(72):.2f}",
    "RH_RAW": f"{artist['Radiohead']:.2f}", "RH_TANH": f"{A['Radiohead']:.2f}",
    "MA_NEAR_RAW": f"{near['Massive Attack']:.2f}", "MUSE_NEAR": f"{N['Muse']:.2f}",
    "A_SCORE": f"{rows[0]['score']:+.2f}",
}.items():
    html = html.replace("{{" + k + "}}", v)
assert "{{" not in html, html[html.index("{{"):html.index("{{") + 40]
open("recommendations.html", "w").write(html)
print("ok", {r['s']: round(r['score'], 3) for r in rows if not r.get('blocked')})
