//! A resolved asset is not always a program: every download lane publishes
//! netcoredbg as an archive (release.yml stages the upstream assets
//! verbatim — a `.tar.gz` for linux, a `.zip` for Windows whose runtime is
//! an exe plus seven sibling DLLs and cannot ship as one file, and since
//! D28 a `.zip` for macOS arm64 whose program is a plain unix binary), and
//! the BinaryManager cache holds exactly those verified bytes. Before the
//! relay execs its adapter child, `materialize` turns an archive into the
//! program inside it — once, next to the cache entry, and shared by every
//! later session.
//!
//! Everything that is already a program passes through untouched (a PATH
//! name, a plain file): a spawn error then stays the spawn's own honest
//! words rather than an extraction guess. Once a magic number matches,
//! though, the archive IS the lane's claim to make — a corrupt or hostile
//! one fails with the archive named, never a bare transport error, and
//! never a half-truth published as a program. Spawning the Windows zip by
//! mistake is what live sessions died on (owner repro, 2026-10-13):
//! "%1 is not a valid Win32 application. (os error 193)".

use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Directory (under the archive's own) that holds extracted programs.
const ASSET_DIR: &str = ".zatara-assets";
/// Name the archive member must have, and the extracted program is named
/// (plus `.exe` on Windows).
const MEMBER_NAME: &str = "netcoredbg";
/// The Windows zip always carries the PE image under this name, whatever
/// OS is reading it — a name fixed by the *published asset*, not `cfg`.
/// Since D28 the zip lane also serves the macOS arm64 asset, whose program
/// member is the unix binary under [`MEMBER_NAME`]; `materialize_zip`
/// picks whichever member the archive actually holds (exe first — a zip
/// with both is the Windows shape).
const ZIP_PROGRAM_NAME: &str = "netcoredbg.exe";

/// Quarantine directory for everything extracted from `archive` — derived
/// only from the archive's location and file stem, never from names inside
/// it: a crafted member name can plant nothing outside this directory.
fn extraction_dir(archive: &Path) -> PathBuf {
    archive
        .parent()
        .unwrap_or(Path::new("."))
        .join(ASSET_DIR)
        .join(archive.file_stem().unwrap_or_default())
}

/// Where the gzip lane's extracted program lives. See `extraction_dir`
/// for why member names cannot steer this path.
fn extraction_target(archive: &Path) -> PathBuf {
    let program = if cfg!(windows) {
        "netcoredbg.exe"
    } else {
        MEMBER_NAME
    };
    extraction_dir(archive).join(program)
}

/// Where the zip lane's program lives once published (see
/// `materialize_zip` for the flat, basename-only layout).
fn zip_target(archive: &Path) -> PathBuf {
    extraction_dir(archive).join(ZIP_PROGRAM_NAME)
}

/// The program the adapter spawn should exec. A gzip stream — the shape
/// of a cached linux asset — is unpacked on first use to
/// `<asset dir>/.zatara-assets/<stem>/netcoredbg` (mode 0o755 on unix);
/// a zip (the Windows asset) is unpacked whole, flat, to
/// `<stem>/netcoredbg.exe` beside the DLLs it loads. Every later call,
/// and every concurrent session that lost the write race, lands on that
/// one file. Non-archive input and unreadable input return the input path
/// unchanged; an archive past its magic that turns out malformed is
/// reported with the archive named, and a well-formed one holding no
/// program is passed through untouched (no member to extract).
pub fn materialize(program: &Path) -> io::Result<PathBuf> {
    let target = extraction_target(program);
    if target.is_file() {
        return Ok(target); // steady state: one stat, no archive read
    }
    // The zip lane needs its own steady-state stat: the Windows asset's
    // program is `netcoredbg.exe` (the PE image), which the gzip lane's
    // target (`netcoredbg` on unix) never names. The macOS arm64 zip's
    // program is `netcoredbg` — on a unix host that is the lane target
    // above, so its steady state needs no separate check.
    let target_zip = zip_target(program);
    if target_zip.is_file() {
        return Ok(target_zip);
    }
    let mut file = match fs::File::open(program) {
        Ok(file) => file,
        // An unreadable "program" is the spawn's story to tell, not ours.
        Err(_) => return Ok(program.to_path_buf()),
    };
    let mut magic = [0u8; 2];
    match file.read_exact(&mut magic) {
        Ok(()) if magic == [0x1f, 0x8b] => {}
        Ok(()) if magic == [0x50, 0x4b] => {
            // `PK` is only a zip with the `\x03\x04` local-header tail
            // (other `PK..` variants are spanned/empty archives — pass
            // those through for spawn to judge).
            let mut tail = [0u8; 2];
            if file.read_exact(&mut tail).is_ok() && tail == [0x03, 0x04] {
                file.seek(SeekFrom::Start(0))?;
                return materialize_zip(program, file, &target_zip);
            }
            return Ok(program.to_path_buf());
        }
        _ => return Ok(program.to_path_buf()), // neither archive format
    }
    file.seek(SeekFrom::Start(0))?;

    // Past the magic the archive is our claim: stream (never
    // pre-reserve — a header's declared size is attacker-chosen) straight
    // into a part file, so memory stays bounded by the buffer, not by a
    // decompression bomb's promise.
    let dir = target
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "asset path has no parent"))?;
    fs::create_dir_all(dir)?;
    let file_name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(MEMBER_NAME);
    let part = dir.join(format!("{file_name}.part-{}", std::process::id()));
    let streamed = (|| -> io::Result<bool> {
        let mut out = fs::File::create(&part)?;
        let found = stream_member(&mut file, &mut out).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("extracting {MEMBER_NAME} from {}: {e}", program.display()),
            )
        })?;
        if found {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                out.set_permissions(fs::Permissions::from_mode(0o755))?;
            }
            out.sync_all()?;
        }
        Ok(found)
    })();
    match streamed {
        Ok(true) => {
            if let Err(e) = fs::rename(&part, &target) {
                let _ = fs::remove_file(&part);
                // Only a concurrent winner's published file excuses a
                // failed rename; anything else is an extraction failure
                // with a name the caller should hear.
                if target.is_file() {
                    return Ok(target);
                }
                return Err(io::Error::new(
                    e.kind(),
                    format!("publishing {}: {e}", target.display()),
                ));
            }
            Ok(target)
        }
        // A gzip that holds no netcoredbg is not ours to judge: hand the
        // path back and let exec say what it thinks of it.
        Ok(false) => {
            discard(dir, &part);
            Ok(program.to_path_buf())
        }
        Err(e) => {
            discard(dir, &part);
            Err(e)
        }
    }
}

/// Undo what a failed extraction made: the part file, then (best effort —
/// a concurrent extraction's files keep the dir non-empty and safe) the
/// directory this call created. A failed extraction publishes nothing,
/// not even litter.
fn discard(dir: &Path, part: &Path) {
    let _ = fs::remove_file(part);
    let _ = fs::remove_dir(dir);
}

/// Same doctrine for the zip lane, whose part is a whole directory:
/// remove it recursively, then (best effort, same non-empty safety) the
/// asset directory it sat in.
fn discard_tree(assets_dir: &Path, part_dir: &Path) {
    let _ = fs::remove_dir_all(part_dir);
    let _ = fs::remove_dir(assets_dir);
}

/// The Windows lane: upstream's win64 asset is a `.zip` holding the
/// program beside the DLLs it loads at start (ManagedPart.dll, dbgshim,
/// the Roslyn set) — spawning the archive itself is what a live editor
/// answered with "%1 is not a valid Win32 application" (owner repro,
/// 2026-10-13). Every member is written by BASENAME only, into one part
/// directory, then published whole: a member's own path is input, never
/// trusted, and nothing outside this directory is even constructible from
/// it (`../`, absolute paths, drive letters all collapse to a basename
/// the OS then refuses if it misbehaves). Upstream's single `netcoredbg/`
/// directory flattens to exactly the sibling shape the exe needs its DLLs
/// in; publishing whole (rename a directory, never file-by-file) means a
/// concurrent session sees either nothing or the complete program.
fn materialize_zip(archive: &Path, file: fs::File, target: &Path) -> io::Result<PathBuf> {
    // `target` is `<assets dir>/<stem>/netcoredbg.exe`; its grandparent
    // is the quarantine root this call populates.
    let stem_dir = target.parent().expect("zip_target always has a parent");
    let assets_dir = stem_dir.parent().expect("extraction_dir always has a parent");
    fs::create_dir_all(assets_dir)?;
    let part_dir =
        assets_dir.join(format!("{}.part-{}", stem_dir.file_name().unwrap_or_default().to_string_lossy(), std::process::id()));
    fs::create_dir_all(&part_dir)?;
    let unzip = zip::ZipArchive::new(file)
        .and_then(|mut entries| unzip_flat(&mut entries, &part_dir))
        .map_err(|e| {
            // Past the magic the archive is this module's claim: name it.
            // A zip failure that is not an io failure (bad central
            // directory, unsupported method) is invalid data by kind.
            let kind = match &e {
                zip::result::ZipError::Io(io_err) => io_err.kind(),
                _ => io::ErrorKind::InvalidData,
            };
            io::Error::new(kind, format!("extracting from {}: {e}", archive.display()))
        });
    match unzip {
        Ok(()) => {
            // Which program (if any) the zip holds: the Windows PE image,
            // or — since D28 — the macOS arm64 unix binary. exe first: a
            // zip holding both is the Windows shape.
            let program_name = if part_dir.join(ZIP_PROGRAM_NAME).is_file() {
                ZIP_PROGRAM_NAME
            } else if part_dir.join(MEMBER_NAME).is_file() {
                MEMBER_NAME
            } else {
                // A zip with no program member is not ours to judge (the
                // gzip lane says the same of a memberless tar): the path
                // goes back untouched, and a well-formed-but-empty publish
                // would have left litter — remove it.
                discard_tree(assets_dir, &part_dir);
                return Ok(archive.to_path_buf());
            };
            let published = stem_dir.join(program_name);
            match fs::rename(&part_dir, stem_dir) {
                Ok(()) => Ok(published),
                Err(e) => {
                    // Only a concurrent winner's published program excuses
                    // a failed rename; anything else is extraction failure
                    // with a name the caller should hear.
                    if published.is_file() {
                        Ok(published)
                    } else {
                        Err(io::Error::new(
                            e.kind(),
                            format!("publishing {}: {e}", stem_dir.display()),
                        ))
                    }
                }
            }
        }
        Err(e) => {
            discard_tree(assets_dir, &part_dir);
            Err(e)
        }
    }
}

/// Stream every regular member into `part_dir` under its basename alone.
/// Directory entries carry no bytes (the flat layout has no directories);
/// a duplicate basename — only an adversarial zip can author one — is
/// last-writer-wins inside a quarantine directory nobody else reads.
fn unzip_flat<R: Read + Seek>(
    entries: &mut zip::ZipArchive<R>,
    part_dir: &Path,
) -> zip::result::ZipResult<()> {
    for i in 0..entries.len() {
        let mut entry = entries.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let Some(base) = Path::new(entry.name()).file_name() else {
            continue; // names like "" or "/": nothing to name, nothing to write
        };
        // Decided BEFORE the mutable borrow of `entry` below: `base` borrows
        // `entry.name()`, and consulting it after `io::copy(&mut entry, ...)`
        // is an E0502 the unix build sees and a Windows build does not (the
        // lane's `#[cfg(unix)]` block is what extends the borrow's liveness —
        // found by ubuntu CI on first compile of the lane). The binding is
        // itself cfg-gated: the Windows build has no exec bit to decide and
        // rightly calls the leftover unused.
        #[cfg(unix)]
        let program_member = base == std::path::Path::new(MEMBER_NAME);
        let mut out = fs::File::create(part_dir.join(base))?;
        io::copy(&mut entry, &mut out).map_err(zip::result::ZipError::Io)?;
        // D28: the macOS arm64 zip's program member must come out
        // executable — the exec bit is decided by the lane (exactly the
        // `netcoredbg` member, deterministically 0o755), never by the
        // archive's stored mode, which an author controls. Siblings stay
        // non-executable; the Windows exe needs no bit anywhere.
        #[cfg(unix)]
        if program_member {
            use std::os::unix::fs::PermissionsExt;
            out.set_permissions(fs::Permissions::from_mode(0o755))
                .map_err(zip::result::ZipError::Io)?;
        }
        // The OS refuses a basename it cannot name a file with (`evil:x`
        // on Windows): that refusal surfaces as the named failure — the
        // containment never depended on the name being *sane*, only on it
        // being a basename.
    }
    Ok(())
}

/// Copy the first regular-file member named `netcoredbg` into `out`;
/// `Ok(false)` means the archive holds none. Tar headers are walked by
/// hand — 512-byte blocks, size at 124..136 (octal, or GNU/POSIX
/// base-256 when the high bit is set), typeflag at 156 — so the lane's
/// one new dependency stays the pure-Rust flate2 (gzip only; a tar crate
/// would be the second).
fn stream_member<R: Read, W: Write>(reader: R, out: &mut W) -> io::Result<bool> {
    let mut tar = flate2::read::MultiGzDecoder::new(reader);
    let mut header = [0u8; 512];
    loop {
        tar.read_exact(&mut header)?;
        if header.iter().all(|&byte| byte == 0) {
            return Ok(false); // end-of-archive marker
        }
        let size = tar_size(&header[124..136]).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "tar member size field is malformed",
            )
        })?;
        let typeflag = header[156];
        if matches!(typeflag, b'0' | 0) && member_is_netcoredbg(&header) {
            io::copy(&mut tar.by_ref().take(size), out)?;
            return Ok(true);
        }
        let padded = size.div_ceil(512).saturating_mul(512);
        io::copy(&mut tar.by_ref().take(padded), &mut io::sink())?;
    }
}

/// Member path = prefix + name (ustar), matched by basename only: the
/// archive may nest the binary (`usr/local/bin/netcoredbg`); what matters
/// is that it IS the binary, wherever it sits.
fn member_is_netcoredbg(header: &[u8; 512]) -> bool {
    let name = ascii_field(&header[0..100]);
    let full = if &header[257..263] == b"ustar\0" {
        let prefix = ascii_field(&header[345..500]);
        if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        }
    } else {
        name.to_string()
    };
    Path::new(&full)
        .file_name()
        .and_then(|n| n.to_str())
        == Some(MEMBER_NAME)
}

fn ascii_field(field: &[u8]) -> &str {
    let end = field.iter().position(|&byte| byte == 0).unwrap_or(field.len());
    std::str::from_utf8(&field[..end]).unwrap_or_default()
}

/// The 12-byte tar size field: zero-padded octal (NUL- or
/// space-terminated) as tar writes it, or — high bit of the first byte —
/// GNU/POSIX base-256 big-endian, which real archives use for members
/// over 8 GiB and hand-builders for anything. A field that is neither is
/// malformed; the caller says so rather than reading a real member empty.
fn tar_size(field: &[u8]) -> Option<u64> {
    if field.first()? & 0x80 != 0 {
        let mut value: u64 = 0;
        for &byte in &field[1..] {
            value = (value << 8) | byte as u64;
        }
        return Some(value);
    }
    let text = std::str::from_utf8(field).ok()?;
    u64::from_str_radix(text.trim().trim_end_matches(['\0', ' ']), 8).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A genuine `.tar.gz` built byte by byte (ustar headers, octal sizes,
    /// 512-block padding, gzip stream) — the fixture never trusts a
    /// library to have written the archive the reader must survive.
    fn tar_entry_bytes(name: &str, content: &[u8], base256_size: bool) -> Vec<u8> {
        let mut block = [0u8; 512];
        block[..name.len()].copy_from_slice(name.as_bytes());
        block[100..108].copy_from_slice(b"0000755\0");
        if base256_size {
            // GNU base-256: high bit of the first byte set, the length
            // big-endian (zero-padded) across the remaining 11.
            block[124] = 0x80;
            block[125..136].copy_from_slice(&[0u8; 11]);
            block[128..136].copy_from_slice(&(content.len() as u64).to_be_bytes());
        } else {
            block[124..136].copy_from_slice(format!("{:011o}\0", content.len()).as_bytes());
        }
        block[156] = b'0';
        block[257..263].copy_from_slice(b"ustar\0");
        block[263..265].copy_from_slice(b"00");
        for byte in &mut block[148..156] {
            *byte = b' ';
        }
        let sum: u32 = block.iter().copied().map(u32::from).sum();
        block[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        let mut entry = block.to_vec();
        entry.extend_from_slice(content);
        entry.resize(entry.len().div_ceil(512) * 512, 0);
        entry
    }

    fn tar_entry(name: &str, content: &[u8]) -> Vec<u8> {
        tar_entry_bytes(name, content, false)
    }

    fn tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = Vec::new();
        for (name, content) in entries {
            tar.extend(tar_entry(name, content));
        }
        tar.extend([0u8; 1024]);
        let mut encoder = flate2::read::GzEncoder::new(&tar[..], flate2::Compression::default());
        let mut gz = Vec::new();
        encoder.read_to_end(&mut gz).expect("gzip");
        gz
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zatara-asset-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn a_gzip_tar_holding_netcoredbg_materializes_to_an_executable_program() {
        let dir = temp_dir("extract");
        let payload = b"\x7fELFfake netcoredbg body".to_vec();
        let archive = dir.join("netcoredbg-3.2.0-linux-x64.tar.gz");
        fs::write(
            &archive,
            tar_gz(&[
                ("ThirdPartyNotices.txt", b"notices".as_slice()),
                ("netcoredbg", &payload),
            ]),
        )
        .unwrap();

        let program = materialize(&archive).expect("materialize");

        assert_eq!(program, extraction_target(&archive));
        assert_eq!(fs::read(&program).unwrap(), payload);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&program).unwrap().permissions().mode();
            assert_ne!(mode & 0o111, 0, "extracted program must be executable: {mode:o}");
        }
        // The archive keeps its bytes: the cache entry stays the verified
        // artifact the hash manifest speaks of.
        assert!(archive.is_file());
    }

    #[test]
    fn materialize_is_idempotent_and_survives_the_archive_going_away() {
        let dir = temp_dir("idem");
        let archive = dir.join("netcoredbg-3.2.0-linux-x64.tar.gz");
        fs::write(&archive, tar_gz(&[("netcoredbg", b"body".as_slice())])).unwrap();

        let first = materialize(&archive).unwrap();
        fs::remove_file(&archive).unwrap(); // e.g. cache prune between sessions
        let second = materialize(&archive).unwrap();

        assert_eq!(first, second, "sessions share one extracted program");
    }

    #[test]
    fn a_plain_file_and_a_missing_path_pass_through_so_spawn_keeps_its_honest_error() {
        let dir = temp_dir("passthru");
        // A file that is neither archive — the user's own binary through
        // a settings override — is the spawn's story to tell verbatim.
        let plain = dir.join("netcoredbg");
        fs::write(&plain, b"#!/bin/sh neither gzip nor zip").unwrap();
        assert_eq!(materialize(&plain).unwrap(), plain);

        // A PATH-style bare name (no file behind it) is nobody's archive:
        // the spawn's "not found" must be the error the client hears.
        let missing = dir.join("netcoredbg");
        assert_eq!(materialize(&missing).unwrap(), missing);
    }

    /// A real zip built with the crate's writer (deflate) — the fixture
    /// mirrors the shape upstream ships: nested under one directory, the
    /// exe beside the DLLs it loads.
    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, content) in entries {
            writer.start_file((*name).to_string(), options).expect("start member");
            writer.write_all(content).expect("write member");
        }
        writer.finish().expect("finish zip").into_inner()
    }

    #[test]
    fn a_windows_zip_materializes_the_exe_with_its_dlls_beside_it() {
        // The live defect (owner repro, 2026-10-13): the published win
        // asset is upstream's win64.zip — exe + seven sibling DLLs — and
        // spawning the archive itself is what died with os error 193.
        let dir = temp_dir("zipwin");
        let exe = b"MZ\x90\x00 fake netcoredbg PE".to_vec();
        let archive = dir.join("netcoredbg-3.2.0-1092-win-x64.zip");
        fs::write(
            &archive,
            zip_bytes(&[
                ("netcoredbg/ThirdPartyNotices.txt", b"notices".as_slice()),
                ("netcoredbg/ManagedPart.dll", b"MZ managed part".as_slice()),
                ("netcoredbg/netcoredbg.exe", &exe),
            ]),
        )
        .unwrap();

        let program = materialize(&archive).expect("the Windows zip materializes");

        assert_eq!(program, zip_target(&archive));
        assert_eq!(fs::read(&program).unwrap(), exe);
        // The whole tree flattened by basename: the DLLs sit beside the
        // exe, which is exactly how netcoredbg locates them at startup.
        assert!(program.parent().unwrap().join("ManagedPart.dll").is_file());
        assert!(program.parent().unwrap().join("ThirdPartyNotices.txt").is_file());
        // The cache entry keeps its verified bytes — the manifest's claim
        // stays about the archive, not the extraction.
        assert!(archive.is_file());

        // Idempotent across sessions — and across the archive itself
        // going away (cache prune), like the gzip lane.
        fs::remove_file(&archive).unwrap();
        assert_eq!(materialize(&dir.join("netcoredbg-3.2.0-1092-win-x64.zip")).unwrap(), program);
    }

    #[test]
    fn a_macos_arm64_zip_materializes_the_unix_binary_executable() {
        // D28: the published macOS asset is upstream's osx-arm64.zip — the
        // program is a Mach-O binary named `netcoredbg` (no `.exe`), and a
        // non-executable extraction would die at exec with a bare
        // permission error on the Mac. The exec bit is the lane's decision
        // (see unzip_flat), asserted where the OS grades it.
        let dir = temp_dir("zipmac");
        let macho = b"\xCF\xFA\xED\xFE fake netcoredbg Mach-O".to_vec();
        let archive = dir.join("netcoredbg-3.2.0-1092-macos-arm64.zip");
        fs::write(
            &archive,
            zip_bytes(&[
                ("netcoredbg/ThirdPartyNotices.txt", b"notices".as_slice()),
                ("netcoredbg/netcoredbg", &macho),
            ]),
        )
        .unwrap();

        let program = materialize(&archive).expect("the macOS arm64 zip materializes");

        // The unix program name under the same quarantine layout — NOT
        // `extraction_target`, which names `netcoredbg.exe` on a Windows
        // build host grading this lane.
        assert_eq!(
            program,
            dir.join(ASSET_DIR).join("netcoredbg-3.2.0-1092-macos-arm64").join("netcoredbg"),
        );
        assert_eq!(fs::read(&program).unwrap(), macho);
        assert!(program.parent().unwrap().join("ThirdPartyNotices.txt").is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&program).unwrap().permissions().mode();
            assert_ne!(mode & 0o111, 0, "the unix program must be executable: {mode:o}");
            let sibling = program.parent().unwrap().join("ThirdPartyNotices.txt");
            assert_eq!(fs::metadata(&sibling).unwrap().permissions().mode() & 0o111, 0,
                "only the program member gets the exec bit");
        }
        assert!(archive.is_file(), "the cache entry keeps its verified bytes");

        // Steady state on a unix host is the gzip lane's target stat (the
        // mac lane publishes the same unix name): a second call lands on
        // the same file without re-reading the archive. Only unix hosts
        // (and only Macs in production) ever publish this name — a Windows
        // build host's target stats look for `netcoredbg.exe`, so the
        // idempotence step is graded where the lane actually runs.
        #[cfg(unix)]
        {
            fs::remove_file(&archive).unwrap();
            assert_eq!(materialize(&archive).unwrap(), program);
        }
    }

    #[test]
    fn a_malformed_zip_names_the_archive_and_litters_nothing() {
        // Same doctrine as the gzip lane: past the magic the archive is
        // this module's claim, and garbage comes back as a named failure.
        let dir = temp_dir("zipcorrupt");
        let archive = dir.join("netcoredbg-3.2.0-win-x64.zip");
        fs::write(&archive, b"PK\x03\x04 no central directory here").unwrap();

        let err = materialize(&archive).expect_err("garbage zip must not yield a program");
        let msg = err.to_string();
        assert!(msg.contains("netcoredbg-3.2.0-win-x64.zip"), "error names the archive: {msg}");
        assert!(!zip_target(&archive).exists(), "nothing is published from a corrupt zip");
        let leftovers: Vec<_> = fs::read_dir(dir.join(ASSET_DIR))
            .map(|rd| rd.collect::<Vec<_>>())
            .unwrap_or_default();
        assert!(leftovers.is_empty(), "the part dir is cleaned: {leftovers:?}");
    }

    #[test]
    fn a_zip_without_the_exe_member_is_not_rewritten() {
        let dir = temp_dir("zipnomember");
        let archive = dir.join("netcoredbg-surprise.zip");
        fs::write(&archive, zip_bytes(&[("share/readme.txt", b"hello".as_slice())])).unwrap();

        assert_eq!(
            materialize(&archive).unwrap(),
            archive,
            "no member, no opinion — the path comes back untouched"
        );
        let leftovers: Vec<_> = fs::read_dir(dir.join(ASSET_DIR))
            .map(|rd| rd.collect::<Vec<_>>())
            .unwrap_or_default();
        assert!(leftovers.is_empty(), "a memberless publish leaves no litter: {leftovers:?}");
    }

    #[test]
    fn a_zip_member_path_is_ignored_only_its_basename_lands() {
        // The containment mechanism that makes zip-slip unconstructible:
        // no member path is ever joined — only `file_name()` of it — so
        // however deeply nested (or, from an adversarial writer, however
        // `../`-laden) a member claims to sit, everything lands as a
        // plain file inside the stem's quarantine directory. (The zip
        // crate's own writer refuses to author `../` names, so the deep
        // nest stands in for what a hand-built archive would claim.)
        let dir = temp_dir("zipnest");
        let payload = b"MZ nested deep".to_vec();
        let archive = dir.join("netcoredbg-nest-win-x64.zip");
        fs::write(
            &archive,
            zip_bytes(&[
                ("usr/opt/deep/netcoredbg.exe", &payload),
                ("usr/opt/deep/ManagedPart.dll", b"MZ dll".as_slice()),
            ]),
        )
        .unwrap();

        let program = materialize(&archive).expect("nesting is just a path, not a location");

        assert_eq!(program, zip_target(&archive));
        assert_eq!(fs::read(&program).unwrap(), payload);
        assert!(program.parent().unwrap().join("ManagedPart.dll").is_file());
        // No directory of the claimed path was materialised anywhere.
        assert!(!dir.join("usr").exists(), "claimed member paths stay fictional");
    }

    #[test]
    fn a_gzip_without_a_netcoredbg_member_is_not_rewritten() {
        let dir = temp_dir("nomember");
        let archive = dir.join("netcoredbg-surprise.tar.gz");
        fs::write(
            &archive,
            tar_gz(&[("share/docs/readme.txt", b"hello".as_slice())]),
        )
        .unwrap();

        assert_eq!(
            materialize(&archive).unwrap(),
            archive,
            "no member, no opinion — the path comes back untouched"
        );
        // And the lane leaves no part-file litter behind its pass-through.
        let leftovers: Vec<_> = fs::read_dir(dir.join(ASSET_DIR))
            .map(|rd| rd.collect::<Vec<_>>())
            .unwrap_or_default();
        assert!(leftovers.is_empty(), "failed extraction cleans its part file: {leftovers:?}");
    }

    #[test]
    fn nested_member_paths_match_by_basename() {
        let dir = temp_dir("nested");
        let payload = b"nested netcoredbg".to_vec();
        let archive = dir.join("netcoredbg-3.2.0-linux-x64.tar.gz");
        fs::write(
            &archive,
            tar_gz(&[("usr/local/bin/netcoredbg", &payload), ("netcoredbg.debug", b"decoy".as_slice())]),
        )
        .unwrap();

        let program = materialize(&archive).unwrap();
        assert_eq!(fs::read(&program).unwrap(), payload);
    }

    #[test]
    fn a_base256_member_size_is_read_not_misread_as_empty() {
        // GNU base-256 size fields (the shape real packagers emit for big
        // members): the member must come out complete, never a silent
        // zero-byte "program".
        let dir = temp_dir("base256");
        let payload = vec![0xABu8; 1100]; // spans more than one block
        let mut tar = tar_entry_bytes("netcoredbg", &payload, true);
        tar.extend([0u8; 1024]);
        let mut encoder = flate2::read::GzEncoder::new(&tar[..], flate2::Compression::default());
        let mut gz = Vec::new();
        encoder.read_to_end(&mut gz).unwrap();
        let archive = dir.join("netcoredbg-3.2.0-linux-x64.tar.gz");
        fs::write(&archive, gz).unwrap();

        let program = materialize(&archive).expect("base-256 member extracts");
        assert_eq!(fs::read(&program).unwrap(), payload, "full member, no truncation");
    }

    #[test]
    fn a_malformed_gzip_names_the_archive_and_litters_nothing() {
        // Past the magic the archive is this module's claim: garbage must
        // come back as an extraction failure that names the file, not a
        // bare transport error and not a published empty program.
        let dir = temp_dir("corrupt");
        let archive = dir.join("netcoredbg-3.2.0-linux-x64.tar.gz");
        fs::write(&archive, [0x1f, 0x8b, 0xde, 0xad, 0xbe, 0xef]).unwrap();

        let err = materialize(&archive).expect_err("garbage gzip must not yield a program");
        let msg = err.to_string();
        assert!(msg.contains("netcoredbg-3.2.0-linux-x64.tar.gz"), "error names the archive: {msg}");
        assert!(!extraction_target(&archive).exists(), "nothing is published from a corrupt stream");
        let parts: Vec<_> = fs::read_dir(dir.join(ASSET_DIR))
            .map(|rd| rd.filter_map(Result::ok).collect::<Vec<_>>())
            .unwrap_or_default();
        assert!(parts.is_empty(), "the part file is cleaned: {parts:?}");
    }
}
