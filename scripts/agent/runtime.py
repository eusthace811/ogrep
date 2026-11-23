#!/usr/bin/env python3
"""Dependency-free local MCP adapter and shared ogrep service launcher (POSIX)."""
from __future__ import annotations

import argparse
import concurrent.futures
import fcntl
import fnmatch
import hashlib
import json
import math
import os
from pathlib import Path
import re
import selectors
import signal
import socket
import subprocess
import sys
import threading
import time

VERSION = "1.2.0"
INSTRUCTIONS = (
    "Prefer search_code for repository content searches and find_files for file lookup. "
    "After edits, refresh_index waits for searchable publication before later indexed queries. "
    "Use freshness=current for a direct filesystem scan. Empty results are not errors. "
    "Use shell search only if these tools are unavailable or cannot express the query. "
    "Indexed queries may lag filesystem changes. Narrow truncated queries by path or file type. "
    "Results list each path once, then its lines as line:text for matches and line-text for context."
)
COMMON = {
    "path": {"type": "string", "default": ".", "description": "Path beneath the repository root."},
    "hidden": {"type": "boolean", "default": False},
    "max_results": {"type": "integer", "minimum": 1, "maximum": 1000, "default": 100},
    "freshness": {"type": "string", "enum": ["indexed", "current"], "default": "indexed"},
}
TOOLS = [
    {
        "name": "search_code",
        "description": "Search repository contents for symbols, references, implementations or configuration. Returns paths, line numbers and context; supports literal text and regex.",
        "inputSchema": {"type": "object", "additionalProperties": False, "required": ["pattern"], "properties": {
            **COMMON,
            "pattern": {"type": "string"},
            "literal": {"type": "boolean", "default": True},
            "ignore_case": {"type": "boolean", "default": False},
            "file_types": {"type": "array", "items": {"type": "string"}},
            "glob": {"type": "array", "items": {"type": "string"}},
            "context_lines": {"type": "integer", "minimum": 0, "maximum": 10, "default": 2},
            "output_mode": {"type": "string", "enum": ["content", "files", "count"], "default": "content"},
        }},
        "annotations": {"readOnlyHint": True, "openWorldHint": False},
    },
    {
        "name": "find_files",
        "description": "Find repository files by basename or repository-relative path glob. Preserves ignore rules; use **/*.rs or *.rs for Rust files.",
        "inputSchema": {"type": "object", "additionalProperties": False, "properties": {
            **COMMON, "pattern": {"type": "string", "default": "*"},
        }},
        "annotations": {"readOnlyHint": True, "openWorldHint": False},
    },
    {
        "name": "refresh_index",
        "description": "Wait for completed edits to become searchable in the entire configured repository. Rereads eligible content and publishes the derived index. Concurrent edits may or may not appear; timeout or cancellation does not acknowledge freshness and server work may continue.",
        "inputSchema": {"type": "object", "additionalProperties": False, "properties": {
            "timeout_secs": {"type": "integer", "minimum": 1, "maximum": 300, "default": 30},
        }},
        "annotations": {"readOnlyHint": False, "destructiveHint": False, "idempotentHint": True, "openWorldHint": False},
    },
]


def git_root(cwd: Path) -> Path | None:
    cwd = cwd.resolve(strict=True)
    try:
        result = subprocess.run(["git", "-C", str(cwd), "rev-parse", "--show-toplevel"],
                                capture_output=True, text=True, timeout=3)
        if result.returncode == 0:
            return Path(result.stdout.strip()).resolve()
    except (OSError, subprocess.TimeoutExpired):
        pass
    return None


def repository(cwd: Path) -> Path:
    return git_root(cwd) or cwd.resolve(strict=True)


def reject_symlinks(path):
    """Service state must not be redirected through symlinked cache components."""
    for component in (path, *path.parents):
        if component.is_symlink():
            raise ValueError(f"Refusing symlink in service state path: {component}")


def validate(name: str, args: dict) -> dict:
    tool = next((t for t in TOOLS if t["name"] == name), None)
    if tool is None or not isinstance(args, dict):
        raise ValueError("Unknown tool or invalid arguments")
    schema = tool["inputSchema"]
    if set(args) - set(schema["properties"]):
        raise ValueError("Unknown argument")
    if any(k not in args for k in schema.get("required", [])):
        raise ValueError("Missing required pattern")
    result = {}
    for key, prop in schema["properties"].items():
        if key not in args and "default" not in prop:
            continue
        value = args.get(key, prop.get("default"))
        expected = {"string": str, "boolean": bool, "integer": int, "array": list}[prop["type"]]
        if type(value) is not expected:
            raise ValueError(f"Invalid type for {key}")
        if isinstance(value, str) and ("\0" in value or len(value) > 16384):
            raise ValueError(f"Invalid string for {key}")
        if isinstance(value, list):
            if len(value) > 100 or any(type(v) is not str or "\0" in v or len(v) > 16384 for v in value):
                raise ValueError(f"Invalid list for {key}")
            if sum(len(v) for v in value) > 65536:
                raise ValueError(f"List too large for {key}")
        if "enum" in prop and value not in prop["enum"]:
            raise ValueError(f"Invalid value for {key}")
        if "minimum" in prop and not prop["minimum"] <= value <= prop["maximum"]:
            raise ValueError(f"Out of range: {key}")
        result[key] = value
    return result


def reap_process(proc):
    try:
        if proc.poll() is None:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except PermissionError:
                # macOS returns EPERM for a process group whose members are exiting.
                # Ignore it only if our child then finishes.
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    pass
                if proc.poll() is None:
                    raise
        proc.wait()
    finally:
        proc.stdout.close()
        proc.stderr.close()


class Runtime:
    def __init__(self, config: dict, cwd: Path | None = None):
        self.config = config
        self.root = Path(config["root"]).resolve() if config.get("root") else repository(cwd or Path.cwd())
        self.binary = config["binary"]
        self.flags = list(config.get("index_flags", []))
        # Apply the installer's .gitignore exclusion in plain directories too.
        # Resolve per session so user-scope installs handle both Git and non-Git.
        if git_root(self.root) is None and "--no-require-git" not in self.flags:
            self.flags.append("--no-require-git")
        identity = json.dumps([str(self.root), self.flags], sort_keys=True).encode()
        cache = Path(config["cache_dir"])
        self.state = cache / hashlib.sha256(identity).hexdigest()[:24]
        self.index = self.state / "index"
        reject_symlinks(self.state)
        reject_symlinks(self.index)

    def status(self):
        try:
            info = json.loads((self.index / "serve.json").read_text())
            if (not isinstance(info, dict) or type(info.get("port")) is not int or type(info.get("pid")) is not int
                    or info["pid"] <= 0):
                return None
            try:
                # A stale record can outlive its server while another process
                # reuses the port; only trust a server we are still running.
                os.kill(info["pid"], 0)
            except OSError:
                return None
            with socket.create_connection(("127.0.0.1", info["port"]), timeout=0.3) as sock:
                sock.sendall(b'{"jsonrpc":"2.0","id":1,"method":"status"}\n')
                with sock.makefile("rb") as stream:
                    reply = json.loads(stream.readline(65536))
        except (OSError, ValueError, KeyError, TypeError):
            return None
        if not isinstance(reply, dict) or reply.get("jsonrpc") != "2.0":
            return None
        error = reply.get("error")
        if (reply.get("id") in (None, 1) and isinstance(error, dict)
                and error.get("code") == -32002):
            # Explicit overload is not an unavailable service. Let the tool
            # report it instead of launching a replacement or a full scan.
            raise ValueError("ogrep server busy; retry later")
        if reply.get("id") != 1:
            return None
        result = reply.get("result")
        if not isinstance(result, dict) or type(result.get("num_files")) is not int:
            return None
        return result

    def ensure_server(self, wait=1.5):
        if self.status() is not None:
            return True
        self.state.mkdir(parents=True, exist_ok=True)
        # The server's own serve.lock is the final authority. This lock avoids
        # spawning many competing children before serve.json has been published.
        with (self.state / "start.lock").open("a") as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return False
            if self.status() is not None:
                return True
            # Keep the lock inherited by the child until it exits. Even slow
            # initialization cannot trigger repeated launches from other sessions.
            with (self.state / "serve.log").open("ab") as log:
                child = subprocess.Popen(
                    [self.binary, "serve", str(self.root), "--index-path", str(self.index), *self.flags],
                    cwd=self.root, stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                    start_new_session=True, pass_fds=(lock.fileno(),),
                )
            threading.Thread(target=child.wait, daemon=True).start()
            deadline = time.monotonic() + wait
            while time.monotonic() < deadline:
                if self.status() is not None:
                    return True
                if child.poll() is not None:
                    break
                time.sleep(0.05)
        return False

    def refresh(self, args, cancel):
        deadline = time.monotonic() + args["timeout_secs"]

        def check_wait():
            if cancel.is_set():
                raise ValueError("Refresh cancelled; server work may continue; freshness was not acknowledged")
            if time.monotonic() >= deadline:
                raise ValueError("Refresh timed out; server work may continue; freshness was not acknowledged")

        # The launcher may still hold start.lock before discovery is published.
        # Keep waiting within this call's budget; readiness alone is no receipt.
        check_wait()
        ready = self.ensure_server(wait=0)
        while not ready:
            check_wait()
            cancel.wait(min(0.05, max(0, deadline - time.monotonic())))
            check_wait()
            ready = self.status() is not None
        check_wait()
        cmd = [self.binary, "refresh", str(self.root), "--index-path", str(self.index),
               "--json", "--timeout-secs", str(max(1, math.ceil(deadline - time.monotonic())))]
        # CLI seconds round up; this adapter enforces the original total budget,
        # including startup and cancellation, while draining bounded output.
        check_wait()
        proc = subprocess.Popen(cmd, cwd=self.root, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                start_new_session=True)
        output, warnings = bytearray(), bytearray()
        try:
            with selectors.DefaultSelector() as selector:
                selector.register(proc.stdout, selectors.EVENT_READ, "out")
                selector.register(proc.stderr, selectors.EVENT_READ, "err")
                while selector.get_map():
                    check_wait()
                    for key, _ in selector.select(min(0.1, max(0, deadline - time.monotonic()))):
                        chunk = os.read(key.fileobj.fileno(), 8192)
                        if not chunk:
                            selector.unregister(key.fileobj)
                        elif key.data == "err":
                            warnings.extend(chunk[:max(0, 4000 - len(warnings))])
                        else:
                            output.extend(chunk)
                            if len(output) > 65536:
                                raise ValueError("Refresh receipt exceeded 64 KiB")
            # A child can close its pipes without exiting. Keep cancellation
            # responsive until it actually terminates as well.
            while proc.poll() is None:
                check_wait()
                cancel.wait(min(0.05, max(0, deadline - time.monotonic())))
            check_wait()
            if proc.returncode != 0:
                raise ValueError(warnings.decode(errors="replace") or f"Refresh failed with exit {proc.returncode}")
            try:
                receipt = json.loads(output)
            except (ValueError, UnicodeError) as exc:
                raise ValueError("Invalid refresh receipt") from exc
            if (not isinstance(receipt, dict) or receipt.get("status") != "refreshed"
                    or receipt.get("root") != str(self.root)
                    or type(receipt.get("elapsed_ms")) is not int or receipt["elapsed_ms"] < 0
                    or type(receipt.get("num_files")) is not int or receipt["num_files"] < 0):
                raise ValueError("Invalid refresh receipt for this repository")
        finally:
            reap_process(proc)
        return {**receipt, "refresh_mode": "content_reconcile", "warnings": warnings.decode(errors="replace")}

    def command(self, name, args):
        path = (self.root / args["path"]).resolve(strict=True)
        if not path.is_relative_to(self.root):
            raise ValueError("Search path must remain beneath the repository root")
        cmd = [self.binary, "--index-path", str(self.index), *self.flags, "--color", "never"]
        current = args["freshness"] == "current"
        if not current:
            try:
                ready = self.ensure_server()
            except OSError:
                ready = False
            # Never use an orphaned on-disk index when startup failed.
            current = not ready
        if current:
            cmd.append("--no-index")
        if args["hidden"]:
            cmd.append("--hidden")
        if name == "find_files":
            cmd += ["--files", "--null", str(path)]
        else:
            if args["literal"]:
                cmd.append("-F")
            if args["ignore_case"]:
                cmd.append("-i")
            for file_type in args.get("file_types", []):
                cmd.append("--type=" + file_type)
            for glob in args.get("glob", []):
                cmd.append("--glob=" + glob)
            mode = args["output_mode"]
            # One result past the cap lets `run` see that output was truncated,
            # while ogrep stops reading files once it has that many.
            stop = ["--max-results", str(args["max_results"] + 1)]
            if mode == "content":
                cmd += ["--json", "-C", str(args["context_lines"]), *stop]
            elif mode == "files":
                cmd += ["-l", "--null", *stop]
            else:
                # CLI count output cannot escape arbitrary filenames. JSON end
                # records provide matched-line counts with unambiguous paths.
                cmd += ["--json", "-C", "0"]
            cmd += ["--", args["pattern"], str(path)]
        # CLI can also scan for positive glob overrides or incomplete coverage.
        mode = "current_scan" if current else "indexed_or_scan"
        return cmd, mode

    def search(self, name, supplied, cancel=None):
        """One tool call as its MCP result. The text is all a model reads: hosts such as Codex show a model
        `structuredContent` instead of the text whenever it is present, so the result carries none."""
        payload = self.run(name, supplied, cancel)
        text = json.dumps(payload, ensure_ascii=False) if name == "refresh_index" else render(payload)
        return {"content": [{"type": "text", "text": text}], "isError": False}

    def run(self, name, supplied, cancel=None):
        args = validate(name, supplied)
        cancel = cancel or threading.Event()
        if name == "refresh_index":
            return self.refresh(args, cancel)
        if cancel.is_set():
            raise ValueError("Search cancelled")
        cmd, mode = self.command(name, args)
        records, warnings = [], bytearray()
        truncated = False
        byte_budget = 48000
        total_bytes = 0
        files_mode = name == "find_files" or args.get("output_mode") == "files"
        delimiter = b"\0" if files_mode else b"\n"
        proc = subprocess.Popen(cmd, cwd=self.root, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                start_new_session=True)
        pending = bytearray()

        def record(raw):
            nonlocal total_bytes, truncated
            text = raw.decode("utf-8", errors="replace")
            if not text:
                return
            if files_mode:
                resolved = Path(text)
                if not resolved.is_absolute():
                    resolved = self.root / resolved
                if not resolved.resolve().is_relative_to(self.root):
                    return
                text = str(resolved.relative_to(self.root))
                pattern = args.get("pattern", "*")
                if name == "find_files" and not (fnmatch.fnmatchcase(text, pattern) or fnmatch.fnmatchcase(Path(text).name, pattern)):
                    return
                item = {"path": text}
            else:
                event = json.loads(text)
                count_mode = args.get("output_mode") == "count"
                if event["type"] not in (("end",) if count_mode else ("match", "context")):
                    return
                data = event["data"]
                item = {"path": data["path"]["text"]}
                if count_mode:
                    item["count"] = data["stats"]["matched_lines"]
                    if not item["count"]:
                        return
                else:
                    item.update({"line": data["line_number"],
                                 "text": data["lines"]["text"].rstrip("\n"), "kind": event["type"]})
                p = Path(item["path"])
                if p.is_absolute():
                    item["path"] = str(p.relative_to(self.root))
            size = len(json.dumps(item, ensure_ascii=False).encode())
            if len(records) >= args["max_results"] or total_bytes + size > byte_budget:
                truncated = True
                return
            total_bytes += size
            records.append(item)

        try:
            deadline = time.monotonic() + 30
            with selectors.DefaultSelector() as selector:
                selector.register(proc.stdout, selectors.EVENT_READ, "out")
                selector.register(proc.stderr, selectors.EVENT_READ, "err")
                while selector.get_map() and not truncated:
                    if cancel.is_set():
                        raise ValueError("Search cancelled")
                    if time.monotonic() > deadline:
                        raise ValueError("Search timed out after 30 seconds; narrow the path")
                    for key, _ in selector.select(0.1):
                        chunk = os.read(key.fileobj.fileno(), 8192)
                        if not chunk:
                            selector.unregister(key.fileobj)
                            continue
                        if key.data == "err":
                            warnings.extend(chunk[:max(0, 4000 - len(warnings))])
                            continue
                        pending.extend(chunk)
                        while delimiter in pending and not truncated:
                            raw, _, rest = pending.partition(delimiter)
                            pending[:] = rest
                            record(raw)
                        if len(pending) > 1024 * 1024:
                            truncated = True
                if pending and not truncated:
                    record(pending)
            if not truncated:
                code = proc.wait(timeout=max(0.1, deadline - time.monotonic()))
                if code not in (0, 1):
                    raise ValueError(warnings.decode(errors="replace") or f"ogrep exited with {code}")
        finally:
            reap_process(proc)
        payload = {"root": str(self.root), "results": records, "truncated": truncated,
                   "search_mode": mode, "warnings": warnings.decode(errors="replace")}
        if truncated:
            payload["hint"] = "Output limited; narrow path, file_types or pattern. Context rows count toward max_results."
        return payload


def render(payload):
    """Search results as text, grouped the way `rg --heading` prints them: each path once, then
    `line:text` for a match and `line-text` for context; `path:count` in count mode. The same results
    cost about half the tokens of one JSON record per line, which repeats every key and path."""
    out = [f"# root: {payload['root']}  mode: {payload['search_mode']}"]
    last = None
    for item in payload["results"]:
        path = show_path(item["path"])
        if "line" in item:
            if item["path"] != last:
                out.append(path)
                last = item["path"]
            out.append(f"{item['line']}{':' if item['kind'] == 'match' else '-'}{item['text']}")
        elif "count" in item:
            out.append(f"{path}:{item['count']}")
        else:
            out.append(path)
    if not payload["results"]:
        out.append("# no results")
    if payload["truncated"]:
        out.append("# truncated: " + payload["hint"])
    if payload["warnings"].strip():
        out.append("# warnings: " + " ".join(payload["warnings"].split()))
    return "\n".join(out)


def show_path(path):
    """A path as one unambiguous line: quoted when it could read as a row, a comment or two lines."""
    if (path != path.strip() or path.startswith("#") or re.match(r"\d+[:-]", path)
            or any(ord(c) < 32 for c in path)):
        return json.dumps(path, ensure_ascii=False)
    return path


def mcp(runtime):
    write_lock = threading.Lock()
    active_lock = threading.Lock()
    active = {}

    def terminate(_signum, _frame):
        # Unwind the executor so active CLI children are cancelled and reaped.
        raise SystemExit(0)

    signal.signal(signal.SIGTERM, terminate)

    def send(message):
        with write_lock:
            print(json.dumps({"jsonrpc": "2.0", **message}), flush=True)

    def search(request, cancel):
        try:
            params = request.get("params", {})
            result = runtime.search(params.get("name"), params.get("arguments", {}), cancel)
        except Exception as exc:
            result = {"content": [{"type": "text", "text": str(exc)}], "isError": True}
        finally:
            with active_lock:
                active.pop(request["id"], None)
        send({"id": request["id"], "result": result})

    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        try:
            for line in sys.stdin:
                try:
                    try:
                        request = json.loads(line)
                    except json.JSONDecodeError:
                        send({"id": None, "error": {"code": -32700, "message": "Parse error"}})
                        continue
                    if not isinstance(request, dict) or request.get("jsonrpc") != "2.0":
                        raise ValueError("Invalid JSON-RPC request")
                    method = request.get("method")
                    if method == "notifications/cancelled":
                        with active_lock:
                            event = active.get(request.get("params", {}).get("requestId"))
                            if event:
                                event.set()
                        continue
                    if "id" not in request:
                        continue
                    request_id = request["id"]
                    if type(request_id) not in (int, str):
                        raise ValueError("Invalid request ID")
                    if method == "initialize":
                        requested = request.get("params", {}).get("protocolVersion")
                        version = requested if requested in ("2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25") else "2025-11-25"
                        result = {"protocolVersion": version, "capabilities": {"tools": {}},
                                  "serverInfo": {"name": "ogrep", "version": VERSION}, "instructions": INSTRUCTIONS}
                    elif method == "ping":
                        result = {}
                    elif method == "tools/list":
                        result = {"tools": TOOLS}
                    elif method == "tools/call":
                        with active_lock:
                            if request_id in active or len(active) >= 16:
                                send({"id": request_id, "error": {"code": -32600, "message": "Duplicate ID or too many requests"}})
                                continue
                            cancel = active[request_id] = threading.Event()
                        pool.submit(search, request, cancel)
                        continue
                    else:
                        send({"id": request_id, "error": {"code": -32601, "message": "Method not found"}})
                        continue
                    send({"id": request_id, "result": result})
                except (ValueError, TypeError, AttributeError) as exc:
                    send({"id": None, "error": {"code": -32600, "message": str(exc)}})
        finally:
            with active_lock:
                for cancel in active.values():
                    cancel.set()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=["mcp", "ensure", "probe"])
    parser.add_argument("--config", required=True)
    args = parser.parse_args()
    runtime = Runtime(json.loads(Path(args.config).read_text()))
    if args.action == "mcp":
        mcp(runtime)
    elif args.action == "ensure":
        if not runtime.ensure_server():
            print(f"ogrep warming up or unavailable; queries can scan. Log: {runtime.state / 'serve.log'}", file=sys.stderr)
    else:
        print(json.dumps({"root": str(runtime.root), "index": str(runtime.index), "status": runtime.status()}))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError) as exc:
        print(f"ogrep-agent: {exc}", file=sys.stderr)
        sys.exit(1)
