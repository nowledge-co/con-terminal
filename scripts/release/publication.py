#!/usr/bin/env python3
"""Reconcile release runs and publish updater feeds only after anonymous delivery."""

import argparse
import copy
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
import urllib.request
import xml.etree.ElementTree as ET

from release_notes import CHANGELOG, render as render_release_notes


SPARKLE = "http://www.andymatuschak.org/xml-namespaces/sparkle"
ET.register_namespace("sparkle", SPARKLE)
WORKFLOWS = ("release-linux.yml", "release-macos.yml", "release-windows.yml")
PENDING = {"queued", "in_progress", "waiting", "pending", "requested"}
TAG = re.compile(r"v\d+\.\d+\.\d+(?:-(?:beta|dev)\.\d+)?\Z")
ROOT = Path(__file__).resolve().parents[2]


def run(*args, cwd=None):
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def gh_json(*args):
    return json.loads(run("gh", *args))


def channel_for(tag):
    if not TAG.fullmatch(tag):
        raise ValueError(f"Invalid release tag: {tag}")
    return "dev" if "-dev." in tag else "beta" if "-beta." in tag else "stable"


def latest_run(runs, tag, sha):
    matching = [r for r in runs if r.get("head_sha") == sha
                and r.get("head_branch") == tag and r.get("event") == "push"]
    # updated_at is not creation order: rerunning an old run must not hide a
    # newer run. A rerun retains its ID and reports its current run_attempt.
    return max(matching, key=lambda r: r["id"], default=None)


def workflow_run(repo, workflow, tag, sha):
    endpoint = f"repos/{repo}/actions/workflows/{workflow}/runs"
    result = gh_json("api", f"{endpoint}?head_sha={sha}&event=push&per_page=100")
    found = latest_run(result["workflow_runs"], tag, sha)
    if found is None:
        # The SHA-filtered Actions index can transiently omit completed runs.
        # Independently list recent runs, then apply BOTH identities locally.
        recent = gh_json("api", f"{endpoint}?event=push&per_page=100")
        found = latest_run(recent["workflow_runs"], tag, sha)
    return found


def wait_for_platforms(repo, tag, sha, attempts=6, delay=15):
    for attempt in range(attempts):
        states = []
        for workflow in WORKFLOWS:
            try:
                item = workflow_run(repo, workflow, tag, sha)
                if item is None:
                    state = "missing"
                elif item["status"] != "completed":
                    state = item["status"]
                else:
                    state = item["conclusion"] or "invalid-completion"
                print(f"{workflow}: {state} (tag={tag}, sha={sha})", flush=True)
                states.append(state)
            except (subprocess.CalledProcessError, ValueError, KeyError) as error:
                print(f"{workflow}: query failed: {error}", file=sys.stderr)
                states.append("query-error")
        if all(state == "success" for state in states):
            return True
        if any(state not in PENDING | {"success", "missing", "query-error"} for state in states):
            raise RuntimeError("A required platform did not succeed; release is blocked")
        if attempt + 1 < attempts:
            time.sleep(delay)
    if any(state in {"missing", "query-error"} for state in states):
        raise RuntimeError("Actions results remained unavailable after retries; rerun finalizer")
    # An observed running sibling will emit its own completion event. Unlike a
    # missing result, this is a legitimate reason to defer without publishing.
    return False


def feed_names(channel):
    return [f"{channel}-{platform}.xml" for platform in
            ("macos-arm64", "macos-x86_64", "linux-x86_64", "windows-x86_64")]


def staged_feeds(pages, repo, tag, release):
    channel = channel_for(tag)
    if channel == "dev":
        return {}
    assets = {asset["name"]: asset for asset in release["assets"]}
    feeds = {}
    for name in feed_names(channel):
        tree = ET.parse(pages / "release-staging" / tag / "appcast" / name)
        items = tree.findall("./channel/item")
        if len(items) != 1:
            raise ValueError(f"Staged feed must contain exactly one item: {name}")
        item = items[0]
        enc = item.find("enclosure")
        platform = name.removeprefix(f"{channel}-").removesuffix(".xml")
        version = tag.removeprefix("v")
        prefix = "con-Beta" if channel == "beta" and platform.startswith("macos") else "con"
        suffix = "dmg" if platform.startswith("macos") else (
            "zip" if platform.startswith("windows") else "tar.gz")
        asset_name = f"{prefix}-{version}-{platform}.{suffix}"
        asset = assets[asset_name]
        url = f"https://github.com/{repo}/releases/download/{tag}/{asset_name}"
        if enc is None or enc.get("url") != url:
            raise ValueError(f"Unexpected staged enclosure URL: {name}")
        if int(enc.get("length", "0")) != asset["size"] or asset["size"] <= 0:
            raise ValueError(f"Staged size differs from release asset: {name}")
        if not enc.get(f"{{{SPARKLE}}}edSignature"):
            raise ValueError(f"Missing signature: {name}")
        build_number(item)
        feeds[name] = tree
    return feeds


def build_number(item):
    value = int(item.findtext(f"{{{SPARKLE}}}version", "0"))
    if value <= 0:
        raise ValueError("Appcast build number must be positive")
    return value


def merge_feed(existing, incoming):
    result = copy.deepcopy(existing if existing is not None else incoming)
    channel = result.find("channel")
    if channel is None:
        raise ValueError("Appcast is missing its channel")
    new_item = incoming.find("./channel/item")
    number = build_number(new_item)
    items = channel.findall("item")
    newest = max((build_number(item) for item in items), default=0)
    for item in items:
        if build_number(item) == number:
            # Immutable tags must not silently replace an already shipped item.
            if item.find("enclosure").attrib != new_item.find("enclosure").attrib:
                raise ValueError("Conflicting enclosure for an existing build number")
        channel.remove(item)
    by_number = {build_number(item): item for item in items}
    by_number[number] = copy.deepcopy(new_item)
    for key in sorted(by_number, reverse=True)[:20]:
        channel.append(by_number[key])
    return result, number >= newest


def probe_downloads(feeds, attempts=6, delay=10):
    for tree in feeds.values():
        enc = tree.find("./channel/item/enclosure")
        url = enc.get("url")
        for attempt in range(attempts):
            try:
                # No token or cache-busting query: verify the EXACT URL clients
                # will use, including any cached draft-era 404 response.
                request = urllib.request.Request(url, method="HEAD")
                with urllib.request.urlopen(request, timeout=30) as response:
                    if response.status != 200:
                        raise ValueError(f"Unexpected HTTP status: {response.status}")
                    if int(response.headers.get("Content-Length", "0")) != int(enc.get("length")):
                        raise ValueError("Anonymous download size differs from signed feed")
                break
            except (OSError, ValueError) as error:
                if attempt + 1 == attempts:
                    raise RuntimeError(f"Anonymous download unavailable: {url}") from error
                time.sleep(delay)


def finish(repo, tag, sha, pages):
    release = gh_json("release", "view", tag, "--repo", repo, "--json", "isDraft,assets")
    feeds = staged_feeds(pages, repo, tag, release)
    installers = installers_at(sha)
    # Validate the candidate installer bytes, not an older Pages deployment.
    # Canonical files stay untouched until public download probes succeed.
    with tempfile.TemporaryDirectory(prefix="con-release-gate-") as candidate:
        candidate = Path(candidate)
        for name, content in installers.items():
            (candidate / name).write_text(content)
        subprocess.run(["bash", str(ROOT / "scripts/release/verify-release-gate.sh"),
                        tag, str(candidate), str(pages / "release-staging" / tag / "appcast")],
                       check=True, env={**os.environ, "GH_REPO": repo})
    if release["isDraft"]:
        run("gh", "release", "edit", tag, "--repo", repo, "--draft=false")
    # A public release alone does not prove anonymous CDN availability.
    probe_downloads(feeds)
    if not feeds:  # Dev releases never touch stable/beta updater endpoints.
        return
    # Use the immutable tag source, shared with the public website. Embed a
    # self-contained snapshot so the native prompt needs no second network
    # request or live website build to explain what the user is installing.
    changelog = run("git", "show", f"{sha}:CHANGELOG.md", cwd=ROOT)
    notes = render_release_notes(changelog, tag)
    for tree in feeds.values():
        item = tree.find("./channel/item")
        for field in ("description", f"{{{SPARKLE}}}releaseNotesLink",
                      f"{{{SPARKLE}}}fullReleaseNotesLink"):
            for existing in item.findall(field):
                item.remove(existing)
        ET.SubElement(item, "description").text = notes
        ET.SubElement(item, f"{{{SPARKLE}}}fullReleaseNotesLink").text = CHANGELOG
    for attempt in range(5):
        run("git", "fetch", "origin", "gh-pages", cwd=pages)
        # This dedicated CI clone is clean after each attempted commit. Start
        # again from the latest remote tree so Flatpak and newer feeds survive.
        run("git", "checkout", "--detach", "origin/gh-pages", cwd=pages)
        current_release = gh_json("release", "view", tag, "--repo", repo, "--json", "isDraft")
        if current_release["isDraft"]:
            raise RuntimeError("Release became private; refusing updater publication")
        current = True
        for name, incoming in feeds.items():
            path = pages / "appcast" / name
            merged, is_current = merge_feed(ET.parse(path) if path.exists() else None, incoming)
            current &= is_current
            path.parent.mkdir(parents=True, exist_ok=True)
            ET.indent(merged, space="  ")
            merged.write(path, encoding="utf-8", xml_declaration=True)
        if current:
            for name, content in installers.items():
                (pages / name).write_text(content)
        run("git", "add", "appcast", "install.sh", "install.ps1", cwd=pages)
        if subprocess.run(["git", "diff", "--cached", "--quiet"], cwd=pages).returncode == 0:
            return
        run("git", "commit", "-m", f"Publish verified updater feeds for {tag}", cwd=pages)
        try:
            run("git", "push", "origin", "HEAD:gh-pages", cwd=pages)
            return
        except subprocess.CalledProcessError:
            if attempt == 4:
                raise
            time.sleep(2 * (attempt + 1))


def installers_at(sha):
    return {name: run("git", "show", f"{sha}:scripts/{name}", cwd=ROOT) + "\n"
            for name in ("install.sh", "install.ps1")}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("wait", "finish"))
    parser.add_argument("tag")
    parser.add_argument("sha")
    parser.add_argument("--pages", type=Path, default=Path("gh-pages"))
    args = parser.parse_args()
    channel_for(args.tag)
    repo = os.environ["GH_REPO"]
    if args.command == "wait":
        ready = wait_for_platforms(repo, args.tag, args.sha)
        with open(os.environ["GITHUB_OUTPUT"], "a") as output:
            output.write(f"ready={str(ready).lower()}\n")
    else:
        finish(repo, args.tag, args.sha, args.pages.resolve())


if __name__ == "__main__":
    main()
