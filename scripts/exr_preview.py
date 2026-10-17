#!/usr/bin/env python3
import argparse
from pathlib import Path

import imageio.v3 as iio
import numpy as np
import OpenEXR


def read_rgb(exr_path: Path) -> np.ndarray:
    part = OpenEXR.File(str(exr_path)).parts[0]
    channels = part.channels

    if "RGBA" in channels:
        data = np.asarray(channels["RGBA"].pixels, dtype=np.float32)[..., :3]
        return data

    rgb = []
    for name in ("R", "G", "B"):
        if name not in channels:
            raise RuntimeError(f"Missing channel '{name}' and no 'RGBA' channel found")
        rgb.append(np.asarray(channels[name].pixels, dtype=np.float32))
    return np.stack(rgb, axis=-1)


def tonemap(rgb: np.ndarray, exposure_stops: float, gamma: float, percentile: float) -> np.ndarray:
    x = np.maximum(rgb, 0.0)

    p = float(np.percentile(x, percentile))
    if p <= 1e-8:
        scale = 1.0
    else:
        scale = (2.0 ** exposure_stops) / p

    x = x * scale
    x = x / (1.0 + x)
    x = np.power(np.clip(x, 0.0, 1.0), 1.0 / gamma)

    return (x * 255.0 + 0.5).astype(np.uint8), p, scale


def main() -> None:
    parser = argparse.ArgumentParser(description="Convert EXR to a viewable PNG preview")
    parser.add_argument("input", type=Path, help="Input .exr file")
    parser.add_argument("-o", "--output", type=Path, default=None, help="Output .png path")
    parser.add_argument("--exposure", type=float, default=2.0, help="Exposure in stops (default: 2.0)")
    parser.add_argument("--gamma", type=float, default=2.2, help="Display gamma (default: 2.2)")
    parser.add_argument("--percentile", type=float, default=99.0, help="Auto exposure percentile (default: 99.0)")
    args = parser.parse_args()

    in_path = args.input
    if not in_path.exists():
        raise FileNotFoundError(f"Input EXR not found: {in_path}")

    out_path = args.output or in_path.with_name(in_path.stem + "_preview.png")

    rgb = read_rgb(in_path)
    png, p, scale = tonemap(rgb, args.exposure, args.gamma, args.percentile)

    out_path.parent.mkdir(parents=True, exist_ok=True)
    iio.imwrite(str(out_path), png)

    print(f"Wrote: {out_path}")
    print(f"Stats: min={float(rgb.min()):.6g} max={float(rgb.max()):.6g} mean={float(rgb.mean()):.6g}")
    print(f"Auto exposure: p{args.percentile}={p:.6g}, scale={scale:.6g}")


if __name__ == "__main__":
    main()
