#!/usr/bin/env python3
"""Offline regression tests for publication ordering and Actions reconciliation."""

import copy
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch
import xml.etree.ElementTree as ET


ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("publication", ROOT / "scripts/release/publication.py")
p = importlib.util.module_from_spec(spec)
spec.loader.exec_module(p)
TAG = "v0.1.0-beta.149"
SHA = "a" * 40
REPO = "nowledge-co/con-terminal"


def workflow(number=1, **kwargs):
    return {"id": number, "head_sha": SHA, "head_branch": TAG, "event": "push",
            "status": "completed", "conclusion": "success", **kwargs}


def feed(number=160, url="https://example.test/release.dmg"):
    root = ET.Element("rss", version="2.0")
    channel = ET.SubElement(root, "channel")
    ET.SubElement(channel, "title").text = "con"
    item = ET.SubElement(channel, "item")
    ET.SubElement(item, f"{{{p.SPARKLE}}}version").text = str(number)
    ET.SubElement(item, "enclosure", {"url": url, "length": "42",
                                      f"{{{p.SPARKLE}}}edSignature": "signed"})
    return ET.ElementTree(root)


class WorkflowTests(unittest.TestCase):
    def test_matches_tag_sha_event_not_updated_order(self):
        runs = [workflow(1, updated_at="future"), workflow(2, status="in_progress"),
                workflow(3, head_branch="v0.1.0-dev.149"), workflow(4, head_sha="b" * 40),
                workflow(5, event="workflow_dispatch")]
        self.assertEqual(p.latest_run(runs, TAG, SHA)["id"], 2)

    def test_empty_sha_index_falls_back_and_filters_locally(self):
        with patch.object(p, "gh_json", side_effect=[{"workflow_runs": []},
                          {"workflow_runs": [workflow(7), workflow(8, head_sha="wrong")]}]) as gh:
            self.assertEqual(p.workflow_run(REPO, p.WORKFLOWS[0], TAG, SHA)["id"], 7)
        self.assertNotIn("head_sha=", gh.call_args_list[1].args[1])

    def test_transient_missing_result_retries(self):
        with patch.object(p, "workflow_run", side_effect=[None, workflow(), workflow(),
                          workflow(), workflow(), workflow()]), patch.object(p.time, "sleep"):
            self.assertTrue(p.wait_for_platforms(REPO, TAG, SHA, attempts=2))

    def test_api_error_retries(self):
        with patch.object(p, "workflow_run", side_effect=[ValueError("bad response"),
                          workflow(), workflow(), workflow(), workflow(), workflow()]), \
                patch.object(p.time, "sleep"):
            self.assertTrue(p.wait_for_platforms(REPO, TAG, SHA, attempts=2))

    def test_missing_results_never_silently_succeed(self):
        with patch.object(p, "workflow_run", return_value=None), patch.object(p.time, "sleep"):
            with self.assertRaisesRegex(RuntimeError, "unavailable"):
                p.wait_for_platforms(REPO, TAG, SHA, attempts=2)

    def test_observed_pending_sibling_defers(self):
        with patch.object(p, "workflow_run", return_value=workflow(status="in_progress")):
            self.assertFalse(p.wait_for_platforms(REPO, TAG, SHA, attempts=1))

    def test_failed_platform_blocks(self):
        with patch.object(p, "workflow_run", return_value=workflow(conclusion="failure")):
            with self.assertRaisesRegex(RuntimeError, "blocked"):
                p.wait_for_platforms(REPO, TAG, SHA)

    def test_non_success_terminal_states_never_silently_defer(self):
        for conclusion in ("neutral", "skipped", None, "unknown-status"):
            with self.subTest(conclusion=conclusion), \
                    patch.object(p, "workflow_run", return_value=workflow(conclusion=conclusion)):
                with self.assertRaisesRegex(RuntimeError, "blocked"):
                    p.wait_for_platforms(REPO, TAG, SHA)


class FeedTests(unittest.TestCase):
    def test_older_retry_cannot_move_latest_backwards(self):
        tree, current = p.merge_feed(feed(161), feed(160))
        self.assertFalse(current)
        self.assertEqual([p.build_number(i) for i in tree.findall("./channel/item")], [161, 160])

    def test_duplicate_is_idempotent_with_different_xml_indentation(self):
        old = feed()
        ET.indent(old)
        merged, current = p.merge_feed(old, feed())
        self.assertTrue(current)
        self.assertEqual(len(merged.findall("./channel/item")), 1)

    def test_conflicting_immutable_build_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "Conflicting"):
            p.merge_feed(feed(), feed(url="https://other.test/package"))

    def test_union_keeps_newest_twenty(self):
        tree = feed(1)
        for number in range(2, 23):
            tree, _ = p.merge_feed(tree, feed(number))
        self.assertEqual([p.build_number(i) for i in tree.findall("./channel/item")],
                         list(range(22, 2, -1)))

    def test_anonymous_probe_uses_exact_url_without_token(self):
        response = Mock(status=200, headers={"Content-Length": "42"})
        response.__enter__ = Mock(return_value=response)
        response.__exit__ = Mock(return_value=False)
        with patch.object(p.urllib.request, "urlopen", return_value=response) as open_url:
            p.probe_downloads({"feed": feed()})
        request = open_url.call_args.args[0]
        self.assertEqual(request.full_url, "https://example.test/release.dmg")
        self.assertEqual(request.get_method(), "HEAD")
        self.assertFalse(request.has_header("Authorization"))

    def test_cached_404_blocks_feeds(self):
        with patch.object(p.urllib.request, "urlopen", side_effect=OSError("404")), \
                patch.object(p.time, "sleep"):
            with self.assertRaisesRegex(RuntimeError, "unavailable"):
                p.probe_downloads({"feed": feed()}, attempts=2)

    def test_invalid_tag_rejected(self):
        for tag in ("../main", "main", "v1.2.3-beta.1/../../other"):
            with self.assertRaises(ValueError):
                p.channel_for(tag)


class PublicationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.pages = Path(self.temp.name)
        self.stage = self.pages / "release-staging" / TAG / "appcast"
        self.stage.mkdir(parents=True)
        self.release = {"isDraft": True, "assets": []}
        installer_patch = patch.object(p, "installers_at", return_value={
            name: (ROOT / "scripts" / name).read_text() for name in ("install.sh", "install.ps1")})
        installer_patch.start()
        self.addCleanup(installer_patch.stop)
        for name in p.feed_names("beta"):
            platform = name.removeprefix("beta-").removesuffix(".xml")
            prefix = "con-Beta" if platform.startswith("macos") else "con"
            suffix = "dmg" if platform.startswith("macos") else (
                "zip" if platform.startswith("windows") else "tar.gz")
            asset = f"{prefix}-{TAG[1:]}-{platform}.{suffix}"
            tree = feed(url=f"https://github.com/{REPO}/releases/download/{TAG}/{asset}")
            tree.write(self.stage / name)
            self.release["assets"].append({"name": asset, "size": 42})

    def test_all_four_feeds_are_prepared(self):
        self.assertEqual(len(p.staged_feeds(self.pages, REPO, TAG, self.release)), 4)

    def test_size_or_wrong_repository_is_rejected(self):
        bad = copy.deepcopy(self.release)
        bad["assets"][0]["size"] = 43
        with self.assertRaisesRegex(ValueError, "size"):
            p.staged_feeds(self.pages, REPO, TAG, bad)
        with self.assertRaisesRegex(ValueError, "URL"):
            p.staged_feeds(self.pages, "other/repo", TAG, self.release)

    def test_missing_platform_blocks_before_promotion(self):
        (self.stage / "beta-macos-arm64.xml").unlink()
        with patch.object(p, "gh_json", return_value=self.release), patch.object(p, "run") as run:
            with self.assertRaises(FileNotFoundError):
                p.finish(REPO, TAG, SHA, self.pages)
        run.assert_not_called()

    def test_multiple_items_cannot_smuggle_an_unpublished_release(self):
        path = self.stage / "beta-macos-arm64.xml"
        tree = ET.parse(path)
        tree.find("channel").append(copy.deepcopy(tree.find("./channel/item")))
        tree.write(path)
        with self.assertRaisesRegex(ValueError, "exactly one"):
            p.staged_feeds(self.pages, REPO, TAG, self.release)

    def test_gate_failure_never_promotes(self):
        with patch.object(p, "gh_json", return_value=self.release), \
                patch.object(p.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "gate")), \
                patch.object(p, "run") as run:
            with self.assertRaises(subprocess.CalledProcessError):
                p.finish(REPO, TAG, SHA, self.pages)
        run.assert_not_called()

    def test_download_failure_happens_after_promotion_but_before_any_git_write(self):
        calls = []
        with patch.object(p, "gh_json", return_value=self.release), \
                patch.object(p.subprocess, "run"), \
                patch.object(p, "run", side_effect=lambda *a, **k: calls.append(a)), \
                patch.object(p, "probe_downloads", side_effect=RuntimeError("404")):
            with self.assertRaisesRegex(RuntimeError, "404"):
                p.finish(REPO, TAG, SHA, self.pages)
        self.assertEqual(calls, [("gh", "release", "edit", TAG, "--repo", REPO, "--draft=false")])

    def test_dev_never_changes_updater_feeds(self):
        with patch.object(p, "gh_json", return_value=self.release), \
                patch.object(p.subprocess, "run"), patch.object(p, "run") as run:
            p.finish(REPO, "v0.1.0-dev.149", SHA, self.pages)
        self.assertEqual(run.call_count, 1)
        self.assertEqual(run.call_args.args[:3], ("gh", "release", "edit"))

    def git_fixture(self):
        remote = self.pages.parent / f"{self.pages.name}-remote.git"
        self.addCleanup(lambda: shutil.rmtree(remote, ignore_errors=True))
        p.run("git", "init", "--bare", str(remote))
        p.run("git", "init", "-b", "gh-pages", cwd=self.pages)
        p.run("git", "config", "user.name", "Fixture", cwd=self.pages)
        p.run("git", "config", "user.email", "fixture@example.test", cwd=self.pages)
        p.run("git", "remote", "add", "origin", str(remote), cwd=self.pages)
        (self.pages / "appcast").mkdir()
        for name in p.feed_names("beta"):
            feed(159).write(self.pages / "appcast" / name)
        for name in ("install.sh", "install.ps1"):
            (self.pages / name).write_text((ROOT / "scripts" / name).read_text())
        (self.pages / "flatpak").mkdir()
        (self.pages / "flatpak" / "keep").write_text("unrelated deployment")
        p.run("git", "add", ".", cwd=self.pages)
        p.run("git", "commit", "-m", "Fixture", cwd=self.pages)
        p.run("git", "push", "origin", "HEAD:gh-pages", cwd=self.pages)
        return remote

    def finish_with_real_git(self, *, conflict=False):
        remote = self.git_fixture()
        original_run = p.run
        original_subprocess = subprocess.run
        effects = []
        source_requests = []
        inserted = False

        def execute(*args, **kwargs):
            nonlocal inserted
            if args[:2] == ("git", "show") and args[2].endswith(":CHANGELOG.md"):
                source_requests.append(args[2])
            if args[0] == "gh":
                effects.append("promote")
                self.release["isDraft"] = False
                return ""
            if args[:2] == ("git", "push"):
                effects.append("push")
                if conflict and not inserted:
                    inserted = True
                    with tempfile.TemporaryDirectory() as other:
                        original_run("git", "clone", "--branch", "gh-pages", str(remote), other)
                        original_run("git", "config", "user.name", "Other", cwd=other)
                        original_run("git", "config", "user.email", "other@example.test", cwd=other)
                        (Path(other) / "flatpak" / "keep").write_text("concurrent deployment")
                        original_run("git", "add", ".", cwd=other)
                        original_run("git", "commit", "-m", "Concurrent Flatpak update", cwd=other)
                        original_run("git", "push", "origin", "HEAD:gh-pages", cwd=other)
            return original_run(*args, **kwargs)

        def gate(args, **kwargs):
            if args[0] == "bash":
                effects.append("gate")
                return subprocess.CompletedProcess(args, 0)
            return original_subprocess(args, **kwargs)

        with patch.object(p, "run", side_effect=execute), \
                patch.object(p, "gh_json", side_effect=lambda *a: copy.deepcopy(self.release)), \
                patch.object(p.subprocess, "run", side_effect=gate), \
                patch.object(p, "probe_downloads", side_effect=lambda *a: effects.append("probe")), \
                patch.object(p.time, "sleep"):
            p.finish(REPO, TAG, p.run("git", "rev-parse", "HEAD", cwd=ROOT), self.pages)
            p.finish(REPO, TAG, p.run("git", "rev-parse", "HEAD", cwd=ROOT), self.pages)
        expected_source = p.run("git", "rev-parse", "HEAD", cwd=ROOT) + ":CHANGELOG.md"
        self.assertEqual(source_requests, [expected_source, expected_source])
        self.assertEqual(effects[:3], ["gate", "promote", "probe"])
        self.assertEqual(effects.count("promote"), 1)
        self.assertGreater(effects.index("push"), effects.index("probe"))
        for name in p.feed_names("beta"):
            items = ET.parse(self.pages / "appcast" / name).findall("./channel/item")
            self.assertEqual([p.build_number(item) for item in items], [160, 159])
            staged_item = ET.parse(self.stage / name).find("./channel/item")
            self.assertEqual(items[0].find("enclosure").attrib, staged_item.find("enclosure").attrib)
            self.assertIn("What’s new", items[0].findtext("description"))
            self.assertIn(TAG[1:], items[0].findtext("description"))
            self.assertNotIn("What’s new", items[1].findtext("description", ""))
            self.assertEqual(items[0].findtext(f"{{{p.SPARKLE}}}fullReleaseNotesLink"),
                             "https://con.nowledge.co/changelog/")
        expected = "concurrent deployment" if conflict else "unrelated deployment"
        self.assertEqual((self.pages / "flatpak" / "keep").read_text(), expected)

    def test_real_git_publication_and_public_retry_are_idempotent(self):
        self.finish_with_real_git()

    def test_real_git_push_race_preserves_concurrent_flatpak_changes(self):
        self.finish_with_real_git(conflict=True)

    def gate_result(self, *, draft, broken_checksum=False):
        release = copy.deepcopy(self.release)
        release["isDraft"] = draft
        manifests = {
            "SHA256SUMS-macos-arm64.txt": [f"con-Beta-{TAG[1:]}-macos-arm64.dmg"],
            "SHA256SUMS-macos-x86_64.txt": [f"con-Beta-{TAG[1:]}-macos-x86_64.dmg"],
            "SHA256SUMS-linux.txt": [f"con-{TAG[1:]}-linux-x86_64.tar.gz"],
            "SHA256SUMS-windows.txt": [f"con-{TAG[1:]}-windows-x86_64.zip"],
        }
        for arch in ("arm64", "x86_64"):
            release["assets"].append({"name": f"con-Beta-{TAG[1:]}-macos-{arch}.zip", "size": 42})
        for name in manifests:
            release["assets"].append({"name": name, "size": 42})
        for name in p.feed_names("beta"):
            path = self.stage / name
            tree = ET.parse(path)
            item = tree.find("./channel/item")
            ET.SubElement(item, f"{{{p.SPARKLE}}}shortVersionString").text = (
                "0.1.0" if "macos" in name else TAG[1:])
            tree.write(path)
        for name in ("install.sh", "install.ps1"):
            (self.pages / name).write_text((ROOT / "scripts" / name).read_text())
        if broken_checksum:
            manifests["SHA256SUMS-linux.txt"] = ["wrong-artifact"]
        binary = self.pages / "bin"
        binary.mkdir()
        gh = binary / "gh"
        gh.write_text(
            "#!/usr/bin/env python3\nimport json,sys\nfrom pathlib import Path\n"
            f"release = {release!r}\nmanifests = {manifests!r}\n"
            "if sys.argv[1:3] == ['release','view']:\n    print(json.dumps(release))\n"
            "elif sys.argv[1:3] == ['release','download']:\n"
            "    dest = Path(sys.argv[sys.argv.index('--dir')+1])\n"
            "    for name, assets in manifests.items():\n"
            "        (dest / name).write_text('\\n'.join('a'*64+'  '+asset for asset in assets))\n"
            "else:\n    raise SystemExit('Unexpected gh invocation')\n")
        gh.chmod(0o755)
        return subprocess.run(
            ["bash", str(ROOT / "scripts/release/verify-release-gate.sh"),
             TAG, str(self.pages), str(self.stage)], text=True, capture_output=True,
            env={**os.environ, "PATH": f"{binary}{os.pathsep}{os.environ['PATH']}", "GH_REPO": REPO})

    def test_actual_gate_checks_staged_payload_for_public_resume(self):
        result = self.gate_result(draft=False)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_actual_gate_checks_staged_payload_before_promotion(self):
        result = self.gate_result(draft=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_actual_gate_rejects_wrong_checksum_manifest(self):
        result = self.gate_result(draft=True, broken_checksum=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("does not reference the Linux tarball", result.stdout + result.stderr)


class WorkflowWiringTests(unittest.TestCase):
    def test_platforms_stage_metadata_instead_of_publishing_live_feeds(self):
        for platform in ("macos", "windows", "linux"):
            source = (ROOT / ".github/workflows" / f"release-{platform}.yml").read_text()
            self.assertIn('git add "release-staging/$GITHUB_REF_NAME/appcast/"', source)
            self.assertNotIn('git add appcast/', source)
            self.assertNotIn('appcast_file="gh-pages/appcast/', source)
            self.assertNotIn("homebrew-tap.git", source)

    def test_recovery_and_homebrew_are_wired_after_finalization(self):
        source = (ROOT / ".github/workflows/release-finalize.yml").read_text()
        self.assertIn("workflow_dispatch:", source)
        self.assertIn("needs: finalize", source)
        self.assertIn('publication.py finish "$TAG" "$SHA"', source)
        ci = (ROOT / ".github/workflows/ci-portable.yml").read_text()
        self.assertIn("python3 scripts/release/test-publication.py", ci)


if __name__ == "__main__":
    unittest.main()
