"""Extract the distributable and verify it works without workspace files."""
import asyncio
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import zipfile

import httpx
import websockets


async def main():
    root = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory(prefix="rustlink-package-") as temporary:
        folder = Path(temporary)
        with zipfile.ZipFile(root / "dist/RustLink-windows-x64.zip") as archive:
            archive.extractall(folder)
        with zipfile.ZipFile(folder / "source.zip") as archive:
            for required in ("Cargo.toml", "Cargo.lock", "src/main.rs", "web/src/browser.rs", "tests/fixtures/oracle.json"):
                assert required in archive.namelist(), required
        data = folder / "isolated-data"
        data.mkdir()
        (data / "config.json").write_bytes((folder / "config.simulated.json").read_bytes())
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        processes = []
        try:
            for command in (
                [str(folder / "simulator.exe")],
                [str(folder / "rustlink.exe"), "--headless", "--strict-port", "--auto-start", "--port", str(port), "--data-dir", str(data)],
            ):
                processes.append(subprocess.Popen(command, cwd=folder, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0)))
            async with httpx.AsyncClient(base_url=f"http://127.0.0.1:{port}", timeout=3) as client:
                for _ in range(100):
                    try:
                        response = await client.get("/api/health")
                        response.raise_for_status()
                        break
                    except httpx.HTTPError:
                        await asyncio.sleep(.1)
                else:
                    raise AssertionError("Packaged gateway did not start")
                assert "text/html" in (await client.get("/")).headers["content-type"]
                assert (await client.get("/pkg/rustlink_web_bg.wasm")).content[:4] == b"\0asm"
                seen = set()
                async with websockets.connect(f"ws://127.0.0.1:{port}/ws/data") as stream:
                    async with asyncio.timeout(10):
                        async for message in stream:
                            event = json.loads(message)
                            if event.get("type") == "data" and all(p["quality"] == "good" for p in event["values"].values()):
                                seen.add(event["device_id"])
                                if len(seen) == 3:
                                    break
                status = (await client.get("/api/collect/status")).json()
                assert status["data"]["mqtt_connected"]
                assert all(process.poll() is None for process in processes)
                print("PASS: extracted package serves embedded HTML/WASM, reads all three simulated PLCs, connects MQTT and contains corresponding source")
        finally:
            for process in reversed(processes):
                if process.poll() is None:
                    process.terminate()
                process.wait(timeout=10)


asyncio.run(main())
