"""Prepare a uiprobe release for the newest stable Slint 1.x on crates.io."""

import json
import os
import re
import tomllib
from pathlib import Path
from urllib.request import urlopen


def numbers(version):
    return tuple(map(int, version.split(".")))


def releases(crate):
    url = f"https://index.crates.io/{crate[:2]}/{crate[2:4]}/{crate}"
    with urlopen(url, timeout=30) as response:
        records = map(json.loads, response)
        return {
            record["vers"]: record
            for record in records
            if not record["yanked"] and re.fullmatch(r"1\.\d+\.\d+", record["vers"])
        }


def read(path):
    """Return a file's text with "\\n" line ends, and the line end the file itself uses."""
    with open(path, encoding="utf-8", newline="") as file:
        text = file.read()
    return text.replace("\r\n", "\n"), "\r\n" if "\r\n" in text else "\n"


def write(path, text, newline):
    # Keep each file's own line ends: README.md uses CRLF, and a runner that rewrote them
    # would put every line of the file in the pull request.
    with open(path, "w", encoding="utf-8", newline=newline) as file:
        file.write(text)


def main():
    manifest = Path("Cargo.toml")
    root, root_newline = read(manifest)
    workspace = tomllib.loads(root)["workspace"]
    deps = workspace["dependencies"]
    old_slint = deps["slint"]["version"].removeprefix("=")
    old_probe = deps["uiprobe"]["version"]
    if not all(pin == f"={old_slint}" for pin in (
        deps["slint"]["version"], deps["slint-build"], deps["i-slint-backend-testing"]
    )):
        raise ValueError("The workspace's three Slint pins must agree")

    slint = releases("slint")
    target = max(slint, key=numbers)
    if numbers(target) <= numbers(old_slint):
        print(f"Already on Slint {old_slint}")
        return
    required = [slint[target]]
    for crate in ("i-slint-backend-testing", "slint-build"):
        versions = releases(crate)
        if target not in versions:
            print(f"Waiting for {crate} {target} on crates.io")
            return
        required.append(versions[target])

    major, minor, patch = numbers(old_probe)
    if major != 0 or minor != numbers(old_slint)[1]:
        raise ValueError("Expected uiprobe 0.x and Slint 1.x with matching minor versions")
    new_minor = numbers(target)[1]
    probe = f"0.{new_minor}.{patch + 1 if minor == new_minor else 0}"
    old_rust = workspace["package"]["rust-version"]
    rust = max([old_rust] + [r["rust_version"] for r in required if r.get("rust_version")],
               key=lambda v: numbers(v) + (0,) * (3 - len(numbers(v))))

    def current_versions(source):
        return (source.replace(old_slint, target)
                .replace(f"1.{minor}.x", f"1.{new_minor}.x")
                .replace(f"0.{minor}.x", f"0.{new_minor}.x"))

    # Limit replacements to the named dependencies. Other crates can share version numbers.
    lines = []
    for line in root.splitlines(keepends=True):
        if line.startswith(("#", "slint =", "slint-build =", "i-slint-backend-testing =")):
            line = current_versions(line)
        elif line.startswith("uiprobe ="):
            line = line.replace(f'"{old_probe}"', f'"{probe}"')
        elif line.startswith("rust-version ="):
            line = line.replace(f'"{old_rust}"', f'"{rust}"')
        lines.append(line)
    write(manifest, "".join(lines), root_newline)

    crate = Path("crates/uiprobe/Cargo.toml")
    source, newline = read(crate)
    if tomllib.loads(source)["package"]["version"] != old_probe:
        raise ValueError("The uiprobe package and workspace versions differ")
    write(crate, source.replace(f'\nversion = "{old_probe}"\n', f'\nversion = "{probe}"\n', 1),
          newline)

    library = Path("crates/uiprobe/src/lib.rs")
    source, newline = read(library)
    if f'pub const SLINT_VERSION: &str = "{old_slint}";' not in source:
        raise ValueError("SLINT_VERSION differs from the workspace pin")
    write(library, current_versions(source), newline)

    for path in (Path("README.md"), Path("docs/cookbook.md")):
        source, newline = read(path)
        # Keep historical statements about previous uiprobe releases.
        source = (current_versions(source).replace(f'"={old_probe}"', f'"={probe}"')
                  .replace(f"uiprobe {old_probe} (Slint", f"uiprobe {probe} (Slint")
                  .replace(f"The harness requires Rust {old_rust}.", f"The harness requires Rust {rust}."))
        write(path, source, newline)

    changelog = Path("CHANGELOG.md")
    source, newline = read(changelog)
    title, entries = source.split("\n", 1)
    note = f"## uiprobe {probe}\n\nUpdate the exact Slint pin to {target}."
    if rust != old_rust:
        note += f" The harness now requires Rust {rust}."
    write(changelog, f"{title}\n\n{note}\n{entries}", newline)
    print(f"Preparing uiprobe {probe} for Slint {target}")
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        print(f"slint={target}", f"uiprobe={probe}", sep="\n", file=output)


if __name__ == "__main__":
    main()
