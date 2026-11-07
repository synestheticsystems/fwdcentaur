#!/usr/bin/env python3
"""Serve local WASM assets and only the configured project's WebSocket through Caddy.

No fwd process, database, project registration, or daemon control endpoint is started here.
One JSON file configures both the browser and proxy. Only this service needs restarting.
"""

import argparse
import ipaddress
import json
import os
from pathlib import Path
import subprocess
from urllib.parse import urlsplit
import uuid
from settings import validate_settings


def endpoint(value, *, loopback=False):
    host = ipaddress.ip_address(value["host"])
    if loopback and not host.is_loopback:
        raise ValueError("The wallboard must listen on a loopback IP address")
    port = value["port"]
    if type(port) is not int or not 1024 <= port <= 65535:
        raise ValueError("Ports must be integers from 1024 to 65535")
    return f"[{host}]:{port}" if host.version == 6 else f"{host}:{port}"


def configuration(settings, dist):
    project = str(uuid.UUID(settings["project_id"]))
    listen = endpoint(settings["listen"], loopback=True)
    upstream = endpoint(settings["upstream"])
    origin = urlsplit(settings["daemon_origin"])
    if (origin.scheme != "http" or origin.username or origin.password
            or origin.path not in ("", "/") or origin.query or origin.fragment
            or not origin.hostname or not ipaddress.ip_address(origin.hostname).is_loopback
            or not origin.port):
        raise ValueError("daemon_origin must be the unchanged daemon's http://loopback-IP:port")
    authority = origin.netloc
    daemon_origin = f"http://{authority}"
    public = json.dumps({**validate_settings(settings), "project_id": project})
    browser_origin = f"http://{listen}"
    local_host = str(ipaddress.ip_address(settings["listen"]["host"]))
    assets = sorted("/" + path.relative_to(dist).as_posix() for path in dist.rglob("*") if path.is_file())
    if "/index.html" not in assets:
        raise ValueError(f"Missing built wallboard: {dist / 'index.html'}")
    return {
        "admin": {"disabled": True, "config": {"persist": False}},
        "storage": {"module": "file_system", "root": str(dist.parent / "runtime" / "caddy-data")},
        "apps": {"http": {"servers": {"wallboard": {
            "listen": [listen],
            "automatic_https": {"disable": True},
            "routes": [
                {
                    "match": [{"host": [local_host], "method": ["GET"], "path": ["/wallboard-config.json"]}],
                    "handle": [{"handler": "static_response", "status_code": 200,
                                "headers": {"Content-Type": ["application/json"], "Cache-Control": ["no-store"]},
                                "body": public}],
                    "terminal": True,
                },
                {
                    "match": [{"host": [local_host], "method": ["GET"], "path": [f"/p/{project}/ws"],
                               "header": {"Origin": [browser_origin], "Upgrade": ["websocket"]}}],
                    "handle": [{"handler": "reverse_proxy", "upstreams": [{"dial": upstream}],
                                "headers": {"request": {"set": {"Host": [authority], "Origin": [daemon_origin]}}}}],
                    "terminal": True,
                },
                {
                    "match": [{"host": [local_host], "method": ["GET", "HEAD"], "path": ["/"] + assets}],
                    "handle": [
                        {"handler": "headers", "response": {"set": {"Cache-Control": ["no-cache"]}}},
                        {"handler": "file_server", "root": str(dist), "index_names": ["index.html"]},
                    ],
                    "terminal": True,
                },
                {"handle": [{"handler": "static_response", "status_code": 404}]},
            ],
        }}}},
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, default=Path(__file__).with_name("wallboard.json"))
    parser.add_argument("--caddy", default="caddy")
    parser.add_argument("--check", action="store_true", help="Validate generated Caddy config without listening")
    args = parser.parse_args()
    root = args.config.resolve().parent
    settings = json.loads(args.config.read_text())
    config = configuration(settings, root / "dist")
    runtime = root / "runtime"
    runtime.mkdir(exist_ok=True)
    generated = runtime / "caddy.json"
    generated.write_text(json.dumps(config, indent=2) + "\n")
    if args.check:
        subprocess.run([args.caddy, "validate", "--config", str(generated)], check=True)
    else:
        os.execvp(args.caddy, [args.caddy, "run", "--config", str(generated)])


if __name__ == "__main__":
    main()
