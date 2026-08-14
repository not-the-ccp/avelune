#!/usr/bin/env python3
"""Generate reproducible canonical ALV1 size/quality curves.

The source is the reconstructed video in web/player/demo.avl. This intentionally
measures encoder policy on one identical Y4M source rather than comparing the
original demo's historical encoder settings.
"""
from __future__ import annotations

import argparse
import csv
import json
import math
import os
from pathlib import Path
import re
import shutil
import statistics
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
PSNR_RE = re.compile(r"average:([^ ]+)")
SSIM_RE = re.compile(r"All:([0-9.]+)")


def run(cmd: list[str], *, capture: bool = False) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        cmd,
        cwd=ROOT,
        check=True,
        text=True,
        stdout=subprocess.PIPE if capture else subprocess.DEVNULL,
        stderr=subprocess.PIPE if capture else subprocess.DEVNULL,
    )


def metric(source: Path, decoded: Path, kind: str) -> float:
    proc = run(
        ["ffmpeg", "-nostdin", "-hide_banner", "-i", str(source), "-i", str(decoded),
         "-lavfi", kind, "-f", "null", "-"],
        capture=True,
    )
    text = proc.stderr
    if kind == "psnr":
        matches = PSNR_RE.findall(text)
        if not matches:
            raise RuntimeError("FFmpeg PSNR summary missing")
        value = matches[-1]
        return math.inf if value == "inf" else float(value)
    matches = SSIM_RE.findall(text)
    if not matches:
        raise RuntimeError("FFmpeg SSIM summary missing")
    return float(matches[-1])


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out-json", default="results/encoder-curves.json")
    ap.add_argument("--out-csv", default="results/encoder-curves.csv")
    ap.add_argument("--repeats", type=int, default=3)
    ap.add_argument("--q", type=int, action="append", dest="qs")
    args = ap.parse_args()
    if args.repeats < 1:
        raise SystemExit("--repeats must be >=1")
    qs = args.qs or [1, 48, 96, 192]
    if not shutil.which("ffmpeg"):
        raise SystemExit("ffmpeg is required")

    run(["cargo", "build", "-p", "avelune-cli", "--release", "--locked"])
    cli = ROOT / "target/release/avelune"
    demo = ROOT / "web/player/demo.avl"
    out_json = ROOT / args.out_json
    out_csv = ROOT / args.out_csv
    out_json.parent.mkdir(parents=True, exist_ok=True)
    out_csv.parent.mkdir(parents=True, exist_ok=True)

    rows: list[dict[str, object]] = []
    with tempfile.TemporaryDirectory(prefix="avelune-curves-") as td_s:
        td = Path(td_s)
        source = td / "source.y4m"
        run([str(cli), "raw", "decode-y4m", str(demo), str(source)])
        for q in qs:
            encoded = td / f"canonical-q{q}.avl"
            decoded = td / f"canonical-q{q}.y4m"
            durations: list[float] = []
            for _ in range(args.repeats):
                started = time.perf_counter()
                run([str(cli), "raw", "encode-y4m", str(source), str(encoded), "--q", str(q), "--preset", "balanced"])
                durations.append(time.perf_counter() - started)
            run([str(cli), "raw", "decode-y4m", str(encoded), str(decoded)])
            psnr = metric(source, decoded, "psnr")
            ssim = metric(source, decoded, "ssim")
            rows.append({
                "implementation": "canonical",
                "q": q,
                "bytes": encoded.stat().st_size,
                "encode_seconds_median": statistics.median(durations),
                "encode_seconds_samples": durations,
                "psnr_average_db": None if math.isinf(psnr) else psnr,
                "lossless_psnr": math.isinf(psnr),
                "ssim_all": ssim,
            })
            print(
                f"canonical q={q:<3d} bytes={encoded.stat().st_size:<8d} "
                f"median={statistics.median(durations):.4f}s "
                f"PSNR={'inf' if math.isinf(psnr) else f'{psnr:.6f}'} SSIM={ssim:.6f}"
            )

    meta = {
        "source": "web/player/demo.avl reconstructed video",
        "qsteps": qs,
        "repeats": args.repeats,
        "timing": "median wall-clock time of release CLI raw encode-y4m; process launch included",
        "quality": "FFmpeg PSNR and SSIM against one common reconstructed Y4M source",
        "results": rows,
    }
    out_json.write_text(json.dumps(meta, indent=2) + "\n")
    with out_csv.open("w", newline="") as f:
        fields = ["implementation", "q", "bytes", "encode_seconds_median", "psnr_average_db", "lossless_psnr", "ssim_all"]
        writer = csv.DictWriter(f, fieldnames=fields)
        writer.writeheader()
        for row in rows:
            writer.writerow({k: row[k] for k in fields})
    print(f"wrote {out_json.relative_to(ROOT)} and {out_csv.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
