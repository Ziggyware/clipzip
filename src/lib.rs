//! Shared implementation for clipzip and clipunzip.
//!
//! The exe name picks the default: clipunzip -> unzip, anything else -> zip.
//! --zip and --unzip always override that default.

use std::env;
use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process;

const ZIP_FORMAT_NAME: &str = "application/zip";

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------
mod clipboard {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::ptr;
    use std::thread::sleep;
    use std::time::Duration;
    use winapi::um::shellapi::{DragQueryFileW, HDROP};
    use winapi::um::winbase::{
        GlobalAlloc, GlobalFree, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
    };
    use winapi::um::winuser::{
        CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
        RegisterClipboardFormatW, SetClipboardData,
    };

    pub const CF_UNICODETEXT: u32 = 13;
    pub const CF_HDROP: u32 = 15;

    fn open() -> bool {
        for _ in 0..20 {
            if unsafe { OpenClipboard(ptr::null_mut()) } != 0 {
                return true;
            }
            sleep(Duration::from_millis(30));
        }
        false
    }

    pub fn zip_format() -> u32 {
        let w: Vec<u16> = super::ZIP_FORMAT_NAME
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        unsafe { RegisterClipboardFormatW(w.as_ptr()) }
    }

    /// Replace the clipboard. A failed format is skipped; Err only if nothing landed.
    pub fn set_multi(items: &[(u32, Vec<u8>)]) -> Result<usize, String> {
        unsafe {
            if !open() {
                return Err("OpenClipboard failed".into());
            }
            EmptyClipboard();
            let mut set = 0usize;
            for (fmt, bytes) in items {
                let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1));
                if h.is_null() {
                    continue;
                }
                let p = GlobalLock(h);
                if p.is_null() {
                    GlobalFree(h);
                    continue;
                }
                ptr::copy_nonoverlapping(bytes.as_ptr(), p as *mut u8, bytes.len());
                GlobalUnlock(h);
                if SetClipboardData(*fmt, h).is_null() {
                    GlobalFree(h);
                    continue;
                }
                set += 1;
            }
            CloseClipboard();
            if set == 0 {
                Err("SetClipboardData failed".into())
            } else {
                Ok(set)
            }
        }
    }

    pub fn get_bytes(fmt: u32) -> Option<Vec<u8>> {
        unsafe {
            if IsClipboardFormatAvailable(fmt) == 0 || !open() {
                return None;
            }
            let h = GetClipboardData(fmt);
            if h.is_null() {
                CloseClipboard();
                return None;
            }
            let size = GlobalSize(h);
            let p = GlobalLock(h) as *const u8;
            if p.is_null() || size == 0 {
                CloseClipboard();
                return None;
            }
            let v = std::slice::from_raw_parts(p, size).to_vec();
            GlobalUnlock(h);
            CloseClipboard();
            Some(v)
        }
    }

    pub fn get_text() -> String {
        match get_bytes(CF_UNICODETEXT) {
            Some(b) => {
                let wide: Vec<u16> = b
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .take_while(|&c| c != 0)
                    .collect();
                String::from_utf16_lossy(&wide)
            }
            None => String::new(),
        }
    }

    pub fn text_bytes(text: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(text.len() * 2 + 2);
        for u in text.encode_utf16().chain(std::iter::once(0)) {
            out.extend_from_slice(&u.to_le_bytes());
        }
        out
    }

    /// DROPFILES (20 bytes) + double-NUL UTF-16 path list.
    pub fn hdrop_bytes(paths: &[String]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&20u32.to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes());
        out.extend_from_slice(&1i32.to_le_bytes());
        for p in paths {
            for u in std::ffi::OsStr::new(p).encode_wide() {
                out.extend_from_slice(&u.to_le_bytes());
            }
            out.extend_from_slice(&0u16.to_le_bytes());
        }
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    pub fn get_file_drop() -> Vec<String> {
        unsafe {
            if IsClipboardFormatAvailable(CF_HDROP) == 0 || !open() {
                return Vec::new();
            }
            let h = GetClipboardData(CF_HDROP);
            if h.is_null() {
                CloseClipboard();
                return Vec::new();
            }
            let hdrop = h as HDROP;
            let count = DragQueryFileW(hdrop, 0xFFFF_FFFF, ptr::null_mut(), 0);
            let mut out = Vec::new();
            for i in 0..count {
                let need = DragQueryFileW(hdrop, i, ptr::null_mut(), 0);
                let mut buf = vec![0u16; need as usize + 1];
                DragQueryFileW(hdrop, i, buf.as_mut_ptr(), need + 1);
                buf.truncate(need as usize);
                out.push(OsString::from_wide(&buf).to_string_lossy().to_string());
            }
            CloseClipboard();
            out
        }
    }
}

// ---------------------------------------------------------------------------
// Base64
// ---------------------------------------------------------------------------
fn b64_encode(data: &[u8]) -> String {
    const C: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for ch in data.chunks(3) {
        let b0 = ch[0] as u32;
        let b1 = *ch.get(1).unwrap_or(&0) as u32;
        let b2 = *ch.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(C[((n >> 18) & 63) as usize] as char);
        out.push(C[((n >> 12) & 63) as usize] as char);
        out.push(if ch.len() > 1 { C[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if ch.len() > 2 { C[(n & 63) as usize] as char } else { '=' });
    }
    out
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut buf = 0u32;
    let mut bits = 0u32;
    let mut out = Vec::new();
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b'\r' | b'\n' | b' ' | b'\t' => continue,
            _ => return None,
        };
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

// ---------------------------------------------------------------------------
// Zip bytes: length prefix defeats GlobalSize rounding; EOCD trim is the fallback
// ---------------------------------------------------------------------------
fn pack_zip(bytes: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + bytes.len());
    v.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    v.extend_from_slice(bytes);
    v
}

fn decode_prefixed(buf: &[u8]) -> Option<Vec<u8>> {
    if buf.len() < 8 || buf.starts_with(b"PK") {
        return None;
    }
    let n = u64::from_le_bytes(buf[0..8].try_into().ok()?) as usize;
    let end = 8usize.checked_add(n)?;
    if n == 0 || end > buf.len() {
        return None;
    }
    let body = &buf[8..end];
    if body.starts_with(b"PK") {
        Some(body.to_vec())
    } else {
        None
    }
}

/// Cut a buffer back to the real zip end. GlobalAlloc rounds up, and a zip
/// reader treats trailing padding as a corrupt comment.
fn trim_zip(buf: &[u8]) -> Option<&[u8]> {
    const SIG: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
    if buf.len() < 22 || !buf.starts_with(b"PK") {
        return None;
    }
    let start = buf.len().saturating_sub(22 + 65535);
    let mut i = buf.len() - 22;
    loop {
        if buf[i..i + 4] == SIG {
            let comment = u16::from_le_bytes([buf[i + 20], buf[i + 21]]) as usize;
            if let Some(end) = i.checked_add(22 + comment) {
                if end <= buf.len() {
                    return Some(&buf[..end]);
                }
            }
        }
        if i == start {
            break;
        }
        i -= 1;
    }
    None
}

fn decode_zip_bytes(buf: &[u8]) -> Option<Vec<u8>> {
    if let Some(z) = decode_prefixed(buf) {
        return Some(z);
    }
    if buf.starts_with(b"PK") {
        if let Some(z) = trim_zip(buf) {
            return Some(z.to_vec());
        }
        return Some(buf.to_vec());
    }
    None
}

// ---------------------------------------------------------------------------
// Fenced bundle (clipin output / clipout input)
// ---------------------------------------------------------------------------
struct Rec {
    name: String,
    data: Vec<u8>,
}

fn backtick_run(s: &str) -> usize {
    s.chars().take_while(|&c| c == '`').count()
}

fn clean_name(raw: &str) -> Option<String> {
    let mut s = raw
        .trim()
        .trim_start_matches('#')
        .trim()
        .trim_matches(|c| matches!(c, '*' | '`' | '[' | ']' | '"' | '\''))
        .trim()
        .trim_end_matches(':')
        .trim()
        .to_string();
    if let Some(i) = s.find(char::is_whitespace) {
        s.truncate(i);
    }
    if let Some(i) = s.find('(') {
        s.truncate(i);
    }
    if let Some(i) = s.find("//") {
        s.truncate(i);
    }
    s = s.trim().trim_end_matches(':').to_string();
    if s.contains('.') || s.contains('/') || s.contains('\\') {
        Some(s)
    } else {
        None
    }
}

/// Safe relative zip path. Rejects `..`, drive letters, and UNC prefixes.
fn safe_name(n: &str) -> Option<String> {
    let n = n.replace('\\', "/");
    if n.contains(':') || n.starts_with('/') {
        return None;
    }
    let parts: Vec<&str> = n.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    if parts.is_empty() || parts.iter().any(|p| *p == "..") {
        return None;
    }
    Some(parts.join("/"))
}

fn parse_bundle(text: &str) -> Vec<Rec> {
    let lines: Vec<&str> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < lines.len() {
        let t = lines[i].trim();
        let open = backtick_run(t);
        if open < 3 {
            i += 1;
            continue;
        }
        let info = t[open..].trim().to_lowercase();
        let tag = info.split_whitespace().next().unwrap_or("");

        let mut name = None;
        let mut k = i;
        while k > 0 {
            k -= 1;
            if lines[k].trim().is_empty() {
                continue;
            }
            name = clean_name(lines[k]);
            break;
        }

        let mut close = None;
        let mut j = i + 1;
        while j < lines.len() {
            let tj = lines[j].trim();
            let run = backtick_run(tj);
            if run >= open && run == tj.chars().count() {
                close = Some(j);
                break;
            }
            j += 1;
        }
        let end = close.unwrap_or(lines.len());
        if close.is_none() {
            eprintln!("Warning: bundle ended inside a fenced block; content may be truncated.");
        }

        match name.as_deref().and_then(safe_name) {
            Some(n) => {
                let body = &lines[(i + 1).min(end)..end];
                let data = if tag == "base64" {
                    b64_decode(&body.join("")).unwrap_or_else(|| body.join("\r\n").into_bytes())
                } else {
                    body.join("\r\n").into_bytes()
                };
                out.push(Rec { name: n, data });
            }
            None => eprintln!(
                "Warning: skipped block at line {} (no usable filename above it).",
                i + 1
            ),
        }
        i = end + 1;
    }
    out
}

fn build_zip(recs: &[Rec]) -> Result<(Vec<u8>, usize), String> {
    let mut uniq: Vec<&Rec> = Vec::new();
    for r in recs {
        if let Some(p) = uniq.iter().position(|u| u.name.eq_ignore_ascii_case(&r.name)) {
            uniq[p] = r;
        } else {
            uniq.push(r);
        }
    }
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for r in &uniq {
        w.start_file(r.name.as_str(), opts).map_err(|e| e.to_string())?;
        w.write_all(&r.data).map_err(|e| e.to_string())?;
    }
    let bytes = w.finish().map_err(|e| e.to_string())?.into_inner();
    Ok((bytes, uniq.len()))
}

/// Name for clipboard text that has no usable `path` + fence blocks.
/// Unix seconds keep it distinct per run without a date library.
fn dummy_name() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("clipboard_{}.txt", secs)
}

/// Wrap plain clipboard text as one record under a dummy name.
/// Lines are stored with CRLF, matching the bundle convention.
fn plain_text_rec(text: &str) -> Rec {
    let body = text.replace("\r\n", "\n").replace('\n', "\r\n");
    Rec {
        name: dummy_name(),
        data: body.into_bytes(),
    }
}

/// Fence longer than any line in `body` that is only backticks, so the
/// content can never close the fence early. Minimum three backticks.
fn fence_for(body: &str) -> String {
    let longest = body
        .lines()
        .map(|l| {
            let t = l.trim();
            if !t.is_empty() && t.chars().all(|c| c == '`') {
                t.len()
            } else {
                0
            }
        })
        .max()
        .unwrap_or(0);
    "`".repeat((longest + 1).max(3))
}

/// Inverse of parse_bundle: `path` line, fence, content, fence, blank line.
/// Text files keep their extension as the fence tag; binary files use a
/// base64 fence so they round-trip through the parser.
fn format_bundle(files: &[(String, Vec<u8>)]) -> String {
    let mut out = String::new();
    for (i, (name, data)) in files.iter().enumerate() {
        if i > 0 {
            out.push_str("\r\n");
        }
        let mut path = name.replace('/', "\\");
        if !path.contains('\\') {
            // Bare names have no separator and would be skipped on the way back in.
            path = format!(".\\{}", path);
        }
        let (tag, body) = match std::str::from_utf8(data) {
            Ok(text) => {
                let ext = Path::new(name)
                    .extension()
                    .map(|e| e.to_string_lossy().to_string())
                    .unwrap_or_default();
                let tag = if ext == "base64" { String::new() } else { ext };
                (tag, text.replace("\r\n", "\n").replace('\n', "\r\n"))
            }
            Err(_) => ("base64".to_string(), b64_encode(data)),
        };
        let fence = fence_for(&body);
        out.push_str(&path);
        out.push_str("\r\n");
        out.push_str(&fence);
        out.push_str(&tag);
        out.push_str("\r\n");
        out.push_str(&body);
        out.push_str("\r\n");
        out.push_str(&fence);
        out.push_str("\r\n");
    }
    out
}

fn human(n: usize) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / 1048576.0)
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{} B", n)
    }
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------
struct Cfg {
    zip: bool,
    unzip: bool,
    list: bool,
    mem: bool,
    b64: bool,
    trace: bool,
    help: bool,
    positional: Option<String>,
}

fn parse_args() -> Cfg {
    let mut c = Cfg {
        zip: false,
        unzip: false,
        list: false,
        mem: false,
        b64: false,
        trace: false,
        help: false,
        positional: None,
    };
    for a in env::args().skip(1) {
        match a.as_str() {
            "--z" | "--zip" | "-z" | "/zip" | "/z" => c.zip = true,
            "--u" | "--unzip" | "-u" | "-x" | "/unzip" | "/u" => c.unzip = true,
            "--l" | "--list" | "-l" | "/list" | "/l" => c.list = true,
            "--m" | "--mem" | "--memory" | "-m" | "/mem" | "/m" => c.mem = true,
            "--b64" | "--b" | "-b" | "/b64" => c.b64 = true,
            "--t" | "--trace" | "-t" | "/trace" | "/t" => c.trace = true,
            "--h" | "--help" | "-h" | "/?" | "/help" | "/h" | "?" => c.help = true,
            s if s.starts_with('-') || s.starts_with('/') => {
                eprintln!("Warning: unknown flag {}", s);
            }
            s => {
                if c.positional.is_none() {
                    c.positional = Some(s.to_string());
                }
            }
        }
    }
    c
}

fn unzip_default(exe: &str) -> bool {
    Path::new(exe)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase()
        .contains("unzip")
}

fn invoked_as_unzip() -> bool {
    unzip_default(&env::args().next().unwrap_or_default())
}

fn should_preview(cfg: &Cfg, as_unzip: bool) -> bool {
    cfg.list || (as_unzip && !cfg.unzip && !cfg.mem && cfg.positional.is_none())
}

fn help_text(as_unzip: bool) -> String {
    let default = if as_unzip { "unzip" } else { "zip" };
    format!(
        "\x1b[1;36mclipzip\x1b[0m / \x1b[1;36mclipunzip\x1b[0m — clipboard <-> zip

  \x1b[0;34mUSAGE\x1b[0m
    clipzip   [name.zip] [flags]    default: zip
    clipunzip [dest_dir] [flags]    no args: preview; dest_dir: extract

  This exe defaults to \x1b[0;33m{default}\x1b[0m. --zip and --unzip override either name.
  Copying clipzip.exe to clipunzip.exe switches the default.

  \x1b[0;34mFLAGS\x1b[0m
    \x1b[0;33m--zip   --z\x1b[0m     Fenced bundle on the clipboard -> zip on the clipboard.
                     Optional name.zip is also written to disk.
    \x1b[0;33m--unzip --u\x1b[0m     Extract clipboard zip under dest_dir (default: cwd).
    \x1b[0;33m--list  --l\x1b[0m     Preview archive contents without writing files.
    \x1b[0;33m--mem   --m\x1b[0m     Unzip in memory: clipboard zip -> formatted text on clipboard
                     (path line, fence, content). Writes no files.
                     The clipboard is the target instead of a folder.
    \x1b[0;33m--b64\x1b[0m           Zip mode: put the archive on the clipboard as Base64 text.
    \x1b[0;33m--trace --t\x1b[0m     Diagnostics
    \x1b[0;33m--help  --h\x1b[0m     This message

  \x1b[0;34mCLIPBOARD\x1b[0m
    Zip publishes raw bytes as '{fmt}' plus an Explorer file-drop of a .zip.
    Unzip reads that format, then a copied .zip file, then Base64 text.

  \x1b[0;34mEXAMPLES\x1b[0m
    clipzip
    clipzip bundle.zip
    clipunzip                   (preview, no files written)
    clipunzip --unzip           (extract into cwd)
    clipunzip .\\out
    clipunzip --list
    clipunzip --mem             (zip -> formatted text on clipboard)
",
        fmt = ZIP_FORMAT_NAME,
    )
}

fn die(msg: &str) -> ! {
    eprintln!("\x1b[1;31mError:\x1b[0m {}", msg);
    process::exit(1);
}

fn cwd() -> PathBuf {
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn do_zip(cfg: &Cfg) {
    let text = clipboard::get_text().trim_start_matches('\u{feff}').to_string();
    if text.trim().is_empty() {
        die("Clipboard has no text to zip.");
    }
    let mut recs = parse_bundle(&text);
    if recs.is_empty() {
        // Not a path + fence bundle: keep the text as one file under a dummy name.
        let rec = plain_text_rec(&text);
        eprintln!(
            "Warning: clipboard text is not in path + fence format; stored as {}.",
            rec.name
        );
        recs.push(rec);
    }
    if cfg.trace {
        for r in &recs {
            eprintln!("  [ENTRY] {} ({})", r.name, human(r.data.len()));
        }
    }
    let (zip_bytes, count) = build_zip(&recs).unwrap_or_else(|e| die(&format!("zip failed: {}", e)));

    if cfg.b64 {
        let s = b64_encode(&zip_bytes);
        if let Err(e) = clipboard::set_multi(&[(clipboard::CF_UNICODETEXT, clipboard::text_bytes(&s))]) {
            die(&e);
        }
        println!(
            "{} file(s) zipped ({}) -> Base64 text on clipboard.",
            count,
            human(zip_bytes.len())
        );
        return;
    }

    let (zip_path, user_named) = match &cfg.positional {
        Some(p) => {
            let mut pb = PathBuf::from(p);
            if !pb.is_absolute() {
                pb = cwd().join(pb);
            }
            if pb.extension().map(|e| !e.eq_ignore_ascii_case("zip")).unwrap_or(true) {
                pb.set_extension("zip");
            }
            (pb, true)
        }
        None => (env::temp_dir().join("clipzip").join("clipzip.zip"), false),
    };
    if let Some(parent) = zip_path.parent() {
        if !parent.as_os_str().is_empty() && fs::create_dir_all(parent).is_err() && user_named {
            die(&format!("could not create {}", parent.display()));
        }
    }
    let file_ok = match fs::write(&zip_path, &zip_bytes) {
        Ok(()) => true,
        Err(e) => {
            if user_named {
                die(&format!("could not write {}: {}", zip_path.display(), e));
            }
            eprintln!("Warning: no .zip file for Explorer paste ({})", e);
            false
        }
    };

    let mut items = vec![(clipboard::zip_format(), pack_zip(&zip_bytes))];
    if file_ok {
        let abs = fs::canonicalize(&zip_path).unwrap_or_else(|_| {
            if zip_path.is_absolute() {
                zip_path.clone()
            } else {
                cwd().join(&zip_path)
            }
        });
        let s = abs.to_string_lossy().to_string();
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s).to_string();
        items.push((clipboard::CF_HDROP, clipboard::hdrop_bytes(&[s])));
    }
    if let Err(e) = clipboard::set_multi(&items) {
        die(&e);
    }
    print!(
        "{} file(s) zipped ({}) and placed on clipboard.",
        count,
        human(zip_bytes.len())
    );
    if user_named {
        print!(" Saved {}", zip_path.display());
    }
    println!();
}

fn read_clipboard_zip(trace: bool) -> Option<(Vec<u8>, String)> {
    if let Some(b) = clipboard::get_bytes(clipboard::zip_format()) {
        if let Some(z) = decode_zip_bytes(&b) {
            if trace {
                eprintln!("  [SRC] {} format", ZIP_FORMAT_NAME);
            }
            return Some((z, "clipboard binary".into()));
        }
    }
    for f in clipboard::get_file_drop() {
        if f.to_lowercase().ends_with(".zip") {
            if let Ok(b) = fs::read(&f) {
                if let Some(z) = decode_zip_bytes(&b) {
                    if trace {
                        eprintln!("  [SRC] file-drop {}", f);
                    }
                    return Some((z, format!("copied file: {}", f)));
                }
            }
        }
    }
    let t = clipboard::get_text();
    let t = t.trim().trim_start_matches('\u{feff}');
    if !t.is_empty() {
        if let Some(b) = b64_decode(t) {
            if let Some(z) = decode_zip_bytes(&b) {
                if trace {
                    eprintln!("  [SRC] Base64 text");
                }
                return Some((z, "Base64 clipboard text".into()));
            }
        }
    }
    None
}

/// --mem: unzip in memory. Each file becomes a `path` line plus fenced content
/// (the format parse_bundle reads), and the text goes on the clipboard.
fn unzip_to_memory(ar: &mut zip::ZipArchive<Cursor<Vec<u8>>>) {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut skipped = 0usize;
    for i in 0..ar.len() {
        let mut f = match ar.by_index(i) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Skipping entry {}: {}", i, e);
                skipped += 1;
                continue;
            }
        };
        let rel = match (safe_name(f.name()), f.enclosed_name()) {
            (Some(n), Some(_)) => n,
            _ => {
                eprintln!("\x1b[0;33mSkipping unsafe path\x1b[0m: {}", f.name());
                skipped += 1;
                continue;
            }
        };
        if f.is_dir() {
            continue;
        }
        let mut data = Vec::new();
        if let Err(e) = f.read_to_end(&mut data) {
            eprintln!("Failed to read {}: {}", rel, e);
            skipped += 1;
            continue;
        }
        files.push((rel, data));
    }

    if files.is_empty() {
        die("No files in the clipboard zip to place in memory.");
    }
    let text = format_bundle(&files);
    if let Err(e) = clipboard::set_multi(&[(clipboard::CF_UNICODETEXT, clipboard::text_bytes(&text))]) {
        die(&e);
    }
    println!(
        "\x1b[2;36m{} file(s) unzipped to memory -> formatted text on clipboard.\x1b[0m",
        files.len()
    );
    if skipped > 0 {
        eprintln!("{} entr{} skipped.", skipped, if skipped == 1 { "y" } else { "ies" });
    }
}

/// Inspect metadata only. Never create directories or decompress file data.
fn preview_archive<W: Write>(
    ar: &mut zip::ZipArchive<Cursor<Vec<u8>>>,
    archive_size: usize,
    source: &str,
    out: &mut W,
) -> io::Result<()> {
    writeln!(out, "Clipboard zip | {} bytes ({}) | {}", archive_size, human(archive_size), source)?;
    writeln!(out, "")?;
    writeln!(out, "{:>12}  {:>12}  {}", "BYTES", "PACKED", "PATH")?;

    let (mut files, mut dirs, mut unsafe_count) = (0usize, 0usize, 0usize);
    let (mut original, mut packed) = (0u64, 0u64);
    for i in 0..ar.len() {
        match ar.by_index(i) {
            Ok(f) => {
                let name = f.name();
                let safe = safe_name(name).is_some() && f.enclosed_name().is_some();
                let kind = if !safe { " [unsafe - skipped]" } else if f.is_dir() { " [dir]" } else { "" };
                writeln!(out, "{:>12}  {:>12}  {}{}", f.size(), f.compressed_size(), name, kind)?;
                if !safe {
                    unsafe_count += 1;
                } else if f.is_dir() {
                    dirs += 1;
                } else {
                    files += 1;
                    original = original.saturating_add(f.size());
                    packed = packed.saturating_add(f.compressed_size());
                }
            }
            Err(e) => {
                unsafe_count += 1;
                writeln!(out, "{:>12}  {:>12}  entry #{} [unreadable: {}]", "-", "-", i + 1, e)?;
            }
        }
    }
    let reduction = if original == 0 { 0.0 } else {
        (1.0 - packed as f64 / original as f64) * 100.0
    };
    writeln!(out, "")?;
    writeln!(
        out,
        "{} file(s), {} folder(s), {} unsafe/unreadable | {} bytes original, {} bytes packed ({:.1}% reduction)",
        files, dirs, unsafe_count, original, packed, reduction
    )?;
    writeln!(out, "Preview only. To extract: clipunzip --unzip [dest_dir] or clipunzip <dest_dir>")
}

fn do_unzip(cfg: &Cfg, preview: bool) {
    let (bytes, source) = read_clipboard_zip(cfg.trace).unwrap_or_else(|| {
        die("Clipboard does not contain a zip (binary, .zip file, or Base64 text).")
    });
    let archive_size = bytes.len();
    let mut ar = zip::ZipArchive::new(Cursor::new(bytes)).unwrap_or_else(|e| die(&format!("invalid zip: {}", e)));
    if preview {
        preview_archive(&mut ar, archive_size, &source, &mut io::stdout())
            .unwrap_or_else(|e| die(&format!("could not display archive: {}", e)));
        return;
    }
    if cfg.mem {
        if cfg.positional.is_some() {
            die("--mem puts the result on the clipboard; do not give a dest_dir.");
        }
        unzip_to_memory(&mut ar);
        return;
    }

    let dest = match &cfg.positional {
        Some(p) => {
            let pb = PathBuf::from(p);
            if pb.is_absolute() { pb } else { cwd().join(pb) }
        }
        None => cwd(),
    };
    if dest.exists() && !dest.is_dir() {
        die(&format!("{} is a file, not a directory.", dest.display()));
    }
    if fs::create_dir_all(&dest).is_err() {
        die(&format!("could not create {}", dest.display()));
    }

    let mut written = 0usize;
    let mut skipped = 0usize;
    for i in 0..ar.len() {
        let mut f = match ar.by_index(i) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Skipping entry {}: {}", i, e);
                skipped += 1;
                continue;
            }
        };
        let rel = match (safe_name(f.name()), f.enclosed_name()) {
            (Some(_), Some(p)) => p.to_path_buf(),
            _ => {
                eprintln!("\x1b[0;33mSkipping unsafe path\x1b[0m: {}", f.name());
                skipped += 1;
                continue;
            }
        };
        if f.is_dir() {
            let _ = fs::create_dir_all(dest.join(&rel));
            continue;
        }
        let out = dest.join(&rel);
        if let Some(parent) = out.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut data = Vec::new();
        if let Err(e) = f.read_to_end(&mut data) {
            eprintln!("Failed to read {}: {}", rel.display(), e);
            skipped += 1;
            continue;
        }
        match fs::write(&out, &data) {
            Ok(()) => {
                println!("{}", out.display());
                written += 1;
            }
            Err(e) => {
                eprintln!("\x1b[1;31mFailed\x1b[0m {}: {}", out.display(), e);
                skipped += 1;
            }
        }
    }

    if written == 0 {
        die("No files written.");
    }
    println!("\x1b[2;36m{} file(s) written under {}\x1b[0m", written, dest.display());
    if skipped > 0 {
        eprintln!("{} entr{} skipped.", skipped, if skipped == 1 { "y" } else { "ies" });
    }
}

pub fn run() {
    let cfg = parse_args();
    let as_unzip = invoked_as_unzip();
    if cfg.help {
        print!("{}", help_text(as_unzip));
        return;
    }
    if cfg.zip && cfg.unzip {
        die("Choose either --zip or --unzip, not both.");
    }
    if cfg.zip && cfg.mem {
        die("Choose either --zip or --mem, not both.");
    }
    // Explicit flags win. --list and --mem imply unzip. --b64 implies zip.
    // Otherwise the exe name decides: clipunzip -> unzip, clipzip -> zip.
    let unzip = if cfg.zip || (cfg.b64 && !cfg.unzip && !cfg.list && !cfg.mem) {
        false
    } else if cfg.unzip || cfg.list || cfg.mem {
        true
    } else {
        as_unzip
    };
    if cfg.trace {
        eprintln!(
            "  [PARSE] exe={} mode={}",
            env::args().next().unwrap_or_default(),
            if unzip { "unzip" } else { "zip" }
        );
    }
    if unzip {
        // No-argument clipunzip behaves like clipout's preview. An explicit
        // --unzip or a destination still performs extraction.
        do_unzip(&cfg, should_preview(&cfg, as_unzip));
    } else {
        do_zip(&cfg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn fence() -> String {
        "`".repeat(3)
    }

    fn empty_cfg() -> Cfg {
        Cfg {
            zip: false, unzip: false, list: false, mem: false, b64: false,
            trace: false, help: false, positional: None,
        }
    }

    #[test]
    fn no_args_previews_but_explicit_extraction_still_works() {
        let mut cfg = empty_cfg();
        assert!(should_preview(&cfg, true));
        assert!(!should_preview(&cfg, false));
        cfg.unzip = true;
        assert!(!should_preview(&cfg, true));
        cfg.unzip = false;
        cfg.positional = Some("out".into());
        assert!(!should_preview(&cfg, true));
        cfg.list = true;
        assert!(should_preview(&cfg, true));
    }

    #[test]
    fn exe_name_selects_default() {
        assert!(!unzip_default(r"C:\Windows\System32\clipzip.exe"));
        assert!(unzip_default(r"C:\tools\clipunzip.exe"));
        assert!(unzip_default("clipunzip"));
        assert!(!unzip_default("clipzip"));
    }

    #[test]
    fn parses_user_sample() {
        let f = fence();
        let text = format!(
            "file_path\\file_name.ext\n{f}ext\nfile content\n{f}\n\nfile_path\\file_name2.ext\n{f}ext\nfile content2\n{f}\n"
        );
        let recs = parse_bundle(&text);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].name, "file_path/file_name.ext");
        assert_eq!(recs[0].data, b"file content");
        assert_eq!(recs[1].name, "file_path/file_name2.ext");
        assert_eq!(recs[1].data, b"file content2");
    }

    #[test]
    fn rejects_traversal_and_decodes_base64() {
        let f = fence();
        let text = format!("..\\secret.txt\n{f}txt\nnope\n{f}\n\npic.bin\n{f}base64\naGVsbG8=\n{f}\n");
        let recs = parse_bundle(&text);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].name, "pic.bin");
        assert_eq!(recs[0].data, b"hello");
    }

    #[test]
    fn longer_fence_wraps_short_fence() {
        let inner = "`".repeat(3);
        let outer = "`".repeat(4);
        let text = format!("notes.md\n{outer}md\n{inner}\ncode\n{inner}\n{outer}\n");
        let recs = parse_bundle(&text);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].data, format!("{inner}\r\ncode\r\n{inner}").into_bytes());
    }

    #[test]
    fn zip_roundtrip_survives_padding_and_prefix() {
        let f = fence();
        let text = format!("file_path\\file_name.ext\n{f}ext\nfile content\n{f}\n");
        let recs = parse_bundle(&text);
        let (bytes, n) = build_zip(&recs).unwrap();
        assert_eq!(n, 1);

        let mut padded = bytes.clone();
        padded.extend_from_slice(&[0u8; 64]);
        let trimmed = decode_zip_bytes(&padded).unwrap();
        assert_eq!(trimmed.len(), bytes.len());

        let packed = pack_zip(&bytes);
        let mut packed_pad = packed.clone();
        packed_pad.extend_from_slice(&[0u8; 32]);
        assert_eq!(decode_zip_bytes(&packed_pad).unwrap(), bytes);

        let mut ar = zip::ZipArchive::new(Cursor::new(trimmed)).unwrap();
        let mut file = ar.by_index(0).unwrap();
        let mut got = String::new();
        file.read_to_string(&mut got).unwrap();
        assert_eq!(got, "file content");
        assert_eq!(file.enclosed_name().unwrap().to_string_lossy(), "file_path/file_name.ext");
    }

    #[test]
    fn mem_implies_unzip_and_never_previews() {
        let mut cfg = empty_cfg();
        cfg.mem = true;
        assert!(!should_preview(&cfg, true));
        assert!(!should_preview(&cfg, false));
    }

    #[test]
    fn plain_text_falls_back_to_dummy_name() {
        let rec = plain_text_rec("hello\nworld");
        assert!(rec.name.starts_with("clipboard_"));
        assert!(rec.name.ends_with(".txt"));
        assert_eq!(rec.data, b"hello\r\nworld");
        assert!(parse_bundle("just some text\nno fence here").is_empty());
    }

    #[test]
    fn format_bundle_roundtrips_through_parser() {
        let files = vec![
            ("file_path/file_name.ext".to_string(), b"file content".to_vec()),
            ("notes.md".to_string(), b"a\r\nb\r\n".to_vec()),
            ("pic.bin".to_string(), vec![0u8, 255, 1]),
            ("fenced.md".to_string(), b"x\r\n```\r\ny".to_vec()),
        ];
        let text = format_bundle(&files);
        let recs = parse_bundle(&text);
        assert_eq!(recs.len(), 4);
        assert_eq!(recs[0].name, "file_path/file_name.ext");
        assert_eq!(recs[0].data, b"file content");
        assert_eq!(recs[1].name, "notes.md");
        assert_eq!(recs[1].data, b"a\r\nb\r\n");
        assert_eq!(recs[2].name, "pic.bin");
        assert_eq!(recs[2].data, vec![0u8, 255, 1]);
        assert_eq!(recs[3].name, "fenced.md");
        assert_eq!(recs[3].data, b"x\r\n```\r\ny");
    }

    #[test]
    fn preview_reports_entries_and_does_not_extract() {
        let records = vec![
            Rec { name: "file_path/a.txt".into(), data: b"hello".to_vec() },
            Rec { name: "file_path/b.txt".into(), data: b"world!".to_vec() },
        ];
        let (bytes, _) = build_zip(&records).unwrap();
        let size = bytes.len();
        let mut ar = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut output = Vec::new();
        preview_archive(&mut ar, size, "clipboard binary", &mut output).unwrap();
        let display = String::from_utf8(output).unwrap();
        assert!(display.contains("file_path/a.txt"));
        assert!(display.contains("file_path/b.txt"));
        assert!(display.contains("5"));
        assert!(display.contains("2 file(s)"));
        assert!(display.contains("11 bytes original"));
        assert!(display.contains("Preview only"));
    }
}
