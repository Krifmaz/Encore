"""compare.py BEFORE AFTER: compare each screenshot pixel by pixel (the status bar cropped) and write a table."""
import os, sys
from PIL import Image, ImageChops

before, after = sys.argv[1], sys.argv[2]
names = sorted(set(os.listdir(before)) | set(os.listdir(after)))
rows, bad = [], 0
for n in names:
    a, b = os.path.join(before, n), os.path.join(after, n)
    if not (os.path.exists(a) and os.path.exists(b)):
        rows.append(f"| {n} | missing on one side |"); bad += 1; continue
    ia, ib = Image.open(a).convert("RGB"), Image.open(b).convert("RGB")
    if ia.size != ib.size:
        rows.append(f"| {n} | sizes differ {ia.size} vs {ib.size} |"); bad += 1; continue
    top = int(ia.size[1] * 0.04)  # the status bar (clock, icons) changes between runs
    box = (0, top, ia.size[0], ia.size[1])
    diff = ImageChops.difference(ia.crop(box), ib.crop(box))
    px = sum(1 for p in diff.getdata() if p != (0, 0, 0))
    rows.append(f"| {n} | {'identical' if px == 0 else f'{px} pixels differ'} |")
    bad += px != 0
report = "| screen | result |\n|---|---|\n" + "\n".join(rows) + "\n"
print(report)
with open(os.environ.get("GITHUB_STEP_SUMMARY", "/dev/null"), "a") as f:
    f.write("## Before/after screenshots\n\n" + report)
sys.exit(1 if bad else 0)
