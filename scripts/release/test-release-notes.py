#!/usr/bin/env python3
"""Offline tests for exact-version, safe, self-contained updater content."""

from html.parser import HTMLParser
from pathlib import Path
import unittest
import xml.etree.ElementTree as ET

from release_notes import body, inline, render, section

ROOT = Path(__file__).resolve().parents[2]
SOURCE = '''# Changelog
## `v0.1.0-beta.173` (Unreleased)
### Added
- Future release only.
## `v0.1.0-beta.172` (Unreleased)
### Fixed
**Panes**
- Keep the `session` running when a pane moves
  to a new window.
  _(PR [#546](https://github.com/nowledge-co/con-terminal/pull/546) by [@wey-gu](https://github.com/wey-gu))_

- Preserve Unicode & <image> content.

### Changed
**CLI — compatibility**
- Confirm agent termination before closing.
## `v0.1.0-beta.171` - 2026-11-07
- Previous release only.
'''


class Document(HTMLParser):
    def __init__(self, source):
        super().__init__()
        self.tags = []
        self.feed(source)

    def handle_starttag(self, tag, attrs):
        self.tags.append((tag, dict(attrs)))


class ReleaseNotesTests(unittest.TestCase):
    def test_exact_tag_not_adjacent_or_prefix(self):
        html = render(SOURCE, 'v0.1.0-beta.172')
        self.assertIn('Keep the <code>session</code> running when a pane moves to a new window.', html)
        self.assertNotIn('Future release only', html)
        self.assertNotIn('Previous release only', html)
        self.assertNotIn('Unreleased', html)
        self.assertNotIn('2026-11-07', html)
        self.assertIn('<h2>Changed</h2>', html)
        self.assertIn('CLI — compatibility', html)

    def test_credits_and_links_survive_wrapped_markdown(self):
        html = render(SOURCE, 'v0.1.0-beta.172')
        self.assertIn('<span class="credit">PR ', html)
        self.assertIn('https://github.com/nowledge-co/con-terminal/pull/546', html)
        self.assertIn('https://github.com/wey-gu', html)
        self.assertEqual(html.count('<li>'), 3)
        self.assertEqual(html.count('<ul>'), html.count('</ul>'))

    def test_html_and_executable_links_are_inert(self):
        html = body(['- <script>alert(1)</script> & [click](javascript:alert)',
                     '- [local](file:///etc/passwd) and **bold**'])
        document = Document(html)
        self.assertFalse(any(tag == 'script' for tag, _ in document.tags))
        self.assertFalse(any(tag == 'a' for tag, _ in document.tags))
        self.assertIn('&lt;script&gt;', html)
        self.assertIn('<strong>bold</strong>', html)
        self.assertIn('&amp;', html)

    def test_no_remote_assets_and_full_changelog_is_available(self):
        html = render(SOURCE, 'v0.1.0-beta.172')
        document = Document(html)
        self.assertFalse(any(tag in ('script', 'link', 'img', 'iframe') for tag, _ in document.tags))
        self.assertIn("default-src 'none'", html)
        self.assertIn('prefers-color-scheme: dark', html)
        self.assertTrue(any(tag == 'a' and attrs.get('href') == 'https://con.nowledge.co/changelog/'
                            for tag, attrs in document.tags))

    def test_missing_release_is_honest_not_latest(self):
        html = render(SOURCE, 'v0.1.0-beta.17')
        self.assertIn('Release details are available in the full changelog.', html)
        self.assertNotIn('Future release only', html)
        self.assertNotIn('Keep the', html)

    def test_invalid_identity_cannot_be_a_release(self):
        for tag in ('../main', '<script>', 'v0.1.0-beta.172/extra'):
            with self.assertRaises(ValueError):
                render(SOURCE, tag)

    def test_full_document_survives_appcast_xml_round_trip(self):
        root = ET.Element('item')
        expected = render(SOURCE, 'v0.1.0-beta.172')
        ET.SubElement(root, 'description').text = expected
        actual = ET.fromstring(ET.tostring(root)).findtext('description')
        self.assertEqual(actual, expected)
        self.assertIn('Unicode &amp; &lt;image&gt;', actual)

    def test_published_dates_and_stable_identity(self):
        source = '## `v1.0.0` - 2026-11-07\n### Added\n- A stable release.\n'
        self.assertIn('con 1.0.0 · 2026-11-07', render(source, 'v1.0.0'))

    def test_actual_changelog_keeps_target_version(self):
        source = (ROOT / 'CHANGELOG.md').read_text()
        heading, lines = section(source, 'v0.1.0-beta.170')
        self.assertIn('2026-11-03', heading)
        self.assertTrue(lines)
        html = render(source, 'v0.1.0-beta.170')
        self.assertIn('Drag tabs into pane layouts', html)
        self.assertIn('https://github.com/frostming', html)
        self.assertNotIn('stable C API', html)


if __name__ == '__main__':
    unittest.main()
