#!/usr/bin/env python3
"""Emit reproducible OSC 7501 lifecycle or load fixtures in a real terminal."""

import argparse
import base64
import json
import math
import sys
import time


def sequence(body, tmux=False, opcode=7501):
    payload = f"\x1b]{opcode};{body}\x07".encode("utf-8")
    if tmux:
        payload = b"\x1bPtmux;" + payload.replace(b"\x1b", b"\x1b\x1b") + b"\x1b\\"
    return payload


def write(payload):
    sys.stdout.buffer.write(payload)
    sys.stdout.buffer.flush()


def text(value):
    return base64.b64encode(value.encode("utf-8")).decode("ascii")


def report(body, args):
    # An ignored opcode preserves byte volume in the load control condition.
    write(sequence(body, args.tmux, 7501 if args.mode == "status" else 7502))


def lifecycle(args):
    phases = [
        ("working", "state=working:id=fixture:app=acceptance:progress=10"),
        ("blocked child", "state=blocked:id=fixture/child:kind=permission:progress=40:msg="
         + text("Waiting for permission")),
        ("idle root, child remains blocked", "state=idle:id=fixture:app=acceptance"),
        ("clear child", "state=clear:id=fixture/child"),
        ("working again", "state=working:id=fixture:app=acceptance:progress=80"),
        ("done", "state=done:id=fixture:app=acceptance:msg=" + text("Finished")),
        ("error", "state=error:id=fixture:app=acceptance:msg=" + text("Example failure")),
    ]
    alternate_active = False
    try:
        for label, body in phases:
            report(body, args)
            print(f"\r\nOSC7501 phase: {label}", flush=True)
            time.sleep(args.seconds)
        if args.alternate_screen:
            write(b"\x1b[?1049h")
            alternate_active = True
            print("Error remains in the alternate screen", flush=True)
            time.sleep(args.seconds)
            write(b"\x1b[?1049l")
            alternate_active = False
        print("\r\nFixture exited: error should remain until input to this surface.", flush=True)
    finally:
        if alternate_active:
            write(b"\x1b[?1049l")


def load(args):
    count = math.ceil(args.seconds * args.rate)
    started = time.monotonic()
    late = 0
    try:
        for step in range(count):
            deadline = started + step / args.rate
            delay = deadline - time.monotonic()
            if delay > 0:
                time.sleep(delay)
            elif step and delay < -1 / args.rate:
                late += 1
            report(f"state=working:id=fixture:app=acceptance:progress={step % 101}", args)
            # Both conditions paint identical visible content at identical rates.
            print(f"fixture output {step:08d} | scroll and type in another pane", flush=True)
    finally:
        report("state=clear:id=fixture", args)
    print(json.dumps({
        "fixture": "osc7501-load", "mode": args.mode, "reports": count,
        "elapsed_seconds": time.monotonic() - started, "late_emissions": late,
        "note": "Emitter timing only; not UI frame rate or input latency.",
    }), flush=True)


def positive(value):
    number = float(value)
    if not math.isfinite(number) or number <= 0:
        raise argparse.ArgumentTypeError("must be a finite positive number")
    return number


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("scenario", choices=["lifecycle", "load"])
    parser.add_argument("--mode", choices=["status", "baseline"], default="status")
    parser.add_argument("--seconds", type=positive, default=3, help="per phase, or total load duration")
    parser.add_argument("--rate", type=positive, default=120, help="load reports per second")
    parser.add_argument("--tmux", action="store_true", help="use tmux passthrough envelopes")
    parser.add_argument("--alternate-screen", action="store_true")
    args = parser.parse_args()
    try:
        (lifecycle if args.scenario == "lifecycle" else load)(args)
    except KeyboardInterrupt:
        return 130
    return 0


if __name__ == "__main__":
    sys.exit(main())
