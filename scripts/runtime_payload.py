"""Prepare the installed npm tree for a Plait host release."""
import base64
import binascii
import hashlib
import json
import os
import re
import shutil
from pathlib import Path
from typing import NamedTuple


class Binary(NamedTuple):
    package: str
    version: str
    sha256: str
    components: tuple[str, ...] = ()


PHOTON = ("Photon dependency notices", "color_quant and NeuQuant")
# Each digest covers the whole file, including every module in a JS container.
BINARIES = {
    "@silvia-odwyer/photon/photon_rs_bg.wasm": Binary(
        "@silvia-odwyer/photon", "0.3.3",
        "06e9f6b245d53b488244e04dfe30e6731c029526101c6400e6490057fcfbdd04", PHOTON),
    "@silvia-odwyer/photon-node/photon_rs_bg.wasm": Binary(
        "@silvia-odwyer/photon-node", "0.3.4",
        "10468181565c56004c867f3a4af96f89a0ef5a63a72f2b5fb12c1f1992a3615c", PHOTON),
    "quickjs-wasi/quickjs.wasm": Binary(
        "quickjs-wasi", "3.6.2",
        "d4c9375f2b1ca4dc95f72c8aa2982a7a9951ac8011490d79c6582df732b4bbd9", ("QuickJS-NG", "WASI libraries")),
    "@earendil-works/pi-tui/native/darwin/prebuilds/darwin-arm64/darwin-platform.node": Binary(
        "@earendil-works/pi-tui", "1.5.2",
        "36e909adf35c2734a2a52b3d65b7dbfb83f2278f306d51b7e32c5045516dfb7f"),
    "@earendil-works/pi-tui/native/linux/prebuilds/linux-arm64/linux-platform-x11.node": Binary(
        "@earendil-works/pi-tui", "1.5.2",
        "63d4545349a25b5c97e352ea6d254cea50ff3c14161212ad3ddb3fc28022ddb5"),
    "@earendil-works/pi-tui/native/linux/prebuilds/linux-x64/linux-platform-x11.node": Binary(
        "@earendil-works/pi-tui", "1.5.2",
        "522d2550f7fa5d0d5bd204f45983f786ce2be9cf80f27f00179640d31d0689d6"),
    "undici/lib/llhttp/llhttp-wasm.js": Binary(
        "undici", "8.12.3",
        "5d8313d6fe8145d832e6af9959b2810523c9a6993d5b93cee09bd7e5f1f2c085", ("llhttp", "WASI libraries")),
    "undici/lib/llhttp/llhttp_simd-wasm.js": Binary(
        "undici", "8.12.3",
        "a3f30f3307ab3c191720cb636d570febb3610c420bc7077f2e225dacfcad3295", ("llhttp SIMD", "WASI libraries")),
    "undici/lib/milo/no-simd.js": Binary(
        "undici", "8.12.3",
        "ece090d1e2e962e0de7428bf542b900b373876e5cf1d7eda43e5cd08a92bf9b7", ("Milo", "memchr")),
    "undici/lib/milo/simd.js": Binary(
        "undici", "8.12.3",
        "3bc5b7120aa8732d6a1fb2692714d549d4025aa27482ba3b47e7b80ff74f23cc", ("Milo SIMD", "memchr")),
    "@earendil-works/pi-coding-agent/dist/bundle/chunks/chunk-VEJDBY3O.js": Binary(
        "@earendil-works/pi-coding-agent", "1.5.2",
        "effbbd5d6247983dc11c7ae5796eb24bda88f9ebf78c707d3f92ca73cbb8b51e",
        ("llhttp", "llhttp SIMD", "Milo", "Milo SIMD", "WASI libraries", "memchr")),
    "long/index.js": Binary(
        "long", "5.3.2",
        "8d6b1d2162ff21170db73d6b91e313e6b90bd35acfdd60453b2792ec433ae69e", ("long.js i64 arithmetic",)),
    "long/umd/index.js": Binary(
        "long", "5.3.2",
        "ea6da8d5fff04abc358b94488dbbb7eeb46e5027cc05303b1761cea94bef7821", ("long.js i64 arithmetic",)),
}


BINARY_SUFFIXES = {".wasm", ".node", ".so", ".dll", ".exe", ".dylib", ".a", ".o"}
MACH_HEADERS = {bytes.fromhex(value) for value in (
    "feedface", "cefaedfe", "feedfacf", "cffaedfe", "cafebabe", "bebafeca", "cafebabf", "bfbafeca"
)}


MAGICS = (b"\0asm", b"\x7fELF", b"!<arch>\n", *MACH_HEADERS)
BASE64_JOIN = re.compile(rb"""(?<=[A-Za-z0-9+/_-])['"]\s*\+\s*['"](?=[A-Za-z0-9+/_-])""")
BASE64 = re.compile(rb"[A-Za-z0-9+/_-]{6,}={0,2}")
NUMBER = rb"(?:0[xX][\da-fA-F]+|0[bB][01]+|[0-9]+)"
JS_SPACE = rb"(?:\s|/\*[^*]*(?:\*(?!/)[^*]*)*\*/|//[^\n]*\n)*"
BYTE_LIST = re.compile(rb"(?<![\w.])" + NUMBER + rb"(?:" + JS_SPACE + rb"," + JS_SPACE + NUMBER + rb"){3,}")
COMMENTS = re.compile(rb"/\*.*?\*/|//[^\n]*", re.DOTALL)


def base64_fragments(magic):
    # Only sextets entirely inside the signature survive arbitrary prefix/suffix bytes.
    for offset in range(3):
        encoded = base64.b64encode(bytes(offset) + magic)
        yield encoded[(offset * 8 + 5) // 6:(offset + len(magic)) * 8 // 6]


ENCODED_MAGIC = re.compile(b"|".join(re.escape(fragment) for magic in (*MAGICS, b"PE\0\0")
                                     for fragment in base64_fragments(magic)))


def contains_code(data):
    return any(magic in data for magic in MAGICS) or (b"MZ" in data and b"PE\0\0" in data)


JS_EXTENSIONS = {".js", ".cjs", ".mjs"}


def is_javascript(path):
    if path.suffix.lower() in JS_EXTENSIONS:
        return True
    if not path.suffix and path.is_file():
        with path.open("rb") as stream:
            return bool(re.match(rb"^#![^\n]*\bnode(?:js)?\b", stream.readline(256)))
    return False


def is_binary(path):
    if path.suffix.lower() in BINARY_SUFFIXES:
        return True
    data = path.read_bytes()
    if data.startswith(b"MZ") or contains_code(data):
        return True
    if path.suffix.lower() in {".js", ".cjs", ".mjs", ".ts"}:
        data = BASE64_JOIN.sub(b"", data)
    encoded = data.translate(bytes.maketrans(b"-_", b"+/"))
    if ENCODED_MAGIC.search(encoded):
        for match in BASE64.finditer(encoded):
            token = match.group()
            if not ENCODED_MAGIC.search(token):
                continue
            try:
                decoded = base64.b64decode(token + b"===")
            except binascii.Error:
                continue
            if contains_code(decoded):
                return True
    for match in BYTE_LIST.finditer(data):
        try:
            decoded = bytes(int(n, 0) for n in COMMENTS.sub(b"", match.group()).split(b","))
        except ValueError:
            continue
        if contains_code(decoded):
            return True
    return False


def binary_inventory(runtime):
    modules = runtime / "node_modules"
    inventory = []
    for path in sorted(runtime.rglob("*")):
        if path.is_symlink() and not path.resolve().is_relative_to(runtime.resolve()):
            raise ValueError(f"File outside runtime: {path}")
        if not path.is_file():
            continue
        relative = path.relative_to(modules).as_posix() if path.is_relative_to(modules) else None
        record = BINARIES.get(relative)
        if record is None:
            if is_binary(path):
                raise ValueError(f"No recorded binary license: {path}")
            continue
        if hashlib.sha256(path.read_bytes()).hexdigest() != record.sha256:
            raise ValueError(f"Unreviewed binary contents: {path}")
        manifest = json.loads((modules / record.package / "package.json").read_text())
        if (manifest["name"], manifest["version"]) != (record.package, record.version):
            raise ValueError(f"No recorded binary license for installed version: {path}")
        inventory.append((relative, record))
    return inventory


def check_host_binaries(host):
    """npm_notices checks runtime. Check everything alongside it before archiving."""
    licenses = {"bin/node": ("NODE_LICENSE.txt",),
                "bin/plait-host": ("LICENSE", "THIRD_PARTY_NOTICES.txt")}
    for parent, dirs, files in os.walk(host):
        parent = Path(parent)
        for name in [*dirs, *files]:
            path = parent / name
            if path.is_symlink() and not path.resolve().is_relative_to(host.resolve()):
                raise ValueError(f"File outside host: {path}")
        if parent == host:
            dirs.remove("runtime")
        for name in files:
            path = parent / name
            relative = path.relative_to(host).as_posix()
            if relative in licenses:
                for notice in licenses[relative]:
                    if not (host / notice).read_text().strip():
                        raise ValueError(f"Empty license for {path}: {notice}")
            elif is_javascript(path):
                raise ValueError(f"Unclassified staged JavaScript: {relative}")
            elif is_binary(path):
                raise ValueError(f"No recorded binary license: {path}")


def prune_runtime(runtime, platform, cpu):
    modules = runtime / "node_modules"
    # Both SDKs publish compiled modules but keep their upstream notices under src.
    for package, relative in (
        ("@anthropic-ai/sdk", "internal/qs/LICENSE.md"),
        ("openai", "internal/qs/LICENSE.md"),
        ("openai", "_vendor/partial-json-parser/LICENSE"),
        ("openai", "_vendor/zod-to-json-schema/LICENSE"),
    ):
        sdk = modules / package
        if sdk.is_dir():
            source = sdk / "src" / relative
            destination = sdk / relative
            if not sdk.resolve().is_relative_to(modules.resolve()) or not all(
                path.resolve().is_relative_to(sdk.resolve()) for path in (source, destination)
            ):
                raise ValueError(f"SDK notice outside package: {source}")
            if destination.exists():
                if source.exists() and source.read_bytes() != destination.read_bytes():
                    raise ValueError(f"Conflicting SDK notices: {source} and {destination}")
            else:
                source.replace(destination)
    development_dirs = {"test", "tests", "__tests__", "example", "examples", "bench", "benchmarks", ".github"}
    # These packages publish their Node implementation alongside its TypeScript sources.
    unused = {modules / package / "src" for package in
              ("openai", "@anthropic-ai/sdk", "zod", "@earendil-works/chord")}
    unused.update({modules / "@earendil-works/chord/docs", modules / "@earendil-works/pi-coding-agent/dist/bun"})
    for parent, directories, files in os.walk(modules):
        parent = Path(parent)
        for name in directories.copy():
            path = parent / name
            if name in development_dirs or path in unused:
                directories.remove(name)
                if path.is_symlink():
                    path.unlink()
                else:
                    shutil.rmtree(path)
        for name in files:
            if name.endswith((".js.map", ".mjs.map", ".cjs.map", ".d.ts.map", ".d.ts", ".d.cts", ".d.mts")):
                (parent / name).unlink()

    # Pi loads only the native clipboard prebuild for the current platform.
    native = modules / "@earendil-works/pi-tui/native"
    for path in sorted(native.rglob("*"), reverse=True):
        if path.is_dir():
            if not any(path.iterdir()):
                path.rmdir()
        elif path.parent != native / platform / "prebuilds" / f"{platform}-{cpu}":
            path.unlink()
    # Pi codemode loads the standalone engine without quickjs-wasi's optional extensions.
    extensions = modules / "quickjs-wasi/extensions"
    if extensions.is_dir():
        shutil.rmtree(extensions)
