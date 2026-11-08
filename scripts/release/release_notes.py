#!/usr/bin/env python3
"""Build a self-contained Sparkle reading surface from the website's changelog.

Only a conservative Markdown subset is interpreted. Raw HTML is always escaped;
there are no remote assets, scripts, tracking or runtime rendering dependencies.
"""

import argparse
from html import escape
from pathlib import Path
import re
from urllib.parse import urlsplit

TAG = re.compile(r"v\d+\.\d+\.\d+(?:-(?:beta|dev)\.\d+)?\Z")
HEADING = re.compile(r"^##\s+`?(v\d+\.\d+\.\d+(?:-(?:beta|dev)\.\d+)?)`?(?=\s|$)(.*)$")
INLINE = re.compile(r"`([^`]+)`|\[([^\]\n]+)\]\(([^\s)]+)\)|\*\*([^*]+)\*\*")
CHANGELOG = "https://con.nowledge.co/changelog/"


def validate_tag(tag):
    if not TAG.fullmatch(tag):
        raise ValueError(f"Invalid release tag: {tag}")


def inline(text):
    result = []
    end = 0
    for match in INLINE.finditer(text):
        result.append(escape(text[end:match.start()]))
        code, label, url, strong = match.groups()
        if code is not None:
            result.append(f"<code>{escape(code)}</code>")
        elif label is not None:
            # Never turn a changelog link into executable content or a local URL.
            parsed = urlsplit(url)
            if parsed.scheme == "https" and parsed.hostname:
                result.append(f'<a href="{escape(url, quote=True)}">{escape(label)}</a>')
            else:
                result.append(escape(label))
        else:
            result.append(f"<strong>{escape(strong)}</strong>")
        end = match.end()
    result.append(escape(text[end:]))
    return "".join(result)


def section(markdown, tag):
    """Exact release only: never substitute a different version’s section."""
    validate_tag(tag)
    lines = markdown.splitlines()
    for index, line in enumerate(lines):
        match = HEADING.match(line)
        if match and match[1] == tag:
            last = next((i for i in range(index + 1, len(lines))
                         if lines[i].startswith("## ")), len(lines))
            return match[2].strip(), lines[index + 1:last]
    return "", []


def body(lines):
    """Render headings, paragraphs, flat lists, inline code/links/emphasis.

    Wrapped source lines join the same paragraph/list item; unrecognized syntax
    remains visible as text instead of silently disappearing.
    """
    output, paragraph = [], []
    in_list = False
    item = False

    def flush():
        if paragraph:
            text = " ".join(paragraph)
            credit = re.search(r"\s*_\((PR .+)\)_$", text)
            if credit:
                rendered = inline(text[:credit.start()])
                rendered += f'<span class="credit">{inline(credit[1])}</span>'
            else:
                rendered = inline(text)
            output.append(f'<{"li" if item else "p"}>{rendered}</{"li" if item else "p"}>')
            paragraph.clear()

    def close_list():
        nonlocal in_list
        if in_list:
            output.append("</ul>")
            in_list = False

    for line in lines:
        stripped = line.strip()
        if not stripped:
            flush()
        elif stripped.startswith(("### ", "#### ")) or re.fullmatch(r"\*\*.+\*\*", stripped):
            flush()
            close_list()
            item = False
            level = "h2" if stripped.startswith("### ") else "h3"
            text = re.sub(r"^#{3,4}\s+", "", stripped)
            if text.startswith("**") and text.endswith("**"):
                text = text[2:-2]
            output.append(f"<{level}>{inline(text)}</{level}>")
        elif line.startswith(("- ", "* ")):
            flush()
            if not in_list:
                output.append("<ul>")
                in_list = True
            item = True
            paragraph.append(line[2:].strip())
        else:
            if not paragraph:
                close_list()
                item = False
            paragraph.append(stripped)
    flush()
    close_list()
    return "\n".join(output)


STYLE = """
:root { color-scheme: light dark; --ground: #fffcf0; --ink: #100f0f;
  --muted: #6f6e69; --link: #205ea6; --wash: rgba(16,15,15,.055); }
@media (prefers-color-scheme: dark) {
  :root { --ground: #100f0f; --ink: #cecdc3; --muted: #9f9d96;
    --link: #66a0c8; --wash: rgba(206,205,195,.08); }
}
* { box-sizing: border-box; }
html { background: var(--ground); color: var(--ink); }
body { margin: 0; padding: 20px 22px 24px;
  font: 14px/1.55 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  overflow-wrap: anywhere; }
article { max-width: 66ch; margin: 0 auto; }
header { display: flex; align-items: baseline; flex-wrap: wrap; gap: 4px 16px; }
h1 { margin: 0; font-size: 18px; line-height: 1.3; font-weight: 650; }
.version { margin: 0; color: var(--muted); font-size: 12px; }
h2 { margin: 20px 0 8px; font-size: 15px; line-height: 1.4; font-weight: 650; }
h3 { margin: 10px 0 6px; font-size: 13px; line-height: 1.4; font-weight: 650; }
p { margin: 8px 0; }
ul { padding-left: 18px; margin: 6px 0 16px; }
li { padding-left: 2px; }
li + li { margin-top: 12px; }
li::marker { color: var(--muted); }
a { color: var(--link); text-underline-offset: 3px; text-decoration-thickness: 1px; }
a:hover { text-decoration-thickness: 2px; }
a:focus-visible { outline: 2px solid var(--link); outline-offset: 3px; border-radius: 2px; }
code { font: .9em ui-monospace, Menlo, Consolas, monospace;
  background: var(--wash); border-radius: 3px; padding: 1px 3px; }
.credit { display: block; margin-top: 5px; color: var(--muted); font-size: 11px; }
.credit a { color: inherit; }
footer { margin-top: 26px; font-size: 12px; }
::selection { background: var(--link); color: var(--ground); }
@media (max-width: 380px) { body { padding: 16px; } h1 { font-size: 18px; } }
"""


def render(markdown, tag):
    heading, lines = section(markdown, tag)
    date = re.search(r"\b\d{4}-\d{2}-\d{2}\b", heading)
    # Legacy tags may still have an Unreleased heading. Do not invent dates;
    # authorized releases now carry their intended date in the tag source.
    version = tag.removeprefix("v")
    label = version if not date else f"{version} · {date[0]}"
    content = body(lines)
    if not content:
        content = '<p>Release details are available in the full changelog.</p>'
    return f'''<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="color-scheme" content="light dark">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'">
<title>con {escape(version)} — What’s new</title>
<style>{STYLE}</style></head><body><article>
<header><h1>What’s new</h1><p class="version">con {escape(label)}</p></header>
{content}
<footer><a href="{CHANGELOG}">Read the full changelog</a></footer>
</article></body></html>
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag")
    parser.add_argument("--changelog", type=Path, default=Path("CHANGELOG.md"))
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    html = render(args.changelog.read_text(), args.tag)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(html)


if __name__ == "__main__":
    main()
