# clipzip / clipunzip

Clipboard to zip and back, for Windows. Sibling of `clipin` / `clipout`.

`clipzip` defaults to zip. `clipunzip` previews the zip when run without arguments; `--unzip` or a destination extracts it. `--zip` and `--unzip` override either name.

```text
clipzip   [name.zip]     fenced bundle on the clipboard -> zip on the clipboard
clipunzip                zip on the clipboard           -> read-only preview
clipunzip --unzip        zip on the clipboard           -> files in cwd
clipunzip [dest_dir]     zip on the clipboard           -> files under dest_dir
```

## Build

Delete a stale `Cargo.lock` if this tree was copied from clipin, then:

```powershell
cargo build --release
```

Binaries:

```text
target\release\clipzip.exe
target\release\clipunzip.exe
```

Copying `clipzip.exe` to `clipunzip.exe` also works. The default follows the exe name.

## Usage

| Command | Default |
|---------|---------|
| `clipzip` | `--zip` |
| `clipunzip` | Preview only (no arguments); extract with `--unzip` or a destination |

| Flag | Effect |
|------|--------|
| `--zip` `--z` | Force zip mode |
| `--unzip` `--u` | Force unzip mode |
| `--list` `--l` | Preview archive metadata, write nothing |
| `--b64` | With zip: clipboard receives Base64 text instead of a binary zip |
| `--trace` `--t` | Diagnostics |
| `--help` `--h` | Help |

### Example

Clipboard text (the `clipin` bundle shape):

````text
file_path\file_name.ext
```ext
file content
```

file_path\file_name2.ext
```ext
file content2
```
````

```powershell
c:\test> clipzip
c:\test> clipunzip          # preview files and byte sizes, writes nothing
c:\test> clipunzip --unzip  # extract into c:\test
```

Writes:

```text
c:\test\file_path\file_name.ext
c:\test\file_path\file_name2.ext
```

The preview shows the clipboard source, archive byte size, each path's original and compressed byte sizes, unsafe-entry warnings, folder count, totals and compression reduction. It does not create directories or extract data. `clipzip bundle.zip` also saves that zip on disk. `clipunzip .\out` extracts under `.\out`.

## Clipboard formats

Zip mode sets both:

1. Registered binary format `application/zip` (length-prefixed raw zip). This is what unzip reads first.
2. An Explorer file-drop of a `.zip` (your `name.zip`, or `%TEMP%\clipzip\clipzip.zip`) so the archive pastes into a folder.

Unzip reads, in order: that binary format, a copied `.zip` file, then Base64 text (`--b64` round-trips).

## Notes

- The path is the nearest non-empty line above a fence. Leading `#`, quotes, and backticks are stripped. A trailing comment after a space is ignored, matching `clipout`.
- A fence is a line of three or more backticks. The closing line must be at least as long, so a longer fence can wrap content that itself contains a short fence.
- A fence tagged `base64` (how `clipin` embeds images) is decoded back to bytes.
- Text is stored with CRLF between lines, matching `clipout`.
- `..`, drive letters, and other absolute paths are refused on the way in and on the way out.
- Duplicate paths: the last block wins.
