import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

# The launcher, two directories up.
LAUNCHER = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(LAUNCHER))
from wallboard import configuration


class ProxyConfiguration(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.dist = Path(self.temp.name)
        (self.dist / "index.html").touch()
        (self.dist / "fwd-web_bg.wasm").touch()
        self.settings = json.loads((LAUNCHER / "wallboard.example.json").read_text())

    def test_one_source_controls_project_tunnel_and_daemon_origin(self):
        self.settings["project_id"] = "11111111-1111-4111-8111-111111111111"
        self.settings["upstream"]["port"] = 54321
        self.settings["daemon_origin"] = "http://127.0.0.1:54320"
        config = configuration(self.settings, self.dist)
        server = config["apps"]["http"]["servers"]["wallboard"]
        self.assertEqual(server["listen"], ["127.0.0.1:3000"])
        routes = server["routes"]
        public = json.loads(routes[0]["handle"][0]["body"])
        self.assertEqual(public["project_id"], self.settings["project_id"])
        proxy = routes[1]
        self.assertEqual(proxy["match"], [{"host": ["127.0.0.1"], "method": ["GET"], "path": [f"/p/{public['project_id']}/one-store/ws"],
                                         "header": {"Origin": ["http://127.0.0.1:3000"], "Upgrade": ["websocket"]}}])
        self.assertEqual(proxy["handle"][0]["upstreams"], [{"dial": "127.0.0.1:54321"}])
        headers = proxy["handle"][0]["headers"]["request"]["set"]
        self.assertEqual(headers, {"Host": ["127.0.0.1:54320"], "Origin": ["http://127.0.0.1:54320"]})
        # Exactly one backend handler, no fallback proxy or privileged HTTP endpoints.
        self.assertEqual(sum(h["handler"] == "reverse_proxy" for r in routes for h in r["handle"]), 1)
        self.assertEqual(routes[-1]["handle"], [{"handler": "static_response", "status_code": 404}])
        self.assertTrue(config["admin"]["disabled"])

    def test_refuses_network_listener_and_malformed_configuration(self):
        for key, value in [("listen", {"host": "0.0.0.0", "port": 3000}),
                           ("upstream", {"host": "127.0.0.1", "port": "12345"}),
                           ("project_id", "bad/project"),
                           ("daemon_origin", "http://192.0.2.2:12345"),
                           ("daemon_origin", "http://127.0.0.1:12345/stop"),
                           ("daemon_origin", 123)]:
            with self.subTest(key=key, value=value):
                settings = dict(self.settings, **{key: value})
                with self.assertRaises(ValueError):
                    configuration(settings, self.dist)

    def test_refuses_a_port_it_cannot_use_by_its_field(self):
        for key in ("listen", "upstream"):
            for port in ("3000", 3000.0, True, None, 1023, 65536):
                with self.subTest(key=key, port=port):
                    settings = dict(self.settings, **{key: dict(self.settings[key], port=port)})
                    with self.assertRaisesRegex(ValueError, rf"^{key}\.port must be an integer from 1024 to 65535$"):
                        configuration(settings, self.dist)

    def test_refuses_a_project_id_that_is_not_a_uuid_string_by_name(self):
        for value in (123, None, True, ["11111111-1111-4111-8111-111111111111"], {}, "bad/project"):
            with self.subTest(project_id=value):
                with self.assertRaisesRegex(ValueError, r"^project_id must be the project's UUID, as a string$"):
                    configuration(dict(self.settings, project_id=value), self.dist)

    def test_refuses_a_configuration_missing_a_field_it_has_no_default_for_by_name(self):
        for key in ("project_id", "listen", "upstream", "daemon_origin"):
            with self.subTest(key=key):
                settings = {name: value for name, value in self.settings.items() if name != key}
                with self.assertRaisesRegex(ValueError, rf"^{key} is missing;"):
                    configuration(settings, self.dist)
        for key in ("listen", "upstream"):
            for part in ("host", "port"):
                with self.subTest(key=key, part=part):
                    given = {name: value for name, value in self.settings[key].items() if name != part}
                    with self.assertRaisesRegex(ValueError, rf"^{key}\.{part} is missing;"):
                        configuration(dict(self.settings, **{key: given}), self.dist)

    def test_the_launcher_stops_on_a_refused_configuration_with_its_sentence_alone(self):
        missing = {name: value for name, value in self.settings.items() if name != "listen"}
        config = Path(self.temp.name) / "wallboard.json"
        for settings, sentence in [(missing, "listen is missing; the launcher has no default for it (README.md, Configuring)"),
                                   (dict(self.settings, project_id=123), "project_id must be the project's UUID, as a string"),
                                   (dict(self.settings, daemon_origin=123),
                                    "daemon_origin must be the unchanged daemon's http://loopback-IP:port")]:
            with self.subTest(sentence=sentence):
                config.write_text(json.dumps(settings))
                run = subprocess.run([sys.executable, str(LAUNCHER / "wallboard.py"), "--config", str(config), "--check"],
                                     capture_output=True, text=True)
                self.assertEqual(run.returncode, 1)
                self.assertEqual(run.stderr, f"{config}: {sentence}\n")

    def test_display_settings_reach_the_browser_and_preserve_old_configuration(self):
        def public(settings):
            routes = configuration(settings, self.dist)["apps"]["http"]["servers"]["wallboard"]["routes"]
            return json.loads(routes[0]["handle"][0]["body"])

        settings = {key: value for key, value in self.settings.items()
                    if key not in ("statuses", "max_rows", "node_width", "theme", "text_scale_percent", "fit_to_screen")}
        defaults = public(settings)
        self.assertEqual(defaults["theme"], "dark-aubergine")
        self.assertEqual(defaults["statuses"], ["in_progress"])
        self.assertEqual(defaults["max_rows"], 8)
        self.assertEqual(defaults["node_width"], 360)
        self.assertEqual(defaults["text_scale_percent"], 100)
        self.assertFalse(defaults["fit_to_screen"])
        chosen = {"statuses": ["proposed", "open", "in_progress", "in_review"],
                  "max_rows": 6, "node_width": 420, "theme": "charcoal", "text_scale_percent": 125, "fit_to_screen": True}
        configured = public(dict(settings, **chosen))
        for key, value in chosen.items():
            self.assertEqual(configured[key], value)
        for theme in ("dark-aubergine", "charcoal", "oxblood-copper", "indigo-saffron", "graphite-coral", "linen-indigo", "porcelain-coral", "sand-plum"):
            self.assertEqual(public(dict(settings, theme=theme))["theme"], theme)

    def test_solar_schedule_requires_explicit_valid_location_and_day_night_palettes(self):
        def public(schedule):
            config = configuration(dict(self.settings, theme_schedule=schedule), self.dist)
            return json.loads(config["apps"]["http"]["servers"]["wallboard"]["routes"][0]["handle"][0]["body"])
        self.assertFalse(public({"enabled": False})["theme_schedule"]["enabled"])
        schedule = {"enabled": True, "latitude": 0, "longitude": 0,
                    "day_theme": "linen-indigo", "night_theme": "indigo-saffron"}
        self.assertEqual(public(schedule)["theme_schedule"], schedule)
        invalid = [[], {"enabled": True}, {"enabled": 1},
                   dict(schedule, latitude=True), dict(schedule, longitude="0"),
                   dict(schedule, latitude=91), dict(schedule, longitude=-181),
                   dict(schedule, latitude=float('nan')), dict(schedule, longitude=float('inf')),
                   dict(schedule, day_theme="charcoal"), dict(schedule, night_theme="sand-plum")]
        for value in invalid:
            with self.subTest(schedule=value):
                with self.assertRaises(ValueError):
                    public(value)

    def test_refuses_closed_statuses_and_unusable_display_limits(self):
        invalid = [("statuses", []), ("statuses", "open"), ("statuses", ["open", "done"]),
                   ("statuses", ["cancelled"]), ("statuses", ["rejected"]),
                   ("statuses", ["open", "open"]), ("statuses", [None]),
                   ("statuses", [{}]), ("max_rows", True), ("max_rows", 2),
                   ("max_rows", 17), ("max_rows", 8.5), ("node_width", 239),
                   ("node_width", 641), ("node_width", "360"),
                   ("text_scale_percent", 79), ("text_scale_percent", 161), ("text_scale_percent", 115.5),
                   ("text_scale_percent", True), ("text_scale_percent", "125"), ("fit_to_screen", 1), ("fit_to_screen", None),
                   ("theme", "system"), ("theme", "light"), ("theme", "dark"), ("theme", None), ("theme", {})]
        for key, value in invalid:
            with self.subTest(key=key, value=value):
                with self.assertRaises(ValueError):
                    configuration(dict(self.settings, **{key: value}), self.dist)

    def test_replay_defaults_validation_and_nested_wasm_modules(self):
        modules = self.dist / "snippets" / "fwd-web-test" / "src" / "wallboard"
        modules.mkdir(parents=True)
        (modules / "wallboard-runtime.mjs").touch()

        def public(settings):
            routes = configuration(settings, self.dist)["apps"]["http"]["servers"]["wallboard"]["routes"]
            self.assertIn("/snippets/fwd-web-test/src/wallboard/wallboard-runtime.mjs", routes[2]["match"][0]["path"])
            return json.loads(routes[0]["handle"][0]["body"])

        self.assertEqual(public(self.settings)["replay"],
                         {"enabled": True, "interval_seconds": 180})
        self.assertEqual(public(self.settings)["sound"], {"enabled": False, "volume": 30})
        self.assertEqual(public(dict(self.settings, sound={"enabled": True, "volume": 12}))["sound"],
                         {"enabled": True, "volume": 12})
        for sound in [None, [], {"enabled": 1}, {"volume": True}, {"volume": -1}, {"volume": 101}]:
            with self.subTest(sound=sound):
                with self.assertRaises(ValueError):
                    public(dict(self.settings, sound=sound))
        replay = {"enabled": False, "interval_seconds": 600}
        self.assertEqual(public(dict(self.settings, replay=replay))["replay"], replay)
        self.assertEqual(public(dict(self.settings, replay=dict(replay, not_a_setting=120)))["replay"], replay)
        for replay in [None, [], {"enabled": 1}, {"interval_seconds": True},
                       {"interval_seconds": 59}, {"interval_seconds": 3601}]:
            with self.subTest(replay=replay):
                with self.assertRaises(ValueError):
                    public(dict(self.settings, replay=replay))


if __name__ == "__main__":
    unittest.main()
