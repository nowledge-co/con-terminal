"""Exercise actual workflow filters, without building Rust or contacting GitHub.

The matcher covers the literal/*/** subset used by these workflow filters;
GitHub remains the event dispatcher. actionlint separately checks YAML/schema.
"""
import re
import unittest
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[2]
HEAVY = ('ci-portable.yml', 'e2e.yml', 'ci-macos-ghostty.yml',
         'ci-macos-packaging.yml', 'publish-flatpak.yml')


def workflow(name):
    # BaseLoader preserves the literal "on" key instead of YAML 1.1's bool.
    return yaml.load((ROOT / '.github/workflows' / name).read_text(), Loader=yaml.BaseLoader)


def matches(path, pattern):
    if any(c in pattern for c in '?[]+!'):
        raise AssertionError(f'Extend the test matcher for new pattern syntax: {pattern}')
    expression = ''
    index = 0
    while index < len(pattern):
        if pattern[index:index + 3] == '**/':
            expression += '(?:.*/)?'
            index += 3
        elif pattern[index:index + 2] == '**':
            expression += '.*'
            index += 2
        elif pattern[index] == '*':
            expression += '[^/]*'
            index += 1
        else:
            expression += re.escape(pattern[index])
            index += 1
    return re.fullmatch(expression, path) is not None


def triggered(name, files, event='push', ref='main', tag=False):
    events = workflow(name)['on']
    if event not in events:
        return False
    if event not in ('push', 'pull_request'):
        return True
    config = events[event] or {}
    if event == 'push':
        wanted, other = ('tags', 'branches') if tag else ('branches', 'tags')
        if wanted in config and not any(matches(ref, p) for p in config[wanted]):
            return False
        if wanted not in config and other in config:
            return False
        if tag:
            return True  # GitHub does not evaluate path filters on tag pushes.
    patterns = config.get('paths')
    if patterns is None:
        return True
    for file in files:
        included = False
        for pattern in patterns:
            negative = pattern.startswith('!')
            if matches(file, pattern[1:] if negative else pattern):
                included = not negative
        if included:
            return True
    return False


class WorkflowPolicy(unittest.TestCase):
    def test_documentation_only_never_starts_native_builds(self):
        for path in ('CHANGELOG.md', 'README.md', 'AGENTS.md', 'HACKING.md',
                     'docs/manifest.json', 'docs/impl/build-system.md',
                     'scripts/docs/validate-manifest.py',
                     'skills/con-terminal/SKILL.md', 'postmortem/incident.md'):
            for event in ('push', 'pull_request'):
                for name in HEAVY:
                    with self.subTest(path=path, event=event, workflow=name):
                        self.assertFalse(triggered(name, [path], event))
                self.assertTrue(triggered('ci-metadata.yml', [path], event))

    def test_mixed_code_and_changelog_still_runs_real_checks(self):
        for event in ('push', 'pull_request'):
            for path in ('crates/con-app/src/main.rs', 'crates/con-terminal/src/lib.rs',
                         'Cargo.lock', 'mise.toml', 'assets/icons/phosphor/check.svg'):
                for name in ('ci-portable.yml', 'e2e.yml'):
                    with self.subTest(path=path, event=event, workflow=name):
                        self.assertTrue(triggered(name, ['CHANGELOG.md', path], event))

    def test_flatpak_tracks_real_build_and_packaging_inputs(self):
        for path in ('crates/con-ghostty/build.rs', '.cargo/config.toml', 'Cargo.toml',
                     'packaging/flatpak/aetherpak.yaml', 'scripts/linux/release.sh',
                     'scripts/release/test-flatpak-icon.py'):
            with self.subTest(path=path):
                self.assertTrue(triggered('publish-flatpak.yml', [path]))

    def test_release_tags_are_never_filtered_by_changed_files(self):
        for name in ('release-macos.yml', 'release-windows.yml', 'release-linux.yml',
                     'publish-flatpak.yml'):
            for tag in ('v0.1.0-beta.172', 'v0.1.0', 'v0.1.0-dev.1'):
                with self.subTest(workflow=name, tag=tag):
                    self.assertTrue(triggered(name, ['CHANGELOG.md'], ref=tag, tag=True))
                    self.assertTrue(triggered(name, [], ref=tag, tag=True))

    def test_flatpak_manual_and_scheduled_publication_remain_available(self):
        for event in ('schedule', 'workflow_dispatch'):
            self.assertTrue(triggered('publish-flatpak.yml', [], event))

    def test_checks_supersede_only_the_same_workflow_and_pr_or_ref(self):
        for name in (*HEAVY[:-1], 'ci-metadata.yml'):
            concurrency = workflow(name)['concurrency']
            self.assertEqual(concurrency['cancel-in-progress'], 'true')
            self.assertIn('github.workflow', concurrency['group'])
            self.assertIn('github.event.pull_request.number || github.ref', concurrency['group'])

    def test_release_publishers_are_not_auto_cancelled(self):
        for name in ('release-macos.yml', 'release-windows.yml', 'release-linux.yml',
                     'release-finalize.yml', 'publish-flatpak.yml'):
            self.assertNotEqual(workflow(name).get('concurrency', {}).get('cancel-in-progress'), 'true')

    def test_changelog_still_rebuilds_the_public_documentation(self):
        self.assertTrue(triggered('docs-website-rebuild.yml', ['CHANGELOG.md']))

    def test_workflow_only_changes_do_not_compile_the_application(self):
        for path in sorted((ROOT / ".github/workflows").glob("*.yml")):
            relative = path.relative_to(ROOT).as_posix()
            for event in ("push", "pull_request"):
                self.assertTrue(triggered("ci-metadata.yml", [relative], event))
                for name in HEAVY:
                    if name == "ci-macos-packaging.yml":
                        continue  # Small shell/Ruby fixture checks are intentional.
                    with self.subTest(path=relative, event=event, workflow=name):
                        self.assertFalse(triggered(name, [relative], event))

    def test_workflow_lint_is_part_of_the_lightweight_lane(self):
        steps = workflow("ci-metadata.yml")["jobs"]["validate"]["steps"]
        self.assertTrue(any("github.com/rhysd/actionlint/cmd/actionlint@v1.7.12"
                            in step.get("run", "") for step in steps))

    def test_native_checks_remain_manually_available(self):
        for name in ("ci-portable.yml", "e2e.yml", "ci-macos-ghostty.yml"):
            self.assertTrue(triggered(name, [], "workflow_dispatch"))

    def test_metadata_tests_do_not_trigger_native_builds(self):
        for path in ('.github/tests/test_workflow_policy.py', '.github/workflows/ci-metadata.yml'):
            self.assertTrue(triggered('ci-metadata.yml', [path], 'pull_request'))
            for name in HEAVY:
                self.assertFalse(triggered(name, [path], 'pull_request'))


if __name__ == '__main__':
    unittest.main()
