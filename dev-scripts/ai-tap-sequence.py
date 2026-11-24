#!/usr/bin/env python3
"""Automated tap-sequence smoke test for touchHLE/HyperHLE on Windows and Linux/X11.

Launches the emulator with --print-fps, performs a sequence of taps at
coordinates relative to the emulator window, captures screenshots and checks
that frames keep being presented and the process stays alive.

Usage:
    python dev-scripts/ai-tap-sequence.py APP [--exe PATH] [--cwd DIR] [--out DIR]
        [--step "WAIT:X,Y[*N]"]... [--final-wait SECONDS]
        [--require-touch-delivery] [--require-nonblack-frame] [--require-frame-change]
        [-- extra emulator args]

Each --step waits WAIT seconds, takes a screenshot, then taps at (X, Y),
where X and Y are fractions (0..1) of the window's client area. Use "-" for
X,Y to take a screenshot without tapping. Append `*N` for N rapid taps, e.g.
`--step "4:0.5,0.5*2"` for a double tap. `--require-touch-delivery` requires
UIKit to report normal begin/end events for each tap, up to its 12-event log cap.
`--require-nonblack-frame` rejects a black final screenshot even if the
compositor reports FPS. `--require-frame-change` requires the final screenshot
to differ from the first step screenshot, catching an unchanged splash
even when frames are still being presented. Use an early screenshot step
and disable emulator overlays and cursor effects with these image checks.
A changing image alone does not prove that a game reached its menu.
Use X,Y>END_X,END_Y@SECONDS for a held drag (SECONDS defaults to 0.5),
for example --step '3:0.6,0.5>0.3,0.7@0.5' to aim and release a shot.

Windows requires Pillow and pywin32. Linux/X11 requires xdotool, xwininfo,
and ImageMagick's import.
"""

import argparse
import re
import subprocess
import sys
import threading
import time
import os
import csv
import json
import statistics
from pathlib import Path

if sys.platform == "win32":
    import win32api
    import win32con
    import win32gui
    from PIL import ImageGrab

FPS_RE = re.compile(r"(?:EAGLContext .*|Core Animation compositor) FPS: ([0-9.]+)")
FRAME_TIMES_RE = re.compile(r"FRAME-TIMES context=(\S+) ms=([0-9.,]+)")


def find_window(process_id=None, pid_hint_title="touchHLE"):
    if sys.platform == "win32":
        found = []
        fallback = []

        def cb(hwnd, _):
            if not win32gui.IsWindowVisible(hwnd):
                return
            window_pid = win32gui.GetWindowThreadProcessId(hwnd)[1]
            if process_id is None or window_pid == process_id:
                found.append(hwnd)
            elif pid_hint_title in win32gui.GetWindowText(hwnd):
                fallback.append(hwnd)

        win32gui.EnumWindows(cb, None)
        candidates = found or fallback
        return candidates[0] if candidates else None
    if process_id is not None:
        result = subprocess.run(
            ["xdotool", "search", "--onlyvisible", "--pid", str(process_id)],
            capture_output=True,
            text=True,
        )
        ids = result.stdout.splitlines()
        if ids:
            return int(ids[0])
    result = subprocess.run(
        ["xdotool", "search", "--onlyvisible", "--name", pid_hint_title],
        capture_output=True,
        text=True,
    )
    ids = result.stdout.splitlines()
    return int(ids[0]) if ids else None


def client_rect(hwnd):
    if sys.platform == "win32":
        left, top, right, bottom = win32gui.GetClientRect(hwnd)
        x0, y0 = win32gui.ClientToScreen(hwnd, (left, top))
        x1, y1 = win32gui.ClientToScreen(hwnd, (right, bottom))
        return x0, y0, x1, y1
    result = subprocess.run(
        ["xwininfo", "-id", f"0x{hwnd:x}"], capture_output=True, text=True, check=True
    )
    width = int(re.search(r"^\s*Width:\s*(\d+)", result.stdout, re.MULTILINE).group(1))
    height = int(re.search(r"^\s*Height:\s*(\d+)", result.stdout, re.MULTILINE).group(1))
    return 0, 0, width, height


def tap(hwnd, fx, fy, tap_count=1):
    x0, y0, x1, y1 = client_rect(hwnd)
    x = int(x0 + (x1 - x0) * fx)
    y = int(y0 + (y1 - y0) * fy)
    if sys.platform == "win32":
        try:
            win32gui.SetForegroundWindow(hwnd)
        except Exception:
            pass
        win32api.SetCursorPos((x, y))
        time.sleep(0.05)
        for n in range(tap_count):
            win32api.mouse_event(win32con.MOUSEEVENTF_LEFTDOWN, 0, 0)
            time.sleep(0.06)
            win32api.mouse_event(win32con.MOUSEEVENTF_LEFTUP, 0, 0)
            if n + 1 < tap_count:
                time.sleep(0.08)
    else:
        subprocess.run(["xdotool", "windowfocus", "--sync", str(hwnd)], check=True)
        subprocess.run(
            ["xdotool", "mousemove", "--sync", "--window", str(hwnd), str(x), str(y)],
            check=True,
        )
        for n in range(tap_count):
            subprocess.run(["xdotool", "mousedown", "1"], check=True)
            time.sleep(0.06)
            subprocess.run(["xdotool", "mouseup", "1"], check=True)
            if n + 1 < tap_count:
                time.sleep(0.08)


def parse_step_position(position):
    count = 1
    if "*" in position:
        position, count_text = position.rsplit("*", 1)
        try:
            count = int(count_text)
        except ValueError as error:
            raise ValueError(f"invalid tap count in step position: {position}*{count_text}") from error
        if count < 1:
            raise ValueError("tap count must be at least 1")
    return position, count


def drag(hwnd, position):
    start, end = position.split(">", 1)
    end, _, duration_text = end.partition("@")
    duration = float(duration_text) if duration_text else 0.5
    if not 0 < duration <= 10:
        raise ValueError("drag duration must be between 0 and 10 seconds")
    fx, fy = (float(v) for v in start.split(","))
    ex, ey = (float(v) for v in end.split(","))
    if not all(0 <= v <= 1 for v in (fx, fy, ex, ey)):
        raise ValueError("drag coordinates must be fractions between 0 and 1")
    x0, y0, x1, y1 = client_rect(hwnd)

    def move(x, y):
        px, py = int(x0 + (x1 - x0) * x), int(y0 + (y1 - y0) * y)
        if sys.platform == "win32":
            win32api.SetCursorPos((px, py))
        else:
            subprocess.run(["xdotool", "mousemove", "--sync", "--window",
                            str(hwnd), str(px), str(py)], check=True)

    if sys.platform == "win32":
        win32gui.SetForegroundWindow(hwnd)
    else:
        subprocess.run(["xdotool", "windowfocus", "--sync", str(hwnd)], check=True)
    move(fx, fy)
    if sys.platform == "win32":
        win32api.mouse_event(win32con.MOUSEEVENTF_LEFTDOWN, 0, 0)
    else:
        subprocess.run(["xdotool", "mousedown", "1"], check=True)
    try:
        steps = max(2, int(duration * 30))
        for index in range(1, steps + 1):
            time.sleep(duration / steps)
            fraction = index / steps
            move(fx + (ex - fx) * fraction, fy + (ey - fy) * fraction)
    finally:
        if sys.platform == "win32":
            win32api.mouse_event(win32con.MOUSEEVENTF_LEFTUP, 0, 0)
        else:
            subprocess.run(["xdotool", "mouseup", "1"], check=True)


def screenshot(hwnd, path):
    if sys.platform == "win32":
        ImageGrab.grab(bbox=client_rect(hwnd), all_screens=True).save(path)
    else:
        subprocess.run(
            ["import", "-window", f"0x{hwnd:x}", str(path)], check=True
        )


def has_nonblack_pixels(path):
    if sys.platform == "win32":
        from PIL import Image

        with Image.open(path) as frame:
            return frame.convert("RGB").getextrema() != ((0, 0), (0, 0), (0, 0))
    result = subprocess.run(
        ["identify", "-format", "%[fx:maxima]", str(path)],
        capture_output=True, text=True, check=True,
    )
    return float(result.stdout) > 0


def frames_differ(first, final):
    if sys.platform == "win32":
        from PIL import Image, ImageChops

        with Image.open(first) as a, Image.open(final) as b:
            if a.size != b.size:
                return True
            return ImageChops.difference(a.convert("RGB"), b.convert("RGB")).getbbox() is not None
    # ImageMagick returns 0 for identical images, 1 for differing images,
    # and 2 for a failure (which must not be treated as visual progress).
    result = subprocess.run(
        ["compare", "-metric", "AE", str(first), str(final), "null:"],
        capture_output=True, text=True,
    )
    if result.returncode not in (0, 1):
        raise RuntimeError(f"could not compare screenshots: {result.stderr.strip()}")
    return result.returncode == 1


def has_frame_content(path):
    """Reject a uniform gray/black/white surface as evidence of game progress."""
    if sys.platform == "win32":
        from PIL import Image, ImageStat

        with Image.open(path) as frame:
            return max(ImageStat.Stat(frame.convert("RGB")).stddev) > 1.0
    result = subprocess.run(
        ["identify", "-format", "%[fx:standard_deviation]", str(path)],
        capture_output=True, text=True, check=True,
    )
    return float(result.stdout) > 1.0 / 255.0


def write_metrics(out, fps_samples, frame_intervals):
    with (out / "fps.csv").open("w", newline="") as stream:
        writer = csv.writer(stream)
        writer.writerow(["elapsed_seconds", "source", "fps"])
        writer.writerows(fps_samples)
    with (out / "frame-times.csv").open("w", newline="") as stream:
        writer = csv.writer(stream)
        writer.writerow(["context", "interval_ms"])
        writer.writerows(frame_intervals)
    values = sorted(ms for _, ms in frame_intervals)
    metrics = {"fps_samples": len(fps_samples), "frame_time_samples": len(values)}
    if values:
        def percentile(fraction):
            index = (len(values) - 1) * fraction
            lower = int(index)
            upper = min(lower + 1, len(values) - 1)
            return values[lower] + (values[upper] - values[lower]) * (index - lower)

        metrics.update(mean_ms=statistics.mean(values), p50_ms=percentile(0.5),
                       p95_ms=percentile(0.95), p99_ms=percentile(0.99), max_ms=max(values))
    (out / "metrics.json").write_text(json.dumps(metrics, indent=2) + "\n")
    return metrics


def wait_for_process(proc, seconds):
    deadline = time.monotonic() + seconds
    while True:
        if proc.poll() is not None:
            return False
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            break
        time.sleep(min(0.25, remaining))
    return proc.poll() is None


def main():
    argv = sys.argv[1:]
    extra = []
    if "--" in argv:
        i = argv.index("--")
        argv, extra = argv[:i], argv[i + 1:]
    p = argparse.ArgumentParser()
    p.add_argument("app")
    repo_root = Path(__file__).resolve().parent.parent
    default_exe = repo_root / "target" / "release" / (
        "touchHLE.exe" if sys.platform == "win32" else "touchHLE"
    )
    p.add_argument("--exe", default=str(default_exe))
    p.add_argument("--cwd", default=str(repo_root))
    p.add_argument("--out", default="tap-test-out")
    p.add_argument("--window-title", default="touchHLE",
                   help="Fallback window title when --exe launches an emulator child")
    p.add_argument("--step", action="append", default=[])
    p.add_argument("--final-wait", type=float, default=10.0)
    p.add_argument("--require-touch-delivery", action="store_true")
    p.add_argument("--require-nonblack-frame", action="store_true")
    p.add_argument("--require-frame-change", action="store_true")
    p.add_argument("--require-frame-content", action="store_true")
    p.add_argument("--require-eagl-frames", action="store_true")
    args = p.parse_args(argv)
    if sys.platform not in ("win32", "linux"):
        p.error("This script supports Windows and Linux/X11 hosts.")

    if args.require_frame_change and not args.step:
        p.error("--require-frame-change needs at least one --step screenshot")

    out = Path(args.out).expanduser().resolve()
    cwd = Path(args.cwd).expanduser().resolve()
    exe = Path(args.exe).expanduser().resolve()
    app = Path(args.app).expanduser().resolve()
    if not app.exists():
        p.error(f"app bundle or IPA does not exist: {app}")
    if not cwd.is_dir():
        p.error(f"working directory does not exist: {cwd}")
    if not exe.is_file():
        p.error(f"emulator executable does not exist: {exe}")
    out.mkdir(parents=True, exist_ok=True)
    log_path = out / "emulator.log"
    log = open(log_path, "w", encoding="utf-8", errors="replace")
    env = os.environ.copy()
    env["RUST_BACKTRACE"] = "full"
    if args.require_touch_delivery:
        env["TOUCHHLE_TRACE_TOUCHES"] = "1"
    try:
        proc = subprocess.Popen(
            [str(exe), str(app), "--print-fps", *extra],
            cwd=str(cwd),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            env=env,
        )
    except OSError as error:
        log.close()
        p.error(f"could not start emulator {exe}: {error}")
    fps_samples = []
    fps_rows = []
    frame_intervals = []

    start = time.time()

    def reader():
        for raw in proc.stdout:
            line = raw.decode("utf-8", "replace")
            log.write(f"[{time.time() - start:7.2f}] {line}")
            log.flush()
            m = FPS_RE.search(line)
            if m:
                fps_samples.append((time.time(), float(m.group(1))))
                source = "compositor" if "Core Animation compositor" in line else "EAGL"
                fps_rows.append((time.time() - start, source, float(m.group(1))))
            frame_times = FRAME_TIMES_RE.search(line)
            if frame_times:
                frame_intervals.extend((frame_times.group(1), float(ms))
                                       for ms in frame_times.group(2).split(","))

    reader_thread = threading.Thread(target=reader, daemon=True)
    reader_thread.start()

    hwnd = None
    for _ in range(60):
        hwnd = find_window(proc.pid, pid_hint_title=args.window_title)
        if hwnd or proc.poll() is not None:
            break
        time.sleep(0.5)
    if not hwnd:
        if proc.poll() is None:
            proc.kill()
        proc.wait()
        reader_thread.join()
        log.close()
        write_metrics(out, fps_rows, frame_intervals)
        print(
            f"FAIL: emulator window never appeared (exit code {proc.returncode}); "
            f"log: {log_path}"
        )
        for line in log_path.read_text(encoding="utf-8", errors="replace").splitlines()[-25:]:
            print(line)
        return 1

    ok = True
    first_shot = None
    for n, step in enumerate(args.step):
        wait, _, pos = step.partition(":")
        position, tap_count = parse_step_position(pos) if pos != "-" else ("-", 0)
        if not wait_for_process(proc, float(wait)):
            print(f"FAIL: emulator exited during step {n} (code {proc.returncode})")
            ok = False
            break
        if proc.poll() is not None:
            print(f"FAIL: emulator exited (code {proc.returncode}) before step {n}")
            ok = False
            break
        shot = out / f"step{n:02d}.png"
        screenshot(hwnd, shot)
        if first_shot is None:
            first_shot = shot
        if position != "-":
            if ">" in position:
                if tap_count != 1:
                    raise ValueError("drag steps cannot repeat with *N")
                drag(hwnd, position)
                print(f"step {n}: screenshot {shot.name}, drag {position}")
                continue
            fx, fy = (float(v) for v in position.split(","))
            tap(hwnd, fx, fy, tap_count)
            action = "double-tap" if tap_count == 2 else f"{tap_count} tap(s)"
            print(f"step {n}: screenshot {shot.name}, {action} ({fx:.2f},{fy:.2f})")
        else:
            print(f"step {n}: screenshot {shot.name}")

    if ok:
        if not wait_for_process(proc, args.final_wait):
            print(f"FAIL: emulator exited during final wait (code {proc.returncode})")
            ok = False
        else:
            screenshot(hwnd, out / "final.png")
            alive = proc.poll() is None
            tail = [f for t, f in fps_samples[-5:]]
            print(f"Render FPS reports: {len(fps_samples)}, last: {tail}")
            if not alive:
                print(f"FAIL: emulator exited with code {proc.returncode}")
                ok = False
            elif len(fps_samples) < 3 or all(f == 0 for f in tail):
                print("FAIL: frames are not being presented")
                ok = False
            elif time.time() - fps_samples[-1][0] > 3.0:
                print("FAIL: frame counter stopped updating before the final check")
                ok = False
            if args.require_nonblack_frame and not has_nonblack_pixels(out / "final.png"):
                print("FAIL: final frame is black despite compositor activity")
                ok = False
            if args.require_frame_content and not has_frame_content(out / "final.png"):
                print("FAIL: final frame is uniform (gray/black/white), with no game content")
                ok = False
            if args.require_frame_change:
                try:
                    changed = frames_differ(first_shot, out / "final.png")
                except RuntimeError as error:
                    print(f"FAIL: {error}")
                    ok = False
                else:
                    if not changed:
                        print("FAIL: final frame is unchanged from the first screenshot")
                        ok = False
                    else:
                        print("Visual progress: final frame differs from the first screenshot")
    if proc.poll() is None:
        proc.kill()
    proc.wait()
    reader_thread.join()
    log.close()
    metrics = write_metrics(out, fps_rows, frame_intervals)
    if frame_intervals:
        print(f"Game frame intervals (ms): {metrics}")
    log_text = log_path.read_text(encoding="utf-8", errors="replace")
    if args.require_eagl_frames and not any(source == "EAGL" for _, source, _ in fps_rows):
        print("FAIL: the game never presented EAGL frames; compositor FPS cannot prove progress")
        ok = False
    failure_patterns = (
        r"App called exit\(",
        r"Shader[^\n]*compile failed",
        r"\[--trace-gl-errors\] glGetError\(\) = 0x[0-9a-f]+",
        r"PANIC in thread",
        r"thread .* panicked",
        r"FATAL SIGNAL",
        r"Ignored UndefinedInstruction",
    )
    failures = [
        line for line in log_text.splitlines()
        if any(re.search(pattern, line, re.IGNORECASE) for pattern in failure_patterns)
    ]
    if failures:
        print("FAIL: fatal guest or OpenGL diagnostics were logged:")
        for line in failures[:10]:
            print(line)
        ok = False
    if args.require_touch_delivery:
        requested = sum(
            0 if step.partition(":")[2] == "-" else parse_step_position(step.partition(":")[2])[1]
            for step in args.step
        )
        log_text = log_path.read_text(encoding="utf-8", errors="replace")
        delivered = log_text.count("TOUCH-DIAG #")
        ended = log_text.count("TOUCH-END #")
        required = min(requested, 12)
        if requested == 0 or delivered < required or ended < required:
            print(
                f"FAIL: requested {requested} tap(s); UIKit saw {delivered} begin(s) "
                f"and {ended} end(s) (required {required})"
            )
            ok = False
        else:
            print(f"Touch delivery: {delivered} begin(s), {ended} end(s)")
    print("PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
