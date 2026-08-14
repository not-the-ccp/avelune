#!/usr/bin/env python3
"""Small deterministic media workload for CI regression artifacts.

This is intentionally *not* a model of all real media. It provides stable cross-version
signals for several orthogonal codec stresses; heterogeneous external corpora remain separate.
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import platform
import re
import statistics
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PSNR_RE = re.compile(r"average:([^ ]+)")
SSIM_RE = re.compile(r"All:([0-9.]+)")


def run(cmd: list[str], capture: bool = False) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        cmd,
        cwd=ROOT,
        check=True,
        text=True,
        stdout=subprocess.PIPE if capture else subprocess.DEVNULL,
        stderr=subprocess.PIPE if capture else subprocess.DEVNULL,
    )


def y4m(path: Path, kind: str, w: int = 96, h: int = 64, frames: int = 24) -> None:
    def planes(t: int) -> tuple[bytearray, bytearray, bytearray]:
        y_plane = bytearray(w * h)
        u_plane = bytearray(w * h // 4)
        v_plane = bytearray(w * h // 4)
        for y in range(h):
            for x in range(w):
                if kind == "cut":
                    value = (x * 3 + y * 5 + (0 if t < frames // 2 else 113)) & 255
                elif kind == "pan":
                    value = ((x + t * 2) * 7 + y * 3) & 255
                elif kind == "ui":
                    value = 32 if ((x // 12 + y // 8 + t // 6) & 1) == 0 else 224
                elif kind == "gradient":
                    value = (x * 255 // max(1, w - 1) + t) & 255
                elif kind == "noise":
                    z = (x * 0x9E3779B1 + y * 0x85EBCA6B + t * 0xC2B2AE35) & 0xFFFFFFFF
                    z ^= z >> 16
                    value = z & 255
                else:
                    value = (x * 11 + y * 17 + t * 19) & 255
                y_plane[y * w + x] = value
        cw, ch = w // 2, h // 2
        for y in range(ch):
            for x in range(cw):
                index = y * cw + x
                if kind == "chroma":
                    u_plane[index] = (x * 19 + t * 7) & 255
                    v_plane[index] = (y * 23 - t * 5) & 255
                else:
                    u_plane[index] = (96 + t * 2 + x) & 255
                    v_plane[index] = (160 - t * 3 + y) & 255
        return y_plane, u_plane, v_plane

    with path.open("wb") as file:
        file.write(f"YUV4MPEG2 W{w} H{h} F24:1 Ip A1:1 C420jpeg\n".encode())
        for t in range(frames):
            file.write(b"FRAME\n")
            for plane in planes(t):
                file.write(plane)


def metric(src: Path, dec: Path, kind: str) -> float | None:
    proc = run(
        [
            "ffmpeg",
            "-nostdin",
            "-hide_banner",
            "-i",
            str(src),
            "-i",
            str(dec),
            "-lavfi",
            kind,
            "-f",
            "null",
            "-",
        ],
        capture=True,
    )
    matches = (PSNR_RE if kind == "psnr" else SSIM_RE).findall(proc.stderr)
    if not matches:
        return None
    if kind == "psnr" and matches[-1] == "inf":
        return None
    return float(matches[-1])


def sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def text(cmd: list[str]) -> str | None:
    try:
        return subprocess.run(
            cmd,
            cwd=ROOT,
            check=True,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        ).stdout.strip()
    except Exception:
        return None


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cli", required=True)
    parser.add_argument("--out-json", required=True)
    parser.add_argument("--out-csv")
    parser.add_argument("--q", type=int, default=96)
    parser.add_argument("--repeats", type=int, default=2)
    parser.add_argument("--implementation", default="canonical")
    args = parser.parse_args()

    cli = Path(args.cli).resolve()
    rows: list[dict[str, object]] = []
    kinds = ["cut", "pan", "ui", "gradient", "noise", "chroma"]
    with tempfile.TemporaryDirectory(prefix="avelune-ci-media-") as td0:
        td = Path(td0)
        for kind in kinds:
            src = td / f"{kind}.y4m"
            enc = td / f"{kind}.avl"
            dec = td / f"{kind}.decoded.y4m"
            y4m(src, kind)
            encode_samples = []
            for _ in range(args.repeats):
                start = time.perf_counter()
                run(
                    [
                        str(cli),
                        "raw",
                        "encode-y4m",
                        str(src),
                        str(enc),
                        "--q",
                        str(args.q),
                        "--preset",
                        "balanced",
                    ]
                )
                encode_samples.append(time.perf_counter() - start)
            decode_samples = []
            for _ in range(args.repeats):
                start = time.perf_counter()
                run([str(cli), "raw", "decode-y4m", str(enc), str(dec)])
                decode_samples.append(time.perf_counter() - start)
            rows.append(
                {
                    "case": kind,
                    "implementation": args.implementation,
                    "q": args.q,
                    "source_sha256": sha(src),
                    "encoded_bytes": enc.stat().st_size,
                    "encoded_sha256": sha(enc),
                    "decoded_sha256": sha(dec),
                    "psnr_db": metric(src, dec, "psnr"),
                    "ssim": metric(src, dec, "ssim"),
                    "encode_seconds_samples": encode_samples,
                    "encode_seconds_median": statistics.median(encode_samples),
                    "decode_seconds_samples": decode_samples,
                    "decode_seconds_median": statistics.median(decode_samples),
                }
            )

    meta = {
        "schema": "avelune-ci-media-v3",
        "implementation": args.implementation,
        "q": args.q,
        "repeats": args.repeats,
        "provenance": {
            "commit": text(["git", "rev-parse", "HEAD"]),
            "dirty": bool(text(["git", "status", "--porcelain"])),
            "rustc": text(["rustc", "--version"]),
            "platform": platform.platform(),
            "machine": platform.machine(),
            "cli_sha256": sha(cli),
        },
        "cases": rows,
    }
    Path(args.out_json).write_text(json.dumps(meta, indent=2) + "\n")
    if args.out_csv:
        fields = [
            "case",
            "implementation",
            "q",
            "source_sha256",
            "encoded_bytes",
            "psnr_db",
            "ssim",
            "encode_seconds_median",
            "decode_seconds_median",
        ]
        with Path(args.out_csv).open("w", newline="") as file:
            writer = csv.DictWriter(file, fieldnames=fields)
            writer.writeheader()
            writer.writerows({key: row[key] for key in fields} for row in rows)


if __name__ == "__main__":
    main()
