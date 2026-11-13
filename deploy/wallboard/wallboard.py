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
import sys
from urllib.parse import urlsplit
import uuid
from settings import validate_settings

# The project's socket on the daemon, as crates/fwd-core/src/daemon_socket.rs names it for the
# daemon and the page, with the `/ws` Ankurah's connector appends. A daemon that keeps every
# project in one store refuses the path the daemons before it used, `/p/<project>/ws`.
SOCKET_PATH = "/p/{project}/one-store/ws"


def given(value, name, where=""):
    """The field `name` of `value`, or a refusal naming it: the launcher has no default for it."""
    if type(value) is not dict or name not in value:
        raise ValueError(f"{where}{name} is missing; the launcher has no default for it (README.md, Configuring)")
    return value[name]


def endpoint(settings, name, *, loopback=False):
    value = given(settings, name)
    host = ipaddress.ip_address(given(value, "host", f"{name}."))
    if loopback and not host.is_loopback:
        raise ValueError("The wallboard must listen on a loopback IP address")
    port = given(value, "port", f"{name}.")
    if type(port) is not int or not 1024 <= port <= 65535:
        raise ValueError(f"{name}.port must be an integer from 1024 to 65535")
    return f"[{host}]:{port}" if host.version == 6 else f"{host}:{port}"


def project_uuid(settings):
    """The UUID of the project to show. A project_id that is not one, as a string, is refused by name."""
    value = given(settings, "project_id")
    # uuid.UUID refuses a malformed string with ValueError, but a value of another type with errors
    # the launcher does not catch, so the type is checked first.
    if type(value) is str:
        try:
            return str(uuid.UUID(value))
        except ValueError:
            pass
    raise ValueError("project_id must be the project's UUID, as a string")


def configuration(settings, dist):
    project = project_uuid(settings)
    listen = endpoint(settings, "listen", loopback=True)
    upstream = endpoint(settings, "upstream")
    daemon = given(settings, "daemon_origin")
    # urlsplit raises errors the launcher does not catch for a value that is not a string, so the
    # type is checked first.
    origin = urlsplit(daemon) if type(daemon) is str else None
    if (origin is None or origin.scheme != "http" or origin.username or origin.password
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
                    "match": [{"host": [local_host], "method": ["GET"], "path": [SOCKET_PATH.format(project=project)],
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
    try:
        config = configuration(json.loads(args.config.read_text()), root / "dist")
    except ValueError as refusal:
        sys.exit(f"{args.config}: {refusal}")
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
