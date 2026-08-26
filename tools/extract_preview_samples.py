"""Pull (width, height, css_w, css_h) triples out of the reference export.

Pairs each message's JSON dimensions with the style Desktop actually wrote on
its <img>, so the Rust preview_size() can be checked against real samples
instead of only against examples we invented.
"""
import io
import json
import re
import sys
from pathlib import Path

ROOT = Path(sys.argv[1])
IMG = re.compile(
    r'<div class="message default clearfix(?: joined)?" id="message(\d+)">'
)
STYLE = re.compile(
    r'<img class="(photo|sticker|animated|video_file)" src="[^"]*" '
    r'style="width: (\d+)px; height: (\d+)px"'
)

samples = []
for topic in sorted(p for p in ROOT.iterdir() if (p / "result.json").exists()):
    data = json.loads(io.open(topic / "result.json", encoding="utf-8").read())
    dims = {}
    for m in data["messages"]:
        if m.get("width") and m.get("height"):
            dims[m["id"]] = (m["width"], m["height"], m.get("media_type"),
                             bool(m.get("photo")))
    pages = sorted(topic.glob("messages*.html"),
                   key=lambda p: (len(p.stem), p.stem))
    for page in pages:
        text = io.open(page, encoding="utf-8").read()
        for block in re.split(r'(?=<div class="message default clearfix)', text):
            head = IMG.match(block)
            if not head:
                continue
            mid = int(head.group(1))
            if mid not in dims:
                continue
            hit = STYLE.search(block)
            if not hit:
                continue
            w, h, kind, is_photo = dims[mid]
            samples.append({
                "w": w, "h": h,
                "css_w": int(hit.group(2)), "css_h": int(hit.group(3)),
                "cls": hit.group(1),
            })

# De-duplicate on the input so the fixture stays small but covers every shape.
seen, unique = set(), []
for s in samples:
    key = (s["w"], s["h"], s["cls"])
    if key in seen:
        continue
    seen.add(key)
    unique.append(s)

out = Path("crates/tgx-html/tests/preview_samples.json")
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text(json.dumps(unique, indent=1), encoding="utf-8")
print(f"{len(samples)} samples, {len(unique)} unique -> {out}")
