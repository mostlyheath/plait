import base64
from contextlib import contextmanager
import hashlib
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from runtime_payload import BINARIES, check_host_binaries, prune_runtime
from third_party_notices import JS_BUNDLES, JS_NOTICES, JS_SOURCES, ROOT, inventory_digest, main, npm_notices


class NoticesTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.host = Path(temporary.name)
        self.runtime = self.host / "runtime"
        self.location = "node_modules/outer/node_modules/@vendor/nested"
        self.package = self.runtime / self.location
        self.package.mkdir(parents=True)
        self.entry = {"version": "1.2.3", "license": "MIT"}
        (self.package / "package.json").write_text(json.dumps(dict(self.entry, name="@vendor/nested")))
        (self.package / "LICENSE").write_text("Nested package license")
        (self.package / "NOTICE").write_text("Nested attribution")
        self.lock = self.runtime / "package-lock.json"
        self.lock.write_text(json.dumps({"packages": {
            "": {}, self.location: self.entry,
            "node_modules/dev": {"dev": True}, "node_modules/other-os": {"optional": True},
        }}))

    def test_rust_report_starts_without_tomllib(self):
        subprocess.run(
            [sys.executable, "-c",
             "import sys; sys.modules['tomllib'] = None; import rust_license_report"],
            cwd=ROOT / "scripts", check=True,
        )

    def test_app_contains_host_and_client_notices(self):
        (self.host / "NODE_LICENSE.txt").write_text("Node distribution license")
        target = self.host / "target"
        target.mkdir()
        (self.host / "assets").symlink_to(ROOT / "assets")
        names = ["plait", "plait-host", "normal-crate", "dev-only", "derive-only", "macro-helper"]
        (self.host / "Cargo.toml").write_text(
            '[workspace]\nresolver = "3"\nmembers = ' + json.dumps(names))
        for name in names:
            crate = self.host / name
            (crate / "src").mkdir(parents=True)
            (crate / "src/lib.rs").touch()
            (crate / "Cargo.toml").write_text(f'[package]\nname = "{name}"\nversion = "1.0.0"\n')
        with (self.host / "plait/Cargo.toml").open("a") as manifest:
            manifest.write('[dependencies]\nnormal-crate = { path = "../normal-crate" }\n'
                           '[dev-dependencies]\nnormal-crate = { path = "../normal-crate", features = ["dev-only"] }\n')
        with (self.host / "normal-crate/Cargo.toml").open("a") as manifest:
            manifest.write('[dependencies]\ndev-only = { path = "../dev-only", optional = true }\n')
        with (self.host / "normal-crate/Cargo.toml").open("a") as manifest:
            manifest.write('derive-only = { path = "../derive-only" }\n')
        with (self.host / "derive-only/Cargo.toml").open("a") as manifest:
            manifest.write('[lib]\nproc-macro = true\n[dependencies]\nmacro-helper = { path = "../macro-helper" }\n')
        subprocess.run(["cargo", "generate-lockfile", "--offline"], cwd=self.host, check=True)
        (target / "client-rust-licenses.txt").write_text("Desktop crates")
        (target / "host-rust-licenses.txt").write_text("Host crates")
        (self.host / "component.json").write_text(json.dumps({"node_version": "26.12.0"}))
        output = self.host / "THIRD_PARTY_NOTICES.txt"
        main([str(self.host), str(output)], root=self.host)
        notices = output.read_text()
        self.assertIn("normal-crate 1.0.0", notices)
        self.assertNotIn("dev-only", notices)
        self.assertNotIn("derive-only", notices)
        self.assertNotIn("macro-helper", notices)
        for credit in ("Host crates", "Desktop crates", "Node.js 26.12.0", "Node distribution license",
                       "@vendor/nested 1.2.3", "Nested package license", "Nested attribution",
                       "IBM Plex Sans", "arcticicestudio", "Stillwater", "Lobe Icons"):
            self.assertIn(credit, notices)

    @contextmanager
    def review_fixture(self, path, components):
        record = {"sha256": hashlib.sha256(path.read_bytes()).hexdigest(), "packages": components}
        owner = "@vendor/nested 1.2.3"
        digest = inventory_digest([(path.name, record["sha256"], components)])
        with patch.dict(JS_BUNDLES, {owner: {path.name: record}}), patch.dict(JS_SOURCES, {owner: digest}):
            yield

    def test_new_changed_and_incompletely_mapped_bundles_fail(self):
        bundle = self.package / "dist.js"
        bundle.write_text('/* named module */ "./node_modules/.pnpm/gensync@1.0.0-beta.2/node_modules/gensync"')
        component = "gensync 1.0.0-beta.2"
        with self.assertRaisesRegex(ValueError, "Review bundled JavaScript"):
            list(npm_notices(self.runtime))
        for components in ([], ["json5 2.2.3"]):
            with self.subTest(components=components), self.review_fixture(bundle, components):
                with self.assertRaisesRegex(ValueError, "Review bundled JavaScript"):
                    list(npm_notices(self.runtime))
        with self.review_fixture(bundle, [component, "json5 2.2.3"]):
            JS_BUNDLES["@vendor/nested 1.2.3"][bundle.name]["packages"] = [component]
            with self.assertRaisesRegex(ValueError, "component list differs"):
                list(npm_notices(self.runtime))
        with self.review_fixture(bundle, [component]):
            self.assertIn("Logan Smyth", "\n".join(npm_notices(self.runtime)))
            bundle.write_text("changed")
            with self.assertRaisesRegex(ValueError, "Review bundled JavaScript"):
                list(npm_notices(self.runtime))

    def test_parser_and_limiter_credits_in_report(self):
        for owner, relative, component, credit in (
            ("@anthropic-ai/sdk 0.129.0", "_vendor/partial-json-parser/parser.js",
             "partial-json-parser (indgov)", "Copyright (c) 2017 indgov"),
            ("@anthropic-ai/sdk 0.129.0", "_vendor/partial-json-parser/parser.mjs",
             "partial-json-parser (indgov)", "Copyright (c) 2017 indgov"),
            ("ws 8.21.0", "lib/limiter.js", "async-limiter", "Copyright (c) 2017 Samuel Reed"),
        ):
            with self.subTest(owner=owner, path=relative):
                components = JS_BUNDLES[owner][relative]["packages"]
                self.assertIn(component, components)
                bundle = self.package / "bundle.js"
                bundle.write_text("export const fixture = true")
                with self.review_fixture(bundle, components):
                    self.assertIn(credit, "\n".join(npm_notices(self.runtime)))

    def test_sdk_notices_survive_source_pruning(self):
        packages = {}
        credit = "Nathan LaFreniere and other [contributors](https://github.com/puruvj/neoqs/graphs/contributors)"
        for name in ("openai", "@anthropic-ai/sdk"):
            location = "node_modules/" + name
            packages[location] = self.entry
            package = self.runtime / location
            package.mkdir(parents=True)
            (package / "package.json").write_text(json.dumps(dict(self.entry, name=name)))
            (package / "LICENSE").write_text("SDK license")
            notices = {"internal/qs/LICENSE.md": credit}
            if name == "openai":
                notices.update({"_vendor/partial-json-parser/LICENSE": "Promplate Dev Team",
                                "_vendor/zod-to-json-schema/LICENSE": "Stefan Terdell"})
            for relative, text in notices.items():
                source = package / "src" / relative
                source.parent.mkdir(parents=True)
                source.write_text(text)
                (package / relative).parent.mkdir(parents=True)
        self.lock.write_text(json.dumps({"packages": packages}))
        prune_runtime(self.runtime, "linux", "x64")
        sections = list(npm_notices(self.runtime))
        self.assertEqual(sections.count(credit), 2)
        self.assertIn("Promplate Dev Team", sections)
        self.assertIn("Stefan Terdell", sections)

    def test_reviewed_bundles_credit_embedded_packages(self):
        for owner, path, component, credit in [
            ("jiti 2.7.0", "dist/babel.cjs", "convert-source-map 2.0.0", "Thorsten Lorenz"),
            ("jiti 2.7.0", "dist/babel.cjs", "gensync 1.0.0-beta.2", "Logan Smyth"),
            ("jiti 2.7.0", "dist/babel.cjs", "json5 2.2.3", "Aseem Kishore"),
            ("jiti 2.7.0", "dist/babel.cjs", "@jridgewell/resolve-uri 3.1.2", "Justin Ridgewell"),
            ("jiti 2.7.0", "dist/jiti.cjs", "get-tsconfig 4.14.0", "Hiroki Osame"),
            ("@earendil-works/pi-coding-agent 1.5.2", "dist/bundle/chunks/jiti-loader-GL4HJFAD.js",
             "get-tsconfig 4.14.0", "Hiroki Osame"),
        ]:
            with self.subTest(path=path, component=component):
                self.assertIn(component, JS_BUNDLES[owner][path]["packages"])
                self.assertTrue(any(credit in (ROOT / "packaging" / source).read_text()
                                    for source in JS_NOTICES[component]))

    def test_script_outside_locked_packages_requires_review(self):
        (self.runtime / "sidecar.mjs").write_text("export const embedded = 42")
        with self.assertRaisesRegex(ValueError, "Unclassified staged JavaScript: sidecar.mjs"):
            list(npm_notices(self.runtime))

    def test_catalog_components_have_notice_sources(self):
        lock = json.loads((ROOT / "runtime/package-lock.json").read_text())["packages"]
        locked = {f"{entry.get('name', path.split('node_modules/')[-1])} {entry['version']}"
                  for path, entry in lock.items() if path and not entry.get("dev")}
        for owner, bundles in JS_BUNDLES.items():
            self.assertIn(owner, JS_SOURCES)
            for filename, record in bundles.items():
                with self.subTest(owner=owner, filename=filename):
                    self.assertRegex(record["sha256"], r"^[a-f0-9]{64}$")
                    self.assertTrue(record["packages"])
                    self.assertFalse(set(record["packages"]) - locked - JS_NOTICES.keys())
        for component, sources in JS_NOTICES.items():
            self.assertTrue(sources, component)
            for source in sources:
                self.assertTrue((ROOT / "packaging" / source).read_text().strip(), component)

    def test_bundle_license_sidecar_is_required(self):
        bundle = self.package / "vendor.cjs"
        bundle.write_text("/*! For license information please see vendor.cjs.LICENSE.txt */")
        with self.review_fixture(bundle, ["@vendor/nested 1.2.3"]):
            with self.assertRaises(FileNotFoundError):
                list(npm_notices(self.runtime))
            (self.package / "vendor.cjs.LICENSE.txt").write_text("Vendored dependency license")
            self.assertIn("Vendored dependency license", list(npm_notices(self.runtime)))

    def test_versioned_banner_requires_matching_notice(self):
        bundle = self.package / "bundle.js"
        for version in ("11.9.0", "12.0.0"):
            bundle.write_text(f"/*! highlight.js v{version} */")
            with self.review_fixture(bundle, ["highlight.js " + version]):
                if version == "11.9.0":
                    self.assertIn("Ivan Sagalaev", "\n".join(npm_notices(self.runtime)))
                else:
                    with self.assertRaisesRegex(ValueError, "highlight.js 12.0.0"):
                        list(npm_notices(self.runtime))

    def test_requires_locked_version(self):
        self.lock.write_text(json.dumps({"packages": {self.location: {"version": "2"}}}))
        with self.assertRaisesRegex(ValueError, "differs from lockfile"):
            list(npm_notices(self.runtime))

    def test_refuses_license_outside_package(self):
        (self.package / "LICENSE").unlink()
        (self.package / "LICENSE").symlink_to(self.lock)
        with self.assertRaisesRegex(ValueError, "outside package"):
            list(npm_notices(self.runtime))

    def test_uses_license_from_pinned_pi_release_when_npm_omits_it(self):
        entry = {"version": "1.5.2", "license": "MIT"}
        self.lock.write_text(json.dumps({"packages": {self.location: entry}}))
        (self.package / "package.json").write_text(json.dumps(dict(entry, name="@earendil-works/pi-ai")))
        (self.package / "LICENSE").unlink()
        with patch.dict(JS_SOURCES, {"@earendil-works/pi-ai 1.5.2": inventory_digest([])}):
            self.assertIn("Copyright (c) 2025 Mario Zechner", "\n".join(npm_notices(self.runtime)))

    def test_unrecorded_staged_binary(self):
        for name, content in [("unrecorded.wasm", b""), ("unrecorded", b"\x7fELF")]:
            with self.subTest(name=name):
                path = self.package / name
                path.write_bytes(content)
                with self.assertRaisesRegex(ValueError, "No recorded binary license"):
                    list(npm_notices(self.runtime))
                path.unlink()

    def test_embedded_code_outside_node_modules(self):
        script = self.runtime / "loader.js"
        for payload in (b"prefix\0asm", base64.b64encode(b"prefix\0asm"), b'AGF" + "zbQEAAAA=',
                        b"new Uint8Array([0, /* a */ 0x61, 115, 109])", base64.b64encode(b"\x7fELFpayload")):
            with self.subTest(payload=payload):
                script.write_bytes(b"load('" + payload + b"')")
                with self.assertRaisesRegex(ValueError, "No recorded binary license"):
                    list(npm_notices(self.runtime))

    def test_each_recorded_module_has_its_notices(self):
        payload = b"\0asm"
        fixture = {}
        packages = {}
        for relative, record in BINARIES.items():
            package = self.runtime / "node_modules" / record.package
            package.mkdir(parents=True, exist_ok=True)
            entry = dict(name=record.package, version=record.version, license="MIT")
            packages["node_modules/" + record.package] = entry
            (package / "package.json").write_text(json.dumps(entry))
            (package / "LICENSE").write_text("Package license")
            path = self.runtime / "node_modules" / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(payload)
            fixture[relative] = record._replace(sha256=hashlib.sha256(payload).hexdigest())
        (self.runtime / "node_modules/undici/lib/milo/LICENSE.md").write_text("Milo ISC notice")
        self.lock.write_text(json.dumps({"packages": packages}))
        js = {}
        for path, record in fixture.items():
            if path.endswith(".js"):
                owner = f"{record.package} {record.version}"
                js.setdefault(owner, {})[path[len(record.package) + 1:]] = {
                    "sha256": record.sha256, "packages": [owner]}
        inventories = {owner: inventory_digest([(path, record["sha256"], record["packages"])
                                                for path, record in files.items()])
                       for owner, files in js.items()}
        with patch.dict(BINARIES, fixture), patch.dict(JS_BUNDLES, js), patch.dict(JS_SOURCES, inventories, clear=True):
            text = "\n".join(npm_notices(self.runtime))
            for path, record in fixture.items():
                for component in record.components:
                    self.assertIn(f"{component} (embedded in {path})", text)
            for credit in ("2016 PistonDevelopers", "1994 Anthony Dekker", "2018.", "Milo ISC notice",
                           "2023-2026 Ben Noordhuis", "Roboto Regular", "The Closure Library Authors"):
                self.assertIn(credit, text)
            (self.runtime / "node_modules/undici/lib/milo/simd.js").write_bytes(payload * 2)
            with self.assertRaisesRegex(ValueError, "Unreviewed binary"):
                list(npm_notices(self.runtime))

    def test_host_files_outside_the_runtime_are_checked(self):
        path = self.host / "extra/plugin"
        path.parent.mkdir()
        path.write_bytes(base64.b64encode(b"\0asm"))
        with self.assertRaisesRegex(ValueError, "No recorded binary license"):
            check_host_binaries(self.host)

    def test_bundle_outside_host_runtime_requires_review(self):
        (self.host / "extra.cjs").write_text("module.exports = 42")
        with self.assertRaisesRegex(ValueError, "Unclassified staged JavaScript: extra.cjs"):
            check_host_binaries(self.host)

    def test_nested_component_notices(self):
        component = self.package / "lib/parser"
        component.mkdir(parents=True)
        for name in ("LICENSE.md", "NOTICE.txt"):
            (component / name).write_text("Parser " + name)
        sections = list(npm_notices(self.runtime))
        self.assertIn("Parser LICENSE.md", sections)
        self.assertIn("Parser NOTICE.txt", sections)

    def test_staging_removes_examples_before_inventory(self):
        directory = self.package / "examples/extensions/doom-overlay"
        directory.mkdir(parents=True)
        (directory / "doom.wasm").write_bytes(b"\0asm")
        prune_runtime(self.runtime, "darwin", "arm64")
        self.assertFalse(directory.exists())
        self.assertIn("Nested package license", list(npm_notices(self.runtime)))

    def test_missing_notice_fails(self):
        (self.package / "LICENSE").unlink()
        with self.assertRaisesRegex(ValueError, "No license text found"):
            list(npm_notices(self.runtime))

    def test_verified_upstream_notices(self):
        for name, version, credit in [
            ("proxy-agent-negotiate", "1.1.0", "Copyright (c) 2013 Nathan Rajlich"),
            ("standardwebhooks", "1.1.1", "Copyright (c) 2023 Svix"),
        ]:
            with self.subTest(name=name):
                entry = {"version": version, "license": "MIT"}
                self.lock.write_text(json.dumps({"packages": {self.location: entry}}))
                (self.package / "package.json").write_text(json.dumps(dict(entry, name=name)))
                (self.package / "LICENSE").unlink(missing_ok=True)
                with patch.dict(JS_SOURCES, {f"{name} {version}": inventory_digest([])}):
                    self.assertIn(credit, "\n".join(npm_notices(self.runtime)))
