import json
import subprocess
import sys
import time

REPO = "/home/wak/Cinna/cinnabar"
MODS = "/tmp/cl-mods"
PRELOAD = "/home/wak/Cinna/cinnaloader/cinnaloader/target/debug/libcinnaloader_preload.so"


class Mcp:
    def __init__(self):
        self.proc = subprocess.Popen(
            [f"{REPO}/target/debug/cinnabar-mcp", "--repo", REPO],
            cwd=REPO,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
        )
        self.next_id = 1
        self.call(
            "initialize",
            {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "cinnaloader-driver", "version": "1"},
            },
        )
        self.notify("notifications/initialized", {})

    def notify(self, method, params):
        self.proc.stdin.write(
            json.dumps({"jsonrpc": "2.0", "method": method, "params": params}) + "\n"
        )
        self.proc.stdin.flush()

    def call(self, method, params, timeout=180):
        request_id = self.next_id
        self.next_id += 1
        payload = {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}
        self.proc.stdin.write(json.dumps(payload) + "\n")
        self.proc.stdin.flush()
        deadline = time.time() + timeout
        while time.time() < deadline:
            line = self.proc.stdout.readline()
            if not line:
                raise SystemExit("mcp server closed")
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                continue
            if message.get("id") == request_id:
                if "error" in message:
                    raise SystemExit(f"{method} failed: {message['error']}")
                return message["result"]
        raise SystemExit(f"{method} timed out")

    def tool(self, name, args, timeout=180):
        result = self.call("tools/call", {"name": name, "arguments": args}, timeout)
        content = result.get("content", [])
        text = "\n".join(part.get("text", "") for part in content if part.get("type") == "text")
        if result.get("isError"):
            raise SystemExit(f"{name} failed: {text}")
        return text


def launch(mcp, args=None, env=None):
    environment = {
        "LD_PRELOAD": PRELOAD,
        "CINNALOADER_DIR": MODS,
        "CINNALOADER_MENU_OPEN": "1",
    }
    environment.update(env or {})
    text = mcp.tool(
        "launch_client",
        {
            "headless": True,
            "size": [1280, 720],
            "args": args
            or ["--address", "127.0.0.1:19132", "--display-name", "Tester", "--no-vsync"],
            "env": environment,
            "timeout_ms": 180000,
        },
    )
    print("launch:", text.replace("\n", " ")[:200], flush=True)


def main():
    commands = sys.argv[1:]
    mcp = Mcp()
    try:
        for command in commands:
            kind, _, rest = command.partition(":")
            if kind == "launch":
                launch(mcp)
            elif kind == "shot":
                print("shot:", mcp.tool("screenshot", {"path": rest})[:200], flush=True)
            elif kind == "click":
                x, y = rest.split(",")
                mcp.tool("input", {"pointer": {"x": float(x), "y": float(y)}})
                time.sleep(0.3)
                mcp.tool("input", {"press": ["MouseLeft"], "press_frames": 3})
                print(f"clicked {x},{y}", flush=True)
            elif kind == "key":
                mcp.tool("input", {"press": [rest], "press_frames": 3})
                print("key:", rest, flush=True)
            elif kind == "sleep":
                time.sleep(float(rest))
            elif kind == "quit":
                mcp.tool("quit", {}, timeout=60)
            else:
                raise SystemExit(f"unknown command {command}")
    finally:
        try:
            mcp.proc.terminate()
        except Exception:
            pass


if __name__ == "__main__":
    main()
